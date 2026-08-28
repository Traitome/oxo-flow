//! Config-change impact analysis: precise checkpoint invalidation (issue
//! #62, refined by issue #198).
//!
//! When a workflow is re-run with changed config values, only the rules that
//! reference the changed keys — plus their DAG downstream — are invalidated.
//! Rules whose structure (shell, inputs, …) changed are detected the same way
//! via per-rule fingerprints. Everything else keeps hitting the checkpoint.
//!
//! Since #198 the reference channels are weighted differently:
//!
//! - A key interpolated into shell/script/IO/envvars/params (`{config.<key>}`)
//!   always invalidates — the changed value bakes into what runs.
//! - A key referenced only inside a `when` condition invalidates ONLY when
//!   the gate's truth value flips under the new config. A gate that stays
//!   true (or false) leaves completed outputs valid, so flag toggles no
//!   longer re-run hours-long chains whose inputs and commands are identical.
//!
//! Two complementary mechanisms, with a strict correctness invariant: the
//! invalidation direction is monotone-safe. Over-invalidation wastes compute;
//! under-invalidation silently reuses stale outputs, which is the bug this
//! module exists to prevent.

use crate::config::ReferenceDef;
use crate::dag::WorkflowDag;
use crate::executor::checkpoint::CheckpointState;
use crate::executor::process::evaluate_condition_with_wildcards_and_base_dir;
use crate::rule::{FilePatterns, Rule};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// Engine-injected config keys that churn on every run (rewritten by
/// `--samples`/`--sample`, sample discovery, and pair consolidation).
/// Excluded from the change-triggering diff to avoid spurious invalidation
/// storms: they only affect rule-set membership, which is self-healing, and
/// any real effect on a rule's baked inputs is caught by the rule
/// fingerprint.
pub fn is_engine_injected_key(key: &str) -> bool {
    key == "samples_list" || key == "pairs_list" || key.starts_with("samples_")
}

/// Canonical string form of a config value.
///
/// Must match the expansion semantics used at execution time
/// (`wildcard_values` construction in the CLI: `String(s) → s`, other values
/// via `to_string()`), so that comparison semantics and expansion semantics
/// agree.
pub fn config_value_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Canonical snapshot form of a config value.
///
/// Sensitive values (declared via `config_meta`) are stored as a SHA-256
/// digest so secrets never land in `checkpoint.json` in plaintext. Change
/// detection still works: both sides are hashed before comparison.
pub fn snapshot_value(value: &toml::Value, sensitive: bool) -> String {
    let canonical = config_value_string(value);
    if sensitive {
        format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
    } else {
        canonical
    }
}

/// Maps each config key to the rules that reference it, split by channel.
///
/// The two channels have very different invalidation semantics (issue #198):
///
/// - **Interpolation** (`{config.<key>}` inside shell/script/IO/envvars/
///   params): the changed value bakes into the command or paths — the rule
///   MUST re-run.
/// - **`when` gate** (bare `config.<key>` inside the condition text): the
///   key only steers WHETHER the rule runs. If the gate's truth value is
///   unchanged under the new config and the inputs are untouched, the
///   completed output is still exactly what the new run would produce — it
///   can be reused from the checkpoint instead of invalidated.
pub struct ConfigReferenceGraph {
    /// Key → rules interpolating `{config.<key>}` at execution time
    /// (shell/script/IO/envvars/params).
    interp_key_rules: HashMap<String, HashSet<String>>,
    /// Key → rules mentioning bare `config.<key>` inside `when`.
    when_key_rules: HashMap<String, HashSet<String>>,
}

impl ConfigReferenceGraph {
    /// Build the reference graph by scanning the post-expansion rules.
    ///
    /// Expansion-time channels (`transform.split`, `scatter`, `expand_inputs`)
    /// are intentionally NOT scanned: their values bake into concrete rule
    /// strings during expansion, so a change to them changes the expanded
    /// rule (input list, shell) and is caught by the rule fingerprint.
    pub fn from_rules(rules: &[Rule]) -> Self {
        // `{config.<key>}` — execution-time channels (shell/script/IO/…).
        let braced = regex::Regex::new(r"\{config\.([^}]+)\}").expect("valid braced regex");
        // Bare `config.<key>` — `when` conditions (evaluator syntax, no braces).
        let bare = regex::Regex::new(r"config\.([A-Za-z0-9_.-]+)").expect("valid bare regex");

        let mut interp_key_rules: HashMap<String, HashSet<String>> = HashMap::new();
        let mut when_key_rules: HashMap<String, HashSet<String>> = HashMap::new();
        for rule in rules {
            for text in braced_texts(rule) {
                for cap in braced.captures_iter(&text) {
                    interp_key_rules
                        .entry(cap[1].to_string())
                        .or_default()
                        .insert(rule.name.clone());
                }
            }
            if let Some(ref when) = rule.when {
                for cap in bare.captures_iter(when) {
                    when_key_rules
                        .entry(cap[1].to_string())
                        .or_default()
                        .insert(rule.name.clone());
                }
            }
        }
        Self {
            interp_key_rules,
            when_key_rules,
        }
    }

    /// All rules interpolating any of the given keys into their execution
    /// surface (conservative invalidation: the value changed what runs).
    pub fn interpolating_rules_referencing<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> HashSet<String> {
        let mut rules = HashSet::new();
        for key in keys {
            if let Some(referencing) = self.interp_key_rules.get(key) {
                rules.extend(referencing.iter().cloned());
            }
        }
        rules
    }

    /// All rules whose `when` condition mentions any of the given keys.
    pub fn when_rules_referencing<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> HashSet<String> {
        let mut rules = HashSet::new();
        for key in keys {
            if let Some(referencing) = self.when_key_rules.get(key) {
                rules.extend(referencing.iter().cloned());
            }
        }
        rules
    }

    /// Returns `true` if no rule references any config key.
    pub fn is_empty(&self) -> bool {
        self.interp_key_rules.is_empty() && self.when_key_rules.is_empty()
    }
}

/// Collect the rule strings whose `{config.<key>}` placeholders are expanded
/// at execution time (via the `wildcard_values` map in the CLI / executor).
fn braced_texts(rule: &Rule) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    if let Some(ref shell) = rule.shell {
        texts.push(shell.clone());
    }
    if let Some(ref script) = rule.script {
        texts.push(script.clone());
    }
    texts.extend(rule.input.to_vec());
    texts.extend(rule.output.to_vec());
    // Dir patterns: the glob itself may reference config values too.
    for patterns in [&rule.input, &rule.output] {
        if let FilePatterns::Dir {
            path,
            pattern: Some(pattern),
        } = patterns
        {
            texts.push(path.clone());
            texts.push(pattern.clone());
        }
    }
    for value in rule.envvars.values() {
        texts.push(value.clone());
    }
    // Params values expand transitively: `{params.<key>}` is replaced first,
    // then the wildcard loop expands any `{config.<key>}` inside the value.
    for value in rule.params.values() {
        texts.push(config_value_string(value));
    }
    texts
}

