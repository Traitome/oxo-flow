//! Config-change impact analysis: precise checkpoint invalidation (issue #62).
//!
//! When a workflow is re-run with changed config values, only the rules that
//! reference the changed keys — plus their DAG downstream — are invalidated.
//! Rules whose structure (shell, inputs, …) changed are detected the same way
//! via per-rule fingerprints. Everything else keeps hitting the checkpoint.
//!
//! Two complementary mechanisms, with a strict correctness invariant: the
//! invalidation direction is monotone-safe. Over-invalidation wastes compute;
//! under-invalidation silently reuses stale outputs, which is the bug this
//! module exists to prevent.

use crate::config::ReferenceDef;
use crate::dag::WorkflowDag;
use crate::executor::checkpoint::CheckpointState;
use crate::rule::{FilePatterns, Rule};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

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

/// Maps each config key to the set of rules that reference it at execution
/// time (`{config.<key>}` in shell/script/IO/envvars/params, or bare
/// `config.<key>` inside `when` conditions).
pub struct ConfigReferenceGraph {
    key_rules: HashMap<String, HashSet<String>>,
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

        let mut key_rules: HashMap<String, HashSet<String>> = HashMap::new();
        for rule in rules {
            let mut refs: HashSet<String> = HashSet::new();

            for text in braced_texts(rule) {
                for cap in braced.captures_iter(&text) {
                    refs.insert(cap[1].to_string());
                }
            }
            if let Some(ref when) = rule.when {
                for cap in bare.captures_iter(when) {
                    refs.insert(cap[1].to_string());
                }
            }

            for key in refs {
                key_rules.entry(key).or_default().insert(rule.name.clone());
            }
        }
        Self { key_rules }
    }

    /// All rules referencing any of the given keys.
    pub fn rules_referencing<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> HashSet<String> {
        let mut rules = HashSet::new();
        for key in keys {
            if let Some(referencing) = self.key_rules.get(key) {
                rules.extend(referencing.iter().cloned());
            }
        }
        rules
    }

    /// Returns `true` if no rule references any config key.
    pub fn is_empty(&self) -> bool {
        self.key_rules.is_empty()
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
pub fn rule_fingerprint(rule: &Rule, interpreter_map: &HashMap<String, String>) -> String {
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
    add("shell", rule.shell.as_deref().unwrap_or(""));
    add("script", rule.script.as_deref().unwrap_or(""));
    add("input", &canonical_file_patterns(&rule.input));
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
pub fn reference_fingerprint(def: &ReferenceDef, config: &HashMap<String, toml::Value>) -> String {
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
    /// Rules directly affected (reference a changed key or mismatch).
    pub directly_affected: Vec<String>,
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
pub fn detect_config_changes(
    checkpoint: &mut CheckpointState,
    rules: &[Rule],
    dag: &WorkflowDag,
    current: &HashMap<String, toml::Value>,
    sensitive_keys: &HashSet<String>,
    interpreter_map: &HashMap<String, String>,
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
    let mut fingerprint_mismatches: Vec<String> = Vec::new();
    for rule in rules {
        let fingerprint = rule_fingerprint(rule, interpreter_map);
        if let Some(stored) = checkpoint.rule_fingerprints.get(&rule.name)
            && *stored != fingerprint
            && checkpoint.is_completed(&rule.name)
        {
            fingerprint_mismatches.push(rule.name.clone());
        }
        current_fingerprints.insert(rule.name.clone(), fingerprint);
    }
    fingerprint_mismatches.sort();

    // ── 3. Bootstrap path: record provenance, keep everything completed ──
    if is_legacy {
        checkpoint.config_snapshot = build_config_snapshot(current, sensitive_keys);
        checkpoint.rule_fingerprints.extend(current_fingerprints);
        return ConfigChangeReport {
            is_legacy: true,
            ..Default::default()
        };
    }

    // ── 4. Affected rules + DAG downstream closure ───────────────────────
    let mut directly_affected: HashSet<String> =
        graph.rules_referencing(all_diff_keys.iter().map(String::as_str));
    directly_affected.extend(fingerprint_mismatches.iter().cloned());

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

    // ── 5. Mutate checkpoint ─────────────────────────────────────────────
    for rule_name in &invalidated {
        checkpoint.completed_rules.remove(rule_name);
    }
    checkpoint.config_snapshot = build_config_snapshot(current, sensitive_keys);
    checkpoint.rule_fingerprints.extend(current_fingerprints);

    let mut directly_affected: Vec<String> = directly_affected.into_iter().collect();
    directly_affected.sort();
    let mut invalidated: Vec<String> = invalidated.into_iter().collect();
    invalidated.sort();

    ConfigChangeReport {
        is_legacy: false,
        changed_keys,
        added_keys,
        removed_keys,
        fingerprint_mismatches,
        directly_affected,
        invalidated,
    }
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
            graph.rules_referencing(["min_quality"]),
            HashSet::from(["fastp_trim".to_string()])
        );
        // Non-config placeholders ({threads} without the config. prefix)
        // must not be captured.
        assert!(graph.rules_referencing(["threads"]).is_empty());
        // `{config.threads}` IS a config reference (key "threads").
        let rules = vec![Rule {
            name: "r".to_string(),
            shell: Some("echo {config.threads}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(graph.rules_referencing(["threads"]).contains("r"));
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
                graph.rules_referencing([key]).contains("align"),
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
        assert!(graph.rules_referencing(["min_qual"]).contains("qc"));
        assert!(graph.rules_referencing(["enable_qc"]).contains("qc"));
    }

    #[test]
    fn graph_tolerates_file_exists_false_positive_as_safe_over_invalidation() {
        // `file_exists("config.yaml")` matches the bare `config.<key>` regex
        // with key "yaml". Over-invalidation only — never stale reuse.
        let rules = vec![Rule {
            name: "r".to_string(),
            when: Some(r#"file_exists("config.yaml")"#.to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(graph.rules_referencing(["yaml"]).contains("r"));
    }

    #[test]
    fn graph_handles_keys_containing_dots() {
        let rules = vec![Rule {
            name: "r".to_string(),
            shell: Some("echo {config.min.qual}".to_string()),
            ..Default::default()
        }];
        let graph = ConfigReferenceGraph::from_rules(&rules);
        assert!(graph.rules_referencing(["min.qual"]).contains("r"));
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
            graph.rules_referencing(["min_quality"]),
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
        assert_eq!(rule_fingerprint(&a, &map), rule_fingerprint(&b, &map));
    }

    #[test]
    fn fingerprint_changes_when_shell_changes() {
        let mut a = make_rule("r", &[], &["o"]);
        a.shell = Some("fastp -q 20".to_string());
        let mut b = a.clone();
        b.shell = Some("fastp -q 30".to_string());
        let map = HashMap::new();
        assert_ne!(rule_fingerprint(&a, &map), rule_fingerprint(&b, &map));
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
        assert_eq!(rule_fingerprint(&a, &map), rule_fingerprint(&b, &map));
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
        assert_ne!(rule_fingerprint(&a, &map), rule_fingerprint(&b, &map));
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
        assert_ne!(rule_fingerprint(&a, &map), rule_fingerprint(&b, &map));
        assert_ne!(rule_fingerprint(&c, &map), rule_fingerprint(&d, &map));
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
            description: None,
        };
        let mut config = HashMap::new();
        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v1".to_string()),
        );
        let f1 = reference_fingerprint(&def, &config);

        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v2".to_string()),
        );
        assert_ne!(f1, reference_fingerprint(&def, &config));

        let mut edited = def.clone();
        edited.build = "bwa index -p {config.ref_dir}/idx2 {input}".to_string();
        config.insert(
            "ref_dir".to_string(),
            toml::Value::String("refs/v2".to_string()),
        );
        assert_ne!(
            reference_fingerprint(&def, &config),
            reference_fingerprint(&edited, &config)
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
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "30")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("new_key", "y")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "20")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("samples_list", "S3,S4")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        checkpoint = cp;
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("pairs_list", "P2,P3")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &diamond_dag(),
            &cfg(&[("min_quality", "30")]),
            &HashSet::new(),
            &HashMap::new(),
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
        );
        checkpoint = cp;

        let report = detect_config_changes(
            &mut checkpoint,
            &rules,
            &dag,
            &cfg(&[("k", "2")]),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(report.invalidated, vec!["orphan".to_string()]);
        assert!(!checkpoint.is_completed("orphan"));
    }
}
