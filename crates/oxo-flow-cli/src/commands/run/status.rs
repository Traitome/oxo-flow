//! status/resume support printing: rule_timings, target-skipped/when-pruned notes, workdir check, staleness reasons, reconcile_failed_rules (#948 P2-5).

use super::*;

/// list and the total — a fake "0.0s" row would pollute both.
pub(crate) fn rule_timings(state: &CheckpointState) -> (Vec<(&str, f64)>, f64) {
    let mut timings: Vec<(&str, f64)> = state
        .benchmarks
        .iter()
        .filter(|(_, b)| b.recorded_as.is_none())
        .map(|(rule, bench)| (rule.as_str(), bench.wall_time_secs))
        .collect();
    // Deterministic order: slowest first (HashSet iteration order is arbitrary)
    timings.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total = timings.iter().map(|(_, secs)| secs).sum();
    (timings, total)
}

/// Explain a target removed from a targeted execution set. A target present
/// in `when_false_rules` has its OWN false gate; one absent from the set was
/// cascade-pruned because an UPSTREAM gate is false — the old single message
/// claimed both were "when-gated false", which read as nonsense for gate-less
/// rules (issue #299).
pub(crate) fn print_target_skipped_note(
    skipped: &str,
    when_false_rules: &std::collections::HashSet<String>,
    run_log: Option<&SharedRunLog>,
) {
    let detail = if when_false_rules.contains(skipped) {
        format!(
            "target '{skipped}' is when-gated false — it never runs; \
             removed from the execution set (its upstream was pruned too)"
        )
    } else {
        format!(
            "target '{skipped}' is unreachable — its upstream is when-gated \
             false; removed from the execution set"
        )
    };
    diagnostic_narrate(format_args!("{} {detail}", "Note:".yellow()), run_log);
}

/// Explain a rule dropped from a FULL-RUN execution set by when-aware
/// propagation (#882/#883): its own upstream is when-gated false, so its
/// inputs can never be produced in this config. Unlike the targeted note
/// above there is no user-selected target — the wording reports the plan
/// change without the "requested target" framing.
pub(crate) fn print_when_pruned_note(rule_name: &str, run_log: Option<&SharedRunLog>) {
    let detail = format!(
        "rule '{rule_name}' is unreachable — its upstream is when-gated \
         false; removed from the full-run execution set"
    );
    diagnostic_narrate(format_args!("{} {detail}", "Note:".yellow()), run_log);
}

/// Existence predicate for the target closure: an input path (config-
/// expanded, relative to the workflow DAG) that already exists in the run
/// workdir is a pre-built input — the executor will find it, so a pruned
/// when-false producer must not gate the consumer. Absolute paths resolve
/// as-is; the run_workdir-check exists so every call site stays anchored in
/// ITS effective workdir (`run` uses the run workdir, dry-run the plan base
/// dir).
pub(crate) fn run_workdir_check(workdir: Option<&std::path::Path>, path: &str) -> bool {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.exists()
    } else {
        workdir
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(p)
            .exists()
    }
}

