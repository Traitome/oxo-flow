//! Checkpoint-aware run preview for `dry-run` (issue #66).
//!
//! Predicts what an actual `run` would do with the same checkpoint:
//! which rules re-execute (selected/invalidated/missing outputs), which
//! cascade downstream, and which stay protected by the checkpoint.
//!
//! The classification reuses the exact same detection machinery `run`
//! uses (config-impact fingerprints, input manifests, DAG downstream
//! closure) but operates on a CLONED checkpoint — the preview is strictly
//! read-only and never mutates the on-disk state.

use anyhow::{Context, Result};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use oxo_flow_core::rule::Rule;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Why a rule will execute (or not) in the predicted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    /// No completed checkpoint entry — it has never run.
    NeverCompleted,
    /// Config change or rule-definition edit invalidated it (issue #62).
    ConfigInvalidated,
    /// Its input files changed since completion (issue #72).
    InputInvalidated,
    /// Declared outputs no longer exist.
    OutputsMissing,
    /// Was completed, but sits downstream of a rule that will execute.
    Cascaded { from: String },
    /// Checkpoint hit — will be skipped.
    Skipped,
    /// Its `when` condition evaluates to false — `run` skips it regardless
    /// of invalidation state, so the preview does too.
    SkippedByWhen,
}

/// One rule in the predicted plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRule {
    pub name: String,
    pub status: RuleStatus,
}

/// The predicted execution plan for one dry-run invocation.
#[derive(Debug, Clone)]
pub struct RunPreview {
    /// Checkpoint the prediction was computed from (may not exist on disk).
    pub checkpoint_path: std::path::PathBuf,
    /// File modification time — the "last run" proxy the checkpoint format
    /// itself does not record.
    pub checkpoint_modified: Option<std::time::SystemTime>,
    /// Completed entries in the checkpoint before this run.
    pub completed_total: usize,
    /// Rules in the execution set that will execute (any non-Skipped status).
    pub plan: Vec<PreviewRule>,
    /// Rules in the execution set that will be skipped.
    pub will_skip: usize,
    /// Completed rules OUTSIDE this execution set — work this run preserves.
    pub protected_outside: usize,
    /// Cascade chains (seed → … → queue-level rule) for the display.
    pub cascade_chains: Vec<Vec<String>>,
}

/// Compute the read-only preview for an execution set.
///
/// Mirrors `run`'s preprocessing exactly: config-impact detection first,
/// then input-manifest comparison, then DAG downstream closure. All
/// mutations happen on a clone of `ck`.
#[allow(clippy::too_many_arguments)]
pub fn preview_run_plan(
    ck: &CheckpointState,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    sensitive_keys: &HashSet<String>,
    interpreter_map: &HashMap<String, String>,
    checkpoint_path: &Path,
) -> RunPreview {
    let completed_original: HashSet<String> = ck.completed_rules.clone();
    let mut clone = ck.clone();

    // 1. Config-impact invalidation (issue #62) — on the clone only.
    let config_report = oxo_flow_core::config_impact::detect_config_changes(
        &mut clone,
        &config.rules,
        dag,
        &config.config,
        sensitive_keys,
        interpreter_map,
    );
    let config_invalidated: HashSet<String> = config_report.invalidated.iter().cloned().collect();

    // 2. Input-manifest invalidation (issue #72) — on the clone only.
    let (manifest_invalidated, _baselined) =
        detect_input_manifest_invalidations(&mut clone, config, order, workdir, wildcard_values);

    // 3. DAG downstream closure of the manifest mismatches — the same
    //    cascade `run` applies.
    let seeds: HashSet<String> = manifest_invalidated.iter().cloned().collect();
    invalidate_with_downstream(&mut clone, dag, &seeds);

    // 4. Classify every rule in the execution set.
    let seeds_for_cascade: HashSet<String> = config_invalidated
        .union(&manifest_invalidated)
        .cloned()
        .collect();
    let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut plan = Vec::with_capacity(order.len());
    let mut will_skip = 0usize;
    for name in order {
        let status = if config
            .get_rule(name)
            .is_some_and(|rule| when_condition_false(rule, config, wildcard_values))
        {
            // Evaluated exactly like run does (typed config values win over
            // string wildcard values); a false condition dominates every
            // other consideration — even forced re-runs.
            RuleStatus::SkippedByWhen
        } else if !completed_original.contains(name) {
            RuleStatus::NeverCompleted
        } else if config_invalidated.contains(name) {
            RuleStatus::ConfigInvalidated
        } else if manifest_invalidated.contains(name) {
            RuleStatus::InputInvalidated
        } else if let Some(rule) = config.get_rule(name)
            && !rule_outputs_exist(rule, workdir, wildcard_values)
        {
            RuleStatus::OutputsMissing
        } else if !clone.completed_rules.contains(name) {
            // Was completed at the start but the closure removed it — a
            // cascaded invalidation. Attribute the nearest seed.
            RuleStatus::Cascaded {
                from: nearest_seed(name, dag, &seeds_for_cascade),
            }
        } else {
            RuleStatus::Skipped
        };
        if matches!(status, RuleStatus::Skipped | RuleStatus::SkippedByWhen) {
            will_skip += 1;
        }
        plan.push(PreviewRule {
            name: name.clone(),
            status,
        });
    }

    // 5. Cascade chains for display: from each seed that was previously
    //    completed, walk dependents within the execution set.
    let mut cascade_chains: Vec<Vec<String>> = Vec::new();
    for seed in seeds_for_cascade.iter() {
        if !completed_original.contains(seed) {
            continue; // never completed — nothing was "infected"
        }
        let chain = cascade_chain(seed, dag, order_set.clone());
        if chain.len() > 1 {
            cascade_chains.push(chain);
        }
    }
    cascade_chains.sort_by(|a, b| a.first().cmp(&b.first()));

    let protected_outside = completed_original
        .iter()
        .filter(|name| !order_set.contains(name.as_str()))
        .count();

    RunPreview {
        checkpoint_path: checkpoint_path.to_path_buf(),
        checkpoint_modified: std::fs::metadata(checkpoint_path)
            .and_then(|m| m.modified())
            .ok(),
        completed_total: completed_original.len(),
        plan,
        will_skip,
        protected_outside,
        cascade_chains,
    }
}

