//! Pre-run findings printers: sensitive-value masking, checkpoint adoption, config-change summary, source-placeholder/module hints, undefined-config and missing-source findings (#948 P2-5).

use super::*;

/// Values of `[config_meta.*] sensitive = true` keys, for runner-side
/// output masking (issue #99 B1). Shared by the local run, the verbose
/// plan print, and the cluster path.
pub(crate) fn sensitive_values_of(config: &WorkflowConfig) -> Vec<String> {
    config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .filter_map(|(key, _)| config.config.get(key))
        .map(oxo_flow_core::config_impact::config_value_string)
        .collect()
}

/// The NAMES of the sensitive config keys — the snapshot-drift warning and
/// config-change detection compare keys, not values (issue #99 B1).
pub(crate) fn sensitive_keys_of(config: &WorkflowConfig) -> std::collections::HashSet<String> {
    config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .map(|(key, _)| key.clone())
        .collect()
}

/// Adopt a loaded checkpoint for the workflow about to run (audit B8).
///
/// A checkpoint lives at `<workdir>/.oxo-flow/checkpoint.json`, so two
/// workflows pointed at the same directory read each other's records and
/// would reuse one another's artifacts: the freshness gate only compares
/// paths, mtimes, and content hashes, never workflow identity. A checkpoint
/// recorded under a different workflow path therefore has its reuse records
/// dropped (every rule re-executes), and one that predates workflow tracking
/// keeps the historical behavior with a note.
pub(crate) fn adopt_checkpoint_for_workflow(
    ck: &mut CheckpointState,
    workflow: &Path,
    run_log: Option<&SharedRunLog>,
) {
    match ck.foreign_workflow(workflow) {
        Some(true) => {
            diagnostic_narrate(
                format_args!(
                    "  {} checkpoint belongs to a different workflow — ignoring its \
                     completed/failed records so nothing is reused (every rule re-executes)",
                    "Note:".yellow()
                ),
                run_log,
            );
            ck.invalidate_reuse_records();
        }
        Some(false) => {}
        None => {
            tracing::debug!(
                "checkpoint predates workflow-identity tracking; reuse checks stay \
                 path/mtime/fingerprint based"
            );
        }
    }
}

/// How the loading command consumes a possibly-broken checkpoint.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CheckpointLoadMode {
    /// `run`: a corrupt file refuses to start (issue #432(a) — silent
    /// degradation would re-execute every completed rule, which costs
    /// hours on real data), and cleanup side effects are persisted.
    Execute,
    /// Read-only previews: never persist; a corrupt file degrades to an
    /// empty state with a warning that names the corruption — dry-run
    /// stays usable as the recovery tool, but the old catch-all "no
    /// checkpoint" message must not mislabel a corrupt file as absent.
    ReadOnlyPreview,
}

/// The checkpoint-load phase `run` and `dry-run` share: load from `path`,
/// adopt for `workflow` (audit B8), and clear stale `running` entries
/// (issue #685 — the previous run died between spawn and any terminal
/// state, so no task is executing those rules now). Returns the state and
/// the names cleared so the caller can narrate them. Under [`CheckpointLoadMode::Execute`]
/// the corruption check fails loudly and the cleanup is persisted; under
/// [`CheckpointLoadMode::ReadOnlyPreview`] nothing is written. `run_log`
/// receives the adoption notes only in Execute mode.
pub(crate) fn load_checkpoint_for_command(
    checkpoint_path: &Path,
    workflow: &Path,
    mode: CheckpointLoadMode,
    run_log: Option<&SharedRunLog>,
) -> anyhow::Result<(CheckpointState, Vec<String>)> {
    let mut state = if checkpoint_path.exists() {
        match CheckpointState::load_from_file(checkpoint_path) {
            Ok(ck) => ck,
            Err(e) => match mode {
                CheckpointLoadMode::Execute => {
                    return Err(anyhow::anyhow!(
                        "checkpoint at {} is corrupt and cannot be loaded ({}); \
                         refusing to start — every completed rule would silently re-run. \
                         Fix or remove the file, then retry.",
                        checkpoint_path.display(),
                        e
                    ));
                }
                CheckpointLoadMode::ReadOnlyPreview => {
                    eprintln!(
                        "{} checkpoint at {} is corrupt and cannot be loaded ({}) — \
                         planning as if every rule never completed; fix or remove the \
                         file for an accurate plan",
                        "⚠".yellow(),
                        checkpoint_path.display(),
                        e
                    );
                    CheckpointState::default()
                }
            },
        }
    } else {
        if matches!(mode, CheckpointLoadMode::ReadOnlyPreview) {
            eprintln!(
                "{} no checkpoint at {} — treating every rule as never completed",
                "⚠".yellow(),
                checkpoint_path.display()
            );
        }
        CheckpointState::default()
    };
    match mode {
        CheckpointLoadMode::Execute => {
            adopt_checkpoint_for_workflow(&mut state, workflow, run_log);
        }
        CheckpointLoadMode::ReadOnlyPreview => {
            adopt_checkpoint_for_workflow(&mut state, workflow, None);
        }
    }
    let stale_running: Vec<String> = state.running_rules().iter().cloned().collect();
    if !stale_running.is_empty() {
        for rule in &stale_running {
            state.clear_running(rule);
        }
        if matches!(mode, CheckpointLoadMode::Execute) {
            // Persist the cleanup so reuse and resume accounting stay
            // honest even if this invocation dies right after.
            if let Err(e) = state.save_to_file(checkpoint_path) {
                tracing::warn!("Failed to persist cleaned checkpoint: {e}");
            }
        }
    }
    Ok((state, stale_running))
}

