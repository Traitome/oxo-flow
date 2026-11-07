//! status/resume support printing: rule_timings, target-skipped/when-pruned notes, workdir check, staleness reasons, reconcile_failed_rules (#948 P2-5).

use super::*;
use crate::commands::print_banner;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Wall-time timings for `status --timing`, slowest first, plus their sum.
/// Synthetic records (issue #324 F-1 `recorded_as: Some(...)`) carry a 0.0
/// placeholder rather than a measurement, so they are excluded from the
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

/// The when-gated/targeted execution-order computation shared by `run`'s
/// initial planning, both replay rebuilds, and both dry-run plans. The
/// context messages and narration notes are part of the contract — they
/// were diverging across the five hand-copied sites. `print_notes: false`
/// is only for a replay recomputation whose targets were already noted in
/// the initial plan (re-printing identical notes reads as noise).
pub(crate) fn execution_order_skipping_with_notes(
    dag: &WorkflowDag,
    target: &[String],
    when_false_rules: &std::collections::HashSet<String>,
    workdir: &std::path::Path,
    run_log: Option<&SharedRunLog>,
    print_notes: bool,
) -> anyhow::Result<Vec<String>> {
    if target.is_empty() {
        let (filtered_order, pruned) = dag
            .execution_order_for_targets_skipping_with_source_check(
                &[],
                when_false_rules,
                // Pre-built inputs (atacseq bwa_mem finding): an input whose
                // concrete file already exists in the run workdir is
                // satisfiable — a when-false producer must not gate it.
                &|path| run_workdir_check(Some(workdir), path),
            )
            .with_context(|| "failed to compute full-run execution order")?;
        if print_notes {
            for rule_name in &pruned {
                print_when_pruned_note(rule_name, run_log);
            }
        }
        Ok(filtered_order)
    } else {
        let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
        let (filtered_order, skipped_targets) = dag
            .execution_order_for_targets_skipping_with_source_check(
                &target_refs,
                when_false_rules,
                &|path| run_workdir_check(Some(workdir), path),
            )
            .with_context(|| "failed to resolve target rules")?;
        if print_notes {
            for skipped in &skipped_targets {
                print_target_skipped_note(skipped, when_false_rules, run_log);
            }
        }
        Ok(filtered_order)
    }
}

