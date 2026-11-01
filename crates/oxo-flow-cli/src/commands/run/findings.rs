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