/// Snapshot drift warning (issue #243) — the checkpoint records the
/// EFFECTIVE config of the original run (workflow defaults + its CLI
/// overrides). This invocation re-parsed the workflow with THIS
/// invocation's overrides — `resume` passes none, so plan-time gates
/// (pair/instance `when`, `{meta.*}` baking) re-judge on defaults and the
/// instance set can drift from the original run before any invalidation
/// machinery sees it. Invalidation covers shell interpolation and gate
/// flips; it cannot resurrect instances expansion already dropped. Warn
/// loudly, naming every key that differs — the operator decides whether
/// to re-pass the overrides. Returns the drifted keys (engine-injected
/// keys excluded; empty when the checkpoint has no completed rules to
/// drift against, so a fresh run never warns).
pub(crate) fn config_snapshot_drift(
    ck: &CheckpointState,
    config: &WorkflowConfig,
    sensitive_keys: &HashSet<String>,
) -> Vec<String> {
    let drifted: Vec<String> = ck
        .config_snapshot
        .keys()
        .filter(|key| {
            !oxo_flow_core::config_impact::is_engine_injected_key(key)
                && match config.config.get(key.as_str()) {
                    Some(value) => {
                        let new = oxo_flow_core::config_impact::snapshot_value(
                            value,
                            sensitive_keys.contains(key.as_str()),
                        );
                        &new != ck.config_snapshot.get(*key).unwrap()
                    }
                    None => true,
                }
        })
        .cloned()
        .collect();
    if drifted.is_empty() || ck.completed_rules.is_empty() {
        return Vec::new();
    }
    eprintln!(
        "{} effective config differs from the original run's snapshot — \
         plan-time gated instances (pair/sample `when`, `{{meta.*}}`) may \
         drift from the recorded run. Re-pass the original overrides to \
         resume the same instance set:",
        "Config drift:".bold().yellow()
    );
    for key in &drifted {
        match config.config.get(key.as_str()) {
            Some(value) => {
                let new = oxo_flow_core::config_impact::snapshot_value(
                    value,
                    sensitive_keys.contains(key.as_str()),
                );
                let old = ck.config_snapshot.get(key.as_str()).unwrap();
                let (old, new) = mask_snapshot_pair(old, &new);
                eprintln!("  {key}: {old} → {new}");
            }
            None => eprintln!("  {key}: (removed from this invocation)"),
        }
    }
    drifted
}

/// Recorded vs current snapshot values are content hashes for keys whose
/// raw values never print (issue #99 B1 family): a `sha256:` on either
/// side means the pair is masked — printing the hash of a secret is
/// still a leak.
fn mask_snapshot_pair(old: &str, new: &str) -> (String, String) {
    if old.starts_with("sha256:") || new.starts_with("sha256:") {
        ("****".to_string(), "****".to_string())
    } else {
        (old.to_string(), new.to_string())
    }
}

/// What the input-manifest phase hands back to the run loop: the rules to
/// force on the next plan and the protected-tree drift evidence the
/// fail-gate after the phase reads.
pub(crate) struct InputManifestPhase {
    pub force_rules: Vec<String>,
    pub protected_drift_lines: Vec<String>,
    pub protected_drift: bool,
}