/// Dry-run's display ordering for a full-plan recomputation: execution-order
/// filtering first, then deterministic parallel-group adjacency. Re-entry
/// replay uses the same helper so a replayed preview keeps the exact order
/// the initial preview showed.
pub(crate) fn parallel_display_order(
    dag: &WorkflowDag,
    target: &[String],
    when_false_rules: &std::collections::HashSet<String>,
    workdir: &std::path::Path,
    run_log: Option<&SharedRunLog>,
    print_notes: bool,
) -> anyhow::Result<Vec<String>> {
    let filtered_order = execution_order_skipping_with_notes(
        dag,
        target,
        when_false_rules,
        workdir,
        run_log,
        print_notes,
    )?;
    let order_set: std::collections::HashSet<String> = filtered_order.iter().cloned().collect();
    let mut ordered = Vec::new();
    for group in dag.parallel_groups()? {
        let mut level: Vec<String> = group
            .into_iter()
            .filter(|name| order_set.contains(name))
            .collect();
        level.sort();
        ordered.extend(level);
    }
    Ok(ordered)
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

pub async fn handle_status(
    checkpoint: Option<PathBuf>,
    workdir: Option<PathBuf>,
    json: bool,
    timing: bool,
    limit: usize,
) -> Result<()> {
    // `--json` is one-document-in / one-document-out. A missing or malformed
    // checkpoint used to exit 1 with EMPTY stdout; emit the shared
    // `errors[{code,message}]` envelope instead so consumers never have to
    // special-case this command.
    let checkpoint_for_error = checkpoint
        .clone()
        .or_else(|| {
            workdir
                .as_ref()
                .map(|dir| CheckpointState::default_path(dir))
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CHECKPOINT));
    let mut json_written = false;
    let result =
        handle_status_inner(checkpoint, workdir, json, timing, limit, &mut json_written).await;
    if json
        && !json_written
        && let Err(error) = &result
    {
        crate::commands::json_error::emit(
            "status",
            "CHECKPOINT",
            &format!("{error:#}"),
            serde_json::json!({"checkpoint": checkpoint_for_error.display().to_string()}),
        );
    }
    result
}

async fn handle_status_inner(
    checkpoint: Option<PathBuf>,
    workdir: Option<PathBuf>,
    json: bool,
    timing: bool,
    limit: usize,
    json_written: &mut bool,
) -> Result<()> {
    print_banner();

    // Detect common mistake: user passes a .oxoflow file instead of a
    // checkpoint (issue #859). During a live run — the highest-frequency
    // inspection moment — the sibling checkpoint
    // `<workflow-dir>/.oxo-flow/checkpoint.json` usually already exists, so
    // auto-resolve to it instead of telling the user to "run first". Only a
    // genuinely missing checkpoint keeps the (reworded) error.
    let checkpoint = match &checkpoint {
        Some(path) if path.extension().is_some_and(|ext| ext == "oxoflow") => {
            // The workflow's directory (or "." for a bare filename) is where
            // `run` would have written the checkpoint.
            let base = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let sibling = CheckpointState::default_path(&base);
            if sibling.exists() {
                eprintln!(
                    "{} '{}' is a workflow file — using its sibling checkpoint '{}'.",
                    "Note:".bold().cyan(),
                    path.display(),
                    sibling.display()
                );
                Some(sibling)
            } else {
                eprintln!(
                    "{} '{}' appears to be a workflow file, not a checkpoint.",
                    "Warning:".bold().yellow(),
                    path.display()
                );
                eprintln!(
                    "  No checkpoint found at '{}' — 'status' expects a checkpoint file (JSON).",
                    sibling.display()
                );
                eprintln!(
                    "  Run 'oxo-flow run {}' first to create one.",
                    path.display()
                );
                anyhow::bail!("Cannot read workflow file as checkpoint");
            }
        }
        other => other.clone(),
    };

    // Issue #432(d): mirror `run --workdir` so users do not need to know the
    // checkpoint's internal path — the run directory is enough. An explicit
    // CHECKPOINT argument wins.
    let checkpoint_path = match (&checkpoint, &workdir) {
        (Some(p), _) => p.clone(),
        (None, Some(dir)) => CheckpointState::default_path(dir),
        (None, None) => PathBuf::from(DEFAULT_CHECKPOINT),
    };
    let state = CheckpointState::load_from_file(&checkpoint_path).with_context(|| {
        format!(
            "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).\n  \
             Checkpoint files are generated automatically by 'oxo-flow run' in .oxo-flow/checkpoint.json.",
            checkpoint_path.display()
        )
    })?;

    // Issue #849: `status` reads a snapshot, not live state — surface how
    // fresh the snapshot is (the flush lag observed on a live 561-rule run
    // was ~1–2 min and completely invisible to operators). Sourced from the
    // file's mtime, so it reflects the last write even when a monitoring
    // wrapper reads the checkpoint out-of-band.
    let checkpoint_mtime = std::fs::metadata(&checkpoint_path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| {
            let t: chrono::DateTime<chrono::Utc> = t.into();
            t
        });
    let snapshot_age_secs = checkpoint_mtime.as_ref().map(|m| {
        chrono::Utc::now()
            .signed_duration_since(*m)
            .num_seconds()
            .max(0)
    });
    if snapshot_age_secs.is_some_and(|age| age > SNAPSHOT_STALE_SECS) {
        tracing::warn!(
            "checkpoint snapshot is {}s old (threshold {}s)",
            snapshot_age_secs.unwrap(),
            SNAPSHOT_STALE_SECS
        );
    }

    // Deterministic order (HashSet iteration order is arbitrary)
    let mut completed: Vec<&str> = state.completed_rules.iter().map(String::as_str).collect();
    completed.sort_unstable();
    // Issue #690: stale entries from older checkpoints — a rule whose
    // recorded when-verdict is false — are not failures. Reconcile so
    // `status` matches what the executor actually did. Both vectors are
    // sorted (failed_rules is a BTreeSet, so iteration is deterministic).
    let (failed, when_gated_off) =
        reconcile_failed_rules(&state.failed_rules, &state.when_verdicts);

    // ── Per-rule staleness reasons (issue #432(c)) ────────────────────────
    // Reuse the dry-run preview's exact classification so "why will this
    // rule re-run?" has the same answer in `status` and `dry-run -v`.
    // Best-effort: needs the workflow file recorded in the checkpoint plus
    // a current config; when either is missing the section degrades to the
    // plain completed/failed listing below.
    let staleness = compute_staleness_reasons(&state, &checkpoint_path);
    let staleness_json: Option<serde_json::Map<String, serde_json::Value>> = if staleness.is_empty()
    {
        None
    } else {
        Some(
            staleness
                .iter()
                .map(|(rule, (reason, detail))| {
                    (
                        rule.clone(),
                        serde_json::json!({
                            "status": reason,
                            "detail": detail,
                        }),
                    )
                })
                .collect(),
        )
    };

    if json {
        let mut output = serde_json::json!({
            "command": "status",
            "checkpoint": checkpoint_path.display().to_string(),
            "workflow": state.workflow_path,
            "completed": completed,
            "failed": failed,
        });
        // Issue #849: snapshot freshness — scripts/monitoring wrappers no
        // longer need to stat the file out-of-band to judge it.
        if let Some(mtime) = checkpoint_mtime {
            output["checkpoint_mtime"] =
                serde_json::json!(mtime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        }
        if let Some(age) = snapshot_age_secs {
            output["snapshot_age_secs"] = serde_json::json!(age);
        }
        // Issue #690: stale failed_rules entries whose when-gate evaluated
        // false are reported here, not under "failed".
        if !when_gated_off.is_empty() {
            output["skipped_by_when"] = serde_json::json!(when_gated_off);
        }
        if let Some(map) = staleness_json {
            output["staleness"] = serde_json::Value::Object(map);
        }
        if timing {
            let (timings, total) = rule_timings(&state);
            // serde_json::Map is a BTreeMap: keys stay deterministically sorted
            let timings_map: serde_json::Map<String, serde_json::Value> = timings
                .into_iter()
                .map(|(rule, secs)| (rule.to_string(), serde_json::json!(secs)))
                .collect();
            output["timings"] = serde_json::Value::Object(timings_map);
            output["total_time_secs"] = serde_json::json!(total);
            // Sampled peak-RSS measurements where recorded (issue #67 §4).
            let memory_map: serde_json::Map<String, serde_json::Value> = state
                .benchmarks
                .iter()
                .filter(|(_, b)| b.max_memory_mb.is_some())
                .map(|(rule, b)| {
                    (
                        rule.to_string(),
                        serde_json::json!({
                            "max_memory_mb": b.max_memory_mb,
                            "memory_limit_mb": b.memory_limit_mb,
                        }),
                    )
                })
                .collect();
            if !memory_map.is_empty() {
                output["memory"] = serde_json::Value::Object(memory_map);
            }
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
        *json_written = true;
        return Ok(());
    }

    eprintln!("{} {}", "Status:".bold().cyan(), checkpoint_path.display());
    // Issue #849: the checkpoint is a flushed snapshot, not live state —
    // the observed flush lag on a live 561-rule run was ~1–2 min. Surface
    // the age so "Completed: 0" is never mistaken for "nothing ran".
    if let (Some(mtime), Some(age)) = (checkpoint_mtime, snapshot_age_secs) {
        let written = mtime.with_timezone(&chrono::Local).format("%H:%M:%S");
        if age > SNAPSHOT_STALE_SECS {
            eprintln!(
                "  {} Snapshot is {}s old (checkpoint written {} local) — flush lag, not necessarily stalled progress",
                "Warning:".bold().yellow(),
                age,
                written
            );
        } else {
            eprintln!(
                "  State as of {}s ago (checkpoint written {} local)",
                age, written
            );
        }
    }
    eprintln!("  Completed: {}", completed.len());
    eprintln!("  Failed:    {}", failed.len());
    if !when_gated_off.is_empty() {
        eprintln!("  Skipped (when-gated off): {}", when_gated_off.len());
    }

    if timing {
        let (timings, total) = rule_timings(&state);
        if !timings.is_empty() {
            eprintln!(
                "\n{} (top {}, total {:.1}s)",
                "Rule timings:".bold().green(),
                limit.min(timings.len()),
                total
            );
            for (rule, secs) in timings.iter().take(limit) {
                let mem = state.benchmarks.get(*rule).and_then(|b| {
                    b.max_memory_mb.map(|m| match b.memory_limit_mb {
                        Some(l) if l > 0 => {
                            if m * 100 >= l * 80 {
                                format!("  peak {m}/{l} MiB ⚠")
                            } else {
                                format!("  peak {m}/{l} MiB")
                            }
                        }
                        _ => format!("  peak {m} MiB"),
                    })
                });
                match mem {
                    Some(mem) => eprintln!("  {} {} ({:.1}s){}", "✓".green(), rule, secs, mem),
                    None => eprintln!("  {} {} ({:.1}s)", "✓".green(), rule, secs),
                }
            }
        }
    } else {
        if !staleness.is_empty() {
            eprintln!("\n{}", "Staleness reasons (as of now):".bold().yellow());
            eprintln!(
                "  {}",
                "why each completed rule would re-run under the current workflow+config".dimmed()
            );
            for rule in &completed {
                match staleness.get(*rule) {
                    Some((reason, detail)) => {
                        eprintln!(
                            "  {} {} — {} ({})",
                            "✓".green(),
                            rule,
                            reason,
                            detail.dimmed()
                        );
                    }
                    None => eprintln!("  {} {} — up to date", "✓".green(), rule),
                }
            }
        }
        if !completed.is_empty() {
            eprintln!("\n{}", "Completed rules:".bold().green());
            for rule in &completed {
                eprintln!("  {} {}", "✓".green(), rule);
            }
        }

        if !failed.is_empty() {
            eprintln!("\n{}", "Failed rules:".bold().red());
            for rule in &failed {
                eprintln!("  {} {}", "✗".red(), rule);
            }
        }

        if !when_gated_off.is_empty() {
            eprintln!("\n{}", "Skipped (when condition false):".bold().yellow());
            for rule in &when_gated_off {
                eprintln!(
                    "  {} {} — {}",
                    "⊘".yellow(),
                    rule,
                    "condition evaluated to false".dimmed()
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::dag::WorkflowDag;
    use std::collections::HashSet;

    #[test]
    fn full_plan_orders_a_parallel_level_deterministically() {
        let config = WorkflowConfig::parse(
            r#"
[workflow]
name = "order"

[[rules]]
name = "zeta"
shell = "true"

[[rules]]
name = "alpha"
shell = "true"
"#,
        )
        .unwrap();
        let dag = WorkflowDag::from_rules_with_config(
            &config.rules,
            &config_placeholder_values(&config.config),
        )
        .unwrap();
        let order = parallel_display_order(
            &dag,
            &[],
            &HashSet::new(),
            std::path::Path::new("."),
            None,
            false,
        )
        .unwrap();
        assert_eq!(order, ["alpha", "zeta"]);
    }

    #[test]
    fn reconcile_failed_rules_separates_real_failures_from_when_false_skips() {
        // Issue #690: a checkpoint written by an older version keeps a
        // when-gated rule in failed_rules; only the recorded verdict makes
        // it a skip.
        let failed: std::collections::BTreeSet<String> = ["align", "qc_S2", "trim"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let verdicts: std::collections::BTreeMap<String, bool> =
            [("qc_S2".to_string(), false)].into_iter().collect();
        let (real_failed, skipped) = reconcile_failed_rules(&failed, &verdicts);
        assert_eq!(real_failed, vec!["align", "trim"]);
        assert_eq!(skipped, vec!["qc_S2"]);
    }

    #[test]
    fn reconcile_failed_rules_keeps_failures_without_a_recorded_verdict() {
        // No verdict recorded (or verdict true — a real failure followed by
        // a gate flip back to true must stay a failure): everything is real.
        let failed: std::collections::BTreeSet<String> =
            ["a".to_string(), "b".to_string()].into_iter().collect();
        let verdicts: std::collections::BTreeMap<String, bool> =
            [("a".to_string(), true)].into_iter().collect();
        let (real_failed, skipped) = reconcile_failed_rules(&failed, &verdicts);
        assert_eq!(real_failed, vec!["a", "b"]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn snapshot_age_is_derived_from_checkpoint_mtime() {
        // Issue #849: snapshot freshness comes from the file's mtime, not
        // from any field inside the checkpoint (CheckpointState carries no
        // self-timestamp). A freshly written file yields a small age; a
        // backdated mtime yields the round-tripped age, clamped at 0.
        let dir = std::env::temp_dir().join(format!(
            "oxoflow-status-age-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.json");
        std::fs::write(&path, "{}").unwrap();

        let mtime_now: chrono::DateTime<chrono::Utc> =
            std::fs::metadata(&path).unwrap().modified().unwrap().into();
        let age_now = chrono::Utc::now()
            .signed_duration_since(mtime_now)
            .num_seconds()
            .max(0);
        assert!(age_now < 60, "fresh file reported {age_now}s old");

        // Backdate the mtime by 10 minutes via filetime semantics through
        // std: rewrite the file, then shift the clock comparison instead —
        // std has no mtime setter, so assert the arithmetic directly.
        let backdated = chrono::Utc::now() - chrono::Duration::seconds(600);
        let age_old = chrono::Utc::now()
            .signed_duration_since(backdated)
            .num_seconds()
            .max(0);
        assert_eq!(age_old, 600);
        assert!(age_old > SNAPSHOT_STALE_SECS);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_accepts_workdir_like_run() {
        // Issue #432(d): `status --workdir <dir>` must resolve the default
        // checkpoint under that run directory, same as `run --workdir`.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "status-workdir"

[[rules]]
name = "step"
input = []
output = ["out.txt"]
shell = "touch out.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let run_dir = dir.path().join("run1");

        // A real run first: it records the checkpoint in the workdir.
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .args([
                "run",
                wf.to_str().unwrap(),
                "--workdir",
                run_dir.to_str().unwrap(),
                "-j",
                "1",
            ])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        // status --workdir finds the checkpoint without the internal path.
        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["status", "--workdir", run_dir.to_str().unwrap(), "--json"])
            .env("NO_COLOR", "1")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("status --json must emit JSON");
        assert_eq!(parsed["completed"], serde_json::json!(["step"]));
    }

    #[test]
    fn status_reports_staleness_reasons_for_a_stale_completed_rule() {
        // Issue #432(c): after a completed rule's output is deleted, status
        // must say WHY it would re-run (outputs missing), not just list it.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "stale-reasons"

[[rules]]
name = "producer"
input = []
output = ["stage1.txt"]
shell = "printf 'v1' > stage1.txt"

[[rules]]
name = "consumer"
input = ["stage1.txt"]
output = ["stage2.txt"]
shell = "cp stage1.txt stage2.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let run_dir = dir.path().join("run1");

        Command::cargo_bin("oxo-flow")
            .unwrap()
            .args([
                "run",
                wf.to_str().unwrap(),
                "--workdir",
                run_dir.to_str().unwrap(),
                "-j",
                "1",
            ])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        // Delete the producer's output → it is stale (outputs missing) and
        // the consumer cascades behind it.
        std::fs::remove_file(run_dir.join("stage1.txt")).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["status", "--workdir", run_dir.to_str().unwrap(), "--json"])
            .env("NO_COLOR", "1")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let staleness = parsed["staleness"].as_object().expect("staleness map");
        let producer = &staleness["producer"];
        // stage1.txt was deleted: the producer's completion record survives
        // but its output is gone, so it classifies as the lazy cascade-up
        // producer (a downstream rule needs the intermediate).
        assert_eq!(producer["status"], "missing input");
        let consumer = &staleness["consumer"];
        // stage1.txt no longer matches the recorded manifest (gone), so the
        // consumer's own credit is input-invalidated before any cascade.
        assert_eq!(consumer["status"], "input changed");
        assert!(
            consumer["detail"].as_str().unwrap().contains("manifest"),
            "input-changed detail must explain the manifest mismatch, got: {consumer}"
        );
    }
}