/// Walk dependents of `seed` (in execution-set order) that were completed —
/// the displayed infection chain.
fn cascade_chain(seed: &str, dag: &WorkflowDag, order_set: HashSet<&str>) -> Vec<String> {
    let mut chain = vec![seed.to_string()];
    let mut frontier = vec![seed.to_string()];
    let mut visited: HashSet<String> = frontier.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        let Ok(dependents) = dag.dependents(&name) else {
            continue;
        };
        let mut next: Vec<String> = dependents
            .into_iter()
            .filter(|d| order_set.contains(d.as_str()))
            .filter(|d| visited.insert(d.clone()))
            .collect();
        next.sort();
        chain.extend(next.iter().cloned());
        frontier.extend(next);
    }
    chain
}

/// Nearest invalidation seed that reaches `rule` through dependents.
fn nearest_seed(rule: &str, dag: &WorkflowDag, seeds: &HashSet<String>) -> String {
    if seeds.contains(rule) {
        return rule.to_string();
    }
    // BFS upstream along dependencies: prefer the first seed in
    // lexicographic order for deterministic output.
    let mut sorted_seeds: Vec<&String> = seeds.iter().collect();
    sorted_seeds.sort();
    for seed in sorted_seeds {
        if let Some(path) = dag_path_exists(seed, rule, dag) {
            let _ = path;
            return seed.clone();
        }
    }
    "<unknown>".to_string()
}

/// Whether `from` can reach `to` via dependency edges (BFS).
fn dag_path_exists(from: &str, to: &str, dag: &WorkflowDag) -> Option<usize> {
    let mut frontier = vec![(from.to_string(), 0)];
    let mut visited: HashSet<String> = frontier.iter().map(|(n, _)| n.clone()).collect();
    while let Some((name, depth)) = frontier.pop() {
        if name == to {
            return Some(depth);
        }
        let Ok(dependents) = dag.dependents(&name) else {
            continue;
        };
        for dependent in dependents {
            if visited.insert(dependent.clone()) {
                frontier.push((dependent, depth + 1));
            }
        }
    }
    None
}

/// Whether every declared output of the rule exists (the same check `run`
/// performs before re-submitting a completed rule).
pub fn rule_outputs_exist(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    rule.output.iter().all(|output| {
        let expanded =
            oxo_flow_core::executor::checkpoint::expand_config_in_path(output, wildcard_values);
        expanded.contains('{') || workdir.join(&expanded).exists()
    })
}

