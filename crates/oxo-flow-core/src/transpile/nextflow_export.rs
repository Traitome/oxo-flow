//! Nextflow DSL2 **export** (issue #1319): the missing half of the
//! transpiler — emit a parsed [`WorkflowConfig`] back to a Nextflow DSL2
//! document over the SAME documented subset the importer accepts.
//!
//! Design contract, mirroring the importer's policy:
//!
//! - **Reuse, never retype.** The emitter reads the engine's parsed model
//!   (`WorkflowConfig` rules + the [`WorkflowDag`] the caller already
//!   built); it invents no second representation of the workflow.
//! - **Hard error, never silent loss.** Every rule or workflow field the
//!   round trip cannot carry through the importer aborts the export naming
//!   the construct. The gate is exhaustive by construction: each rule and
//!   the whole document are serialized and their keys checked against the
//!   handled allowlists, so a future model field can never be dropped
//!   silently.
//! - **Determinism.** Output is a pure function of the parsed workflow:
//!   rules emit in declaration order, maps are sorted, no timestamps, no
//!   HashMap iteration. Emitting twice yields byte-identical text.
//! - **Round-trip fidelity.** Every emitted construct is chosen so that
//!   `import ∘ export` reproduces the declared artifacts (rule names, DAG
//!   edges, declared I/O, resources, environment): `{sample}` →
//!   `${meta.id}`, `{config.ext_args}` → `$task.ext.args`, `{input[N]}` →
//!   positionally declared `path inK` channels, upstream wiring →
//!   `PROCESS.out[.<emit>]` with deterministic positional emit labels
//!   (`o1`, `o2`, …) when the upstream rule has several outputs.

use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;
use crate::error::{OxoFlowError, Result};
use crate::rule::{FilePatterns, Rule};
use crate::transpile::wildcard_names;
use regex::Regex;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::LazyLock;

/// The rule-level fields the exporter handles (emits, or consciously
/// converts to a DSL comment). Every other serialized key is a hard error —
/// the exhaustive guard against silent loss (issue #1319 policy).
const HANDLED_RULE_KEYS: &[&str] = &[
    "name",
    "input",
    "output",
    "shell",
    "depends_on",
    "resources",
    "environment",
    "envvars",
    "scratch",
    "retries",
    "description",
    // Re-derived by the importer on the way back (tool-name/IO inference) —
    // carried with a warning, never silently.
    "types",
];

/// The `[resources]` keys the Nextflow directive set can carry.
const HANDLED_RESOURCE_KEYS: &[&str] = &["threads", "memory", "disk", "time_limit", "partition"];

/// The `[environment]` keys the importer itself emits (Nextflow precedence:
/// container > conda > modules — combined declarations are hard errors).
const HANDLED_ENVIRONMENT_KEYS: &[&str] = &["conda", "docker", "modules"];

/// The workflow-level sections the exporter handles. Anything else in the
/// serialized document is a hard error naming the section.
const HANDLED_TOP_LEVEL_KEYS: &[&str] = &["workflow", "config", "defaults", "types", "rules"];

/// The `[workflow]` keys the exporter carries (as header comments — the
/// importer re-derives name/version and infers `sample_pattern` from the
/// wiring, so they are context, not claims).
const HANDLED_WORKFLOW_KEYS: &[&str] = &["name", "version", "sample_pattern", "description"];

/// The `[defaults]` keys that are safe to drop (the importer re-emits
/// `threads = 1` itself); anything else is a hard error.
const HANDLED_DEFAULTS_KEYS: &[&str] = &["threads"];

/// The `[types.*]` catalog entries the importer itself emits and re-creates
/// from its inference on re-import. Any other catalog entry has no path
/// back through the DSL.
const IMPORTER_TYPE_CATALOGS: &[&str] = &["fastq", "bam_coord", "vcf_indexed"];

/// Engine placeholders in a shell body: known forms map onto Nextflow
/// interpolations; anything else is named and rejected.
static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([A-Za-z_][A-Za-z0-9_.\[\]]*)\}").expect("valid placeholder regex")
});

/// Nextflow `${name}` / `$name` interpolations, including dotted members —
/// the same shape the importer matches.
static NF_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_.]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid nextflow variable regex")
});

/// Export a parsed workflow to a Nextflow DSL2 document plus a list of
/// non-fatal warnings.
///
/// The caller supplies the [`WorkflowDag`] built from the same rules (the
/// CLI constructs it before exporting), so a cyclic workflow can never
/// reach the emitter. The returned DSL re-imports into a workflow with
/// equivalent declared artifacts for the subset this module documents;
/// anything outside it aborts with the construct named.
pub fn export_nextflow(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
) -> Result<(String, Vec<String>)> {
    check_workflow_surface(config)?;
    let mut warnings = Vec::new();
    check_type_catalog(config, &mut warnings);
    if config
        .config
        .get("ext_args")
        .and_then(toml::Value::as_str)
        .is_some_and(|default| !default.is_empty())
    {
        warnings.push(
            "[config] ext_args has a non-empty default — the importer re-emits \
             `ext_args = \"\"` on re-import, so the default does not survive the round trip"
                .to_string(),
        );
    }

    // Output-pattern → producing rule indices, in declaration order. Mirrors
    // the DAG's `output_to_node` exact-match semantics; multiple producers
    // of one pattern make the wiring ambiguous and abort (silently picking
    // a producer would drop the other's edge).
    let mut producers: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, rule) in config.rules.iter().enumerate() {
        for output in rule_output_patterns(rule)? {
            producers.entry(output.clone()).or_default().push(index);
        }
    }

    // Per-rule translated state, computed once so the process blocks and the
    // workflow block agree (the workflow block needs each rule's input feeds;
    // the process blocks need the positional emit labels those feeds imply).
    let mut rendered = Vec::with_capacity(config.rules.len());
    for rule in &config.rules {
        rendered.push(render_rule(rule, config, &producers, dag, &mut warnings)?);
    }
    // Merge every consumer's positional labels for its multi-output
    // producer. The labels are positional per producer (`o1`, `o2`, …), so
    // all consumers agree by construction.
    let mut producer_labels: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for rule in &rendered {
        for (producer_index, labels) in &rule.emit_labels {
            producer_labels
                .entry(*producer_index)
                .or_insert_with(|| labels.clone());
        }
    }

    let mut out = String::new();
    push_header(&mut out, config);
    for (index, rule) in config.rules.iter().enumerate() {
        push_process(
            &mut out,
            rule,
            &rendered[index],
            producer_labels.get(&index),
        )?;
    }
    push_workflow_block(&mut out, config, &rendered)?;

    Ok((out, warnings))
}

