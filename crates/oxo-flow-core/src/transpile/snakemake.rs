//! Snakemake transpiler for converting Snakemake rules into oxo-flow workflows.
//!
//! The supported subset is deliberately narrow, and every construct outside
//! it is a hard error that names the source line. Emitting a workflow with a
//! silently dropped directive, an invented path, or a shell placeholder that
//! only fails at run time is worse than refusing the import.

use super::{
    escape_toml_basic, infer_bio_types, normalize_memory, push_types_contract, render_shell,
    wildcard_names,
};
use crate::error::{OxoFlowError, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Extracted intermediate representation of a Snakemake rule.
///
/// Kept as the public parsing view for API compatibility; the emitter uses
/// the richer internal `ParsedRule` representation.
#[derive(Debug, Clone, Default)]
pub struct SnakemakeRule {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub threads: Option<u32>,
    pub memory: Option<String>,
    pub time: Option<String>,
    pub conda: Option<String>,
    pub container: Option<String>,
    pub shell: String,
    pub temporary: bool,
    pub temp_outputs: Vec<String>,
    pub protected_outputs: Vec<String>,
}

/// Parsed rules, non-fatal warnings, and global wildcard constraints.
type ParsedSnakemake = (Vec<ParsedRule>, Vec<String>, BTreeMap<String, String>);

/// One file declaration on an `input:`/`output:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PathSource {
    /// A literal path/pattern from the source.
    Literal(String),
    /// `rules.<name>.output` — resolved to that rule's single output.
    RuleOutput(String),
}

#[derive(Debug, Clone)]
struct DeclaredPath {
    name: Option<String>,
    source: PathSource,
    temp: bool,
    protected: bool,
    ancient: bool,
    /// Source line of the declaration (diagnostics).
    line: usize,
    /// Declared through expand(...) — fan-in/fan-out validation applies.
    from_expand: bool,
    /// expand(...) carried an explicit value list (not materialized).
    expand_values: bool,
}

#[derive(Debug, Clone, Default)]
struct ParsedRule {
    name: String,
    line: usize,
    inputs: Vec<DeclaredPath>,
    outputs: Vec<DeclaredPath>,
    threads: Option<u32>,
    memory: Option<String>,
    time: Option<String>,
    conda: Option<String>,
    container: Option<String>,
    modules: Vec<String>,
    shell: String,
    temporary: bool,
    temp_outputs: Vec<String>,
    protected_outputs: Vec<String>,
    ancient: Vec<String>,
    log: Option<String>,
    benchmark: Option<String>,
    priority: i32,
    retries: Option<u32>,
    gpu: Option<u32>,
    disk: Option<String>,
    envvars: BTreeMap<String, String>,
    depends_on: Vec<String>,
    localrule: bool,
    /// `rule all`-style sink: no outputs and no command; dropped from output.
    sink: bool,
}

/// Transpile Snakemake source code into an oxo-flow TOML pipeline.
pub fn transpile_snakemake(source: &str) -> Result<String> {
    transpile_snakemake_with_warnings(source).map(|(toml, _)| toml)
}

/// Transpile Snakemake source code, returning the TOML and every non-fatal
/// translation warning (currently only sink-rule and marker downgrades).
pub fn transpile_snakemake_with_warnings(source: &str) -> Result<(String, Vec<String>)> {
    let (rules, mut warnings, wildcard_constraints) = parse_snakemake(source)?;
    if rules.is_empty() {
        return Err(config_error(
            "no Snakemake rule definitions found in source",
        ));
    }
    let (toml, emit_warnings) = emit_workflow(&rules, &wildcard_constraints)?;
    warnings.extend(emit_warnings);
    Ok((toml, warnings))
}

/// Parse all rules and return the legacy public view.
pub fn parse_snakemake_rules(source: &str) -> Result<Vec<SnakemakeRule>> {
    let (rules, _, _) = parse_snakemake(source)?;
    Ok(rules
        .into_iter()
        .map(|rule| SnakemakeRule {
            name: rule.name,
            inputs: rule
                .inputs
                .iter()
                .map(|path| match &path.source {
                    PathSource::Literal(value) => value.clone(),
                    PathSource::RuleOutput(upstream) => format!("rules.{upstream}.output"),
                })
                .collect(),
            outputs: rule
                .outputs
                .iter()
                .map(|path| match &path.source {
                    PathSource::Literal(value) => value.clone(),
                    PathSource::RuleOutput(upstream) => format!("rules.{upstream}.output"),
                })
                .collect(),
            threads: rule.threads,
            memory: rule.memory,
            time: rule.time,
            conda: rule.conda,
            container: rule.container,
            shell: rule.shell,
            temporary: rule.temporary,
            temp_outputs: rule.temp_outputs,
            protected_outputs: rule.protected_outputs,
        })
        .collect())
}

fn config_error(message: impl Into<String>) -> OxoFlowError {
    OxoFlowError::Config {
        message: message.into(),
    }
}

fn line_error(line: usize, message: impl Into<String>) -> OxoFlowError {
    OxoFlowError::Parse {
        path: std::path::PathBuf::from("<snakemake source>"),
        message: format!("line {line}: {}", message.into()),
    }
}