/// Canonical form of a `FilePatterns` value: list order is semantic, map keys
/// are sorted (HashMap order is not deterministic).
fn canonical_file_patterns(patterns: &FilePatterns) -> String {
    match patterns {
        FilePatterns::List(values) => values.join("\u{1}"),
        FilePatterns::Map(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            sorted
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("\u{1}")
        }
        FilePatterns::Dir { path, pattern } => {
            format!("{path}|{}", pattern.as_deref().unwrap_or(""))
        }
    }
}

/// Canonical form of a `HashMap<String, String>` with sorted keys.
fn canonical_string_map(map: &HashMap<String, String>) -> String {
    let sorted: BTreeMap<_, _> = map.iter().collect();
    sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// Canonical form of a `HashMap<String, toml::Value>` with sorted keys.
fn canonical_toml_map(map: &HashMap<String, toml::Value>) -> String {
    let sorted: BTreeMap<_, _> = map.iter().collect();
    sorted
        .iter()
        .map(|(k, v)| format!("{k}={}", config_value_string(v)))
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// SHA-256 fingerprint of the fields that determine a rule's output content.
///
/// Deliberately EXCLUDES threads/memory/resources: they are performance
/// knobs, and changing `-j` or resource hints must not invalidate results.
/// Includes `pre_exec`/`on_success`/`on_failure` (they are part of the rule
/// definition and can write outputs or alter state) and the resolved
/// interpreter for script rules.
pub fn rule_fingerprint(
    rule: &Rule,
    interpreter_map: &HashMap<String, String>,
    shell_prelude: Option<&str>,
) -> String {
    rule_fingerprint_impl(rule, interpreter_map, shell_prelude, true)
}

/// Same fingerprint with the `input` field EXCLUDED (issue #142 M1): for an
/// expand_inputs-over-injected-key rule the baked input list IS the
/// `--samples` selection, so the full fingerprint differs on every subset
/// run while this one stays identical. A match proves the rule definition —
/// shell, outputs, env, conditions, everything else — is unchanged.
pub fn rule_fingerprint_without_input(
    rule: &Rule,
    interpreter_map: &HashMap<String, String>,
    shell_prelude: Option<&str>,
) -> String {
    rule_fingerprint_impl(rule, interpreter_map, shell_prelude, false)
}

fn rule_fingerprint_impl(
    rule: &Rule,
    interpreter_map: &HashMap<String, String>,
    shell_prelude: Option<&str>,
    include_input: bool,
) -> String {
    let mut hasher = Sha256::new();
    // Field name + value pairs separated by NUL bytes: unambiguous framing,
    // deterministic ordering (maps sorted by key, lists in semantic order).
    let mut add = |label: &str, content: &str| {
        hasher.update(label.as_bytes());
        hasher.update([0u8]);
        hasher.update(content.as_bytes());
        hasher.update([0u8]);
    };

    add("name", &rule.name);
    // The workflow shell prelude changes every rule's execution semantics
    // (issue #92): enabling or editing it must invalidate completed rules —
    // their outputs were produced under different shell strictness.
    add("shell_prelude", shell_prelude.unwrap_or(""));
    add("shell", rule.shell.as_deref().unwrap_or(""));
    add("script", rule.script.as_deref().unwrap_or(""));
    if include_input {
        add("input", &canonical_file_patterns(&rule.input));
    }
    add("output", &canonical_file_patterns(&rule.output));
    add("envvars", &canonical_string_map(&rule.envvars));
    add("params", &canonical_toml_map(&rule.params));
    add("when", rule.when.as_deref().unwrap_or(""));
    add("pre_exec", rule.pre_exec.as_deref().unwrap_or(""));
    add("on_success", rule.on_success.as_deref().unwrap_or(""));
    add("on_failure", rule.on_failure.as_deref().unwrap_or(""));
    add(
        "input_function",
        rule.input_function.as_deref().unwrap_or(""),
    );
    add("env_group", rule.env_group.as_deref().unwrap_or(""));

    // Resolved interpreter mirrors build_execution_command: the override
    // wins (if valid), otherwise the extension is looked up in the map.
    let resolved_interpreter = rule.script.as_ref().and_then(|script_path| {
        let base = script_path.split_whitespace().next().unwrap_or(script_path);
        crate::executor::process::detect_interpreter(
            base,
            rule.interpreter.as_deref(),
            interpreter_map,
        )
    });
    add("interpreter", resolved_interpreter.as_deref().unwrap_or(""));

    // EnvironmentSpec is a fixed-field struct (no maps) — toml serialization
    // is deterministic. Environment changes alter tool versions and results.
    let env = toml::to_string(&rule.environment).unwrap_or_default();
    add("environment", &env);

    format!("sha256:{:x}", hasher.finalize())
}

/// SHA-256 fingerprint of a reference definition plus the config values its
/// build command references.
///
/// A mismatch (build/source/output edited, or a referenced config key
/// changed) means the artifact must be rebuilt.
///
/// `resolved_source` is the source path with `{config.*}` already expanded
/// (the caller resolves it against the workdir). When it names a readable
/// local file, the file's size, mtime, and — below
/// [`crate::executor::checkpoint::MANIFEST_HASH_MAX_BYTES`] — content hash
/// join the fingerprint (issue #97): the source PATH string alone cannot
/// detect a same-path replacement, which left artifacts silently stale
/// (live: a regenerated genome.fa never rebuilt its STAR index). A missing
/// or unreadable source degrades to the path-string-only fingerprint — the
/// same policy as the freshness checks, so a first build in a pre-fetch
/// state never errors out. Note the asymmetry: when the checkpoint already
/// stores a content-bearing fingerprint, a later missing source mismatches
/// and the rebuild fails loudly — the safe direction, never silently
/// serving an artifact whose source state cannot be verified.
pub fn reference_fingerprint(
    def: &ReferenceDef,
    config: &HashMap<String, toml::Value>,
    resolved_source: Option<&Path>,
) -> String {
    let mut hasher = Sha256::new();
    let mut add = |label: &str, content: &str| {
        hasher.update(label.as_bytes());
        hasher.update([0u8]);
        hasher.update(content.as_bytes());
        hasher.update([0u8]);
    };

    add("source", def.source.as_deref().unwrap_or(""));
    add("output", &def.output);
    add("build", &def.build);
    // A rebuild under a different environment is a different artifact.
    add("environment", &format!("{:?}", def.environment));

    // Config values referenced anywhere in source/output/build, sorted by key
    // so HashMap iteration order never leaks into the fingerprint.
    let braced = regex::Regex::new(r"\{config\.([^}]+)\}").expect("valid braced regex");
    let mut referenced: BTreeMap<String, String> = BTreeMap::new();
    for text in [def.source.as_deref().unwrap_or(""), &def.output, &def.build] {
        for cap in braced.captures_iter(text) {
            let key = cap[1].to_string();
            if let Some(value) = config.get(&key) {
                referenced.insert(key, config_value_string(value));
            }
        }
    }
    for (key, value) in referenced {
        add(&key, &value);
    }

    // Source content guard (issue #97): size + mtime + content hash, through
    // the SAME helpers as input manifests (issue #72) so the two
    // invalidation layers cannot drift. A directory or unreadable file
    // contributes nothing beyond the path string hashed above.
    if let Some(path) = resolved_source
        && let Ok(md) = std::fs::metadata(path)
        && md.is_file()
    {
        add("source:size", &md.len().to_string());
        add(
            "source:mtime",
            &crate::executor::checkpoint::mtime_nanos(&md).to_string(),
        );
        if let Some(hash) = crate::executor::checkpoint::content_hash_if_small(path, &md) {
            add("source:hash", &hash);
        }
    }

    format!("sha256:{:x}", hasher.finalize())
}

/// Outcome of comparing the checkpoint against the current workflow + config.
#[derive(Debug, Clone, Default)]
pub struct ConfigChangeReport {
    /// The checkpoint predates config tracking: nothing was invalidated and
    /// a snapshot/fingerprints were recorded going forward (one-time window,
    /// documented behavior).
    pub is_legacy: bool,
    /// Keys present in both snapshot and current config with different values.
    pub changed_keys: Vec<String>,
    /// Keys present only in the current config (previously expanded to the
    /// literal placeholder — outputs differ, so referencing rules re-run).
    pub added_keys: Vec<String>,
    /// Keys present only in the snapshot (expansion now produces the literal
    /// placeholder — outputs differ, so referencing rules re-run).
    pub removed_keys: Vec<String>,
    /// Rules whose stored structural fingerprint differs from the current one.
    pub fingerprint_mismatches: Vec<String>,
    /// Completed rules whose fingerprint differed ONLY in the sample-derived
    /// input list (`expand_inputs` over an engine-injected key, issue #142
    /// M1): NOT invalidated — toggling `--samples` must not re-run gather
    /// rules. The input-manifest check still re-verifies set + content, so a
    /// genuine input edit invalidates there.
    pub sample_selection_exempt: Vec<String>,
    /// Rules directly affected (reference a changed key or mismatch).
    pub directly_affected: Vec<String>,
    /// Completed rules whose `when` gate references changed keys but whose
    /// verdict is unchanged under the new config — NOT invalidated; their
    /// checkpoint entry is reused (issue #198). Safe because interpolation
    /// channels invalidate separately and the input-manifest check still
    /// verifies file sets independently.
    pub when_gate_exempt: Vec<String>,
    /// Rules invalidated because their `when` gate's truth value flipped
    /// between the recorded run and the current config, in either direction
    /// (issue #198): a flipped producer changes what its consumers see.
    pub when_flip_invalidated: Vec<String>,
    /// Full invalidation set: directly affected rules plus their transitive
    /// DAG dependents. These were removed from `completed_rules`.
    pub invalidated: Vec<String>,
}

/// Compare the checkpoint against the current workflow and config, mutate the
/// checkpoint accordingly, and report what changed.
///
/// Side effects on `checkpoint`:
/// - rules in the invalidation set are removed from `completed_rules`;
/// - `config_snapshot` is rewritten from `current` (engine-injected keys
///   excluded, sensitive keys hashed);
/// - `rule_fingerprints` is updated for every rule in `rules` (bootstrap for
///   legacy checkpoints and rules entering a run for the first time).
#[allow(clippy::too_many_arguments)]
pub fn detect_config_changes(
    checkpoint: &mut CheckpointState,
    rules: &[Rule],
    dag: &WorkflowDag,
    current: &HashMap<String, toml::Value>,
    sensitive_keys: &HashSet<String>,
    interpreter_map: &HashMap<String, String>,
    shell_prelude: Option<&str>,
    base_dir: Option<&Path>,
) -> ConfigChangeReport {
    // A checkpoint with neither snapshot nor fingerprints predates config
    // tracking: bootstrap only (no invalidation) — documented one-time window.
    let is_legacy =
        checkpoint.config_snapshot.is_empty() && checkpoint.rule_fingerprints.is_empty();

    // ── 1. Config key diff (engine-injected keys excluded) ───────────────
    let mut changed_keys: Vec<String> = Vec::new();
    let mut added_keys: Vec<String> = Vec::new();
    let mut removed_keys: Vec<String> = Vec::new();
    let mut all_diff_keys: Vec<String> = Vec::new();
    for (key, current_value) in current {
        if is_engine_injected_key(key) {
            continue;
        }
        let current_str = snapshot_value(current_value, sensitive_keys.contains(key));
        match checkpoint.config_snapshot.get(key) {
            None => {
                added_keys.push(key.clone());
                all_diff_keys.push(key.clone());
            }
            Some(old) if *old != current_str => {
                changed_keys.push(key.clone());
                all_diff_keys.push(key.clone());
            }
            _ => {}
        }
    }
    for key in checkpoint.config_snapshot.keys() {
        if !is_engine_injected_key(key) && !current.contains_key(key) {
            removed_keys.push(key.clone());
            all_diff_keys.push(key.clone());
        }
    }
    for list in [&mut changed_keys, &mut added_keys, &mut removed_keys] {
        list.sort();
    }
    all_diff_keys.sort();
    all_diff_keys.dedup();

    // ── 2. Rule fingerprints (all rules in this run) ─────────────────────
    let graph = ConfigReferenceGraph::from_rules(rules);
    let mut current_fingerprints: HashMap<String, String> = HashMap::new();
    let mut current_no_input_fingerprints: HashMap<String, String> = HashMap::new();
    let mut fingerprint_mismatches: Vec<String> = Vec::new();
    let mut sample_selection_exempt: Vec<String> = Vec::new();
    for rule in rules {
        let fingerprint = rule_fingerprint(rule, interpreter_map, shell_prelude);
        if let Some(stored) = checkpoint.rule_fingerprints.get(&rule.name)
            && *stored != fingerprint
            && checkpoint.is_completed(&rule.name)
        {
            // Issue #142 M1: an expand_inputs-over-injected-key rule bakes
            // the --samples selection into its input list, so the full
            // fingerprint differs on every subset run. When the
            // input-excluded fingerprint still matches, the ONLY change is
            // the selection — not an invalidation (the input-manifest check
            // re-verifies set + content later, so a genuine input edit still
            // invalidates there). Checkpoints from older binaries carry no
            // input-excluded fingerprints — those keep invalidating.
            let selection_only = expand_inputs_refs_engine_injected(rule)
                && checkpoint.rule_fingerprints_no_input.get(&rule.name)
                    == Some(&rule_fingerprint_without_input(
                        rule,
                        interpreter_map,
                        shell_prelude,
                    ));
            if selection_only {
                sample_selection_exempt.push(rule.name.clone());
            } else {
                fingerprint_mismatches.push(rule.name.clone());
            }
        }
        current_fingerprints.insert(rule.name.clone(), fingerprint);
        current_no_input_fingerprints.insert(
            rule.name.clone(),
            rule_fingerprint_without_input(rule, interpreter_map, shell_prelude),
        );
    }
    fingerprint_mismatches.sort();
    sample_selection_exempt.sort();

    // ── 3. When-gate verdicts under the CURRENT config (issue #198) ──────
    // The same evaluator the executor uses at execution time, with an empty
    // wildcard context: expansion bakes kept instances' bindings into their
    // `when` as literals, which this evaluator resolves identically.
    let current_verdicts: HashMap<String, bool> = rules
        .iter()
        .filter_map(|rule| {
            rule.when.as_ref().map(|condition| {
                (
                    rule.name.clone(),
                    evaluate_condition_with_wildcards_and_base_dir(
                        condition,
                        current,
                        &HashMap::new(),
                        base_dir,
                    ),
                )
            })
        })
        .collect();

    // ── 4. Bootstrap path: record provenance, keep everything completed ──
    if is_legacy {
        checkpoint.config_snapshot = build_config_snapshot(current, sensitive_keys);
        checkpoint.rule_fingerprints.extend(current_fingerprints);
        checkpoint
            .rule_fingerprints_no_input
            .extend(current_no_input_fingerprints);
        checkpoint.when_verdicts.extend(current_verdicts);
        return ConfigChangeReport {
            is_legacy: true,
            ..Default::default()
        };
    }

    // ── 5. Affected rules + DAG downstream closure ───────────────────────
    // Interpolation is the conservative channel: a changed value baked into
    // shell/IO/params changes what runs.
    let mut directly_affected: HashSet<String> =
        graph.interpolating_rules_referencing(all_diff_keys.iter().map(String::as_str));
    directly_affected.extend(fingerprint_mismatches.iter().cloned());

    // `when`-gated references (issue #198): a gate that keeps its truth
    // value between runs leaves completed outputs valid — skip them instead
    // of invalidating whole chains whenever a flag toggles. A flipped gate
    // invalidates in BOTH directions: false→false stays skipped-free reuse,
    // but true→false means the completed output must stop being served and
    // false→true means the producer starts contributing fresh output to its
    // consumers.
    let mut when_flip_invalidated: Vec<String> = Vec::new();
    let mut when_gate_exempt: Vec<String> = Vec::new();
    for rule_name in graph.when_rules_referencing(all_diff_keys.iter().map(String::as_str)) {
        if directly_affected.contains(&rule_name) {
            continue; // interpolation or fingerprint mismatch already handles it
        }
        let Some(current_verdict) = current_verdicts.get(&rule_name) else {
            continue;
        };
        match checkpoint.when_verdicts.get(&rule_name) {
            Some(stored) if stored == current_verdict => {
                // Gate unchanged: a completed entry stays valid (nothing in
                // this channel baked the changed value into its execution);
                // a non-completed rule simply isn't cached — leave it be.
                if checkpoint.is_completed(&rule_name) {
                    when_gate_exempt.push(rule_name);
                }
            }
            Some(_) => {
                when_flip_invalidated.push(rule_name.clone());
                directly_affected.insert(rule_name);
            }
            None => {
                // No recorded verdict (checkpoint predates issue #198 or the
                // rule entered a run for the first time): adopt the same
                // one-time conservative window as fingerprints — invalidate,
                // record, and never again churn on unchanged gates.
                when_flip_invalidated.push(rule_name.clone());
                directly_affected.insert(rule_name);
            }
        }
    }

    let mut invalidated: HashSet<String> = directly_affected.clone();
    let mut frontier: Vec<String> = directly_affected.iter().cloned().collect();
    while let Some(rule_name) = frontier.pop() {
        // Tolerate rules that are not DAG nodes (orphans): dependents() may
        // error; there is simply no downstream edge to follow.
        if let Ok(dependents) = dag.dependents(&rule_name) {
            for dependent in dependents {
                if invalidated.insert(dependent.clone()) {
                    frontier.push(dependent);
                }
            }
        }
    }

    // ── 6. Mutate checkpoint ─────────────────────────────────────────────
    for rule_name in &invalidated {
        checkpoint.completed_rules.remove(rule_name);
    }
    checkpoint.config_snapshot = build_config_snapshot(current, sensitive_keys);
    checkpoint.rule_fingerprints.extend(current_fingerprints);
    checkpoint
        .rule_fingerprints_no_input
        .extend(current_no_input_fingerprints);
    checkpoint.when_verdicts.extend(current_verdicts);

    let mut directly_affected: Vec<String> = directly_affected.into_iter().collect();
    directly_affected.sort();
    when_flip_invalidated.sort();
    when_gate_exempt.sort();
    let mut invalidated: Vec<String> = invalidated.into_iter().collect();
    invalidated.sort();

    ConfigChangeReport {
        is_legacy: false,
        changed_keys,
        added_keys,
        removed_keys,
        fingerprint_mismatches,
        sample_selection_exempt,
        directly_affected,
        when_gate_exempt,
        when_flip_invalidated,
        invalidated,
    }
}

/// Whether any `expand_inputs` pattern of the rule resolves against an
/// engine-injected config key (`samples_list` / `samples_<group>` /
/// `pairs_list`). Only such rules can have a fingerprint mismatch that is
/// purely the `--samples` selection (issue #142 M1).
fn expand_inputs_refs_engine_injected(rule: &Rule) -> bool {
    rule.expand_inputs.iter().any(|exp| {
        exp.variables.values().any(|var_ref| {
            let key = var_ref.strip_prefix("config.").unwrap_or(var_ref);
            is_engine_injected_key(key)
        })
    })
}

/// Build the snapshot map: engine-injected keys excluded, sensitive values
/// stored as SHA-256 digests.
fn build_config_snapshot(
    current: &HashMap<String, toml::Value>,
    sensitive_keys: &HashSet<String>,
) -> HashMap<String, String> {
    current
        .iter()
        .filter(|(key, _)| !is_engine_injected_key(key))
        .map(|(key, value)| {
            (
                key.clone(),
                snapshot_value(value, sensitive_keys.contains(key)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(name: &str, inputs: &[&str], outputs: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            input: FilePatterns::List(inputs.iter().map(|s| s.to_string()).collect()),
            output: FilePatterns::List(outputs.iter().map(|s| s.to_string()).collect()),
            shell: Some(format!("echo {name}")),
            ..Default::default()
        }
    }

    fn diamond_rules() -> Vec<Rule> {
        vec![
            make_rule("a", &[], &["a.out"]),
            make_rule("b", &["a.out"], &["b.out"]),
            make_rule("c", &["a.out"], &["c.out"]),
            make_rule("d", &["b.out", "c.out"], &["d.out"]),
        ]
    }

    fn diamond_dag() -> WorkflowDag {
        WorkflowDag::from_rules(&diamond_rules()).unwrap()
    }

    // ── Reference graph extraction ──────────────────────────────────────

    #[test]
    fn graph_extracts_braced_references_from_shell() {
        let rules = vec![Rule {
            name: "fastp_trim".to_string(),
            shell: Some("fastp -q {config.min_quality} -t {threads}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert_eq!(
            graph.interpolating_rules_referencing(["min_quality"]),
            HashSet::from(["fastp_trim".to_string()])
        );
        // Non-config placeholders ({threads} without the config. prefix)
        // must not be captured.
        assert!(
            graph
                .interpolating_rules_referencing(["threads"])
                .is_empty()
        );
        // `{config.threads}` IS a config reference (key "threads").
        let rules = vec![Rule {
            name: "r".to_string(),
            shell: Some("echo {config.threads}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(
            graph
                .interpolating_rules_referencing(["threads"])
                .contains("r")
        );
    }

    #[test]
    fn graph_extracts_references_from_script_and_io_and_envvars_and_params() {
        let mut params = HashMap::new();
        params.insert("mode".to_string(), toml::Value::String("fast".to_string()));
        params.insert(
            "filter".to_string(),
            toml::Value::String("{config.adapter}".to_string()),
        );
        let mut envvars = HashMap::new();
        envvars.insert("REF".to_string(), "{config.ref_dir}/genome.fa".to_string());

        let rules = vec![Rule {
            name: "align".to_string(),
            script: Some("scripts/run_{config.mode}.sh".to_string()),
            input: FilePatterns::List(vec!["{config.data_dir}/reads.fq".to_string()]),
            output: FilePatterns::List(vec!["{config.out_dir}/aln.bam".to_string()]),
            params,
            envvars,
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        for key in ["mode", "adapter", "ref_dir", "data_dir", "out_dir"] {
            assert!(
                graph
                    .interpolating_rules_referencing([key])
                    .contains("align"),
                "expected key {key} to reference align"
            );
        }
    }

    #[test]
    fn graph_extracts_when_references_without_braces() {
        let rules = vec![Rule {
            name: "qc".to_string(),
            when: Some("config.min_qual >= 20 && config.enable_qc".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(graph.when_rules_referencing(["min_qual"]).contains("qc"));
        assert!(graph.when_rules_referencing(["enable_qc"]).contains("qc"));
    }

    #[test]
    fn graph_tolerates_file_exists_false_positive_as_safe_over_invalidation() {
        // `file_exists("config.yaml")` matches the bare `config.<key>` regex
        // with key "yaml": an adoption-only false positive of the when
        // channel — over-invalidation at worst, never stale reuse.
        let rules = vec![Rule {
            name: "r".to_string(),
            when: Some(r#"file_exists("config.yaml")"#.to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(graph.when_rules_referencing(["yaml"]).contains("r"));
    }

    #[test]
    fn graph_handles_keys_containing_dots() {
        let rules = vec![Rule {
            name: "r".to_string(),
            shell: Some("echo {config.min.qual}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(
            graph
                .interpolating_rules_referencing(["min.qual"])
                .contains("r")
        );
    }

    #[test]
    fn graph_empty_when_no_references() {
        let graph = ConfigReferenceGraph::from_rules(&diamond_rules());
        assert!(graph.is_empty());
    }

    #[test]
    fn graph_dedups_repeated_references() {
        let rules = vec![Rule {
            name: "r".to_string(),
            shell: Some("{config.min_quality} {config.min_quality} {config.other}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert_eq!(
            graph.interpolating_rules_referencing(["min_quality"]),
            HashSet::from(["r".to_string()])
        );
    }

    // ── Value canonicalization ──────────────────────────────────────────

    #[test]
    fn config_value_string_matches_expansion_semantics() {
        assert_eq!(
            config_value_string(&toml::Value::String("20".to_string())),
            "20"
        );
        assert_eq!(config_value_string(&toml::Value::Integer(30)), "30");
        assert_eq!(config_value_string(&toml::Value::Float(1.5)), "1.5");
        assert_eq!(config_value_string(&toml::Value::Boolean(true)), "true");
        let arr = toml::Value::Array(vec![
            toml::Value::String("a".to_string()),
            toml::Value::String("b".to_string()),
        ]);
        assert_eq!(config_value_string(&arr), r#"["a", "b"]"#);
    }

    #[test]
    fn snapshot_value_hashes_sensitive_values() {
        let plain = toml::Value::String("s3cret-token".to_string());
        let stored = snapshot_value(&plain, true);
        assert!(!stored.contains("s3cret-token"));
        assert!(stored.starts_with("sha256:"));
        // Same value → same digest (stable comparison).
        assert_eq!(stored, snapshot_value(&plain, true));
        // Insensitive values stay readable.
        assert_eq!(snapshot_value(&plain, false), "s3cret-token");
    }

    // ── Rule fingerprints ───────────────────────────────────────────────

    #[test]
    fn fingerprint_deterministic_regardless_of_hashmap_order() {
        let mut a = make_rule("r", &[], &["o"]);
        let mut b = make_rule("r", &[], &["o"]);
        for i in 0..8 {
            a.params
                .insert(format!("k{i}"), toml::Value::String(format!("v{i}")));
        }
        for i in (0..8).rev() {
            b.params
                .insert(format!("k{i}"), toml::Value::String(format!("v{i}")));
        }
        let map = HashMap::new();
        assert_eq!(
            rule_fingerprint(&a, &map, None),
            rule_fingerprint(&b, &map, None)
        );
    }

    #[test]
    fn fingerprint_changes_when_shell_changes() {
        let mut a = make_rule("r", &[], &["o"]);
        a.shell = Some("fastp -q 20".to_string());
        let mut b = a.clone();
        b.shell = Some("fastp -q 30".to_string());
        let map = HashMap::new();
        assert_ne!(
            rule_fingerprint(&a, &map, None),
            rule_fingerprint(&b, &map, None)
        );
    }

    #[test]
    #[allow(deprecated)] // shorthand threads/memory fields are the point of this test
    fn fingerprint_ignores_threads_and_memory() {
        let mut a = make_rule("r", &[], &["o"]);
        a.threads = Some(2);
        a.memory = Some("4G".to_string());
        let mut b = a.clone();
        b.threads = Some(16);
        b.memory = Some("64G".to_string());
        let map = HashMap::new();
        assert_eq!(
            rule_fingerprint(&a, &map, None),
            rule_fingerprint(&b, &map, None)
        );
    }

    #[test]
    fn fingerprint_catches_combine_input_list_shrink() {
        // transform split.n 4→3 changes the combine rule's baked input list;
        // the fingerprint must catch it (expansion-time channel backstop).
        let a = make_rule(
            "combine",
            &[
                ".oxo-flow/chunks/0",
                ".oxo-flow/chunks/1",
                ".oxo-flow/chunks/2",
                ".oxo-flow/chunks/3",
            ],
            &["out"],
        );
        let mut b = a.clone();
        b.input = FilePatterns::List(vec![
            ".oxo-flow/chunks/0".to_string(),
            ".oxo-flow/chunks/1".to_string(),
            ".oxo-flow/chunks/2".to_string(),
        ]);
        let map = HashMap::new();
        assert_ne!(
            rule_fingerprint(&a, &map, None),
            rule_fingerprint(&b, &map, None)
        );
    }

    #[test]
    fn fingerprint_includes_hooks_and_interpreter() {
        let mut a = make_rule("r", &[], &["o"]);
        a.pre_exec = Some("mkdir -p tmp".to_string());
        let mut b = a.clone();
        b.on_success = Some("echo done".to_string());
        let mut c = make_rule("r", &[], &["o"]);
        c.script = Some("run.py".to_string());
        c.interpreter = Some("/usr/bin/python3".to_string());
        let mut d = c.clone();
        d.interpreter = Some("/opt/python3.12/bin/python".to_string());

        let map = HashMap::new();
        assert_ne!(
            rule_fingerprint(&a, &map, None),
            rule_fingerprint(&b, &map, None)
        );
        assert_ne!(
            rule_fingerprint(&c, &map, None),
            rule_fingerprint(&d, &map, None)
        );
    }

    // ── Reference fingerprints ───────────────────────────────────────────

    #[test]
    fn reference_fingerprint_changes_on_build_edit_or_config_change() {
        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("genome.fa".to_string()),
            output: "genome.idx".to_string(),
            build: "bwa index -p {config.ref_dir}/idx {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let mut config = HashMap::new();
        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v1".to_string()),
        );
        let f1 = reference_fingerprint(&def, &config, None);

        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v2".to_string()),
        );
        assert_ne!(f1, reference_fingerprint(&def, &config, None));

        let mut edited = def.clone();
        edited.build = "bwa index -p {config.ref_dir}/idx2 {input}".to_string();
        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v2".to_string()),
        );
        assert_ne!(
            reference_fingerprint(&def, &config, None),
            reference_fingerprint(&edited, &config, None)
        );
    }

    #[test]
    fn reference_fingerprint_detects_same_path_source_content_replacement() {
        // issue #97: replacing the source file at the SAME path must change
        // the fingerprint — the path string alone cannot see it (live: STAR
        // index silently stale after genome.fa regeneration, twice).
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("genome.fa");
        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("genome.fa".to_string()),
            output: "genome.idx".to_string(),
            build: "bwa index {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let config = HashMap::new();

        // Same-size content rewrite changes the fingerprint.
        std::fs::write(&source, b">chr1\nAAAA").unwrap();
        let before = reference_fingerprint(&def, &config, Some(&source));
        std::fs::write(&source, b">chr1\nBBBB").unwrap();
        let after = reference_fingerprint(&def, &config, Some(&source));
        assert_ne!(
            before, after,
            "same-path content rewrite must change the fingerprint"
        );
    }

    #[test]
    fn reference_fingerprint_hashes_content_when_size_and_mtime_are_equal() {
        // The content-hash branch is what catches a same-size rewrite that
        // preserves mtime (cp -p, rsync -t, git checkouts) — pin it by
        // forcing identical size AND mtime across a content change.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("genome.fa");
        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("genome.fa".to_string()),
            output: "genome.idx".to_string(),
            build: "bwa index {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let config = HashMap::new();
        let fixed = filetime::FileTime::from_unix_time(1_700_000_000, 0);

        std::fs::write(&source, b">chr1\nAAAA").unwrap();
        filetime::set_file_mtime(&source, fixed).unwrap();
        let before = reference_fingerprint(&def, &config, Some(&source));

        std::fs::write(&source, b">chr1\nBBBB").unwrap();
        filetime::set_file_mtime(&source, fixed).unwrap();
        let after = reference_fingerprint(&def, &config, Some(&source));

        assert_ne!(
            before, after,
            "same size + same mtime + different content must differ (content hash)"
        );
    }

    #[test]
    fn reference_fingerprint_tracks_source_mtime() {
        // Identical content at a different mtime must still change the
        // fingerprint — the size+mtime policy for large files relies on it.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("genome.fa");
        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("genome.fa".to_string()),
            output: "genome.idx".to_string(),
            build: "bwa index {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let config = HashMap::new();

        std::fs::write(&source, b">chr1\nAAAA").unwrap();
        let before = reference_fingerprint(&def, &config, Some(&source));
        let current =
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(&source).unwrap());
        filetime::set_file_mtime(
            &source,
            filetime::FileTime::from_unix_time(current.unix_seconds() + 60, 0),
        )
        .unwrap();
        let after = reference_fingerprint(&def, &config, Some(&source));

        assert_ne!(
            before, after,
            "a source mtime change must change the fingerprint"
        );
    }

    #[test]
    fn reference_fingerprint_skips_content_hash_above_manifest_threshold() {
        // Files above MANIFEST_HASH_MAX_BYTES contribute size+mtime only —
        // fingerprinting stays metadata-cheap for genome-scale sources.
        // Sparse file: set_len is metadata-only, the test stays instant.
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.fa");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(crate::executor::checkpoint::MANIFEST_HASH_MAX_BYTES + 1)
            .unwrap();
        drop(file);

        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("big.fa".to_string()),
            output: "big.idx".to_string(),
            build: "index {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let config = HashMap::new();
        let fp1 = reference_fingerprint(&def, &config, Some(&big));
        let fp2 = reference_fingerprint(&def, &config, Some(&big));
        assert_eq!(fp1, fp2, "unchanged large source must be deterministic");

        let current =
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(&big).unwrap());
        filetime::set_file_mtime(
            &big,
            filetime::FileTime::from_unix_time(current.unix_seconds() + 60, 0),
        )
        .unwrap();
        let fp3 = reference_fingerprint(&def, &config, Some(&big));
        assert_ne!(
            fp1, fp3,
            "large sources still detect mtime changes (size+mtime policy)"
        );
    }

    #[test]
    fn reference_fingerprint_degrades_to_path_only_when_source_unreadable() {
        // A missing (pre-fetch, dry-run) or unreadable source must degrade
        // to the historical path-string-only fingerprint — no error, same
        // shape as `file_is_newer`'s degrade policy.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-there.fa");
        let def = ReferenceDef {
            name: "ref".to_string(),
            source: Some("not-there.fa".to_string()),
            output: "not-there.idx".to_string(),
            build: "index {input}".to_string(),
            threads: None,
            memory: None,
            environment: None,
            description: None,
        };
        let config = HashMap::new();
        assert_eq!(
            reference_fingerprint(&def, &config, Some(&missing)),
            reference_fingerprint(&def, &config, None)
        );
    }

    // ── detect_config_changes ────────────────────────────────────────────

    fn cfg(pairs: &[(&str, &str)]) -> HashMap<String, toml::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), toml::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn changed_key_invalidates_referencing_rule_and_downstream() {
        let mut rules = diamond_rules();
        rules[1].shell = Some("fastp -q {config.min_quality}".to_string()); // b references key

        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        // Prime with a snapshot matching the OLD config.
        let mut cp = checkpoint.clone();
        let _report0 = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "20")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "30")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );

        assert_eq!(report.changed_keys, vec!["min_quality"]);
        assert!(report.directly_affected.contains(&"b".to_string()));
        // b + downstream d invalidated; upstream a and sibling c untouched.
        let invalidated: HashSet<&String> = report.invalidated.iter().collect();
        assert!(invalidated.contains(&"b".to_string()));
        assert!(invalidated.contains(&"d".to_string()));
        assert!(!invalidated.contains(&"a".to_string()));
        assert!(!invalidated.contains(&"c".to_string()));
        // Checkpoint mutation: b and d no longer completed.
        assert!(!checkpoint.is_completed("b"));
        assert!(!checkpoint.is_completed("d"));
        assert!(checkpoint.is_completed("a"));
        assert!(checkpoint.is_completed("c"));
    }

    #[test]
    fn added_and_removed_keys_invalidate() {
        let mut rules = diamond_rules();
        rules[0].shell = Some("echo {config.new_key} {config.gone_key}".to_string());
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        // Prime with a snapshot.
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("gone_key", "x")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("new_key", "y")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(report.added_keys, vec!["new_key"]);
        assert_eq!(report.removed_keys, vec!["gone_key"]);
        assert!(!checkpoint.is_completed("a"));
        assert!(!checkpoint.is_completed("d")); // downstream of a
    }

    #[test]
    fn unchanged_config_invalidates_nothing() {
        let mut rules = diamond_rules();
        rules[1].shell = Some("echo {config.min_quality}".to_string());
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "20")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "20")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.invalidated.is_empty());
        assert!(report.changed_keys.is_empty());
        assert_eq!(checkpoint.completed_rules.len(), 4);
    }

    #[test]
    fn engine_injected_keys_do_not_trigger_invalidation() {
        let rules = diamond_rules();
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("samples_list", "S1,S2")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("samples_list", "S3,S4")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.changed_keys.is_empty());
        assert!(report.invalidated.is_empty());

        // config.pairs_list is engine-injected the same way (rewritten by
        // pair consolidation / --samples filtering): it must not trigger
        // invalidation either.
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("pairs_list", "P1,P2")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("pairs_list", "P2,P3")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.changed_keys.is_empty());
        assert!(report.invalidated.is_empty());
    }

    #[test]
    fn sensitive_keys_stored_hashed_and_changes_detected() {
        let rules = diamond_rules();
        let mut checkpoint = CheckpointState::new();
        let mut sensitive = HashSet::new();
        sensitive.insert("api_token".to_string());
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("api_token", "hunter2")]),
            &sensitive,
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;
        let stored = checkpoint.config_snapshot.get("api_token").unwrap();
        assert!(!stored.contains("hunter2"));
        assert!(stored.starts_with("sha256:"));

        // A changed sensitive value is still detected.
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("api_token", "hunter3")]),
            &sensitive,
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(report.changed_keys, vec!["api_token"]);
    }

    #[test]
    fn fingerprint_mismatch_invalidates_rule_and_downstream() {
        let rules = diamond_rules();
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        // Prime fingerprints for the OLD rule set (diamond without edits).
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        // Edit c's shell.
        let mut edited = rules.clone();
        edited[2].shell = Some("echo changed".to_string());

        let report = detect_config_changes(
            &mut checkpoint,
            &edited,
            &diamond_dag(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.fingerprint_mismatches.contains(&"c".to_string()));
        assert!(!checkpoint.is_completed("c"));
        assert!(!checkpoint.is_completed("d"));
        assert!(checkpoint.is_completed("a"));
        assert!(checkpoint.is_completed("b"));
    }

    #[test]
    fn legacy_checkpoint_bootstraps_without_invalidation() {
        let rules = diamond_rules();
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        // Fresh state → no snapshot, no fingerprints (legacy).
        assert!(checkpoint.config_snapshot.is_empty());
        assert!(checkpoint.rule_fingerprints.is_empty());

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "30")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.is_legacy);
        assert!(report.invalidated.is_empty());
        assert_eq!(checkpoint.completed_rules.len(), 4);
        // Snapshot + fingerprints bootstrapped for all rules.
        assert!(checkpoint.config_snapshot.contains_key("min_quality"));
        assert_eq!(checkpoint.rule_fingerprints.len(), 4);
    }

    #[test]
    fn change_detected_after_legacy_bootstrap() {
        let mut rules = diamond_rules();
        rules[1].shell = Some("echo {config.min_quality}".to_string());
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        let _ = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "20")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "30")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(!report.is_legacy);
        assert!(!checkpoint.is_completed("b"));
        assert!(!checkpoint.is_completed("d"));
    }

    #[test]
    fn completed_rule_without_fingerprint_is_adopted_not_invalidated() {
        // A rule completed before config tracking, entering a run for the
        // first time: adopt its current fingerprint (documented one-time
        // window), do not invalidate.
        let rules = diamond_rules();
        let mut checkpoint = CheckpointState::new();
        for name in ["a", "b", "c", "d"] {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        // Non-legacy checkpoint: snapshot exists, but fingerprints only for a.
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;
        checkpoint.rule_fingerprints.remove("b");

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(report.fingerprint_mismatches.is_empty());
        assert!(report.invalidated.is_empty());
        assert_eq!(checkpoint.completed_rules.len(), 4);
        assert!(checkpoint.rule_fingerprints.contains_key("b"));
    }

    #[test]
    fn affected_rule_not_in_dag_does_not_panic() {
        // Orphan rule (no DAG edges) referencing a changed key: itself is
        // invalidated; dependents lookup must be tolerated.
        let mut orphan = make_rule("orphan", &[], &["orphan.out"]);
        orphan.shell = Some("echo {config.k}".to_string());
        let rules = vec![orphan];
        let dag = WorkflowDag::from_rules(&rules).unwrap();
        let mut checkpoint = CheckpointState::new();
        checkpoint.mark_completed(
            "orphan",
            super::super::executor::checkpoint::BenchmarkRecord {
                rule: "orphan".to_string(),
                wall_time_secs: 1.0,
                max_memory_mb: None,
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
            },
        );
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &dag,
            &cfg(&[("k", "1")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &dag,
            &cfg(&[("k", "2")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(report.invalidated, vec!["orphan".to_string()]);
        assert!(!checkpoint.is_completed("orphan"));
    }

    // ── When-gate verdicts (issue #198) ──────────────────────────────────

    fn completed_checkpoint(names: &[&str]) -> CheckpointState {
        let mut checkpoint = CheckpointState::new();
        for name in names {
            checkpoint.mark_completed(
                name,
                super::super::executor::checkpoint::BenchmarkRecord {
                    rule: name.to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        checkpoint
    }

    fn typed_cfg(pairs: &[(&str, toml::Value)]) -> HashMap<String, toml::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn when_only_gate_unchanged_keeps_completed_rule() {
        // The #198 core case: a rule whose `when` REFERENCES toggled keys but
        // whose gate stays true keeps its checkpoint entry — the toggle
        // neither baked into its command nor changed its inputs.
        let mut rules = diamond_rules();
        rules[1].when = Some("(config.enabled_a || config.enabled_b)".to_string()); // b

        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "true"), ("enabled_b", "false")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        // Toggle: a → false, b → true. The DISJUNCTION stays true.
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "false"), ("enabled_b", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );

        assert!(report.invalidated.is_empty(), "{:?}", report.invalidated);
        assert_eq!(report.when_gate_exempt, vec!["b".to_string()]);
        assert!(report.when_flip_invalidated.is_empty());
        assert_eq!(checkpoint.completed_rules.len(), 4);
        // Verdicts re-recorded under the new config for the next diff.
        assert_eq!(checkpoint.when_verdicts.get("b"), Some(&true));
    }

    #[test]
    fn when_flip_true_to_false_invalidates_with_downstream() {
        let mut rules = diamond_rules();
        rules[1].when = Some("config.enabled_a".to_string()); // b

        // Run 1: gate true, chain completes.
        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        // Run 2: gate flips true→false. The completed output must stop being
        // served — the executor pre-marks completed rules as Success without
        // ever re-evaluating `when` (issue #198) — and downstream d follows.
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "false")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(
            report.when_flip_invalidated,
            vec!["b".to_string()],
            "true→false flip must invalidate"
        );
        let invalidated: HashSet<&String> = report.invalidated.iter().collect();
        assert!(invalidated.contains(&"b".to_string()));
        assert!(invalidated.contains(&"d".to_string()));
        assert!(!checkpoint.is_completed("b"));
        assert!(!checkpoint.is_completed("d"));
    }

    #[test]
    fn when_flip_false_to_true_invalidates_too() {
        let mut rules = diamond_rules();
        rules[1].when = Some("config.enabled_a".to_string()); // b

        // Run 1: gate false, chain completes anyway (authored workflows may
        // complete consumers against pre-existing files).
        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "false")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        // Run 2: gate flips false→true — b starts producing for consumers;
        // completed dependents cannot be silently reused across that change.
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("enabled_a", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(
            report.when_flip_invalidated,
            vec!["b".to_string()],
            "false→true flip must invalidate"
        );
        assert!(!checkpoint.is_completed("d"));
    }

    #[test]
    fn flipped_false_to_true_producer_cascades_to_completed_consumer() {
        // A producer SKIPPED under the old gate now runs; its fresh output
        // lands mid-run, so its completed consumer cannot be reused.
        let producer = Rule {
            name: "extra_prod".to_string(),
            input: FilePatterns::List(vec![]),
            output: FilePatterns::List(vec!["extra.out".to_string()]),
            shell: Some("echo x > extra.out".to_string()),
            when: Some("config.make_extra".to_string()),
            ..Default::default()
        };
        let consumer = make_rule("consume", &["extra.out"], &["final.out"]);
        let rules = vec![producer, consumer];
        let dag = WorkflowDag::from_rules(&rules).unwrap();

        // Run 1: gate false (producer skipped, never completed).
        let mut checkpoint = completed_checkpoint(&["consume"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &dag,
            &cfg(&[("make_extra", "false")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        // Run 2: gate flips on. Even though the producer has no completed
        // entry, its flip cascades to the completed consumer.
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &dag,
            &cfg(&[("make_extra", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert!(
            report
                .when_flip_invalidated
                .contains(&"extra_prod".to_string())
        );
        assert!(
            !checkpoint.is_completed("consume"),
            "consumer of a newly-activated producer must re-run"
        );
    }

    #[test]
    fn interpolation_channel_still_invalidates_even_when_gate_unchanged() {
        // Conservative channel: `{config.k}` in the shell means the changed
        // value alters what runs, regardless of any unchanged when-verdict.
        let mut rules = diamond_rules();
        rules[1].shell = Some("fastp -q {config.k}".to_string());
        rules[1].when = Some("config.gate".to_string());

        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &cfg(&[("k", "20"), ("gate", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("k", "30"), ("gate", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        let invalidated: HashSet<&String> = report.invalidated.iter().collect();
        assert!(invalidated.contains(&"b".to_string()));
        assert!(invalidated.contains(&"d".to_string()));
        assert!(report.when_gate_exempt.is_empty());
        assert!(!checkpoint.is_completed("b"));
    }

    #[test]
    fn missing_stored_verdict_invalidates_once_then_stabilizes() {
        // Checkpoints written by pre-#198 binaries carry no verdicts — same
        // adoption story as issue #142 M1's input-excluded fingerprints: one
        // conservative invalidation, then stable gate-aware reuse forever.
        let mut rules = diamond_rules();
        rules[1].when = Some("(config.feature || config.alt_path)".to_string()); // b

        // Old-binary checkpoint: snapshot exists, verdicts map empty, and its
        // recorded feature value differs from the incoming run's.
        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        checkpoint
            .config_snapshot
            .insert("feature".to_string(), "false".to_string());

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("feature", "true"), ("alt_path", "false")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(
            report.when_flip_invalidated,
            vec!["b".to_string()],
            "no stored verdict → conservative once-only invalidation"
        );
        assert!(!checkpoint.is_completed("b"));
        assert_eq!(checkpoint.when_verdicts.get("b"), Some(&true));

        // Adoption done. A later toggle of the sibling key leaves the gate
        // (still a disjunction containing a true term) unchanged: reuse.
        checkpoint.mark_completed(
            "b",
            super::super::executor::checkpoint::BenchmarkRecord {
                rule: "b".to_string(),
                wall_time_secs: 1.0,
                max_memory_mb: None,
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
            },
        );
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("feature", "true"), ("alt_path", "true")]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(report.when_gate_exempt, vec!["b".to_string()]);
        assert!(report.invalidated.is_empty());
        assert!(checkpoint.is_completed("b"));
    }

    #[test]
    fn numeric_when_comparison_flips_use_typed_current_config() {
        // Typed config values reach the evaluator unharmed: an integer
        // threshold comparison flips exactly at the boundary.
        let mut rules = diamond_rules();
        rules[1].when = Some("config.min_qual >= 20".to_string()); // b

        let mut checkpoint = completed_checkpoint(&["a", "b", "c", "d"]);
        let mut cp = checkpoint.clone();
        let _ = detect_config_changes(
            &mut cp,
            &rules,
            &diamond_dag(),
            &typed_cfg(&[("min_qual", toml::Value::Integer(25))]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &typed_cfg(&[("min_qual", toml::Value::Integer(10))]),
            &HashSet::new(),
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(report.when_flip_invalidated, vec!["b".to_string()]);
        assert!(!checkpoint.is_completed("d"));
    }
}