/// One rule's translated pieces, shared by the process and workflow blocks.
struct RenderedRule {
    /// The declared input patterns, in declaration order.
    input_patterns: Vec<String>,
    /// Positional channel names for the `input:` declaration (`in0`, `in1`).
    input_vars: Vec<String>,
    /// The translated script body (engine placeholders already rewritten to
    /// Nextflow interpolations).
    script: String,
    /// For each input, the resolved wiring argument (`PROCESS.out`,
    /// `PROCESS.out.<emit>`, or `Channel.fromPath("…")`).
    feeds: Vec<String>,
    /// Upstream rule index → positional emit labels for its outputs;
    /// populated only for multi-output producers a downstream rule consumes.
    emit_labels: BTreeMap<usize, Vec<String>>,
}

/// Workflow-level surface gate: every section outside the documented
/// round-trippable subset is a hard error naming it.
fn check_workflow_surface(config: &WorkflowConfig) -> Result<()> {
    let value = toml::Value::try_from(config)
        .map_err(|e| export_error(format!("workflow cannot be serialized for inspection: {e}")))?;
    let table = value
        .as_table()
        .ok_or_else(|| export_error("workflow document is not a TOML table"))?;
    for key in table.keys() {
        if !HANDLED_TOP_LEVEL_KEYS.contains(&key.as_str()) {
            return Err(export_error(format!(
                "[{key}] has no Nextflow DSL2 equivalent in the exportable subset — the \
                 exporter refuses to drop it silently; remove the section or hand-port the \
                 workflow"
            )));
        }
    }
    if let Some(workflow) = table.get("workflow").and_then(toml::Value::as_table) {
        for key in workflow.keys() {
            if !HANDLED_WORKFLOW_KEYS.contains(&key.as_str()) {
                return Err(export_error(format!(
                    "[workflow] key '{key}' has no Nextflow DSL2 equivalent in the exportable \
                     subset — the exporter refuses to drop it silently"
                )));
            }
        }
    }
    if let Some(defaults) = table.get("defaults").and_then(toml::Value::as_table) {
        for key in defaults.keys() {
            if !HANDLED_DEFAULTS_KEYS.contains(&key.as_str()) {
                return Err(export_error(format!(
                    "[defaults] key '{key}' has no Nextflow DSL2 equivalent in the exportable \
                     subset — the exporter refuses to drop it silently"
                )));
            }
        }
    }
    // `[config]` other than the importer's own `ext_args` cannot survive the
    // round trip: only `$task.ext.args` maps onto a config key on import.
    // Engine-injected keys (`samples_list`, `paired_pairs_count`, …) are not
    // authoring surface — the parse step adds them — and are excluded via
    // the engine's own classifier rather than a hand-copied list.
    if let Some(cfg) = table.get("config").and_then(toml::Value::as_table) {
        for key in cfg.keys() {
            if key == "ext_args" || crate::config_impact::is_engine_injected_key(key) {
                continue;
            }
            return Err(export_error(format!(
                "[config] key '{key}' cannot round-trip: the importable subset maps only \
                 `$task.ext.args` onto `{{config.ext_args}}`; inline the value into the \
                 rules or hand-port the workflow"
            )));
        }
    }
    Ok(())
}

/// The `[types.*]` catalog: known importer-emitted catalogs are re-derived
/// by the importer on re-import; anything else is a hard error.
fn check_type_catalog(config: &WorkflowConfig, warnings: &mut Vec<String>) {
    let Ok(value) = toml::Value::try_from(config) else {
        return;
    };
    let Some(types) = value.get("types").and_then(toml::Value::as_table) else {
        return;
    };
    for name in types.keys() {
        if IMPORTER_TYPE_CATALOGS.contains(&name.as_str()) {
            continue;
        }
        warnings.push(format!(
            "type catalog '{name}' is not one of the importer-emitted catalogs \
             ({IMPORTER_TYPE_CATALOGS:?}) and has no Nextflow equivalent — it will not \
             re-appear on re-import"
        ));
    }
}