/// The input-manifest comparison phase (issue #72): a completed rule is
/// reused only when the file set its inputs resolved to at completion time
/// (paths + size + mtime) still matches. Globs and Dir inputs detect
/// added/removed files; plain files detect edits. A mismatch invalidates
/// the rule and its DAG downstream (the same cascade as config changes);
/// legacy checkpoints (no manifest recorded) adopt the current set as a
/// one-time baseline — the same policy as the config snapshot (issue #62).
/// Detection is shared with dry-run's read-only preview (issue #66); this
/// side MUTATES the checkpoint (cascades, baselines) and narrates every
/// category, then persists both detections' side effects in one save
/// (issue #72 + #935 P1). Protected-tree drift state (issue #935 P1) is
/// returned so the fail-gate in the caller can read it.
#[allow(clippy::too_many_arguments)] // phase plumbing: detection inputs + persistence + narration target
pub(crate) fn apply_input_manifest_phase(
    ck: &mut CheckpointState,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    checkpoint_path: &Path,
    run_log: Option<&SharedRunLog>,
) -> InputManifestPhase {
    let mut force_rules: Vec<String> = Vec::new();
    let (
        mismatched,
        missing_inputs,
        baselined,
        sample_selection_driven,
        input_diffs,
        symlink_notices,
    ) = crate::commands::run_preview::detect_input_manifest_invalidations(
        ck,
        config,
        dag,
        order,
        workdir,
        wildcard_values,
    );
    // Issue #142 M1: a rule whose only input-manifest change is the
    // engine-injected sample list is NOT invalidated — toggling --samples
    // must not overwrite cohort-level gather outputs with a subset table.
    // Emit the documented warning; the detection function already proved
    // (re-expansion + content-identity) the change is sample-selection-only.
    if !sample_selection_driven.is_empty() {
        let names: Vec<&str> = sample_selection_driven.iter().map(String::as_str).collect();
        diagnostic_narrate(
            format_args!(
                "  {} skipped {} whose inputs only changed with the --samples selection: {} — outputs still cover the previous run's full sample set; use --rerun to regenerate with the new selection",
                "⚠".yellow(),
                oxo_flow_core::plural(names.len(), "rule", "rules"),
                names.join(", ")
            ),
            run_log,
        );
    }
    // Cascade-up: missing inputs (tombstoned temporaries) re-run their
    // completed producers first — the same semantics the preview shows.
    // Empty upstream means NO completed producer exists for the missing
    // inputs (e.g. a deleted external source file with no producer rule
    // at all) — re-running "0 producers" silently and exiting 0 would
    // claim success from stale outputs, so surface each affected rule's
    // missing patterns loudly instead (never "re-running 0 producer(s)").
    if !missing_inputs.is_empty() {
        let upstream = crate::commands::run_preview::cascade_up(ck, dag, &missing_inputs);
        force_rules.extend(upstream.iter().cloned());
        if upstream.is_empty() {
            let lines = missing_source_lines(&missing_inputs, config, workdir, wildcard_values);
            diagnostic_narrate(
                format_args!(
                    "  {} {} missing and cannot be regenerated in this run — affected rule{}:\n{}",
                    "⚠".yellow(),
                    oxo_flow_core::plural(
                        missing_inputs.len(),
                        "required input",
                        "required inputs"
                    ),
                    if missing_inputs.len() == 1 { ":" } else { "s:" },
                    lines.join("\n")
                ),
                run_log,
            );
        } else {
            diagnostic_narrate(
                format_args!(
                    "  {} missing intermediate inputs — re-running {}: {}",
                    "↻".yellow(),
                    oxo_flow_core::plural(upstream.len(), "producer rule", "producer rules"),
                    upstream.join(", ")
                ),
                run_log,
            );
        }
    }
    if !mismatched.is_empty() {
        let invalidated =
            crate::commands::run_preview::invalidate_with_downstream(ck, dag, &mismatched);
        force_rules.extend(invalidated.iter().cloned());
        // Issue #432(a): say WHY completed credit was lost when the
        // likely cause is a prior failed run's side effects — the issue
        // reports users reading this as "restarts lose credit" when it
        // is deterministic invalidation (shared-dir side effects shift
        // glob/dir input sets, which cascades downstream).
        let prior_failures = !ck.failed_rules.is_empty();
        diagnostic_narrate(
            format_args!(
                "  {} input changes invalidated {}: {}{}",
                "↻".yellow(),
                oxo_flow_core::plural(invalidated.len(), "rule", "rules"),
                invalidated.join(", "),
                if prior_failures {
                    " — the previous run failed after some rules wrote into shared input dirs (logs/ QC dirs); their inputs changed on disk, so affected rules re-run. This is expected invalidation, not lost credit"
                } else {
                    ""
                }
            ),
            run_log,
        );
        // Issue #926 P0: say WHAT changed — one describe() line per
        // affected file, so a size collapse to 0 B shows `(TRUNCATED)`
        // and symlink churn shows `[symlink]` instead of a bare rule
        // name. The diff map only carries invalidated seeds (not their
        // cascaded dependents, whose inputs did not change on disk).
        let mut diff_names: Vec<&String> = mismatched.iter().collect();
        diff_names.sort();
        for name in diff_names {
            if let Some(lines) = input_diffs.get(name) {
                for line in lines {
                    diagnostic_narrate(format_args!("    {} {}", "·".yellow(), line), run_log);
                }
            }
        }
    }
    // Issue #926 P0 item 3: first-seen symlink notice — regardless of
    // invalidation (a same-content rewrite through a symlink leaves the
    // manifests matching, which is exactly the incident #926 documents),
    // every first-seen symlinked input is named once per run.
    if !symlink_notices.is_empty() {
        for line in &symlink_notices {
            diagnostic_narrate(format_args!("  {} {}", "ℹ".cyan(), line), run_log);
        }
    }
    if baselined > 0 {
        diagnostic_narrate(
            format_args!(
                "  Note: checkpoint predates input tracking: recorded baseline input manifests for {} completed; future input changes will invalidate them automatically",
                oxo_flow_core::plural(baselined, "rule", "rules")
            ),
            run_log,
        );
    }
    // ── Protected-tree drift (issue #935 P1) ─────────────────────────
    // Whole-tree baselines opt-in via [engine] protected_data_trees:
    // first observation adopts, later observations diff and report one
    // describe() line per drifted file. Run FAILS on drift unless
    // --allow-protected-drift — this is the fast, loud out-of-band
    // damage detector (a truncated vendor FASTQ surfaces here within
    // minutes, not one rule at a time).
    let (protected_drift_lines, protected_baselined, protected_warnings) =
        crate::commands::run_preview::detect_protected_tree_drift(ck, config, workdir);
    let protected_drift = !protected_drift_lines.is_empty();
    for line in &protected_warnings {
        diagnostic_narrate(format_args!("  {} {}", "⚠".yellow(), line), run_log);
    }
    if protected_baselined > 0 {
        diagnostic_narrate(
            format_args!(
                "  Note: {} — future changes will be flagged by plan/dry-run/run",
                oxo_flow_core::plural(
                    protected_baselined,
                    "baseline recorded for protected tree",
                    "baselines recorded for protected trees"
                )
            ),
            run_log,
        );
    }
    if protected_drift {
        diagnostic_narrate(
            format_args!(
                "  {} PROTECTED TREE DRIFT detected in {} file(s):",
                "✗".red().bold(),
                protected_drift_lines.len()
            ),
            run_log,
        );
        for line in &protected_drift_lines {
            diagnostic_narrate(format_args!("    {} {}", "·".red(), line), run_log);
        }
    }
    // One save covers both detections (issue #72 + #935 P1): manifest
    // baselines/diffs and protected-tree baselines must both persist.
    if (!mismatched.is_empty() || baselined > 0 || protected_baselined > 0 || protected_drift)
        && let Err(e) = ck.save_to_file(checkpoint_path)
    {
        tracing::warn!(
            error = %e,
            "failed to save checkpoint after input-manifest/protected-tree detection"
        );
    }
    InputManifestPhase {
        force_rules,
        protected_drift_lines,
        protected_drift,
    }
}