/// Parse the source, hard-failing on every unsupported construct.
fn parse_snakemake(source: &str) -> Result<ParsedSnakemake> {
    let lines: Vec<&str> = source.lines().collect();
    let mut rules: Vec<ParsedRule> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut localrules: BTreeSet<String> = BTreeSet::new();
    let mut wildcard_constraints: BTreeMap<String, String> = BTreeMap::new();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = strip_comment(raw).trim().to_string();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        let line_no = i + 1;

        if let Some(rest) = trimmed.strip_prefix("rule ") {
            let Some(name) = rest.strip_suffix(':').map(str::trim) else {
                return Err(line_error(
                    line_no,
                    format!("expected `rule <name>:` but found `{trimmed}`"),
                ));
            };
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(line_error(
                    line_no,
                    format!(
                        "rule name `{name}` is not a plain identifier — the importer requires \
                         alphanumeric/underscore names to emit a rule"
                    ),
                ));
            }
            let (rule, next) = parse_rule(&lines, i, name, line_no, &mut warnings)?;
            rules.push(rule);
            i = next;
            continue;
        }
        if trimmed.starts_with("checkpoint ") {
            return Err(line_error(
                line_no,
                "Snakemake checkpoints are dynamic and cannot be translated — replace the \
                 checkpoint with a regular rule or import the pipeline by hand",
            ));
        }
        if let Some(rest) = trimmed.strip_prefix("localrules:") {
            for name in split_list(rest) {
                if !name.is_empty() {
                    localrules.insert(name);
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("wildcard_constraints:") {
            // Both the inline form and the indented block form are supported.
            let mut parse_pairs = |text: &str, line: usize| -> Result<()> {
                for part in split_list(text) {
                    let Some((name, value)) = part.split_once('=') else {
                        return Err(line_error(
                            line,
                            format!(
                                "cannot parse wildcard constraint `{part}` — expected \
                                 `<wildcard>=<regex>`"
                            ),
                        ));
                    };
                    wildcard_constraints.insert(
                        name.trim().to_string(),
                        value.trim().trim_matches(['"', '\'']).to_string(),
                    );
                }
                Ok(())
            };
            parse_pairs(rest, line_no)?;
            let mut j = i + 1;
            while j < lines.len() {
                let next = strip_comment(lines[j]).trim().to_string();
                if next.is_empty() {
                    j += 1;
                    continue;
                }
                if !lines[j].starts_with(char::is_whitespace) {
                    break;
                }
                parse_pairs(&next, j + 1)?;
                j += 1;
            }
            i = j;
            continue;
        }
        if trimmed.starts_with("include:")
            || trimmed.starts_with("subworkflow ")
            || trimmed.starts_with("module ")
            || trimmed.starts_with("use rule ")
            || trimmed.starts_with("configfile:")
            || trimmed.starts_with("ruleorder:")
            || trimmed.starts_with("onsuccess:")
            || trimmed.starts_with("onerror:")
            || trimmed.starts_with("onerror:")
        {
            return Err(line_error(
                line_no,
                format!(
                    "`{}` is a global Snakemake directive that changes which rules run; the \
                     importer cannot expand it safely — inline the included rules or import \
                     the pipeline by hand",
                    trimmed
                ),
            ));
        }

        return Err(line_error(
            line_no,
            format!(
                "unsupported top-level construct `{trimmed}` — the importer only translates \
                 `rule`, `localrules`, and `wildcard_constraints` blocks"
            ),
        ));
    }

    if rules.is_empty() {
        return Ok((rules, warnings, wildcard_constraints));
    }

    let mut names = BTreeSet::new();
    for rule in &rules {
        if !names.insert(rule.name.clone()) {
            return Err(line_error(
                rule.line,
                format!(
                    "duplicate rule name '{}' — rule names must be unique",
                    rule.name
                ),
            ));
        }
    }
    for rule in &mut rules {
        if localrules.contains(&rule.name) {
            rule.localrule = true;
        }
    }
    resolve_rule_outputs(&mut rules)?;
    for rule in &rules {
        validate_input_domains(rule, &mut warnings)?;
    }
    Ok((rules, warnings, wildcard_constraints))
}

/// Resolve `rules.X.output` references to the upstream rule's single output
/// and add the dependency edge.
fn resolve_rule_outputs(rules: &mut [ParsedRule]) -> Result<()> {
    let outputs: BTreeMap<String, Vec<String>> = rules
        .iter()
        .map(|rule| {
            let paths = rule
                .outputs
                .iter()
                .filter_map(|path| match &path.source {
                    PathSource::Literal(value) => Some(value.clone()),
                    PathSource::RuleOutput(_) => None,
                })
                .collect();
            (rule.name.clone(), paths)
        })
        .collect();

    for rule in rules.iter_mut() {
        let mut deps = Vec::new();
        for path in &mut rule.inputs {
            let upstream = match &path.source {
                PathSource::RuleOutput(upstream) => upstream.clone(),
                PathSource::Literal(_) => continue,
            };
            let upstream_outputs = outputs.get(&upstream).ok_or_else(|| {
                line_error(
                    rule.line,
                    format!(
                        "input references `rules.{upstream}.output`, but no rule named \
                         '{upstream}' is defined in this source"
                    ),
                )
            })?;
            match upstream_outputs.as_slice() {
                [] => {
                    return Err(line_error(
                        rule.line,
                        format!(
                            "input references `rules.{upstream}.output`, but rule '{upstream}' \
                             declares no output the importer can map"
                        ),
                    ));
                }
                [single] => {
                    path.source = PathSource::Literal(single.clone());
                    if !deps.contains(&upstream) {
                        deps.push(upstream.clone());
                    }
                }
                _ => {
                    return Err(line_error(
                        rule.line,
                        format!(
                            "input references `rules.{upstream}.output`, but rule '{upstream}' \
                             declares {} outputs — select one with `rules.{upstream}.output.<name>` \
                             (named output selection is not supported; import this rule by hand)",
                            upstream_outputs.len()
                        ),
                    ));
                }
            }
        }
        rule.depends_on = deps;
    }
    Ok(())
}

/// Parse one `rule NAME:` block starting at `start` (0-based line index).
fn parse_rule(
    lines: &[&str],
    start: usize,
    name: &str,
    line_no: usize,
    warnings: &mut Vec<String>,
) -> Result<(ParsedRule, usize)> {
    let mut rule = ParsedRule {
        name: name.to_string(),
        line: line_no,
        ..Default::default()
    };
    let mut i = start + 1;
    let mut directive: Option<String> = None;
    let mut body_indent: Option<usize> = None;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = strip_comment(raw).trim().to_string();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        // A new top-level construct ends this rule.
        if !raw.starts_with(char::is_whitespace)
            && (trimmed.starts_with("rule ")
                || trimmed.starts_with("checkpoint ")
                || trimmed.starts_with("include:")
                || trimmed.starts_with("localrules:")
                || trimmed.starts_with("wildcard_constraints:")
                || trimmed.starts_with("configfile:")
                || trimmed.starts_with("subworkflow ")
                || trimmed.starts_with("module ")
                || trimmed.starts_with("use rule ")
                || trimmed.starts_with("ruleorder:")
                || trimmed.starts_with("onsuccess:")
                || trimmed.starts_with("onerror:"))
        {
            break;
        }
        let current_line = i + 1;

        let indent = raw.len() - raw.trim_start().len();
        let at_directive_indent = body_indent.is_none_or(|base| indent <= base);
        let directive_name = at_directive_indent
            .then(|| trimmed.split_once(':').map(|(head, _)| head.trim()))
            .flatten()
            .filter(|head| {
                !head.is_empty() && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });

        if let Some(head) = directive_name {
            body_indent.get_or_insert(indent);
            let rest = trimmed
                .split_once(':')
                .map(|(_, tail)| tail.trim())
                .unwrap_or("");
            match head {
                "input" | "output" => {
                    directive = Some(head.to_string());
                    parse_path_line(&mut rule, head == "input", rest, current_line, warnings)?;
                }
                "threads" => {
                    directive = None;
                    rule.threads = Some(parse_u32(rest, current_line, "threads")?);
                }
                "resources" => {
                    directive = Some("resources".into());
                    parse_resources(rest, &mut rule, current_line)?;
                }
                "conda" => {
                    directive = Some("conda".into());
                    if !rest.is_empty() {
                        rule.conda = Some(unquote(rest).to_string());
                        directive = None;
                    }
                }
                "container" | "singularity" => {
                    directive = Some("container".into());
                    if !rest.is_empty() {
                        let value = unquote(rest)
                            .trim_start_matches("docker://")
                            .trim_start_matches("shub://")
                            .to_string();
                        rule.container = Some(value);
                        directive = None;
                    }
                }
                "envmodules" => {
                    if rest.is_empty() {
                        directive = Some("envmodules".into());
                    } else {
                        directive = None;
                        rule.modules.extend(parse_modules(rest));
                    }
                }
                "shell" => {
                    directive = Some("shell".into());
                    let (shell, next) = collect_shell(lines, i, rest, current_line)?;
                    rule.shell = shell;
                    i = next;
                    continue;
                }
                "log" => {
                    directive = Some("log".into());
                    if !rest.is_empty() {
                        rule.log = Some(unquote(rest).to_string());
                        directive = None;
                    }
                }
                "benchmark" => {
                    directive = Some("benchmark".into());
                    if !rest.is_empty() {
                        rule.benchmark = Some(unquote(rest).to_string());
                        directive = None;
                        warnings.push(format!(
                            "line {current_line}: `benchmark:` becomes a declared path, but \
                             oxo-flow does not auto-populate benchmark files — the rule's \
                             command must write it"
                        ));
                    }
                }
                "priority" => {
                    directive = None;
                    rule.priority = rest.parse::<i32>().map_err(|_| {
                        line_error(
                            current_line,
                            format!("priority must be an integer, found `{rest}`"),
                        )
                    })?;
                }
                "retries" => {
                    directive = None;
                    rule.retries = Some(parse_u32(rest, current_line, "retries")?);
                }
                // Constructs that change what runs or how: hard errors.
                "params"
                | "run"
                | "script"
                | "notebook"
                | "wildcard_constraints"
                | "shadow"
                | "group"
                | "default_target"
                | "handlers"
                | "envvars_unsupported" => {
                    return Err(line_error(
                        current_line,
                        format!(
                            "Snakemake `{head}:` is not supported by the importer — it changes \
                             the command or rule behavior in a way the deterministic subset \
                             cannot preserve. Rewrite the rule with literal input/output paths \
                             and a `shell:` body, or hand-port this rule"
                        ),
                    ));
                }
                "envvars" => {
                    directive = Some("envvars".into());
                    parse_envvars(rest, &mut rule, current_line)?;
                }
                other => {
                    return Err(line_error(
                        current_line,
                        format!(
                            "unsupported Snakemake directive `{other}:` — the importer refuses \
                             to drop it silently; remove it or hand-port this rule"
                        ),
                    ));
                }
            }
            i += 1;
            continue;
        }

        // Continuation line of the current directive.
        if let Some(ref dir) = directive {
            match dir.as_str() {
                "input" | "output" => {
                    parse_path_line(&mut rule, dir == "input", &trimmed, current_line, warnings)?;
                }
                "resources" => parse_resources(&trimmed, &mut rule, current_line)?,
                "conda" => {
                    rule.conda = Some(unquote(&trimmed).to_string());
                    directive = None;
                }
                "container" => {
                    let value = unquote(&trimmed)
                        .trim_start_matches("docker://")
                        .trim_start_matches("shub://")
                        .to_string();
                    rule.container = Some(value);
                    directive = None;
                }
                "log" => {
                    rule.log = Some(unquote(&trimmed).to_string());
                    directive = None;
                }
                "benchmark" => {
                    rule.benchmark = Some(unquote(&trimmed).to_string());
                    directive = None;
                    warnings.push(format!(
                        "line {current_line}: `benchmark:` becomes a declared path, but \
                         oxo-flow does not auto-populate benchmark files — the rule's command \
                         must write it"
                    ));
                }
                "envvars" => parse_envvars(&trimmed, &mut rule, current_line)?,
                "envmodules" => rule.modules.extend(parse_modules(&trimmed)),
                _ => {
                    return Err(line_error(
                        current_line,
                        format!("unexpected continuation line `{trimmed}`"),
                    ));
                }
            }
            i += 1;
            continue;
        }

        return Err(line_error(
            current_line,
            format!(
                "unsupported rule-body construct `{trimmed}` — the importer only supports \
                 input/output, threads, resources, conda/container/envmodules, log/benchmark, \
                 priority, retries, envvars, and shell"
            ),
        ));
    }

    // Sink rules (`rule all`) are targets, not processing rules; an oxo-flow
    // rule with no command is rejected by validate (E022), so dropping the
    // sink is the only honest translation.
    if rule.shell.trim().is_empty() {
        if rule.outputs.is_empty() && !rule.inputs.is_empty() {
            rule.sink = true;
            warnings.push(format!(
                "rule '{}' (line {line_no}) is a target-only/sink rule with no command — \
                 dropped from the generated workflow; its inputs still define the DAG leaves",
                rule.name
            ));
        } else {
            return Err(line_error(
                line_no,
                format!(
                    "rule '{}' has no shell/run/script body the importer can translate — \
                     refusing to emit a rule that would be skipped at run time",
                    rule.name
                ),
            ));
        }
    }

    for path in &rule.outputs {
        let PathSource::Literal(value) = &path.source else {
            continue;
        };
        if path.temp {
            rule.temporary = true;
            rule.temp_outputs.push(value.clone());
        }
        if path.protected {
            rule.protected_outputs.push(value.clone());
        }
    }
    for path in &rule.inputs {
        if path.ancient
            && let PathSource::Literal(value) = &path.source
        {
            rule.ancient.push(value.clone());
        }
    }

    Ok((rule, i))
}