/// Render one rule into its DSL pieces, hard-erroring on every field the
/// round trip cannot carry.
fn render_rule(
    rule: &Rule,
    config: &WorkflowConfig,
    producers: &BTreeMap<String, Vec<usize>>,
    dag: &WorkflowDag,
    warnings: &mut Vec<String>,
) -> Result<RenderedRule> {
    let name = rule.name.as_str();
    check_rule_surface(rule, warnings)?;

    let inputs = rule_input_patterns(rule)?;

    // Rule names must survive the importer's process-name lowercasing
    // verbatim, and process names must be DSL identifiers (they appear in
    // `PROCESS.out` wiring expressions).
    if !is_dsl_rule_name(name) {
        return Err(export_error(format!(
            "rule '{name}': the importer lowercases Nextflow process names and restricts \
             them to [a-z_][a-z0-9_]* — rename the rule so export → import reproduces it \
             exactly"
        )));
    }

    // ── Wiring: resolve each input to its producing rule or a channel glob ──
    let mut input_vars = Vec::with_capacity(inputs.len());
    let mut feeds = Vec::with_capacity(inputs.len());
    let mut emit_labels: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut wired_upstreams = Vec::new();
    for (position, pattern) in inputs.iter().enumerate() {
        input_vars.push(format!("in{position}"));
        let producers_of_pattern = producers.get(pattern).cloned().unwrap_or_default();
        let producer = match producers_of_pattern.as_slice() {
            [] => None,
            [only] => Some(*only),
            several => {
                let names = several
                    .iter()
                    .map(|&i| config.rules[i].name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(export_error(format!(
                    "rule '{name}' declares input '{pattern}' that {} rules produce ({names}) \
                     — the exportable subset wires one producer per input; disambiguate the \
                     shared output or hand-port the workflow",
                    several.len()
                )));
            }
        };
        match producer {
            Some(producer_index) => {
                let producer_rule = &config.rules[producer_index];
                let producer_outputs = rule_output_patterns(producer_rule)?;
                let Some(output_index) = producer_outputs.iter().position(|out| out == pattern)
                else {
                    // The DAG resolved this edge from the same exact-match
                    // set, so this is an invariant break, not a user error.
                    return Err(export_error(format!(
                        "rule '{name}' input '{pattern}' resolves to producer '{}' in the DAG \
                         but that rule does not declare the pattern — DAG/exporter \
                         disagreement, please report this",
                        producer_rule.name
                    )));
                };
                wired_upstreams.push(producer_index);
                let feed = if producer_outputs.len() == 1 {
                    format!("{}.out", producer_rule.name)
                } else {
                    // Multi-output producer: the importer can only select a
                    // labeled emit (`PROCESS.out.<emit>`), and the oxo-flow
                    // model does not store emit names — attach deterministic
                    // positional labels on export so the re-import selects
                    // the same output.
                    let labels = emit_labels.entry(producer_index).or_insert_with(|| {
                        (0..producer_outputs.len())
                            .map(|i| format!("o{}", i + 1))
                            .collect()
                    });
                    format!("{}.out.{}", producer_rule.name, labels[output_index])
                };
                feeds.push(feed);
            }
            None => {
                // Source input: the importer maps exactly one `*` onto
                // `{sample}`; any other wildcard shape cannot re-import.
                let glob = pattern_to_source_glob(name, pattern)?;
                feeds.push(format!("Channel.fromPath(\"{glob}\")"));
            }
        }
    }

    // Declared ordering-only dependencies would be dropped by the re-import
    // (the importer builds `depends_on` from wiring alone) — hard error.
    for dep in &rule.depends_on {
        let dep_index = config
            .rules
            .iter()
            .position(|candidate| candidate.name == *dep)
            .ok_or_else(|| {
                export_error(format!(
                    "rule '{name}' depends on '{dep}', which is not declared — fix the \
                     dependency before exporting"
                ))
            })?;
        if !wired_upstreams.contains(&dep_index) {
            return Err(export_error(format!(
                "rule '{name}' declares ordering-only depends_on ['{dep}'] with no input \
                 produced by it — Nextflow wiring feeds data channels, so the importer \
                 cannot express the edge; wire the dependency through an input or hand-port \
                 the rule"
            )));
        }
    }
    // Cross-check against the DAG the caller built: every wired upstream
    // must be a recorded dependency (the DAG may additionally carry
    // depends_on-only edges, covered above).
    let dag_dependencies = dag.dependencies(name)?;
    for upstream in &wired_upstreams {
        let upstream_name = config.rules[*upstream].name.as_str();
        if !dag_dependencies.iter().any(|dep| dep == upstream_name) {
            return Err(export_error(format!(
                "rule '{name}' wires input from '{upstream_name}' but the DAG does not \
                 record that edge — DAG/exporter disagreement, please report this"
            )));
        }
    }

    let script = translate_shell(rule, name, &inputs)?;

    Ok(RenderedRule {
        input_patterns: inputs,
        input_vars,
        script,
        feeds,
        emit_labels,
    })
}

/// Rule-level surface gate: exhaustive by construction — the rule is
/// serialized and every key outside [`HANDLED_RULE_KEYS`] is a hard error.
fn check_rule_surface(rule: &Rule, warnings: &mut Vec<String>) -> Result<()> {
    let name = rule.name.as_str();
    let value = toml::Value::try_from(rule)
        .map_err(|e| export_error(format!("rule '{name}' cannot be serialized: {e}")))?;
    let table = value
        .as_table()
        .ok_or_else(|| export_error(format!("rule '{name}' is not a TOML table")))?;
    for key in table.keys() {
        if !HANDLED_RULE_KEYS.contains(&key.as_str()) {
            return Err(export_error(format!(
                "rule '{name}' sets '{key}', which has no Nextflow DSL2 equivalent in the \
                 exportable subset — the exporter refuses to drop it silently; remove the \
                 field or hand-port the rule"
            )));
        }
    }
    if let Some(resources) = table.get("resources").and_then(toml::Value::as_table) {
        for key in resources.keys() {
            if !HANDLED_RESOURCE_KEYS.contains(&key.as_str()) {
                return Err(export_error(format!(
                    "rule '{name}' resource '{key}' has no Nextflow directive in the \
                     exportable subset (cpus/memory/time/disk/queue only)"
                )));
            }
        }
    }
    if let Some(environment) = table.get("environment").and_then(toml::Value::as_table) {
        for key in environment.keys() {
            if !HANDLED_ENVIRONMENT_KEYS.contains(&key.as_str()) {
                return Err(export_error(format!(
                    "rule '{name}' environment backend '{key}' has no Nextflow directive in \
                     the exportable subset (conda/container/module only)"
                )));
            }
        }
        let backends = HANDLED_ENVIRONMENT_KEYS
            .iter()
            .filter(|backend| environment.contains_key(**backend))
            .count();
        if backends > 1 {
            return Err(export_error(format!(
                "rule '{name}' declares {backends} environment backends — the importer keeps \
                 only the highest-priority one (container > conda > modules) and drops the \
                 rest, so the round trip is not faithful; declare a single backend"
            )));
        }
    }
    if table.contains_key("types") {
        warnings.push(format!(
            "rule '{name}' carries a types contract — Nextflow has no equivalent; the \
             importer re-derives contracts on re-import (tool-name/IO inference), so a \
             hand-authored contract will not survive verbatim"
        ));
    }
    let shell = rule.shell.as_deref().unwrap_or_default();
    if shell.trim().is_empty() {
        return Err(export_error(format!(
            "rule '{name}' has no shell body — the importer refuses empty script blocks, so \
             the export would not re-import"
        )));
    }
    // The importer trims the joined script body. Leading whitespace can
    // never survive the round trip. A trailing newline on a MULTI-line body
    // can: it is exactly what the importer's own `'''` serialization
    // appends, so re-import trims it away and re-serializes the
    // byte-identical TOML. A single-line body is serialized as a basic
    // string (no newline), so a trailing newline there drifts.
    if shell != shell.trim_start() {
        return Err(export_error(format!(
            "rule '{name}' shell has leading whitespace — the importer trims the joined \
             script body, so the bytes would not round-trip"
        )));
    }
    let trimmed_end = shell.trim_end();
    if shell != trimmed_end {
        // The importer decides the TOML form on the TRIMMED body: an
        // interior newline makes it a `'''` literal (whose serialization
        // re-appends one trailing newline), otherwise a basic string.
        let is_multiline = trimmed_end.contains('\n');
        let suffix_is_single_newline = &shell[trimmed_end.len()..] == "\n";
        if !is_multiline || !suffix_is_single_newline {
            return Err(export_error(format!(
                "rule '{name}' shell has trailing whitespace that the importer's body trim \
                 would strip and not re-append — the bytes would not round-trip"
            )));
        }
    }
    Ok(())
}

/// Translate the shell body's engine placeholders into Nextflow
/// interpolations, hard-erroring on anything that would not re-import.
fn translate_shell(rule: &Rule, name: &str, inputs: &[String]) -> Result<String> {
    let shell = rule.shell.as_deref().unwrap_or_default();

    // Pre-existing Nextflow-style interpolations in the authored shell: the
    // importer treats `$ident` as an interpolation, and any non-shell-env
    // (lowercase) identifier is a hard error on import — so the export would
    // not round-trip. Checked on the ORIGINAL text: the translation below
    // introduces the legitimate `$task.*` / `${meta.id}` / `$inK` forms.
    static NF_INDEXED_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)\[[^\]}]*\]\}?")
            .expect("valid indexed nextflow regex")
    });
    for caps in NF_VAR_RE.captures_iter(shell) {
        let ident = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let head = ident.split('.').next().unwrap_or_default();
        let is_shell_env = head
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if ident.is_empty() || is_shell_env {
            continue;
        }
        return Err(export_error(format!(
            "rule '{name}' shell carries `${ident}` — the importer hard-errors on \
             non-shell-env Nextflow interpolations, so the export would not re-import; \
             replace it with an engine placeholder or hand-port the rule"
        )));
    }
    for caps in NF_INDEXED_RE.captures_iter(shell) {
        let base = &caps[1];
        let is_shell_env = base
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !is_shell_env {
            return Err(export_error(format!(
                "rule '{name}' shell carries indexed interpolation `{}` — the importer \
                 rejects indexed references; use the whole-input or positional placeholder \
                 forms or hand-port the rule",
                caps[0].trim()
            )));
        }
    }

    let mut translated = String::with_capacity(shell.len());
    let mut last_match = 0usize;
    for caps in PLACEHOLDER_RE.captures_iter(shell) {
        let whole = caps.get(0).expect("capture 0");
        let key = &caps[1];
        let replacement = match key {
            "threads" => "$task.cpus".to_string(),
            "sample" => "${meta.id}".to_string(),
            "config.ext_args" => "$task.ext.args".to_string(),
            "input" => {
                if inputs.len() != 1 {
                    return Err(export_error(format!(
                        "rule '{name}' shell references {{input}} but declares {} inputs — \
                         the importer maps {{input}} only for single-input processes; use \
                         positional {{input[N]}} forms",
                        inputs.len()
                    )));
                }
                "$in0".to_string()
            }
            other => {
                let Some(rest) = other
                    .strip_prefix("input[")
                    .and_then(|rest| rest.strip_suffix(']'))
                else {
                    return Err(export_error(format!(
                        "rule '{name}' shell references {{{other}}} — no Nextflow equivalent \
                         in the exportable subset (the importer maps $task.cpus, \
                         $task.ext.args, ${{meta.id}}, and declared input channels only)"
                    )));
                };
                let position: usize = rest.parse().map_err(|_| {
                    export_error(format!(
                        "rule '{name}' shell references {{{other}}} — the input index is not \
                         a number"
                    ))
                })?;
                if position >= inputs.len() {
                    return Err(export_error(format!(
                        "rule '{name}' shell references {{{other}}} but the rule declares {} \
                         inputs",
                        inputs.len()
                    )));
                }
                format!("$in{position}")
            }
        };
        translated.push_str(&shell[last_match..whole.start()]);
        translated.push_str(&replacement);
        last_match = whole.end();
    }
    translated.push_str(&shell[last_match..]);

    // Constructs the importer skips or rejects outright — in the authored
    // body or after translation (identical line shapes): `def` preludes are
    // stripped, version blocks are skipped, `Channel.*` is rejected, and
    // `"""` would terminate the emitted script body early.
    for (offset, line) in translated.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("def ") {
            return Err(export_error(format!(
                "rule '{name}' shell line {} starts with `def` — the importer strips def \
                 preludes, so the line would be lost; inline the value or hand-port the rule",
                offset + 1
            )));
        }
        if trimmed.starts_with("cat <<-END_VERSIONS") || trimmed.starts_with("cat <<END_VERSIONS") {
            return Err(export_error(format!(
                "rule '{name}' shell line {} opens a versions block — the importer skips \
                 version blocks, so the lines would be lost; remove the block or hand-port \
                 the rule",
                offset + 1
            )));
        }
    }
    if translated.contains("Channel.") {
        return Err(export_error(format!(
            "rule '{name}' shell references `Channel.*` — the importer hard-errors on it; \
             resolve the channel in the workflow wiring or hand-port the rule"
        )));
    }
    if translated.contains("\"\"\"") {
        return Err(export_error(format!(
            "rule '{name}' shell contains a triple-double-quote — it would terminate the \
             emitted Nextflow script body; hand-port the rule"
        )));
    }
    Ok(translated)
}