/// Compare completed rules' input manifests against the current file set.
///
/// Returns (mismatched rule names, number of legacy-baseline adoptions).
/// Mutates `ck` by recording baselines for legacy checkpoints — exactly
/// like `run` does; pass a clone for a read-only preview.
pub fn detect_input_manifest_invalidations(
    ck: &mut CheckpointState,
    config: &WorkflowConfig,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> (HashSet<String>, usize) {
    let mut mismatched: HashSet<String> = HashSet::new();
    let mut baselined = 0usize;
    for name in order {
        if !ck.completed_rules.contains(name) {
            continue;
        }
        let Some(rule) = config.get_rule(name) else {
            continue;
        };
        match oxo_flow_core::executor::checkpoint::snapshot_input_manifest(
            rule,
            workdir,
            wildcard_values,
        ) {
            Ok(Some(current)) => match ck.input_manifests.get(name) {
                Some(recorded)
                    if oxo_flow_core::executor::checkpoint::manifests_match(recorded, &current) => {
                }
                Some(_) => {
                    mismatched.insert(name.clone());
                }
                None => {
                    // Legacy baseline: adopt the current set.
                    ck.record_input_manifest(name, current);
                    baselined += 1;
                }
            },
            Ok(None) => {}
            Err(_) => {
                // Inputs cannot be resolved — cannot verify, so don't reuse.
                mismatched.insert(name.clone());
            }
        }
    }
    (mismatched, baselined)
}

/// Remove `seeds` and their DAG downstream from the completed set, returning
/// every affected name (the same cascade `run` applies, issue #72).
pub(crate) fn invalidate_with_downstream(
    ck: &mut CheckpointState,
    dag: &WorkflowDag,
    seeds: &HashSet<String>,
) -> Vec<String> {
    let mut invalidated: HashSet<String> = seeds.clone();
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        if let Ok(dependents) = dag.dependents(&name) {
            for dependent in dependents {
                if invalidated.insert(dependent.clone()) {
                    frontier.push(dependent);
                }
            }
        }
    }
    for name in &invalidated {
        ck.completed_rules.remove(name);
    }
    let mut names: Vec<String> = invalidated.into_iter().collect();
    names.sort();
    names
}

/// Whether the rule's `when` condition evaluates to false against the
/// merged config — the same inputs `run` evaluates it with (typed config
/// values win over string wildcard values; process.rs mirrors this).
fn when_condition_false(
    rule: &Rule,
    config: &WorkflowConfig,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    let Some(condition) = rule.when.as_deref() else {
        return false;
    };
    let mut config_values: HashMap<String, toml::Value> = config.config.clone();
    for (k, v) in wildcard_values {
        if let Some(key) = k.strip_prefix("config.") {
            config_values
                .entry(key.to_string())
                .or_insert_with(|| toml::Value::String(v.clone()));
        }
    }
    !oxo_flow_core::executor::process::evaluate_condition(condition, &config_values)
}