/// `expand()` can be a fan-OUT input (the rule's output carries the same
/// wildcard) or a fan-IN input (one output consumes many per-sample files).
///
/// Fan-out maps to a normal wildcard input, with a warning that the explicit
/// value list is not materialized. Fan-in cannot be represented by a
/// wildcard rule — every instance would write the same output — so it is a
/// hard error pointing at the engine's `expand_inputs` fan-in construct.
fn validate_input_domains(rule: &ParsedRule, warnings: &mut Vec<String>) -> Result<()> {
    if rule.sink {
        return Ok(());
    }
    let output_wildcards: BTreeSet<String> = rule
        .outputs
        .iter()
        .filter_map(|path| match &path.source {
            PathSource::Literal(pattern) => Some(wildcard_names(pattern)),
            PathSource::RuleOutput(_) => None,
        })
        .flatten()
        .collect();

    // Any constant output in a rule driven by wildcard inputs is a
    // collision hazard: every instance writes that path. This covers the
    // all-constant fan-in shape AND the mixed wildcard/constant shape
    // (e.g. `out/{sample}.bam` plus `reports/summary.txt`).
    let input_wildcards: BTreeSet<String> = rule
        .inputs
        .iter()
        .filter_map(|path| match &path.source {
            PathSource::Literal(pattern) => Some(wildcard_names(pattern)),
            PathSource::RuleOutput(_) => None,
        })
        .flatten()
        .collect();
    let constant_outputs: Vec<(&str, usize)> = rule
        .outputs
        .iter()
        .filter_map(|path| match &path.source {
            PathSource::Literal(pattern) if wildcard_names(pattern).is_empty() => {
                Some((pattern.as_str(), path.line))
            }
            _ => None,
        })
        .collect();
    if !constant_outputs.is_empty() && !input_wildcards.is_empty() {
        let constant_list = constant_outputs
            .iter()
            .map(|(pattern, _)| format!("'{pattern}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let uncovered = input_wildcards
            .iter()
            .filter(|wildcard| !output_wildcards.contains(*wildcard))
            .cloned()
            .collect::<Vec<_>>();
        let uncovered_clause = if uncovered.is_empty() {
            String::new()
        } else {
            format!(
                "; input wildcard(s) {{{}}} are not covered by any output wildcard",
                uncovered.join(", ")
            )
        };
        let line = constant_outputs
            .first()
            .map(|(_, line)| *line)
            .unwrap_or(rule.line);
        return Err(line_error(
            line,
            format!(
                "mixed wildcard/constant outputs are not supported: rule '{}' declares \
                 constant output(s) {constant_list} alongside wildcard output(s), while its \
                 inputs carry wildcard(s) {{{}}}{uncovered_clause}. Every instance would \
                 write the constant output(s), so they would collide. Make every output carry \
                 the fan-out wildcard(s), or use the engine's `expand_inputs` fan-in \
                 construct / hand-port this rule",
                rule.name,
                input_wildcards
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    for path in &rule.inputs {
        if !path.from_expand {
            continue;
        }
        let PathSource::Literal(pattern) = &path.source else {
            continue;
        };
        let wildcards = wildcard_names(pattern);
        let fan_out = !wildcards.is_empty()
            && wildcards
                .iter()
                .all(|wildcard| output_wildcards.contains(wildcard));
        if !fan_out {
            let missing = wildcards
                .iter()
                .filter(|wildcard| !output_wildcards.contains(*wildcard))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            return Err(line_error(
                path.line,
                format!(
                    "expand() fan-in is not supported: input pattern '{pattern}' carries \
                     wildcard(s) {{{missing}}} that do not appear in any output of rule '{}'. \
                     Emitting a wildcard input would turn this into a per-sample rule whose \
                     instances overwrite the same output. Use the engine's `expand_inputs` \
                     fan-in construct (workflow-format reference) or hand-port this rule",
                    rule.name
                ),
            ));
        }
        if path.expand_values {
            warnings.push(format!(
                "line {}: expand() explicit value list is not materialized — the pattern \
                 '{pattern}' was imported, but declare the wildcard domain with \
                 sample_pattern / [[sample_groups]] / [[values]] before running",
                path.line
            ));
        }
    }
    Ok(())
}

fn parse_u32(value: &str, line: usize, field: &str) -> Result<u32> {
    value.parse::<u32>().map_err(|_| {
        line_error(
            line,
            format!(
                "{field} must be an integer literal, found `{value}` — dynamic {field} \
                 expressions are not supported"
            ),
        )
    })
}

/// Parse one input/output declaration (possibly a continuation line).
fn parse_path_line(
    rule: &mut ParsedRule,
    input: bool,
    text: &str,
    line: usize,
    warnings: &mut Vec<String>,
) -> Result<()> {
    for item in split_top_level(text) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // The `=` splitter MUST be top-level aware: expand(...) carries
        // keyword arguments (`sample=[...]`) whose `=` is inside the call.
        let (name, value) = match split_top_level_once(item, '=') {
            Some((name, value)) => (Some(name.trim().to_string()), value.trim()),
            None => (None, item),
        };
        let mut path = parse_path_value(value, line)?;
        path.name = name;
        // `touch()` only asserts the file exists; mapping it to a regular
        // output preserves that contract and `validate` still checks it.
        if value.trim_start().starts_with("touch(") {
            warnings.push(format!(
                "line {line}: `touch(...)` marker on '{}' treated as a plain output — the \
                 engine verifies declared outputs exist, which matches touch()'s contract",
                rule.name
            ));
        }
        if input {
            rule.inputs.push(path);
        } else {
            rule.outputs.push(path);
        }
    }
    Ok(())
}

fn parse_path_value(value: &str, line: usize) -> Result<DeclaredPath> {
    let value = value.trim();
    let (mut inner, mut temp, mut protected, mut ancient) = (value, false, false, false);

    loop {
        if let Some(rest) = inner.strip_prefix("temp(") {
            temp = true;
            inner = rest.strip_suffix(')').unwrap_or(rest).trim();
            continue;
        }
        if let Some(rest) = inner.strip_prefix("protected(") {
            protected = true;
            inner = rest.strip_suffix(')').unwrap_or(rest).trim();
            continue;
        }
        if let Some(rest) = inner.strip_prefix("ancient(") {
            ancient = true;
            inner = rest.strip_suffix(')').unwrap_or(rest).trim();
            continue;
        }
        if let Some(rest) = inner.strip_prefix("touch(") {
            inner = rest.strip_suffix(')').unwrap_or(rest).trim();
            continue;
        }
        break;
    }

    if let Some(rest) = inner.strip_prefix("directory(") {
        let _ = rest;
        return Err(line_error(
            line,
            "Snakemake `directory(...)` outputs/inputs are not supported — the engine has no \
             directory-artifact contract; list the concrete files instead",
        ));
    }
    if let Some(rest) = inner.strip_prefix("rules.") {
        let Some((upstream, selector)) = rest.split_once(".output") else {
            return Err(line_error(
                line,
                format!(
                    "unsupported dynamic input/output `{value}` — only `rules.<name>.output` is \
                     supported"
                ),
            ));
        };
        if !selector.trim().is_empty() {
            return Err(line_error(
                line,
                format!(
                    "named output selection `rules.{upstream}.output{selector}` is not \
                     supported — reference the rule's single output (`rules.{upstream}.output`) \
                     or hand-port this rule"
                ),
            ));
        }
        return Ok(DeclaredPath {
            name: None,
            source: PathSource::RuleOutput(upstream.to_string()),
            temp,
            protected: false,
            ancient: false,
            line,
            from_expand: false,
            expand_values: false,
        });
    }
    if let Some(rest) = inner.strip_prefix("expand(") {
        let pattern = expand_pattern(rest).ok_or_else(|| {
            line_error(
                line,
                format!(
                    "cannot parse `{value}` — expand() must contain one quoted path pattern \
                     with a wildcard, e.g. expand(\"data/{{sample}}.txt\", sample=SAMPLES)"
                ),
            )
        })?;
        if !pattern.contains('{') {
            return Err(line_error(
                line,
                format!(
                    "expand() pattern '{pattern}' has no wildcard — the importer cannot tell \
                     which values it produces; use a literal path"
                ),
            ));
        }
        return Ok(DeclaredPath {
            name: None,
            source: PathSource::Literal(pattern),
            temp,
            protected,
            ancient,
            line,
            from_expand: true,
            expand_values: split_top_level_once(rest, '=').is_some(),
        });
    }
    if inner.starts_with("lambda")
        || inner.starts_with("config[")
        || inner.starts_with("config.")
        || inner.starts_with("params.")
        || inner.starts_with("wildcards.")
        || inner.starts_with("checkpoints.")
        || inner.starts_with("unpack(")
        || inner.starts_with("collect(")
        || inner.starts_with("lookup(")
    {
        return Err(line_error(
            line,
            format!(
                "dynamic input/output `{value}` depends on Snakemake's runtime objects \
                 (config/params/wildcards/checkpoints/lambda) — the importer cannot resolve it \
                 deterministically; replace it with a literal pattern or import by hand"
            ),
        ));
    }
    let was_quoted = inner.starts_with('"') || inner.starts_with('\'');
    if !was_quoted {
        // Every supported non-literal form was handled above. An unquoted
        // remainder means the declaration was split or parsed incorrectly
        // (e.g. a top-level `=` inside expand()). Never accept it as a path:
        // brackets, parentheses, `=`, and commas are the exact junk this
        // guard exists to reject.
        return Err(line_error(
            line,
            format!(
                "cannot parse input/output declaration `{value}` — expected a quoted path, \
                 expand(...), temp(...), protected(...), ancient(...), or rules.<name>.output"
            ),
        ));
    }
    let literal = unquote(inner);
    if literal.is_empty() {
        return Err(line_error(
            line,
            format!("input/output declaration `{value}` is an empty quoted path"),
        ));
    }
    Ok(DeclaredPath {
        name: None,
        source: PathSource::Literal(literal.to_string()),
        temp,
        protected,
        ancient,
        line,
        from_expand: false,
        expand_values: false,
    })
}

/// Extract the single quoted pattern from `expand(...)`'s contents.
fn expand_pattern(mut inner: &str) -> Option<String> {
    inner = inner.strip_suffix(')').unwrap_or(inner).trim();
    let quote = inner.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &inner[quote.len_utf8()..];
    let end = rest.find(quote)?;
    let pattern = &rest[..end];
    let tail = rest[end + quote.len_utf8()..].trim();
    // Anything after the first pattern other than the keyword arguments is
    // an expand() form (a list of patterns) we cannot map faithfully.
    if !tail.is_empty() && !tail.starts_with(',') {
        return None;
    }
    Some(pattern.to_string())
}

fn parse_resources(text: &str, rule: &mut ParsedRule, line: usize) -> Result<()> {
    for part in split_top_level(text) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once('=') else {
            return Err(line_error(
                line,
                format!("cannot parse resource `{part}` — expected `<name>=<value>`"),
            ));
        };
        let key = key.trim();
        let value = unquote(value.trim());
        match key {
            "mem_mb" => {
                let mb = value.parse::<u64>().map_err(|_| {
                    line_error(
                        line,
                        format!("resources.mem_mb must be an integer, found `{value}`"),
                    )
                })?;
                rule.memory = Some(format!("{mb}M"));
            }
            "mem_gb" => {
                let gb = value.parse::<u64>().map_err(|_| {
                    line_error(
                        line,
                        format!("resources.mem_gb must be an integer, found `{value}`"),
                    )
                })?;
                rule.memory = Some(format!("{gb}G"));
            }
            "time" | "runtime" => rule.time = Some(value.to_string()),
            "gpu" => {
                rule.gpu = Some(value.parse::<u32>().map_err(|_| {
                    line_error(
                        line,
                        format!("resources.gpu must be an integer, found `{value}`"),
                    )
                })?);
            }
            "disk_mb" => {
                let mb = value.parse::<u64>().map_err(|_| {
                    line_error(
                        line,
                        format!("resources.disk_mb must be an integer, found `{value}`"),
                    )
                })?;
                rule.disk = Some(format!("{mb}M"));
            }
            other => {
                return Err(line_error(
                    line,
                    format!(
                        "unsupported Snakemake resource `{other}` — the importer cannot honor \
                         it (known: mem_mb, mem_gb, time/runtime, gpu, disk_mb); remove it or \
                         hand-port this rule"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Parse simple `NAME: "value"` envvar assignments (Snakemake 7+ `envvars:`).
fn parse_envvars(text: &str, rule: &mut ParsedRule, line: usize) -> Result<()> {
    for part in split_top_level(text) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once(':') else {
            return Err(line_error(
                line,
                format!("cannot parse envvars entry `{part}` — expected `<NAME>: <value>`"),
            ));
        };
        rule.envvars
            .insert(name.trim().to_string(), unquote(value.trim()).to_string());
    }
    Ok(())
}

/// Collect a `shell:` body, including multi-line triple-quoted forms.
fn collect_shell(
    lines: &[&str],
    start: usize,
    first: &str,
    line: usize,
) -> Result<(String, usize)> {
    let first = first.trim();
    // `shell:` with the body on the following line(s) is the common
    // Snakemake style; skip blank/comment lines to the first body line.
    if first.is_empty() {
        let mut j = start + 1;
        while j < lines.len() && strip_comment(lines[j]).trim().is_empty() {
            j += 1;
        }
        if j >= lines.len() {
            return Err(line_error(
                line + 1,
                "shell: has no body — a Snakemake shell directive must be a quoted string",
            ));
        }
        return collect_shell(lines, j, strip_comment(lines[j]).trim(), j + 1);
    }
    let (delim, prefix) = if first.starts_with("\"\"\"") {
        ("\"\"\"", "\"\"\"")
    } else if first.starts_with("'''") {
        ("'''", "'''")
    } else if first.starts_with('"') {
        ("\"", "\"")
    } else if first.starts_with('\'') {
        ("'", "'")
    } else if first.is_empty() {
        return Err(line_error(
            line,
            "shell: has no body — a Snakemake shell directive must be a quoted string",
        ));
    } else {
        return Err(line_error(
            line,
            format!("shell: body must be a quoted string, found `{first}`"),
        ));
    };

    if delim.len() == 1 {
        let rest = &first[prefix.len()..];
        let Some(end) = find_unescaped(rest, delim.chars().next().unwrap()) else {
            return Err(line_error(
                line,
                "unterminated shell string — use triple quotes for multi-line bodies",
            ));
        };
        let body = rest[..end].replace("\\\"", "\"").replace("\\'", "'");
        return Ok((body, start + 1));
    }

    let after = &first[prefix.len()..];
    if let Some(end) = after.find(delim) {
        return Ok((after[..end].to_string(), start + 1));
    }
    let mut body = String::new();
    if !after.trim().is_empty() {
        body.push_str(after);
        body.push('\n');
    }
    let mut j = start + 1;
    while j < lines.len() {
        let trimmed = lines[j].trim();
        if let Some(end) = trimmed.find(delim) {
            body.push_str(&trimmed[..end]);
            return Ok((body.trim().to_string(), j + 1));
        }
        body.push_str(lines[j]);
        body.push('\n');
        j += 1;
    }
    Err(line_error(
        line,
        "unterminated triple-quoted shell body — the importer cannot find its end",
    ))
}

fn find_unescaped(text: &str, needle: char) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == needle {
            return Some(idx);
        }
    }
    None
}

/// Emit the generated workflow and translate shell placeholders.
fn emit_workflow(
    rules: &[ParsedRule],
    wildcard_constraints: &BTreeMap<String, String>,
) -> Result<(String, Vec<String>)> {
    let mut warnings = Vec::new();
    let active: Vec<&ParsedRule> = rules.iter().filter(|rule| !rule.sink).collect();
    if active.is_empty() {
        return Err(config_error(
            "the source contains only sink rules — there is no command to translate",
        ));
    }

    let mut toml = String::new();
    toml.push_str("[workflow]\n");
    let wf_name = active
        .first()
        .map(|rule| rule.name.to_lowercase())
        .unwrap_or_else(|| "imported_pipeline".into());
    toml.push_str(&format!(
        "name = \"{}_pipeline\"\n",
        escape_toml_basic(&wf_name)
    ));
    toml.push_str("version = \"0.1.0\"\n\n");
    toml.push_str("[defaults]\nthreads = 1\n\n");
    if !wildcard_constraints.is_empty() {
        toml.push_str("[wildcard_constraints]\n");
        for (name, regex) in wildcard_constraints {
            toml.push_str(&format!(
                "\"{}\" = \"{}\"\n",
                escape_toml_basic(name),
                escape_toml_basic(regex)
            ));
        }
        toml.push('\n');
    }

    let inferred: Vec<(Option<&'static str>, Option<&'static str>)> = active
        .iter()
        .map(|rule| {
            let inputs = literal_paths(&rule.inputs);
            let outputs = literal_paths(&rule.outputs);
            infer_bio_types(&rule.name, &inputs, &outputs)
        })
        .collect();
    let mut used_types = BTreeSet::new();
    for (in_type, out_type) in &inferred {
        used_types.extend(in_type.iter().chain(out_type.iter()).copied());
    }
    if used_types.contains("fastq") {
        toml.push_str("[types.fastq]\nformat = \"fastq\"\ncompression = \"gzip\"\n\n");
    }
    if used_types.contains("bam_coord") {
        toml.push_str(
            "[types.bam_coord]\nformat = \"bam\"\nsort_order = \"coordinate\"\nindex = \"bai\"\n\n",
        );
    }
    if used_types.contains("vcf_indexed") {
        toml.push_str(
            "[types.vcf_indexed]\nformat = \"vcf\"\ncompression = \"bgzip\"\nindex = \"tbi\"\n\n",
        );
    }

    for (index, rule) in active.iter().enumerate() {
        let shell = translate_shell(rule)?;
        toml.push_str("[[rules]]\n");
        toml.push_str(&format!("name = \"{}\"\n", escape_toml_basic(&rule.name)));
        if !rule.depends_on.is_empty() {
            let deps = rule
                .depends_on
                .iter()
                .map(|dep| format!("\"{}\"", escape_toml_basic(dep)))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("depends_on = [{deps}]\n"));
        }

        let mut res_fields = Vec::new();
        if let Some(threads) = rule.threads {
            res_fields.push(format!("threads = {threads}"));
        }
        if let Some(memory) = &rule.memory {
            res_fields.push(format!(
                "memory = \"{}\"",
                escape_toml_basic(&normalize_memory(memory))
            ));
        }
        if let Some(time) = &rule.time {
            res_fields.push(format!("time_limit = \"{}\"", escape_toml_basic(time)));
        }
        if let Some(gpu) = rule.gpu {
            res_fields.push(format!("gpu = {gpu}"));
        }
        if let Some(disk) = &rule.disk {
            res_fields.push(format!("disk = \"{}\"", escape_toml_basic(disk)));
        }
        if !res_fields.is_empty() {
            toml.push_str(&format!("resources = {{ {} }}\n", res_fields.join(", ")));
        }

        if let Some(container) = &rule.container {
            if rule.conda.is_some() {
                warnings.push(format!(
                    "rule '{}' declares both container: and conda: — the container wins \
                     (matching Snakemake's runtime choice); the conda fallback is not emitted",
                    rule.name
                ));
            }
            if !rule.modules.is_empty() {
                return Err(line_error(
                    rule.line,
                    format!(
                        "rule '{}' declares both container: and envmodules: — the importer \
                         cannot represent both faithfully; keep one backend",
                        rule.name
                    ),
                ));
            }
            toml.push_str(&format!(
                "environment = {{ docker = \"{}\" }}\n",
                escape_toml_basic(container)
            ));
        } else if let Some(conda) = &rule.conda {
            if !rule.modules.is_empty() {
                return Err(line_error(
                    rule.line,
                    format!(
                        "rule '{}' declares both conda: and envmodules: — keep one environment \
                         backend",
                        rule.name
                    ),
                ));
            }
            toml.push_str(&format!(
                "environment = {{ conda = \"{}\" }}\n",
                escape_toml_basic(conda)
            ));
        } else if !rule.modules.is_empty() {
            let modules = rule
                .modules
                .iter()
                .map(|module| format!("\"{}\"", escape_toml_basic(module)))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("environment = {{ modules = [{modules}] }}\n"));
        }

        let envvars: Vec<(String, String)> = rule
            .envvars
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !envvars.is_empty() {
            let entries = envvars
                .iter()
                .map(|(key, value)| {
                    format!(
                        "\"{}\" = \"{}\"",
                        escape_toml_basic(key),
                        escape_toml_basic(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("envvars = {{ {entries} }}\n"));
        }

        if let Some(log) = &rule.log {
            toml.push_str(&format!("log = \"{}\"\n", escape_toml_basic(log)));
        }
        if let Some(benchmark) = &rule.benchmark {
            toml.push_str(&format!(
                "benchmark = \"{}\"\n",
                escape_toml_basic(benchmark)
            ));
        }
        if rule.priority != 0 {
            toml.push_str(&format!("priority = {}\n", rule.priority));
        }
        if let Some(retries) = rule.retries {
            toml.push_str(&format!("retries = {retries}\n"));
        }
        if rule.localrule {
            toml.push_str("localrule = true\n");
        }
        if rule.temporary {
            toml.push_str("temporary = true\n");
        }
        if !rule.temp_outputs.is_empty() {
            toml.push_str(&format!(
                "temp_output = [{}]\n",
                quote_list(&rule.temp_outputs)
            ));
        }
        if !rule.protected_outputs.is_empty() {
            toml.push_str(&format!(
                "protected_output = [{}]\n",
                quote_list(&rule.protected_outputs)
            ));
        }
        if !rule.ancient.is_empty() {
            toml.push_str(&format!("ancient = [{}]\n", quote_list(&rule.ancient)));
        }

        let inputs = literal_paths(&rule.inputs);
        if !inputs.is_empty() {
            toml.push_str(&format!("input = [{}]\n", quote_list(&inputs)));
        }
        let outputs = literal_paths(&rule.outputs);
        if !outputs.is_empty() {
            toml.push_str(&format!("output = [{}]\n", quote_list(&outputs)));
        }
        toml.push_str(&render_shell(&shell));

        let (in_type, out_type) = inferred[index];
        push_types_contract(
            &mut toml,
            in_type.and_then(|ty| inputs.first().map(|channel| (channel.as_str(), ty))),
            out_type.and_then(|ty| outputs.first().map(|channel| (channel.as_str(), ty))),
        );
        toml.push('\n');
    }

    Ok((toml, warnings))
}

fn literal_paths(paths: &[DeclaredPath]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| match &path.source {
            PathSource::Literal(value) => Some(value.clone()),
            PathSource::RuleOutput(_) => None,
        })
        .collect()
}

fn quote_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{}\"", escape_toml_basic(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Translate Snakemake shell placeholders into engine placeholders.
///
/// Only recognized Snakemake namespaces are touched; bash `${VAR}`, brace
/// expansion, and awk `{print ...}` text pass through unchanged.
fn translate_shell(rule: &ParsedRule) -> Result<String> {
    let input_names: BTreeMap<&str, usize> = rule
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(index, path)| path.name.as_deref().map(|name| (name, index)))
        .collect();
    let output_names: BTreeMap<&str, usize> = rule
        .outputs
        .iter()
        .enumerate()
        .filter_map(|(index, path)| path.name.as_deref().map(|name| (name, index)))
        .collect();

    let mut out = String::with_capacity(rule.shell.len());
    let mut rest = rule.shell.as_str();
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let token = &after[..close];
        match translate_token(token, &input_names, &output_names, rule)? {
            Some(replacement) => out.push_str(&replacement),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn translate_token(
    token: &str,
    input_names: &BTreeMap<&str, usize>,
    output_names: &BTreeMap<&str, usize>,
    rule: &ParsedRule,
) -> Result<Option<String>> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let (head, tail) = token
        .split_once('.')
        .map_or((token, None), |(head, tail)| (head, Some(tail)));
    match head {
        "input" | "output" => {
            let names = if head == "input" {
                input_names
            } else {
                output_names
            };
            let Some(tail) = tail else {
                return Ok(Some(format!("{{{head}}}")));
            };
            if tail.chars().all(|c| c.is_ascii_digit()) {
                return Ok(Some(format!("{{{head}[{tail}]}}")));
            }
            let Some(index) = names.get(tail) else {
                return Err(line_error(
                    rule.line,
                    format!(
                        "rule '{}' shell references `{{{head}.{tail}}}`, but no {head} is named \
                         '{tail}' — name the {head} explicitly or use positional `{{{head}}}`",
                        rule.name
                    ),
                ));
            };
            // `{input}`/`{output}` expand to ALL entries (space-joined), so
            // a named reference must use the indexed form whenever the rule
            // declares more than one entry. Using `{input}` for index 0 in a
            // multi-input rule duplicated every other input in the command.
            let total = if head == "input" {
                rule.inputs.len()
            } else {
                rule.outputs.len()
            };
            Ok(Some(if total == 1 {
                format!("{{{head}}}")
            } else {
                format!("{{{head}[{index}]}}")
            }))
        }
        "wildcards" => match tail {
            Some(name) if !name.is_empty() => Ok(Some(format!("{{{name}}}"))),
            _ => Err(line_error(
                rule.line,
                format!(
                    "rule '{}' shell references `{{wildcards}}` without a wildcard name — \
                     use `{{wildcards.<name>}}`",
                    rule.name
                ),
            )),
        },
        "threads" if tail.is_none() => Ok(Some("{threads}".into())),
        "log" => {
            if rule.log.is_none() {
                return Err(line_error(
                    rule.line,
                    format!(
                        "rule '{}' shell references `{{log}}`, but the rule declares no log: \
                         path",
                        rule.name
                    ),
                ));
            }
            Ok(Some("{log}".into()))
        }
        "benchmark" => {
            if rule.benchmark.is_none() {
                return Err(line_error(
                    rule.line,
                    format!(
                        "rule '{}' shell references `{{benchmark}}`, but the rule declares no \
                         benchmark: path",
                        rule.name
                    ),
                ));
            }
            Ok(Some("{benchmark}".into()))
        }
        "params" | "resources" | "rule" | "snakemake" | "checkpoints" => Err(line_error(
            rule.line,
            format!(
                "rule '{}' shell references `{{{token}}}`, which depends on Snakemake's \
                     runtime object model — the importer cannot resolve it. Inline the value, \
                     switch to an engine placeholder ({{config.*}}, {{input}}, {{output}}), or \
                     hand-port this rule",
                rule.name
            ),
        )),
        _ => {
            // Unknown brace text belongs to the shell (awk, brace expansion).
            Ok(None)
        }
    }
}

/// Split at the first top-level occurrence of `needle` (outside quotes and
/// nested `()[]{}`). Returns `None` when the needle never occurs at depth 0.
fn split_top_level_once(text: &str, needle: char) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if ch == needle && depth == 0 => {
                return Some((&text[..idx], &text[idx + ch.len_utf8()..]));
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Parse an `envmodules:` list (`"a/1.0", "b/2.0"` or `"a/1.0 b/2.0"`).
fn parse_modules(text: &str) -> Vec<String> {
    unquote(text)
        .split([',', ' '])
        .map(str::trim)
        .map(unquote)
        .filter(|module| !module.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_list(text: &str) -> Vec<String> {
    text.split([',', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '#' => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> &str {
    // A directive whose quoted value sits on the following line arrives with
    // the list separator glued on (`"docker://image:tag",`). Stripping it
    // before the quotes is load-bearing: otherwise the closing quote survives
    // inside the value (`image:tag",`) and the generated workflow fails
    // validation (E016 shell-unsafe `"`). Real-world Snakefiles end list items
    // with a comma, so this is the common shape, not a corner case.
    let value = value.trim().trim_end_matches(',').trim();
    value.trim_matches('"').trim_matches('\'').trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    #[test]
    fn supported_fields_map_exactly() {
        let source = r#"
wildcard_constraints:
    sample="[A-Za-z0-9_]+"

localrules: qc

rule qc:
    input:
        "raw/{sample}.fastq"
    output:
        "qc/{sample}.txt"
    threads: 4
    resources:
        mem_mb=8000,
        runtime="2h",
        gpu=1,
        disk_mb=1024
    log:
        "logs/{sample}.log"
    benchmark:
        "bench/{sample}.tsv"
    priority: 7
    retries: 2
    envvars:
        FOO: "bar"
    envmodules:
        "fastqc/0.12.1"
    shell:
        "fastqc {input} > {output} 2> {log}"
"#;
        let (toml, warnings) =
            transpile_snakemake_with_warnings(source).expect("supported fields must transpile");
        assert!(
            warnings
                .iter()
                .all(|warning| warning.contains("benchmark:")),
            "only the benchmark warning is expected: {warnings:?}"
        );
        let config = WorkflowConfig::parse(&toml).expect("generated TOML must parse");
        assert_eq!(
            config
                .wildcard_constraints
                .get("sample")
                .map(String::as_str),
            Some("[A-Za-z0-9_]+")
        );
        let rule = &config.rules[0];
        assert_eq!(rule.name, "qc");
        assert_eq!(rule.resources.threads, 4);
        assert_eq!(rule.resources.memory.as_deref(), Some("8000M"));
        assert_eq!(rule.resources.time_limit.as_deref(), Some("2h"));
        assert_eq!(rule.resources.gpu, Some(1));
        assert_eq!(rule.resources.disk.as_deref(), Some("1024M"));
        assert_eq!(rule.log.as_deref(), Some("logs/{sample}.log"));
        assert_eq!(rule.benchmark.as_deref(), Some("bench/{sample}.tsv"));
        assert_eq!(rule.priority, 7);
        assert_eq!(rule.retries, 2);
        assert!(rule.localrule);
        assert_eq!(rule.envvars.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(rule.environment.modules, vec!["fastqc/0.12.1".to_string()]);
    }

    #[test]
    fn unsupported_constructs_are_named_hard_errors() {
        let cases = [
            (
                "rule x:\n    input:\n        \"a/{s}.txt\"\n    params:\n        p=1\n    output:\n        \"b/{s}.txt\"\n    shell:\n        \"cp {input} {output}\"\n",
                "params:",
            ),
            (
                "rule x:\n    input:\n        \"a/{s}.txt\"\n    output:\n        \"b/{s}.txt\"\n    run:\n        pass\n",
                "run:",
            ),
            (
                "rule x:\n    input:\n        \"a/{s}.txt\"\n    output:\n        \"b/{s}.txt\"\n    script:\n        \"scripts/x.py\"\n",
                "script:",
            ),
            ("include: \"rules/other.smk\"\n", "include:"),
            (
                "rule x:\n    input:\n        \"a/{s}.txt\"\n    output:\n        directory(\"out\")\n    shell:\n        \"cp {input} out\"\n",
                "directory",
            ),
            (
                "rule x:\n    input:\n        lambda w: f\"a/{w.s}.txt\"\n    output:\n        \"b/{s}.txt\"\n    shell:\n        \"cp {input} {output}\"\n",
                "lambda",
            ),
            (
                "rule x:\n    input:\n        \"a/{s}.txt\"\n    output:\n        \"b/{s}.txt\"\n    shadow: \"minimal\"\n    shell:\n        \"cp {input} {output}\"\n",
                "shadow",
            ),
        ];
        for (source, marker) in cases {
            let error = transpile_snakemake(source)
                .expect_err("unsupported construct must fail the import");
            let message = error.to_string();
            assert!(
                message.contains(marker),
                "error must name `{marker}`: {message}"
            );
            assert!(
                message.contains("line "),
                "error must carry a line: {message}"
            );
        }
    }

    #[test]
    fn rules_output_reference_resolves_to_the_path_and_dependency() {
        let source = r#"
rule upstream:
    output:
        "out/{sample}.bam"
    shell:
        "make {output}"

rule downstream:
    input:
        rules.upstream.output
    output:
        "out/{sample}.bai"
    shell:
        "index {input} {output}"
"#;
        let toml = transpile_snakemake(source).expect("rules.output must resolve");
        let config = WorkflowConfig::parse(&toml).expect("valid workflow");
        assert_eq!(config.rules[1].depends_on, vec!["upstream".to_string()]);
        assert_eq!(
            config.rules[1].input,
            vec!["out/{sample}.bam".to_string()].into()
        );
    }

    #[test]
    fn named_placeholders_map_and_unknown_names_are_errors() {
        let source = r#"
rule pair:
    input:
        r1="raw/{sample}_1.fastq",
        r2="raw/{sample}_2.fastq"
    output:
        bam="out/{sample}.bam"
    shell:
        "tool {input.r1} {input.r2} {output.bam}"
"#;
        let toml = transpile_snakemake(source).expect("named I/O must transpile");
        assert!(
            toml.contains("tool {input[0]} {input[1]} {output}"),
            "multi-input named refs must use indexed placeholders: {toml}"
        );

        let bad = source.replace("{input.r1}", "{input.nope}");
        let error = transpile_snakemake(&bad).expect_err("unknown named input must fail");
        assert!(error.to_string().contains("input.nope"), "{error}");
    }

    #[test]
    fn params_and_resources_placeholders_are_hard_errors() {
        for token in [
            "{params.adapter}",
            "{resources.mem_mb}",
            "{rule}",
            "{checkpoints.x}",
        ] {
            let source = format!(
                "rule x:\n    input:\n        \"a/{{sample}}.txt\"\n    output:\n        \"b/{{sample}}.txt\"\n    shell:\n        \"tool {token} {{input}} {{output}}\"\n"
            );
            let error =
                transpile_snakemake(&source).expect_err("runtime-object placeholder must fail");
            assert!(
                error.to_string().contains(token),
                "error must name `{token}`: {error}"
            );
        }
    }

    #[test]
    fn comments_and_blank_lines_do_not_become_paths() {
        let source = r#"
rule x:
    input:
        # a comment.with.dots
        "data/{sample}.txt"
    output:
        "out/{sample}.txt"
    shell:
        "cp {input} {output}"
"#;
        let (rules, _, _) = parse_snakemake(source).expect("comments are not paths");
        assert_eq!(rules[0].inputs.len(), 1);
    }

    #[test]
    fn duplicate_rule_names_are_rejected() {
        let source = r#"
rule x:
    output:
        "a.txt"
    shell:
        "touch {output}"

rule x:
    output:
        "b.txt"
    shell:
        "touch {output}"
"#;
        let error = transpile_snakemake(source).expect_err("duplicate names must fail");
        assert!(error.to_string().contains("duplicate"), "{error}");
    }

    #[test]
    fn expand_with_explicit_values_keeps_the_pattern_and_warns() {
        let source = r#"
rule quantify:
    input:
        expand("raw/{sample}.fastq", sample=["A", "B"])
    output:
        "quant/{sample}.sf"
    shell:
        "salmon quant -r {input} -o quant/{sample}"
"#;
        let (toml, warnings) =
            transpile_snakemake_with_warnings(source).expect("expand must transpile");
        assert!(
            toml.contains(r#"input = ["raw/{sample}.fastq"]"#),
            "expand must yield the wildcard pattern: {toml}"
        );
        assert!(
            !toml.contains(r#"["A""#) && !toml.contains(r#""])"#),
            "expand keyword arguments must never leak into a path: {toml}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("explicit value list is not materialized")),
            "the un-materialized value list must be warned about: {warnings:?}"
        );
        let config = WorkflowConfig::parse(&toml).expect("valid workflow");
        assert_eq!(
            config.rules[0].input,
            vec!["raw/{sample}.fastq".to_string()].into()
        );
        assert_eq!(
            config.rules[0].output,
            vec!["quant/{sample}.sf".to_string()].into()
        );
    }

    #[test]
    fn top_level_equals_split_ignores_nested_arguments() {
        assert_eq!(
            split_top_level_once(r#"expand("a/{x}.txt", x=["A", "B"])"#, '='),
            None
        );
        assert_eq!(
            split_top_level_once(r#"r1="a/{x}_1.txt""#, '='),
            Some((r#"r1"#, r#""a/{x}_1.txt""#))
        );
    }

    #[test]
    fn multiline_directive_values_drop_the_list_separator() {
        // Real-world shape (troycomi/snakemake-training 2_containers/
        // Snakefile): the quoted value is on the line after the directive and
        // ends with a list comma. Before the fix the quote survived inside the
        // value and the emitted workflow failed validation with E016.
        let source = r#"
rule index_reference:
    input:
        "GENOME_PATH/genome.fna"
    output:
        "GENOME_PATH/genome.fna.fai"
    log:
        "logs/index_reference.log",
    container:
        "docker://biocontainers/samtools:v1.3_cv3",
    shell:
        "samtools faidx {input}"
"#;
        let toml = transpile_snakemake(source).expect("multi-line directives must transpile");
        let config = WorkflowConfig::parse(&toml).expect("valid workflow");
        let rule = &config.rules[0];
        assert_eq!(
            rule.environment.docker.as_deref(),
            Some("biocontainers/samtools:v1.3_cv3"),
            "container spec must not keep the trailing comma/quote: {toml}"
        );
        assert_eq!(
            rule.log.as_deref(),
            Some("logs/index_reference.log"),
            "log path must not keep the trailing comma/quote: {toml}"
        );
    }

    #[test]
    fn unquoted_declaration_is_a_hard_error() {
        let source = r#"
rule x:
    input:
        data/{sample}.txt
    output:
        "out/{sample}.txt"
    shell:
        "cp {input} {output}"
"#;
        let error = transpile_snakemake(source).expect_err("unquoted path must fail");
        assert!(
            error
                .to_string()
                .contains("cannot parse input/output declaration"),
            "{error}"
        );
    }

    #[test]
    fn expand_fan_in_is_a_hard_error_with_actionable_guidance() {
        let source = r#"
rule report:
    input:
        expand("quant/{sample}.sf", sample=["A", "B"])
    output:
        "results/report.txt"
    shell:
        "cat {input} > {output}"
"#;
        let error = transpile_snakemake(source).expect_err("fan-in expand must fail");
        let message = error.to_string();
        assert!(message.contains("fan-in"), "{message}");
        assert!(message.contains("expand_inputs"), "{message}");
        assert!(
            message.contains("line 6"),
            "the error must point at the constant output declaration: {message}"
        );
    }

    #[test]
    fn rules_output_fan_in_is_a_hard_error() {
        let source = r#"
rule quantify:
    input:
        "raw/{sample}.fastq"
    output:
        "quant/{sample}.sf"
    shell:
        "salmon -r {input} -o quant/{sample}"

rule report:
    input:
        rules.quantify.output
    output:
        "results/report.txt"
    shell:
        "cat {input} > {output}"
"#;
        let error = transpile_snakemake(source).expect_err("fan-in rules.output must fail");
        assert!(error.to_string().contains("fan-in"), "{error}");
    }

    #[test]
    fn mixed_wildcard_and_constant_outputs_are_rejected() {
        let source = r#"
rule mixed:
    input:
        "raw/{sample}.fastq"
    output:
        "out/{sample}.bam",
        "reports/summary.txt"
    shell:
        "align {input} > {output[0]}; echo ok > {output[1]}"
"#;
        let error = transpile_snakemake(source).expect_err("mixed outputs must fail");
        let message = error.to_string();
        assert!(message.contains("mixed wildcard/constant"), "{message}");
        assert!(message.contains("reports/summary.txt"), "{message}");
        assert!(message.contains("line 7"), "{message}");
        assert!(message.contains("expand_inputs"), "{message}");
    }

    #[test]
    fn constant_outputs_without_wildcard_inputs_are_allowed() {
        let source = r#"
rule single:
    input:
        "data/static.txt"
    output:
        "results/summary.txt"
    shell:
        "wc -l {input} > {output}"
"#;
        let toml = transpile_snakemake(source).expect("single-instance constant output");
        let config = WorkflowConfig::parse(&toml).expect("valid workflow");
        assert_eq!(config.rules.len(), 1);
        assert_eq!(
            config.rules[0].output,
            vec!["results/summary.txt".to_string()].into()
        );
    }
}