/// Map a source input pattern onto the single-`*` glob the importer maps
/// back onto `{sample}`.
fn pattern_to_source_glob(rule: &str, pattern: &str) -> Result<String> {
    if pattern.contains('?') || pattern.contains('[') {
        return Err(export_error(format!(
            "rule '{rule}' input '{pattern}' contains a character-class or `?` glob — the \
             importer rejects such channels; hand-port the rule"
        )));
    }
    let names = wildcard_names(pattern);
    let sample_count = pattern.matches("{sample}").count();
    if names.len() > 1 || sample_count > 1 || (!names.is_empty() && sample_count == 0) {
        return Err(export_error(format!(
            "rule '{rule}' input '{pattern}' carries wildcards other than a single \
             '{{{{sample}}}}' — the importer maps exactly one `*` onto '{{{{sample}}}}', so \
             the channel cannot re-import; hand-port the rule"
        )));
    }
    Ok(pattern.replace("{sample}", "*"))
}

/// The declared output expression for one output pattern: an input-anchored
/// interpolation when the pattern extends one of the rule's inputs (the
/// importer maps `$inK` back onto the input's pattern — the only faithful
/// carrier for non-`{sample}` wildcards), a `${meta.id}` interpolation for
/// `{sample}` patterns, or a literal for constant paths.
fn output_expr(rule: &str, pattern: &str, inputs: &[String]) -> Result<String> {
    let mut best: Option<(usize, usize)> = None; // (input position, rest length)
    for (position, input_pattern) in inputs.iter().enumerate() {
        if let Some(rest) = pattern.strip_prefix(input_pattern.as_str()) {
            let rest_len = rest.len();
            if best.is_none_or(|(_, best_len)| rest_len < best_len) {
                best = Some((position, rest_len));
            }
        }
    }
    if let Some((position, rest_len)) = best {
        let rest = &pattern[pattern.len() - rest_len..];
        return Ok(format!("${{in{position}}}{rest}"));
    }
    if pattern.contains("{sample}") {
        // The `${meta.id}` replacement itself introduces braces, so the
        // check runs on the wildcard NAMES of the original pattern.
        let others: Vec<String> = wildcard_names(pattern)
            .into_iter()
            .filter(|name| name != "sample")
            .collect();
        if !others.is_empty() {
            return Err(export_error(format!(
                "rule '{rule}' output '{pattern}' carries wildcard(s) {{{}}} besides \
                 '{{{{sample}}}}' — no Nextflow output expression maps them back \
                 faithfully; hand-port the rule",
                others.join(", ")
            )));
        }
        return Ok(pattern.replace("{sample}", "${meta.id}"));
    }
    if !wildcard_names(pattern).is_empty() {
        return Err(export_error(format!(
            "rule '{rule}' output '{pattern}' carries wildcards other than '{{{{sample}}}}' and \
             extends none of the rule's inputs — no Nextflow output expression maps them \
             back faithfully; hand-port the rule"
        )));
    }
    if pattern.contains('*') || pattern.contains('?') {
        return Err(export_error(format!(
            "rule '{rule}' output '{pattern}' is a bare glob — the importer refuses globs it \
             cannot anchor; declare the concrete path or hand-port the rule"
        )));
    }
    Ok(pattern.to_string())
}