/// Merge a profile's `[config]` table into the workflow config — the same
/// semantics `run` applies. Profile values only FILL IN keys the workflow
/// does not set (`or_insert`); profile lookup is workflow-dir-only:
/// `<workflow-dir>/profiles/<NAME>.toml`, then `.oxoflow`; a missing
/// profile warns and continues (matching run, issue #76 audit).
pub(crate) fn merge_profile(
    config: &mut WorkflowConfig,
    profile_name: &str,
    workflow_dir: &Path,
) -> Result<()> {
    use colored::Colorize;

    let profile_paths = [
        workflow_dir
            .join("profiles")
            .join(format!("{profile_name}.toml")),
        workflow_dir
            .join("profiles")
            .join(format!("{profile_name}.oxoflow")),
    ];
    let profile_path = profile_paths.iter().find(|p| p.exists());
    let Some(path) = profile_path else {
        eprintln!(
            "{} Profile '{}' not found in profiles/ directory",
            "Warning:".bold().yellow(),
            profile_name
        );
        return Ok(());
    };
    let profile_content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read profile {}", path.display()))?;
    // NOTE: `toml::Value::from_str` in toml 1.x parses a SINGLE inline
    // value, not a document — `[config]` would fail with "unexpected
    // content". Parse the document explicitly (this also fixes the
    // pre-existing run --profile bug the shared extraction surfaced).
    let profile_toml: toml::Value = toml::from_str(&profile_content)
        .with_context(|| format!("failed to parse profile {}", path.display()))?;
    if let Some(config_table) = profile_toml.get("config").and_then(toml::Value::as_table) {
        for (key, value) in config_table {
            config
                .config
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        eprintln!(
            "{} Merged {} config values from profile '{}'",
            "Profile:".bold().cyan(),
            config_table.len(),
            profile_name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;
    use oxo_flow_core::executor::checkpoint::snapshot_input_manifest;

    /// Fixture: two samples, trim → align per sample, one queue-level combine.
    /// All input/output files exist so snapshots resolve.
    fn fixture(
        dir: &std::path::Path,
    ) -> (
        WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
    ) {
        let toml = r#"
[workflow]
name = "t"
version = "1.0"

[config]
ref = "ref.fa"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "trim"
input = ["raw/{sample}.fq"]
output = ["trimmed/{sample}.fq"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "align"
input = ["trimmed/{sample}.fq"]
output = ["aligned/{sample}.bam"]
depends_on = ["trim"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "combine"
input = ["aligned/*.bam"]
output = ["combined.bam"]
depends_on = ["align"]
shell = "cat {input[0]} > {output[0]} && echo {config.ref}"
"#;
        // Files the rules read and write.
        for f in [
            "raw/S1.fq",
            "raw/S2.fq",
            "trimmed/S1.fq",
            "trimmed/S2.fq",
            "aligned/S1.bam",
            "aligned/S2.bam",
            "combined.bam",
        ] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "data1").unwrap();
        }

        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let mut wildcard_values = HashMap::new();
        for (key, value) in &config.config {
            wildcard_values.insert(
                format!("config.{key}"),
                value.as_str().unwrap_or_default().to_string(),
            );
        }
        (config, dag, order, wildcard_values)
    }

    fn completed_checkpoint(
        config: &WorkflowConfig,
        order: &[String],
        dir: &std::path::Path,
        wildcard_values: &HashMap<String, String>,
    ) -> CheckpointState {
        let mut ck = CheckpointState::new();
        for name in order {
            ck.completed_rules.insert(name.clone());
            if let Some(rule) = config.get_rule(name)
                && let Ok(Some(manifest)) = snapshot_input_manifest(rule, dir, wildcard_values)
            {
                ck.record_input_manifest(name, manifest);
            }
        }
        ck
    }

    fn sensitive() -> HashSet<String> {
        HashSet::new()
    }

    fn run_preview(
        ck: &CheckpointState,
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        order: &[String],
        dir: &std::path::Path,
        wildcard_values: &HashMap<String, String>,
    ) -> RunPreview {
        preview_run_plan(
            ck,
            config,
            dag,
            order,
            dir,
            wildcard_values,
            &sensitive(),
            &config.workflow.interpreter_map,
            &dir.join(".oxo-flow/checkpoint.json"),
        )
    }

    fn status_of<'a>(preview: &'a RunPreview, name: &str) -> &'a RuleStatus {
        &preview
            .plan
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing from plan"))
            .status
    }

    fn fixture_with_when(
        threshold: &str,
    ) -> (
        WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
    ) {
        let toml = format!(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threshold = {threshold}

[[rules]]
name = "fast"
input = ["in.txt"]
output = ["out.txt"]
when = "config.threshold >= 10"
shell = "cp in.txt out.txt"
"#
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.apply_defaults();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = config
            .config
            .iter()
            .map(|(k, v)| {
                (
                    format!("config.{k}"),
                    v.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        (config, dag, order, wildcard_values)
    }

    #[test]
    fn when_false_dominates_every_other_consideration() {
        let (config, dag, order, wildcard_values) = fixture_with_when("5");
        let ck = CheckpointState::new();
        let preview = run_preview(
            &ck,
            &config,
            &dag,
            &order,
            std::path::Path::new("."),
            &wildcard_values,
        );
        assert_eq!(
            status_of(&preview, "fast"),
            &RuleStatus::SkippedByWhen,
            "a false when condition skips the rule even though it never completed"
        );
        assert_eq!(preview.will_skip, 1);
    }

    #[test]
    fn when_true_classifies_normally() {
        let (config, dag, order, wildcard_values) = fixture_with_when("10");
        let ck = CheckpointState::new();
        let preview = run_preview(
            &ck,
            &config,
            &dag,
            &order,
            std::path::Path::new("."),
            &wildcard_values,
        );
        assert_eq!(status_of(&preview, "fast"), &RuleStatus::NeverCompleted);
        assert_eq!(preview.will_skip, 0);
    }

    #[test]
    fn merge_profile_fills_missing_keys_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        std::fs::write(
            dir.path().join("profiles/batch.toml"),
            r#"[config]
threshold = 20
mode = "fast"
"#,
        )
        .unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threshold = 5
"#,
        )
        .unwrap();
        merge_profile(&mut config, "batch", dir.path()).unwrap();
        assert_eq!(
            config.config["threshold"].as_integer(),
            Some(5),
            "existing keys are never overwritten"
        );
        assert_eq!(
            config.config["mode"].as_str(),
            Some("fast"),
            "missing keys are filled in"
        );
    }

    #[test]
    fn merge_profile_missing_profile_warns_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"
"#,
        )
        .unwrap();
        assert!(merge_profile(&mut config, "nope", dir.path()).is_ok());
    }

    #[test]
    fn empty_checkpoint_means_everything_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = CheckpointState::new();
        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(preview.plan.len(), 5);
        assert!(
            preview
                .plan
                .iter()
                .all(|r| r.status == RuleStatus::NeverCompleted)
        );
        assert_eq!(preview.will_skip, 0);
        assert_eq!(preview.protected_outside, 0);
        assert!(preview.cascade_chains.is_empty());
    }

    #[test]
    fn up_to_date_workflow_skips_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert!(preview.plan.iter().all(|r| r.status == RuleStatus::Skipped));
        assert_eq!(preview.will_skip, 5);
        assert_eq!(preview.completed_total, 5);
        assert!(preview.cascade_chains.is_empty());
    }

    #[test]
    fn changed_input_invalidates_rule_and_cascades_downstream() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // S1's raw data is re-downloaded: different size + mtime.
        std::fs::write(dir.path().join("raw/S1.fq"), "completely new content").unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "trim_cohort_S1"),
            &RuleStatus::InputInvalidated
        );
        // The engine rewrites `depends_on = ["trim"]` to EVERY expanded trim
        // (both samples) — align rules for S2 also sit downstream of
        // trim_cohort_S1. The preview surfaces this conservative coupling:
        // one sample's data change infects the whole queue.
        assert_eq!(
            status_of(&preview, "align_cohort_S1"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        assert_eq!(
            status_of(&preview, "align_cohort_S2"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        assert_eq!(
            status_of(&preview, "combine"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        // Only S2's trim itself stays protected.
        assert_eq!(status_of(&preview, "trim_cohort_S2"), &RuleStatus::Skipped);
        assert_eq!(preview.will_skip, 1);
        // The infection chain is surfaced for display.
        assert!(preview.cascade_chains.iter().any(|c| c
            == &vec![
                "trim_cohort_S1".to_string(),
                "align_cohort_S1".to_string(),
                "align_cohort_S2".to_string(),
                "combine".to_string()
            ]));
    }

    #[test]
    fn missing_output_marks_rerun_but_leaves_manifest_intact() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // The queue output was deleted; its inputs are untouched.
        std::fs::remove_file(dir.path().join("combined.bam")).unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(status_of(&preview, "combine"), &RuleStatus::OutputsMissing);
        assert_eq!(status_of(&preview, "trim_cohort_S1"), &RuleStatus::Skipped);
    }

    #[test]
    fn target_subset_counts_protected_outside() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // Target only S1's trim: 1 in the set, 4 completed outside it.
        let targets = ["trim_cohort_S1".to_string()];
        let subset: Vec<String> = dag
            .execution_order_for_targets(&targets.iter().map(String::as_str).collect::<Vec<_>>())
            .unwrap();
        let preview = run_preview(&ck, &config, &dag, &subset, dir.path(), &wildcard_values);
        assert_eq!(preview.plan.len(), 1);
        assert_eq!(preview.protected_outside, 4);
        assert_eq!(preview.will_skip, 1);
    }

    #[test]
    fn config_change_invalidates_referencing_rules() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // Bootstrap a config snapshot (legacy path records the baseline),
        // then change a key the queue rule references in its shell.
        oxo_flow_core::config_impact::detect_config_changes(
            &mut ck,
            &config.rules,
            &dag,
            &config.config,
            &sensitive(),
            &config.workflow.interpreter_map,
        );

        let mut changed_config = config.clone();
        changed_config
            .config
            .insert("ref".to_string(), toml::Value::String("other.fa".into()));

        let mut changed_wildcards = wildcard_values.clone();
        changed_wildcards.insert("config.ref".to_string(), "other.fa".into());

        let preview = run_preview(
            &ck,
            &changed_config,
            &dag,
            &order,
            dir.path(),
            &changed_wildcards,
        );
        assert_eq!(
            status_of(&preview, "combine"),
            &RuleStatus::ConfigInvalidated
        );
        assert_eq!(status_of(&preview, "trim_cohort_S1"), &RuleStatus::Skipped);
    }

    #[test]
    fn legacy_checkpoint_adopts_baseline_without_invalidating() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        // Legacy: completed but no manifests, no snapshots.
        ck.input_manifests.clear();
        ck.config_snapshot.clear();
        ck.rule_fingerprints.clear();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert!(preview.plan.iter().all(|r| r.status == RuleStatus::Skipped));
    }
}