pub(crate) fn print_truncated_list(label: &str, names: &[String]) -> String {
    // Truncate the list — a cohort-wide shell edit can mismatch hundreds of
    // expanded rule instances (e.g. step5 × 100 samples).
    let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
    let extra = names.len().saturating_sub(3);
    let suffix = if extra > 0 {
        format!(", … (+{extra} more)")
    } else {
        String::new()
    };
    format!("  {label} {}{}", shown.join(", "), suffix)
}

/// Print the config-change impact summary (issue #62).
///
/// Distinguishes the full invalidation set (checkpoint mutation; includes
/// rules outside this run's targets/samples) from the rules that will
/// actually re-execute in THIS run (intersection with `order`). Under
/// `--rerun` everything is forced anyway, so the summary is suppressed —
/// the snapshot/fingerprint refresh still happened silently.
#[allow(clippy::too_many_arguments)] // report + snapshot + config + log plumbing; all needed
pub(crate) fn print_config_change_summary(
    report: &ConfigChangeReport,
    old_snapshot: &std::collections::BTreeMap<String, String>,
    config: &WorkflowConfig,
    sensitive_keys: &HashSet<String>,
    order: &[String],
    completed_in_run: usize,
    rerun: bool,
    run_log: Option<&SharedRunLog>,
) {
    if rerun {
        return;
    }
    if report.is_legacy {
        diagnostic_narrate(
            format_args!(
                "  {} checkpoint predates config tracking: recorded a baseline snapshot; \
                 future config changes will invalidate affected rules automatically",
                "Note:".yellow()
            ),
            run_log,
        );
        return;
    }
    if report.changed_keys.is_empty()
        && report.added_keys.is_empty()
        && report.removed_keys.is_empty()
        && report.fingerprint_mismatches.is_empty()
        && report.when_flip_invalidated.is_empty()
    {
        return;
    }

    let mut lines: Vec<String> = vec!["Config change:".bold().cyan().to_string()];
    for key in &report.changed_keys {
        if sensitive_keys.contains(key) {
            lines.push(format!("  {key}: **** → ****"));
        } else {
            let old = old_snapshot.get(key).map(String::as_str).unwrap_or("?");
            let new = config
                .config
                .get(key)
                .map(config_value_string)
                .unwrap_or_else(|| "?".to_string());
            lines.push(format!("  {key}: {old} → {new}"));
        }
    }
    for key in &report.added_keys {
        lines.push(format!("  {key}: (new key)"));
    }
    for key in &report.removed_keys {
        lines.push(format!("  {key}: (removed)"));
    }
    if !report.fingerprint_mismatches.is_empty() {
        lines.push(print_truncated_list(
            "rule definition changed:",
            &report.fingerprint_mismatches,
        ));
    }

    if !report.when_flip_invalidated.is_empty() {
        // Truncate like fingerprint lists — flips can cascade across many
        // expanded instances.
        lines.push(print_truncated_list(
            "when-condition flipped:",
            &report.when_flip_invalidated,
        ));
    }
    if !report.when_gate_exempt.is_empty() {
        // Issue #198: gates whose truth value survived the toggle keep their
        // checkpoint entries — say so, or the reuse looks like a bug.
        lines.push(print_truncated_list(
            "when-condition unchanged, reused:",
            &report.when_gate_exempt,
        ));
    }

    let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
    let rerun_this_run = report
        .invalidated
        .iter()
        .filter(|name| order_set.contains(name.as_str()))
        .count();
    lines.push(format!(
        "  → invalidated {} ({} directly affected), re-running {}/{} this run, skipping {}",
        report.invalidated.len(),
        report.directly_affected.len(),
        rerun_this_run,
        order.len(),
        completed_in_run,
    ));
    // The summary is multi-line; narrate it as one block so the run log keeps
    // the same grouping the console shows.
    diagnostic_narrate(format_args!("{}", lines.join("\n")), run_log);
}

