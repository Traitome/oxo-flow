//! Post-execution completion handling: workflow completion event, final counts narration,
//! report snapshot, provenance audit warnings, terminal hooks & webhooks,
//! failed & blocked rules summaries, output existence verification,
//! chunk & temporary file cleanup (tombstoning), pilot scaling projection & AI report,
//! and final JSON summary emission.

use super::*;
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use oxo_flow_core::wildcard;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Contextual state needed to finalize workflow execution and produce post-run summaries.
pub(crate) struct CompletionContext<'a> {
    pub run_started: std::time::Instant,
    pub success_count: usize,
    pub fail_count: usize,
    pub non_required_fail_count: usize,
    pub skipped_count: usize,
    pub checkpoint: &'a mut CheckpointState,
    pub checkpoint_path: &'a Path,
    pub failures: &'a [(String, String)],
    pub blocked: &'a [(String, String)],
    pub workflow: &'a Path,
    pub workdir_actual: &'a Path,
    pub tmp_dir_resolved: Option<&'a Path>,
    pub run_log: &'a SharedRunLog,
    pub no_report_snapshot: bool,
    pub provenance: bool,
    pub provenance_recorded: usize,
    pub config: &'a WorkflowConfig,
    pub order: &'a [String],
    pub wildcard_values: &'a HashMap<String, String>,
    pub disk_thresholds: Option<(u64, u64)>,
    pub dag: &'a WorkflowDag,
    pub samples_filter: &'a [String],
    pub json_summary: &'a mut RunJsonSummary,
}