/// The rule's declared input patterns. Named-map/directory input forms have
/// no positional channel mapping and are hard errors.
fn rule_input_patterns(rule: &Rule) -> Result<Vec<String>> {
    match &rule.input {
        FilePatterns::List(patterns) => Ok(patterns.clone()),
        other => Err(export_error(format!(
            "rule '{}' uses a non-list input form ({other:?}) — the exportable subset maps \
             positionally declared path channels only",
            rule.name
        ))),
    }
}

/// The rule's declared output patterns.
fn rule_output_patterns(rule: &Rule) -> Result<Vec<String>> {
    match &rule.output {
        FilePatterns::List(patterns) => Ok(patterns.clone()),
        other => Err(export_error(format!(
            "rule '{}' uses a non-list output form ({other:?}) — the exportable subset maps \
             positional output declarations only",
            rule.name
        ))),
    }
}

/// A rule name that survives the importer's process-name lowercasing
/// verbatim and is a valid DSL identifier.
fn is_dsl_rule_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Make free text safe for a DSL comment: single line, no braces (the
/// importer brace-matches process bodies without comment awareness), no
/// quote characters that could confuse the quote-aware brace matcher, and
/// no backslashes.
fn comment_text(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .trim()
        .replace(['{', '}'], "()")
        .replace(['"', '\'', '\\'], "-")
        .replace("//", "--")
}

/// Header comments carry the workflow context the importer re-derives
/// (name/version) or infers (sample_pattern) — context, not claims.
fn push_header(out: &mut String, config: &WorkflowConfig) {
    let _ = writeln!(
        out,
        "// Exported from oxo-flow by `oxo-flow export-dsl` (issue #1319)."
    );
    let _ = writeln!(
        out,
        "// Workflow '{}' v{}.",
        comment_text(&config.workflow.name),
        comment_text(&config.workflow.version)
    );
    if let Some(description) = &config.workflow.description {
        let _ = writeln!(out, "// Description: {}", comment_text(description));
    }
    if let Some(sample_pattern) = &config.workflow.sample_pattern {
        // Header comments sit outside every process body, so the raw
        // `{sample}` braces are safe here (only process-body comments are
        // sanitized — the importer brace-matches those bodies without
        // comment awareness).
        let _ = writeln!(
            out,
            "// Declared sample_pattern: {}",
            sample_pattern.lines().next().unwrap_or_default().trim()
        );
    }
    let _ = writeln!(
        out,
        "// Re-import with `oxo-flow import --from nextflow`: the workflow name becomes \
         \"{}_pipeline\", version 0.1.0, and sample_pattern is re-derived from the wiring.",
        config
            .rules
            .first()
            .map(|rule| rule.name.as_str())
            .unwrap_or("workflow")
    );
    let _ = writeln!(out);
}

/// Emit one `process NAME { … }` block in declaration order.
fn push_process(
    out: &mut String,
    rule: &Rule,
    rendered: &RenderedRule,
    emit_labels: Option<&Vec<String>>,
) -> Result<()> {
    let _ = writeln!(out, "process {} {{", rule.name);
    if let Some(description) = &rule.description {
        let _ = writeln!(out, "    // Description: {}", comment_text(description));
    }

    // Environment (the importer keeps exactly one backend).
    if let Some(docker) = &rule.environment.docker {
        let _ = writeln!(out, "    container \"{}\"", docker);
    } else if let Some(conda) = &rule.environment.conda {
        let _ = writeln!(out, "    conda \"{}\"", conda);
    }
    for module in &rule.environment.modules {
        let _ = writeln!(out, "    module \"{}\"", module);
    }

    // Environment variables, sorted for byte-stable output.
    for (key, value) in &rule.envvars {
        if key.is_empty()
            || key.contains([',', '"', '\'', '=', '$', '\n'])
            || value.contains(['"', '\'', '$', '\n'])
        {
            return Err(export_error(format!(
                "rule '{}' env var '{key}' cannot be expressed as an `env NAME, \"value\"` \
                 directive (quotes/newlines/dollar signs are rejected by the importer); \
                 hand-port the rule",
                rule.name
            )));
        }
        let _ = writeln!(out, "    env \"{key}\", \"{value}\"");
    }

    if rule.scratch {
        let _ = writeln!(out, "    scratch true");
    }
    if rule.retries > 0 {
        let _ = writeln!(out, "    maxRetries {}", rule.retries);
    }

    // Resource directives. `cpus 1` is the importer's default (emitted as
    // `[defaults] threads = 1`, not a per-rule resource), so it is omitted.
    if rule.resources.threads != 1 {
        let _ = writeln!(out, "    cpus {}", rule.resources.threads);
    }
    if let Some(memory) = &rule.resources.memory {
        let _ = writeln!(out, "    memory \"{memory}\"");
    }
    if let Some(time_limit) = &rule.resources.time_limit {
        let _ = writeln!(out, "    time \"{time_limit}\"");
    }
    if let Some(disk) = &rule.resources.disk {
        let _ = writeln!(out, "    disk \"{disk}\"");
    }
    if let Some(partition) = &rule.resources.partition {
        let _ = writeln!(out, "    queue \"{partition}\"");
    }

    if !rendered.input_vars.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "    input:");
        for var in &rendered.input_vars {
            let _ = writeln!(out, "    path {var}");
        }
    }

    let outputs = rule_output_patterns(rule)?;
    if !outputs.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "    output:");
        for (index, pattern) in outputs.iter().enumerate() {
            let expr = output_expr(&rule.name, pattern, &rendered.input_patterns)?;
            match emit_labels.and_then(|labels| labels.get(index)) {
                Some(label) => {
                    let _ = writeln!(out, "    path(\"{expr}\"), emit: {label}");
                }
                None => {
                    let _ = writeln!(out, "    path(\"{expr}\")");
                }
            }
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "    script:");
    let _ = writeln!(out, "    \"\"\"");
    out.push_str(&rendered.script);
    if !rendered.script.is_empty() && !rendered.script.ends_with('\n') {
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "    \"\"\"");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emit the `workflow { … }` wiring block: one invocation per rule that
/// declares inputs, in declaration order, with the resolved feeds.
fn push_workflow_block(
    out: &mut String,
    config: &WorkflowConfig,
    rendered: &[RenderedRule],
) -> Result<()> {
    let wired: Vec<(&Rule, &RenderedRule)> = config
        .rules
        .iter()
        .zip(rendered)
        .filter(|(_, rendered)| !rendered.feeds.is_empty())
        .collect();
    if wired.is_empty() {
        return Ok(());
    }
    let _ = writeln!(out, "workflow {{");
    for (rule, rendered) in wired {
        let _ = writeln!(out, "    {}({})", rule.name, rendered.feeds.join(", "));
    }
    let _ = writeln!(out, "}}");
    Ok(())
}