/// Per-rule staleness reasons for `status` (issue #432(c)).
///
/// Answers "why will this rule re-run?" with the SAME classification the
/// dry-run preview uses, by re-parsing the workflow recorded in the
/// checkpoint and running the shared invalidation detection on a throwaway
/// state clone (detection mutates the state it is given).
///
/// Returns an empty map when the answer cannot be computed — workflow file
/// missing/moved, config no longer parses, or no completed rules — so the
/// caller degrades to the plain listing instead of failing.
#[allow(clippy::type_complexity)]
pub(crate) fn compute_staleness_reasons(
    state: &CheckpointState,
    checkpoint_path: &std::path::Path,
) -> std::collections::BTreeMap<String, (String, String)> {
    use crate::commands::run_preview::RuleStatus;
    let mut reasons = std::collections::BTreeMap::new();
    if state.completed_rules.is_empty() {
        return reasons;
    }
    let Some(workflow_path) = state.workflow_path.as_ref() else {
        return reasons;
    };
    let workflow = std::path::PathBuf::from(workflow_path);
    if !workflow.exists() {
        return reasons;
    }
    let Ok(config) = WorkflowConfig::from_file(&workflow) else {
        return reasons;
    };
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    let Ok(dag) = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    ) else {
        return reasons;
    };
    let Ok(raw_order) = dag.execution_order() else {
        return reasons;
    };
    let order_set: std::collections::HashSet<String> = raw_order.into_iter().collect();
    let mut order: Vec<String> = Vec::new();
    let Ok(groups) = dag.parallel_groups() else {
        return reasons;
    };
    for group in groups {
        let mut level: Vec<String> = group
            .into_iter()
            .filter(|n| order_set.contains(n))
            .collect();
        level.sort();
        order.extend(level);
    }

    // Read-only: preview_run_plan clones internally and mutates only that
    // clone (detection may record legacy baselines there) — the on-disk
    // checkpoint is never touched from `status`. The pristine state must be
    // passed so completed-rule credit is classified as such.
    let wildcard_values = config_placeholder_values(&config.config);
    let sensitive_keys: std::collections::HashSet<String> = config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .map(|(key, _)| key.clone())
        .collect();
    let interpreter_map = config.workflow.interpreter_map.clone();
    // `run` and dry-run resolve this identity gate before any preview; from
    // `status` the checkpoint's own recorded path is the workflow (same file
    // it was saved under), so adoption is a no-op — but run it anyway to keep
    // the classification path byte-identical with run's.
    let mut adopted = state.clone();
    crate::commands::run::adopt_checkpoint_for_workflow(&mut adopted, &workflow, None);
    let preview = crate::commands::run_preview::preview_run_plan(
        &adopted,
        &config,
        &dag,
        &order,
        &workflow_dir,
        &wildcard_values,
        &sensitive_keys,
        &interpreter_map,
        checkpoint_path,
        false,
        false,
    );
    for entry in &preview.plan {
        // Only rules recorded as completed are in scope: the question is
        // "why would this COMPLETED rule re-run".
        if !state.completed_rules.contains(&entry.name) {
            continue;
        }
        let (reason, detail) = match entry.status {
            RuleStatus::Skipped | RuleStatus::SkippedByWhen | RuleStatus::SkippedFresh => {
                continue;
            }
            // After adoption a completed rule that survived classification
            // but is gone from the adopted state means the identity gate
            // invalidated it — the credit really is gone, so say so.
            RuleStatus::NeverCompleted if !adopted.completed_rules.contains(&entry.name) => (
                "checkpoint reset".to_string(),
                "completed credit was invalidated (foreign workflow adoption)".to_string(),
            ),
            RuleStatus::NeverCompleted => (
                "output missing".to_string(),
                "completion record present but the executor would re-run — outputs are gone or failed cleanup removed them".to_string(),
            ),
            RuleStatus::ConfigInvalidated => (
                "config changed".to_string(),
                "an effective config value differs from the recorded snapshot".to_string(),
            ),
            RuleStatus::InputInvalidated => (
                "input changed".to_string(),
                "recorded input manifest (paths/size/mtime) no longer matches disk".to_string(),
            ),
            RuleStatus::OutputsMissing => (
                "output missing".to_string(),
                "recorded output is gone from the workdir".to_string(),
            ),
            RuleStatus::Cascaded { from: ref f1 } => (
                "cascaded".to_string(),
                format!("upstream rule '{f1}' will re-run"),
            ),
            RuleStatus::CascadedUpstream { from: ref f2 } => (
                "missing input".to_string(),
                format!(
                    "a downstream rule needs an intermediate that is gone; producer '{f2}' re-runs first"
                ),
            ),
            RuleStatus::Forced => ("forced".to_string(), "--rerun was requested".to_string()),
        };
        reasons.insert(entry.name.clone(), (reason, detail));
    }
    reasons
}

/// Split the checkpoint's failed rules into real failures and rules whose
/// runtime `when` gate evaluated to false (issue #690).
///
/// The executor records `Skipped` for a when-false verdict and (since #690)
/// removes any stale failure record itself, but checkpoints written by older
/// versions can still carry the stale entry — `status` then rendered the
/// rule as `✗ Failed` forever. This display-side reconcile keeps those dirty
/// checkpoints honest: an entry whose recorded verdict is `false` is reported
/// as skipped-by-when, not failed. Both outputs are sorted (`failed_rules`
/// is a BTreeSet, so iteration is already deterministic).
pub(crate) fn reconcile_failed_rules<'a>(
    failed_rules: &'a std::collections::BTreeSet<String>,
    when_verdicts: &'a std::collections::BTreeMap<String, bool>,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let mut real_failed = Vec::new();
    let mut skipped_when_false = Vec::new();
    for rule in failed_rules {
        if when_verdicts.get(rule) == Some(&false) {
            skipped_when_false.push(rule.as_str());
        } else {
            real_failed.push(rule.as_str());
        }
    }
    (real_failed, skipped_when_false)
}