/// Executes all post-loop completion tasks and returns the final workflow execution Result.
pub(crate) async fn handle_run_completion(ctx: CompletionContext<'_>) -> anyhow::Result<()> {
    emit_execution_event(oxo_flow_core::executor::ExecutionEvent::WorkflowCompleted {
        total_duration_ms: ctx.run_started.elapsed().as_millis() as u64,
        succeeded: ctx.success_count,
        failed: ctx.fail_count,
        skipped: ctx.skipped_count,
    });
    if ctx.non_required_fail_count > 0 {
        progress_narrate(
            format_args!(
                "\n{} {} succeeded, {} skipped, {} failed, {} non-required failed",
                "Done:".bold(),
                ctx.success_count,
                ctx.skipped_count,
                ctx.fail_count,
                ctx.non_required_fail_count
            ),
            ctx.run_log,
        );
    } else {
        progress_narrate(
            format_args!(
                "\n{} {} succeeded, {} skipped, {} failed",
                "Done:".bold(),
                ctx.success_count,
                ctx.skipped_count,
                ctx.fail_count
            ),
            ctx.run_log,
        );
    }

    // Automatic report snapshot (issue #83 P1-14): capture the final
    // checkpoint as a JSON report plus an index.json entry. One call site
    // covers every terminal path of the run loop (and resume, which shares
    // this summary via run_command); dry-run never reaches here. Snapshot
    // errors are warnings — a reporting hiccup must never fail the run.
    if !ctx.no_report_snapshot
        && let Err(e) = crate::commands::output::snapshot_report(
            ctx.workflow,
            ctx.workdir_actual,
            ctx.checkpoint,
        )
    {
        progress_narrate(
            format_args!("  {} Report snapshot failed: {e}", "⚠".yellow()),
            ctx.run_log,
        );
    }

    // --provenance recorded nothing: say so loudly. The historical failure
    // mode was silent — the user asked for tracking, got an empty checksum
    // map, and `provenance verify` later reported "Run with --provenance"
    // after they had done exactly that (issue #276).
    if ctx.provenance && ctx.provenance_recorded == 0 {
        progress_narrate(
            format_args!(
                "  {} --provenance recorded 0 checksums — no literal outputs completed \
                 (templated or wildcard outputs are tracked per instance only after fan-out); \
                 'provenance verify' will find nothing to check",
                "⚠".yellow()
            ),
            ctx.run_log,
        );
    }

    // Workflow-level terminal hooks (issue #227 item 1): on_complete /
    // on_error fire on every terminal path of the run loop, and the
    // configured `[webhook]` endpoint receives the matching event.
    run_terminal_hook(
        ctx.config,
        ctx.workdir_actual,
        ctx.tmp_dir_resolved,
        ctx.success_count,
        ctx.fail_count,
        ctx.skipped_count,
    )
    .await;
    notify_webhook(
        ctx.config,
        if ctx.fail_count > 0 {
            oxo_flow_core::webhook::WebhookEvent::WorkflowFailed
        } else {
            oxo_flow_core::webhook::WebhookEvent::WorkflowCompleted
        },
        Some((ctx.success_count, ctx.fail_count, ctx.skipped_count)),
    )
    .await;

    // With --keep-going, execution continues past failures, so list every failed
    // rule (and why) in one place rather than making the user hunt for them.
    // Issue #887: with modules, failures and blocked rules group under the
    // same module headers as the plan listing; the flat per-line shapes are
    // unchanged (web SSE / report tooling parse these lines).
    if !ctx.failures.is_empty() {
        progress_narrate(
            format_args!("\n{}", "Failed rules:".bold().red()),
            ctx.run_log,
        );
        let names: Vec<&str> = ctx.failures.iter().map(|(name, _)| name.as_str()).collect();
        match module_groups_for_rules(&names, &ctx.config.module_rules) {
            Some(groups) => {
                for (group, members) in &groups {
                    progress_narrate(
                        format_args!("  {} ({}):", run_group_title(*group).bold(), members.len()),
                        ctx.run_log,
                    );
                    for name in members {
                        if let Some((_, reason)) = ctx.failures.iter().find(|(n, _)| n == name) {
                            progress_narrate(
                                format_args!("    {} {} — {}", "✗".red(), name.bold(), reason),
                                ctx.run_log,
                            );
                        }
                    }
                }
            }
            None => {
                for (name, reason) in ctx.failures {
                    progress_narrate(
                        format_args!("  {} {} — {}", "✗".red(), name.bold(), reason),
                        ctx.run_log,
                    );
                }
            }
        }
    }

    // Blocked rules did not run and produced no outputs. Listing them keeps the
    // fallout of a failure visible without disguising it as a genuine failure.
    if !ctx.blocked.is_empty() {
        progress_narrate(
            format_args!("\n{}", "Blocked rules (did not run):".bold().yellow()),
            ctx.run_log,
        );
        let names: Vec<&str> = ctx.blocked.iter().map(|(name, _)| name.as_str()).collect();
        match module_groups_for_rules(&names, &ctx.config.module_rules) {
            Some(groups) => {
                for (group, members) in &groups {
                    progress_narrate(
                        format_args!("  {} ({}):", run_group_title(*group).bold(), members.len()),
                        ctx.run_log,
                    );
                    for name in members {
                        if let Some((_, dep)) = ctx.blocked.iter().find(|(n, _)| n == name) {
                            progress_narrate(
                                format_args!(
                                    "    {} {} — depends on '{}'",
                                    "⊘".yellow(),
                                    name.bold(),
                                    dep
                                ),
                                ctx.run_log,
                            );
                        }
                    }
                }
            }
            None => {
                for (name, dep) in ctx.blocked {
                    progress_narrate(
                        format_args!("  {} {} — depends on '{}'", "⊘".yellow(), name.bold(), dep),
                        ctx.run_log,
                    );
                }
            }
        }
    }

    // Verify output files exist for completed rules
    if ctx.success_count > 0 {
        let mut missing_outputs = Vec::new();
        let mut verified = 0usize;
        let mut total_size: u64 = 0;
        for rule_name in ctx.order {
            if ctx.checkpoint.is_completed(rule_name)
                && let Some(rule) = ctx.config.get_rule(rule_name)
            {
                for output in &rule.output {
                    if !output.contains('{') {
                        let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            output,
                            ctx.wildcard_values,
                        );
                        let resolved = ctx.workdir_actual.join(&expanded);
                        if resolved.exists() {
                            verified += 1;
                            if let Ok(meta) = std::fs::metadata(&resolved) {
                                total_size += meta.len();
                            }
                        } else if !ctx.workdir_actual.join(output).exists() {
                            missing_outputs.push(format!("  {}: {}", rule_name, output));
                        } else {
                            verified += 1;
                        }
                    }
                }
            }
        }
        if verified > 0 {
            let size_str = if total_size > 1_073_741_824 {
                format!("{:.1}GB", total_size as f64 / 1_073_741_824.0)
            } else if total_size > 1_048_576 {
                format!("{:.1}MB", total_size as f64 / 1_048_576.0)
            } else {
                format!("{}B", total_size)
            };
            progress_narrate(
                format_args!(
                    "{} {} output files verified ({} total)",
                    "✓".green(),
                    verified,
                    size_str
                ),
                ctx.run_log,
            );
        }
        if !missing_outputs.is_empty() {
            progress_narrate(
                format_args!(
                    "{} {} were not found:",
                    "⚠".yellow(),
                    oxo_flow_core::plural(missing_outputs.len(), "output file", "output files")
                ),
                ctx.run_log,
            );
            for m in &missing_outputs {
                progress_narrate(format_args!("{}", m.dimmed()), ctx.run_log);
            }
        }
    }

    // Transform chunk cleanup (transform.cleanup = true): delete the chunk
    // inputs of completed combine rules once the whole run has finished
    // successfully. Deferred to here (instead of per-rule success) so that
    // a failed run keeps its chunks for debugging, and a resumed run never
    // regenerates chunks while the combine stays pre-skipped (which would
    // orphan the freshly created chunk files).
    if ctx.fail_count == 0 {
        // Not gated by the #893 disk floor below: chunk inputs are small
        // scratch files, and deleting them regardless of free space prevents
        // unbounded accumulation between runs.
        for rule in &ctx.config.rules {
            if rule.cleanup_chunks && ctx.checkpoint.is_completed(&rule.name) {
                // Migrate the chunk checksums BEFORE deleting the files
                // (issue #315 F2): a checkpoint that lists already-deleted
                // files as ordinary outputs would be an audit-trail lie —
                // verify would report them missing. If the migration
                // cannot be persisted, keep the files and skip the
                // deletion entirely.
                //
                // Checksums only exist under `--provenance`. With no
                // recorded checksum there is nothing that can lie, so the
                // deletion proceeds without a persistence round-trip
                // (issue #324 F-2) — previously the empty migration made
                // cleanup a silent no-op on every default run.
                let mut migrated: Vec<String> = Vec::new();
                for chunk in rule.input.iter() {
                    if let Some(sha) = ctx.checkpoint.checksums.remove(&chunk.to_string()) {
                        ctx.checkpoint
                            .record_cleaned_checksum(&chunk.to_string(), sha);
                        migrated.push(chunk.to_string());
                    }
                }
                if !migrated.is_empty()
                    && let Err(e) = ctx.checkpoint.save_to_file(ctx.checkpoint_path)
                {
                    tracing::warn!(
                        error = %e,
                        "failed to persist chunk migration — keeping chunk files"
                    );
                    // Roll the in-memory state back so a later save (e.g.
                    // tombstoning) does not persist the migration without
                    // the deletion.
                    for path in &migrated {
                        if let Some(sha) = ctx.checkpoint.cleaned_checksums.remove(path) {
                            ctx.checkpoint.checksums.insert(path.clone(), sha);
                        }
                    }
                    continue;
                }
                oxo_flow_core::executor::checkpoint::cleanup_transform_chunks(
                    rule,
                    ctx.workdir_actual,
                )
                .await;
            }
        }

        // Temporary rules: after a FULLY successful run, delete their
        // outputs once every dependent is complete and record a tombstone.
        // A future run regenerates them via cascade-up (missing inputs
        // re-run the completed producer). Leaf rules keep their outputs.
        // #893: when `[engine]` disk thresholds are configured, the sweep
        // only fires under the SAME free-space floor as the mid-run
        // reclaim — with disk comfortable, temporary outputs are kept so
        // dependent runs skip their producers instead of re-materializing
        // them every time. Without `[engine]`, the historical unconditional
        // sweep applies.
        let sweep_outputs = match endofrun_prune_decision(
            oxo_flow_core::scheduler::check_available_disk_mb(ctx.workdir_actual),
            ctx.disk_thresholds,
        ) {
            EndOfRunPruneDecision::Skip {
                free_mb,
                reclaim_mb,
            } => {
                diagnostic_narrate(
                    format_args!(
                        "ℹ free space {free_mb}MB ≥ reclaim_free_disk {reclaim_mb}MB — temporary outputs kept (disk pressure alone triggers re-materialization; remove the [engine] thresholds to always prune)"
                    ),
                    Some(ctx.run_log),
                );
                false
            }
            EndOfRunPruneDecision::KeepOnUnknown => {
                diagnostic_narrate(
                    format_args!(
                        "ℹ free space unavailable on this platform — temporary outputs kept as a precaution ([engine] thresholds are set)"
                    ),
                    Some(ctx.run_log),
                );
                false
            }
            EndOfRunPruneDecision::Sweep => true,
        };
        let mut new_tombstones: Vec<(String, Vec<String>)> = Vec::new();
        for rule in &ctx.config.rules {
            if !sweep_outputs || !rule.temporary || !ctx.checkpoint.is_completed(&rule.name) {
                continue;
            }
            // #843: mid-run reclamation may already have tombstoned this
            // rule — re-deleting would double-unlink. Skip only while the
            // reclaimed outputs are genuinely gone; a tombstoned rule whose
            // outputs are back on disk (restored by hand) must be
            // re-deleted and its tombstone refreshed. Regeneration clears
            // the tombstone at execution time (mark_completed site).
            if ctx
                .checkpoint
                .tombstones
                .get(&rule.name)
                .is_some_and(|paths| paths.iter().all(|p| !ctx.workdir_actual.join(p).exists()))
            {
                continue;
            }
            let dependents = ctx.dag.dependents(&rule.name).unwrap_or_default();
            // Issue #844: output_pattern producers register no DAG edges by
            // design (dag.rs excludes pattern-claimed outputs from template
            // matching), so `dependents` alone misclassifies them as leaves.
            // Detect consumers structurally instead: compile the producer's
            // config-expanded output_pattern and match every other rule's
            // config-expanded input paths against it. Consumer instantiation
            // bakes instance inputs ("chunks/1.txt"), which match
            // `^chunks/(?P<i>\S+)\.txt$`; consumer TEMPLATES still carry a
            // `{` placeholder and are skipped so they cannot falsely match.
            let mut consumers: HashSet<String> = dependents.iter().cloned().collect();
            if let Some(pattern) = &rule.output_pattern {
                let expanded_pattern = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    pattern,
                    ctx.wildcard_values,
                );
                if let Ok(re) = wildcard::pattern_to_regex(&expanded_pattern) {
                    for other in &ctx.config.rules {
                        if other.name == rule.name {
                            continue;
                        }
                        for input in other.input.iter() {
                            let expanded =
                                oxo_flow_core::executor::checkpoint::expand_config_in_path(
                                    input,
                                    ctx.wildcard_values,
                                );
                            if !expanded.contains('{') && re.is_match(&expanded) {
                                consumers.insert(other.name.clone());
                                break;
                            }
                        }
                    }
                }
            }
            if consumers.is_empty() {
                progress_narrate(
                    format_args!(
                        "  {} temporary rule '{}' is a leaf — outputs kept",
                        "ℹ".dimmed(),
                        rule.name
                    ),
                    ctx.run_log,
                );
                continue;
            }
            if !consumers.iter().all(|d| ctx.checkpoint.is_completed(d)) {
                let pending: Vec<&str> = consumers
                    .iter()
                    .filter(|d| !ctx.checkpoint.is_completed(d))
                    .map(String::as_str)
                    .collect();
                diagnostic_narrate(
                    format_args!(
                        "  {} temporary rule '{}' kept: dependents not complete yet ({})",
                        "!".yellow(),
                        rule.name,
                        pending.join(", ")
                    ),
                    Some(ctx.run_log),
                );
                continue;
            }
            let mut deleted = Vec::new();
            // Output patterns that still contain `{` after config
            // expansion and resolved to no concrete instance path
            // (issue #844 diagnostic) — collected during the loop below
            // so a pattern that DID expand is never falsely reported.
            let mut skipped: Vec<String> = Vec::new();
            // Instance bindings recorded for this producer template by the
            // output_pattern discovery pass (issue #844): template → one
            // combo per completed instance. Empty for plain [[values]]/
            // [[pairs]] fan-outs (their instances run under their own names
            // and expand their outputs at plan time) and for legacy
            // checkpoints.
            let template = ctx.config.output_pattern_template_of(&rule.name);
            let domains = template
                .as_deref()
                .and_then(|t| ctx.checkpoint.output_pattern_domains.get(t))
                .cloned()
                .unwrap_or_default();
            // #844: an output_pattern producer has an EMPTY `output` list —
            // the pattern itself is the deletion target, expanded per
            // recorded instance combo in the loop below. Chain it in so the
            // same expansion logic covers both declaration styles.
            for output in rule.output.iter().chain(rule.output_pattern.iter()) {
                let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    output,
                    ctx.wildcard_values,
                );
                if !expanded.contains('{') {
                    if oxo_flow_core::executor::output_invalidation::is_protected_path(
                        rule,
                        ctx.wildcard_values,
                        &expanded,
                    ) {
                        // #843: protected outputs are user-declared
                        // keep-forever files — the same gate reclamation
                        // applies, so the sweep must not delete what the
                        // monitor wouldn't.
                        continue;
                    }
                    deleted.push(expanded);
                    continue;
                }
                // #844: a pattern-level path still containing `{` after
                // config expansion is a wildcard output (e.g. per-sample
                // BAMs). Delete only the concrete instance paths that
                // actually ran — expand every recorded combo, never the
                // pattern itself.
                let mut resolved_any = false;
                for combo in &domains {
                    let instance = wildcard::expand_pattern(&expanded, combo)
                        .unwrap_or_else(|_| expanded.clone());
                    if instance.contains('{') {
                        continue;
                    }
                    resolved_any = true;
                    if oxo_flow_core::executor::output_invalidation::is_protected_path(
                        rule,
                        ctx.wildcard_values,
                        &instance,
                    ) {
                        continue;
                    }
                    if !deleted.contains(&instance) {
                        deleted.push(instance);
                    }
                }
                if !resolved_any {
                    // Expansion failure only — instances that resolved but
                    // matched `protected_output` are intentionally kept,
                    // not skipped.
                    if domains.is_empty() {
                        skipped.push(format!(
                            "{output} (no recorded instance bindings — legacy checkpoint or non-output_pattern fan-out; use `oxo-flow clean`)"
                        ));
                    } else {
                        skipped.push(format!(
                            "{expanded} (wildcard did not resolve against recorded combos)"
                        ));
                    }
                }
            }
            if !skipped.is_empty() {
                // Diagnostic (issue #844): a silent no-op is the worst
                // outcome — name the rule and the patterns that could not
                // be expanded to instance paths.
                diagnostic_narrate(
                    format_args!(
                        "  {} temporary wildcard outputs of '{}' left in place: instance paths could not be expanded: {}",
                        "!".yellow(),
                        rule.name,
                        skipped.join("; ")
                    ),
                    Some(ctx.run_log),
                );
            }
            if !deleted.is_empty() {
                new_tombstones.push((rule.name.clone(), deleted));
            }
        }
        if !new_tombstones.is_empty() {
            // Same audit rule as chunk cleanup (issue #315 F2): persist
            // tombstones + the cleaned migration BEFORE deleting, and skip
            // the deletion when the persistence fails.
            let mut rollback: Vec<String> = Vec::new();
            for (rule, paths) in &new_tombstones {
                ctx.checkpoint
                    .tombstones
                    .insert(rule.clone(), paths.clone());
                for path in paths {
                    if let Some(sha) = ctx.checkpoint.checksums.remove(path) {
                        ctx.checkpoint.record_cleaned_checksum(path, sha);
                        rollback.push(path.clone());
                    }
                }
            }
            if let Err(e) = ctx.checkpoint.save_to_file(ctx.checkpoint_path) {
                tracing::warn!(
                    error = %e,
                    "failed to persist tombstone migration — keeping temporary outputs"
                );
                for (rule, _) in &new_tombstones {
                    ctx.checkpoint.tombstones.remove(rule);
                }
                for path in &rollback {
                    if let Some(sha) = ctx.checkpoint.cleaned_checksums.remove(path) {
                        ctx.checkpoint.checksums.insert(path.clone(), sha);
                    }
                }
            } else {
                for (rule, paths) in &new_tombstones {
                    for expanded in paths {
                        let resolved = ctx.workdir_actual.join(expanded);
                        // Symlink-aware classification (review): a symlink is
                        // always unlinked as the link itself — never the
                        // bytes it points at.
                        let _ = match std::fs::symlink_metadata(&resolved) {
                            Ok(md) if md.file_type().is_symlink() => {
                                std::fs::remove_file(&resolved)
                            }
                            Ok(md) if md.is_dir() => std::fs::remove_dir_all(&resolved),
                            Ok(_) => std::fs::remove_file(&resolved),
                            Err(_) => Ok(()),
                        };
                    }
                    progress_narrate(
                        format_args!(
                            "  {} temporary outputs deleted for '{}' ({}, regenerated on demand)",
                            "⊘".dimmed(),
                            rule,
                            oxo_flow_core::plural(paths.len(), "file", "files")
                        ),
                        ctx.run_log,
                    );
                }
            }
        }
    }

    // Pilot summary: after a --samples subset run, report what the pilot
    // implies for the full cohort (simple linear projection). With [ai]
    // enabled, append a plain-language interpretation.
    if !ctx.samples_filter.is_empty() {
        let pilot_count = oxo_flow_core::scientific_preflight::count_samples(ctx.config);
        let total_count = WorkflowConfig::from_file(ctx.workflow)
            .map(|c| oxo_flow_core::scientific_preflight::count_samples(&c))
            .unwrap_or(pilot_count);
        let elapsed = ctx.run_started.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let per_sample = if pilot_count > 0 {
            elapsed_secs / pilot_count as f64
        } else {
            0.0
        };
        let projected_secs = per_sample * total_count as f64;

        progress_narrate(
            format_args!("\n{}", "Pilot summary:".bold().cyan()),
            ctx.run_log,
        );
        progress_narrate(
            format_args!(
                "  Samples: {}/{} (pilot) | Wall time: {:.1}s | Per-sample: {:.1}s | Projected full run: ~{:.0}s",
                pilot_count, total_count, elapsed_secs, per_sample, projected_secs
            ),
            ctx.run_log,
        );
        let scientific =
            oxo_flow_core::scientific_preflight::analyze_scientific_constraints(ctx.config);
        if !scientific.is_empty() {
            // Issue #441: in the run's post-expansion config each template
            // appears once per sample instance, so a 1-template finding
            // printed as "3 finding(s)" for a 3-sample pilot. Group by the
            // finding's content — identical (code, message, suggestion)
            // across instances is ONE distinct finding with M instances.
            type WarningGroup<'a> = (
                (&'a str, &'a str, &'a str),
                Vec<&'a oxo_flow_core::scientific_preflight::ScientificWarning>,
            );
            let mut distinct: Vec<WarningGroup> = Vec::new();
            for w in &scientific {
                let key = (w.code.as_str(), w.message.as_str(), w.suggestion.as_str());
                match distinct.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, group)) => group.push(w),
                    None => distinct.push((key, vec![w])),
                }
            }
            progress_narrate(
                format_args!(
                    "  {} ({} across sample rules) — see 'oxo-flow dry-run --samples ...'",
                    oxo_flow_core::plural(
                        distinct.len(),
                        "distinct scientific preflight finding",
                        "distinct scientific preflight findings"
                    ),
                    oxo_flow_core::plural(scientific.len(), "instance", "instances")
                ),
                ctx.run_log,
            );
            for (key, group) in &distinct {
                progress_narrate(
                    format_args!("    ⚠ [{}] {} (×{})", key.0, group[0].rule, group.len()),
                    ctx.run_log,
                );
            }
        }

        // AI interpretation when the workflow opts in via [ai].enabled.
        if let Some(provider) =
            crate::commands::ai_template::try_resolve_ai(Some(ctx.workflow), false)
        {
            let warnings_text = scientific
                .iter()
                .map(|w| format!("- [{}] {}: {}", w.code, w.rule, w.message))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
                "A pilot run of {} out of {} completed in {:.1}s ({} succeeded, {} failed).\n\
                 Per-sample wall time: {:.1}s; projected full run: ~{:.0}s.\n\
                 Scientific preflight findings:\n{}\n\n\
                 Write a short pilot report (3-5 sentences): whether the pilot results are\n\
                 healthy, whether scaling up is advisable, and what to fix first.",
                oxo_flow_core::plural(pilot_count, "sample", "samples"),
                total_count,
                elapsed_secs,
                ctx.success_count,
                ctx.fail_count,
                per_sample,
                projected_secs,
                if warnings_text.is_empty() {
                    "none"
                } else {
                    &warnings_text
                }
            );
            match provider
                .chat(
                    "You are a bioinformatics workflow consultant summarizing a pilot run.",
                    &prompt,
                )
                .await
            {
                Ok(response) => {
                    progress_narrate(format_args!(""), ctx.run_log);
                    progress_narrate(
                        format_args!("{}", "Pilot report (AI):".bold().green()),
                        ctx.run_log,
                    );
                    progress_narrate(format_args!("{response}"), ctx.run_log);
                }
                Err(e) => {
                    progress_narrate(format_args!("  AI pilot report failed: {e}"), ctx.run_log)
                }
            }
        }
    }

    // JSON output mode (issue #142 H6): the summary is emitted on EVERY
    // path — completed, failed, and aborted — so `--json` consumers can
    // rely on the document regardless of how the run ended.
    ctx.json_summary.emit(
        if ctx.fail_count > 0 {
            "failed"
        } else {
            "completed"
        },
        &RunCounts {
            succeeded: ctx.success_count,
            skipped: ctx.skipped_count,
            failed: ctx.fail_count,
            non_required_failed: ctx.non_required_fail_count,
            blocked: ctx.blocked.len(),
        },
        rule_resource_rows(ctx.checkpoint),
    );

    // The verdict is independent of --keep-going (issue #133): keep-going
    // changes SCHEDULING (failures don't stop the run), never the verdict —
    // a run with required failures exits non-zero so scripts and the web
    // delegator (which classifies by exit code) see the truth.
    // `required = false` failures keep exit 0 (issue #99 B2).
    if ctx.fail_count > 0 {
        return Err(anyhow::anyhow!("workflow execution failed"));
    }

    Ok(())
}