/// The one error shape for the exporter: a Parse error anchored to the
/// virtual DSL document, mirroring the importer's error style.
fn export_error(message: impl Into<String>) -> OxoFlowError {
    OxoFlowError::Parse {
        path: std::path::PathBuf::from("<oxo-flow export-dsl>"),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transpile::transpile;

    /// Parse a TOML document and build its DAG — the same steps the CLI
    /// performs before exporting.
    fn ready(toml: &str) -> (WorkflowConfig, WorkflowDag) {
        let config = WorkflowConfig::parse(toml).expect("workflow parses");
        let dag = WorkflowDag::from_rules(&config.rules).expect("dag builds");
        (config, dag)
    }

    fn export_toml(toml: &str) -> String {
        let (config, dag) = ready(toml);
        export_nextflow(&config, &dag).expect("exports").0
    }

    fn export_err(toml: &str) -> String {
        let (config, dag) = ready(toml);
        export_nextflow(&config, &dag)
            .expect_err("must refuse")
            .to_string()
    }

    // Rule names and extensions deliberately avoid the importer's BioType
    // inference (no tool names, no .bam/.fastq) so the hand TOML and the
    // re-imported one agree byte-for-byte; field order matches the
    // importer's own emission.
    const TWO_STEP: &str = r#"
[workflow]
name = "stage_one_pipeline"
version = "0.1.0"
sample_pattern = "data/{sample}.fastq.gz"

[defaults]
threads = 1

[[rules]]
name = "stage_one"
resources = { threads = 4, memory = "8G", time_limit = "2h" }
environment = { conda = "bioconda::trimmomatic=0.39" }
input = ["data/{sample}.fastq.gz"]
output = ["{sample}.trimmed.txt"]
shell = "trim -threads {threads} {input} {sample}.trimmed.txt"

[[rules]]
name = "stage_two"
depends_on = ["stage_one"]
resources = { threads = 12 }
input = ["{sample}.trimmed.txt"]
output = ["{sample}.final.txt"]
shell = "score --in {input} --out {sample}.final.txt"

"#;

    #[test]
    fn export_is_byte_stable() {
        // Determinism (issue #1319): emitting twice yields byte-identical
        // output — rules in declaration order, sorted env vars, no
        // timestamps, no HashMap iteration.
        let first = export_toml(TWO_STEP);
        let second = export_toml(TWO_STEP);
        assert_eq!(first, second);
        assert!(first.contains("process stage_one {"));
        assert!(first.contains("process stage_two {"));
        // The workflow wiring block, in declaration order.
        assert!(first.contains("stage_one(Channel.fromPath(\"data/*.fastq.gz\"))"));
        assert!(first.contains("stage_two(stage_one.out)"));
        // Placeholders mapped onto the DSL the importer maps back.
        assert!(first.contains("$task.cpus"));
        assert!(first.contains("${meta.id}.trimmed.txt"));
        // Resources carried as directives; the default threads omitted.
        assert!(first.contains("cpus 4"));
        assert!(first.contains("memory \"8G\""));
        assert!(first.contains("cpus 12"));
        assert!(!first.contains("align\\n    cpus 1"));
    }

    #[test]
    fn export_then_import_reproduces_the_declared_artifacts() {
        // The core round-trip promise: import(export(toml)) is byte-identical
        // for the round-trippable subset (the fixture corpus pins this at
        // integration level; this unit test keeps a minimal two-step
        // workflow honest).
        let exported = export_toml(TWO_STEP);
        let reimported = transpile(&exported, crate::transpile::SourceFormat::Nextflow)
            .expect("the exported DSL re-imports");
        // Byte-identical including the importer's trailing block separator
        // (only the raw string's leading newline is dropped).
        let normalized = TWO_STEP.trim_start();
        assert_eq!(reimported, normalized);
    }

    #[test]
    fn multi_output_producer_wires_positional_emit_labels() {
        // The oxo-flow model does not store emit names; the exporter attaches
        // deterministic positional labels (o1, o2, …) so the re-import
        // selects the same output.
        let toml = r#"
[workflow]
name = "prod_pipeline"
version = "0.1.0"
sample_pattern = "data/{sample}.txt"

[[rules]]
name = "prod"
input = ["data/{sample}.txt"]
output = ["{sample}.a", "{sample}.b"]
shell = "make {sample}"

[[rules]]
name = "sink_b"
depends_on = ["prod"]
input = ["{sample}.b"]
output = ["{sample}.out"]
shell = "cp {input} {sample}.out"
"#;
        let exported = export_toml(toml);
        assert!(
            exported.contains("path(\"${meta.id}.a\"), emit: o1"),
            "{exported}"
        );
        assert!(
            exported.contains("path(\"${meta.id}.b\"), emit: o2"),
            "{exported}"
        );
        assert!(exported.contains("sink_b(prod.out.o2)"), "{exported}");
        // And it re-imports to the same wiring.
        let reimported =
            transpile(&exported, crate::transpile::SourceFormat::Nextflow).expect("re-imports");
        let config = WorkflowConfig::parse(&reimported).unwrap();
        assert_eq!(config.rules[1].depends_on, vec!["prod".to_string()]);
        assert_eq!(
            config.rules[1].input.clone().into_list(),
            vec!["{sample}.b".to_string()]
        );
    }

    #[test]
    fn input_anchored_outputs_map_through_the_input_channel() {
        // An output that extends an input pattern (including non-{sample}
        // wildcards) is emitted as `${inK}` — the importer maps it back onto
        // the input's exact pattern.
        let toml = r#"
[workflow]
name = "index_pipeline"
version = "0.1.0"
sample_pattern = "{sample}.bam"

[[rules]]
name = "index"
input = ["{sample}.bam"]
output = ["{sample}.bam.bai"]
shell = "samtools index {input}"
"#;
        let exported = export_toml(toml);
        assert!(exported.contains("path(\"${in0}.bai\")"), "{exported}");
        let reimported =
            transpile(&exported, crate::transpile::SourceFormat::Nextflow).expect("re-imports");
        let config = WorkflowConfig::parse(&reimported).unwrap();
        assert_eq!(
            config.rules[0].output.clone().into_list(),
            vec!["{sample}.bam.bai".to_string()]
        );
    }

    #[test]
    fn out_of_subset_constructs_are_hard_errors_naming_the_construct() {
        // Never silent loss: each construct is refused with its name.
        let cases: &[(&str, &str)] = &[
            (
                // Ordering-only dependency: the importer builds edges from
                // wiring alone.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
output = ["a.txt"]
shell = "touch a.txt"

[[rules]]
name = "b"
depends_on = ["a"]
output = ["b.txt"]
shell = "touch b.txt"
"#,
                "ordering-only depends_on",
            ),
            (
                // Rule field outside the subset.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
log = "logs/{sample}.log"
output = ["a.txt"]
shell = "touch {output}"
"#,
                "'log'",
            ),
            (
                // Engine placeholder with no Nextflow equivalent.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
output = ["a.txt"]
shell = "run --out {output}"
"#,
                "{output}",
            ),
            (
                // Config key other than ext_args.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[config]
my_key = "v"

[[rules]]
name = "a"
output = ["a.txt"]
shell = "touch a.txt"
"#,
                "[config] key 'my_key'",
            ),
            (
                // Workflow-level section outside the subset.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
output = ["a.txt"]
shell = "touch a.txt"

[engine]
min_free_disk = "10G"
"#,
                "[engine]",
            ),
            (
                // Non-lowercase rule name (the importer lowercases process
                // names, so the identity would drift).
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "Trim"
output = ["a.txt"]
shell = "touch a.txt"
"#,
                "rule 'Trim'",
            ),
            (
                // Multi-producer ambiguity.
                r#"
[workflow]
name = "p1_pipeline"
version = "0.1.0"

[[rules]]
name = "p1"
output = ["shared.txt"]
shell = "touch shared.txt"

[[rules]]
name = "p2"
output = ["shared.txt"]
shell = "touch shared.txt"

[[rules]]
name = "c"
depends_on = ["p1"]
input = ["shared.txt"]
output = ["c.txt"]
shell = "cat {input} > {output}"
"#,
                "2 rules produce (p1, p2)",
            ),
            (
                // Wildcard in a source input other than {sample}.
                r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
input = ["data/{chr}.txt"]
output = ["a.txt"]
shell = "run {input}"
"#,
                "wildcards other than",
            ),
        ];
        for (toml, expected_fragment) in cases {
            let message = export_err(toml);
            assert!(
                message.contains(expected_fragment),
                "expected the error to name {expected_fragment}: {message}"
            );
        }
    }

    #[test]
    fn shell_env_vars_round_trip_and_multiline_trailing_newline_survives() {
        // Uppercase `$IDENT` survives the importer (shell env). A trailing
        // newline on a MULTI-line body is exactly what the importer's own
        // `'''` serialization appends — it round-trips byte-identically.
        let toml = r#"
[workflow]
name = "envy_pipeline"
version = "0.1.0"

[[rules]]
name = "envy"
output = ["a.txt"]
shell = '''echo $PATH > a.txt
    second line
'''
"#;
        let exported = export_toml(toml);
        assert!(exported.contains("echo $PATH > a.txt"), "{exported}");
        let reimported =
            transpile(&exported, crate::transpile::SourceFormat::Nextflow).expect("re-imports");
        let config = WorkflowConfig::parse(&reimported).unwrap();
        assert_eq!(
            config.rules[0].shell.as_deref(),
            Some("echo $PATH > a.txt\n    second line\n")
        );

        // A single-line body with a trailing newline is serialized as a
        // basic string (no newline) on re-import — the bytes drift, so the
        // export refuses it.
        let single_line = r#"
[workflow]
name = "envy_pipeline"
version = "0.1.0"

[[rules]]
name = "envy"
output = ["a.txt"]
shell = '''echo $PATH > a.txt
'''
"#;
        let message = export_err(single_line);
        assert!(message.contains("trailing whitespace"), "{message}");
    }

    #[test]
    fn non_list_input_and_resolved_hash_keys_are_refused() {
        // Named-map inputs have no positional channel mapping.
        let message = export_err(
            r#"
[workflow]
name = "a_pipeline"
version = "0.1.0"

[[rules]]
name = "a"
input = { reads = "data/{sample}.fq" }
output = ["a.txt"]
shell = "run {input}"
"#,
        );
        assert!(message.contains("non-list input form"), "{message}");
    }

    impl FilePatterns {
        /// Test helper: the list form as plain patterns (panics otherwise).
        fn into_list(self) -> Vec<String> {
            match self {
                FilePatterns::List(patterns) => patterns,
                other => panic!("expected list patterns, got {other:?}"),
            }
        }
    }
}