/// Quote a path for safe embedding in a reference build command (executed
/// via `bash -c` / `sh -c`): wrap in single quotes and escape embedded
/// quotes with the POSIX `'\''` sequence — the same idiom the environment
/// wrapper uses when embedding commands into `sh -c '...'` and the cluster
/// renderer uses for its `cd '...'`. Without the quoting, a source path
/// containing spaces or shell metacharacters would be spliced bare into the
/// command (issue #136 tier-2 audit).
pub(crate) fn quote_shell_path(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Substitute the reference builder's `{source}` placeholder with the
/// expanded source path, shell-quoted so the path survives as one argument
/// (issue #136 tier-2 audit — the raw splice broke on spaces/metacharacters).
pub(crate) fn substitute_source_placeholder(build_cmd: &str, expanded_source: &str) -> String {
    build_cmd.replace("{source}", &quote_shell_path(expanded_source))
}

/// Render the "known modules" list for an unknown-module error. `run` and
/// `dry-run` share the phrasing — in particular the explicit empty-list
/// hint, so a workflow without includes cannot print a trailing "known
/// modules: " (issue #136 tier-2 audit).
pub(crate) fn known_modules_hint(
    module_rules: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    if module_rules.is_empty() {
        "(none — no [[include]] modules)".to_string()
    } else {
        module_rules
            .keys()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Group rule names by their owning `[[include]]` module for run output
/// (issue #887). Attribution mirrors the metro export exactly: a
/// `module::rule` namespace prefix wins, then member/instance matching via
/// [`oxo_flow_core::dag::owning_module`]; rules no module claims land in the
/// main-file bucket (`None`), titled `Main workflow` by [`run_group_title`].
/// Groups keep first-appearance order over the input and the input's order
/// within each group. `None` overall = the workflow declares no modules —
/// grouping is opt-in-by-structure, so single-file output stays flat.
pub(crate) fn module_groups_for_rules<'a>(
    names: &[&'a str],
    module_rules: &'a HashMap<String, Vec<String>>,
) -> Option<Vec<(Option<&'a str>, Vec<&'a str>)>> {
    if module_rules.is_empty() {
        return None;
    }
    // Flattened (member, module) pairs — the same shape to_metro builds.
    let module_members: Vec<(&str, &str)> = module_rules
        .iter()
        .flat_map(|(module, members)| {
            members
                .iter()
                .map(move |member| (member.as_str(), module.as_str()))
        })
        .collect();
    let mut groups: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    let mut group_index: HashMap<Option<&str>, usize> = HashMap::new();
    for &name in names {
        let group = if let Some((prefix, _)) = name.split_once("::") {
            Some(prefix)
        } else {
            oxo_flow_core::dag::owning_module(name, &module_members)
        };
        let idx = *group_index.entry(group).or_insert_with(|| {
            groups.push((group, Vec::new()));
            groups.len() - 1
        });
        groups[idx].1.push(name);
    }
    Some(groups)
}

/// Display title for a run-output group: curated module names via
/// [`oxo_flow_core::stage::module_display`], `Main workflow` for the
/// main-file bucket.
pub(crate) fn run_group_title(group: Option<&str>) -> String {
    match group {
        Some(module) => oxo_flow_core::stage::module_display(module),
        None => "Main workflow".to_string(),
    }
}

/// Issue #142 H1 gate: an unknown `{config.*}` placeholder used to expand to
/// literal text while the run exited 0 — silent wrong outputs. This is the
/// same E005 detector `validate` uses, applied as a hard gate by both `run`
/// and `dry-run`, so the three surfaces can never disagree about a typo'd
/// key. Returns the human-readable findings (rule, key, fix) or empty.
pub(crate) fn undefined_config_findings(config: &WorkflowConfig) -> Vec<String> {
    config
        .rules
        .iter()
        .flat_map(|rule| oxo_flow_core::format::undefined_config_refs(rule, config))
        .map(|d| {
            format!(
                "rule '{}': {} ({})",
                d.rule.as_deref().unwrap_or("<unknown>"),
                d.message,
                d.suggestion.unwrap_or_default()
            )
        })
        .collect()
}

/// Fail-fast gate for EXTERNAL source inputs: a required input pattern that
/// resolves to concrete missing files AND has no producer rule in the DAG can
/// never be regenerated by this run. Previously such a run could complete
/// silently when every rule already held checkpoint credit — the manifest
/// detector flagged the missing source, but the cascade-up found no completed
/// producer (external files have none) and the run exited 0 claiming
/// success while its declared inputs no longer existed on disk.
///
/// Scoped to no-producer patterns only: generated intermediates have
/// producers and stay owned by the checkpoint cascade path, which handles
/// tombstones and recorded when-verdicts (this gate runs before the
/// checkpoint is even loaded, so it must not duplicate that logic).
/// Optional rules, when-gated-off rules, and scatter rules with zero
/// fan-out can legitimately have absent inputs and are skipped — the same
/// stances the validate/quality gates take (issues #493, #616).
///
/// Two further engine-tolerated shapes must not be flagged:
/// - `[[references]]` outputs: the reference-build machinery creates them
///   during the run (after this gate) and they are not DAG nodes;
/// - inputs living in the `sample_pattern` discovery directory: bare
///   relative inputs resolve from the shell cwd (= workdir) at execution,
///   but sample data legitimately lives where sample_pattern found it.
///
/// Returns human-readable findings, or empty when every required external
/// source is present.
pub(crate) fn missing_source_findings(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    workflow_dir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    // Outputs the reference-build machinery will create during this run —
    // absent from the DAG by design, so a missing one is not a dead source.
    let reference_outputs: HashSet<String> = config
        .references
        .iter()
        .map(|r| {
            oxo_flow_core::executor::checkpoint::expand_config_in_path(&r.output, wildcard_values)
        })
        .collect();
    // Directory sample_pattern discovery scanned (parse.rs anchoring):
    // absolute patterns anchor at their own parent, relative ones at the
    // workflow file's parent. Bare relative inputs may live here instead
    // of the workdir.
    let sample_search_dir = config.workflow.sample_pattern.as_ref().and_then(|pattern| {
        let expanded =
            oxo_flow_core::executor::checkpoint::expand_config_in_path(pattern, wildcard_values);
        let path = Path::new(&expanded);
        if path.is_absolute() {
            path.parent().map(|p| p.to_path_buf())
        } else {
            // Relative pattern: parse.rs anchors it at the workflow file's
            // parent (mirror of the sample_pattern split there).
            let joined = workflow_dir.join(path);
            Some(
                joined
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| workflow_dir.to_path_buf()),
            )
        }
    });
    // Would-be outputs of fan-out instances vetoed by their `when`,
    // config-expanded at compare time: the record site (`expand_rule_patterns`,
    // #882) expands only `\w+` wildcards and `{values.*}`, so `{config.*}`
    // tokens stay raw in the set while the gate's patterns are expanded
    // (issue #915 review).
    let when_skipped: HashSet<String> = config
        .when_skipped_outputs
        .iter()
        .map(|recorded| {
            oxo_flow_core::executor::checkpoint::expand_config_in_path(recorded, wildcard_values)
        })
        .collect();
    let mut findings = Vec::new();
    for name in order {
        let Some(rule) = config.get_rule(name) else {
            continue;
        };
        // A `when`-gated-off rule never runs in this configuration — its
        // inputs are not required to exist (issue #493 stance).
        if crate::commands::run_preview::when_condition_false(rule, config, wildcard_values) {
            continue;
        }
        // A scatter rule with zero fan-out never instantiates (issue #616).
        if rule.scatter.as_ref().is_some_and(|scatter| {
            if !scatter.values.is_empty() {
                return false;
            }
            match scatter.values_from.as_deref() {
                None => true,
                Some(values_from) => config
                    .resolve_config_list(values_from)
                    .is_some_and(|values| values.is_empty()),
            }
        }) {
            continue;
        }
        // Optional rules may be absent by design.
        if rule.optional.is_optional() {
            continue;
        }
        let missing = oxo_flow_core::executor::checkpoint::missing_input_patterns(
            rule,
            workdir,
            wildcard_values,
        );
        for pattern in missing {
            if reference_outputs.contains(&pattern) {
                // Built by the [[references]] machinery later in this run.
                continue;
            }
            // Only external sources gate here — a missing pattern WITH a
            // producer belongs to the manifest/cascade path below.
            if dag.producer_of(&pattern).is_some() {
                continue;
            }
            // Issue #915: a fan-out instance vetoed by its `when` never
            // instantiates, so its declared outputs have no producer here —
            // but they were recorded at expansion (#882). A missing input
            // matching one of those recorded patterns belongs to a rule
            // that will never run in this cohort, not to a dead source.
            if when_skipped.contains(&pattern) {
                continue;
            }
            // A bare relative input may resolve inside the sample_pattern
            // discovery directory rather than the workdir.
            let absolute = Path::new(&pattern).is_absolute();
            if !absolute
                && sample_search_dir
                    .as_ref()
                    .is_some_and(|dir| dir.join(&pattern).exists())
            {
                continue;
            }
            findings.push(format!(
                "rule '{name}': input '{pattern}' does not exist and no rule produces it"
            ));
        }
    }
    findings
}

/// Per-rule listing for the loud empty-cascade warning: names each affected
/// rule and the concrete input patterns that cannot be resolved. Extracted
/// so tests can assert the message content without driving a full run.
pub(crate) fn missing_source_lines(
    missing_inputs: &HashSet<String>,
    config: &WorkflowConfig,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    let mut names: Vec<&String> = missing_inputs.iter().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let patterns = config
                .get_rule(name)
                .map(|rule| {
                    oxo_flow_core::executor::checkpoint::missing_input_patterns(
                        rule,
                        workdir,
                        wildcard_values,
                    )
                })
                .unwrap_or_default();
            let listing = if patterns.is_empty() {
                "(unresolvable)".to_string()
            } else {
                patterns.join(", ")
            };
            format!("    {name} — missing: {listing}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_config_with(toml_body: &str) -> WorkflowConfig {
        let toml = format!(
            r#"
[workflow]
name = "t"
version = "1.0"
{toml_body}
"#
        );
        WorkflowConfig::parse(&toml).expect("test workflow config parses")
    }

    #[test]
    fn snapshot_drift_names_changed_and_removed_keys_only() {
        // issue #243: changed and removed keys drift; untouched keys and
        // engine-injected keys (samples churn) never warn.
        let mut ck = CheckpointState::default();
        ck.completed_rules.insert("gen".to_string());
        ck.config_snapshot
            .insert("threads".to_string(), "4".to_string());
        ck.config_snapshot
            .insert("same".to_string(), "keep".to_string());
        ck.config_snapshot
            .insert("samples_list".to_string(), "old".to_string());
        ck.config_snapshot
            .insert("gone".to_string(), "x".to_string());
        let config = workflow_config_with(
            r#"
[config]
threads = "8"
same = "keep"
samples_list = "new"
"#,
        );
        let sensitive = std::collections::HashSet::new();
        let drifted = config_snapshot_drift(&ck, &config, &sensitive);
        assert_eq!(drifted, vec!["gone".to_string(), "threads".to_string()]);
    }

    #[test]
    fn snapshot_drift_stays_silent_without_completed_rules() {
        // A fresh checkpoint has nothing to drift against — a fresh run
        // with different defaults is not drift, it is the first run.
        let ck = CheckpointState::default();
        let config = workflow_config_with("[config]\nthreads = \"8\"\n");
        let sensitive = std::collections::HashSet::new();
        assert!(config_snapshot_drift(&ck, &config, &sensitive).is_empty());
    }

    #[test]
    fn snapshot_pair_hash_is_masked_on_either_side() {
        // Printing the hash of a secret is still a leak (issue #99 B1
        // family): a sha256: prefix on the recorded OR the current value
        // masks the whole pair.
        assert_eq!(
            mask_snapshot_pair("sha256:abc", "sha256:def"),
            ("****".to_string(), "****".to_string())
        );
        assert_eq!(
            mask_snapshot_pair("plain", "sha256:def"),
            ("****".to_string(), "****".to_string())
        );
        assert_eq!(
            mask_snapshot_pair("plain", "other"),
            ("plain".to_string(), "other".to_string())
        );
    }

    fn corrupt_checkpoint(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join(".oxo-flow");
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("checkpoint.json");
        std::fs::write(&file, "{ not json at all").unwrap();
        file
    }

    #[test]
    fn execute_mode_refuses_to_start_on_a_corrupt_checkpoint() {
        // issue #432(a): silent degradation would re-execute every
        // completed rule — run must fail loudly instead.
        let tmp = tempfile::tempdir().unwrap();
        let path = corrupt_checkpoint(tmp.path());
        let err = load_checkpoint_for_command(
            &path,
            std::path::Path::new("t.oxoflow"),
            CheckpointLoadMode::Execute,
            None,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("refusing to start"), "{err}");
    }

    #[test]
    fn preview_mode_degrades_on_corruption_and_stays_read_only() {
        // Dry-run is the recovery tool: a corrupt file degrades to empty
        // state with a warning that names the corruption (not "no
        // checkpoint"), and the file on disk is left untouched.
        let tmp = tempfile::tempdir().unwrap();
        let path = corrupt_checkpoint(tmp.path());
        let before = std::fs::read_to_string(&path).unwrap();
        let (state, stale) = load_checkpoint_for_command(
            &path,
            std::path::Path::new("t.oxoflow"),
            CheckpointLoadMode::ReadOnlyPreview,
            None,
        )
        .unwrap();
        assert!(state.completed_rules.is_empty());
        assert!(stale.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn stale_running_entries_are_cleared_once_per_mode() {
        // issue #685: persisted `running` entries mean the previous run
        // died between spawn and any terminal state. Execute persists the
        // cleanup; the read-only preview clears only its in-memory copy.
        for mode in [
            CheckpointLoadMode::Execute,
            CheckpointLoadMode::ReadOnlyPreview,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join(".oxo-flow");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("checkpoint.json");
            let mut seeded = CheckpointState::default();
            seeded.mark_running("gen");
            seeded.save_to_file(&path).unwrap();

            let (state, stale) =
                load_checkpoint_for_command(&path, std::path::Path::new("t.oxoflow"), mode, None)
                    .unwrap();
            assert_eq!(stale, vec!["gen".to_string()]);
            assert!(state.running_rules().is_empty());

            let on_disk = CheckpointState::load_from_file(&path).unwrap();
            match mode {
                CheckpointLoadMode::Execute => {
                    assert!(on_disk.running_rules().is_empty());
                }
                CheckpointLoadMode::ReadOnlyPreview => {
                    assert!(on_disk.running_rules().contains("gen"));
                }
            }
        }
    }
}
