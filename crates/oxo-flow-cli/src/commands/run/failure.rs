//! Failure handling and abort coordination: abort on failure, sibling process cleanup,
//! demotion to cancelled, AI error recovery, and transitive dependency blocking.

use super::*;
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use oxo_flow_core::executor::{JobRecord, JobStatus, LocalExecutor, output_tail_bytes_from_env};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Mutex;

/// Parameters needed to clean up and report on a run failure abort.
pub(crate) struct AbortContext<'a> {
    pub executor: &'a LocalExecutor,
    pub join_set: &'a mut tokio::task::JoinSet<(String, JobStatus, JobRecord)>,
    pub task_rule: &'a HashMap<tokio::task::Id, String>,
    pub failed_rules_set: &'a Mutex<HashSet<String>>,
    pub checkpoint: &'a Mutex<CheckpointState>,
    pub checkpoint_path: &'a Path,
    pub failures: &'a Mutex<Vec<(String, String)>>,
    pub fail_count: &'a AtomicUsize,
    pub non_required_fail_count: &'a AtomicUsize,
    pub success_count: &'a AtomicUsize,
    pub skipped_count: &'a AtomicUsize,
    pub blocked: &'a Mutex<Vec<(String, String)>>,
    pub config: &'a WorkflowConfig,
    pub workflow: &'a Path,
    pub workdir_actual: &'a Path,
    pub tmp_dir_resolved: Option<&'a Path>,
    pub run_log: &'a SharedRunLog,
    pub json_summary: &'a mut RunJsonSummary,
    pub is_tty: bool,
    pub ai_recover: bool,
    pub ai_max_retries: Option<u32>,
}

/// Abort in-flight execution on failure (when not in keep_going mode):
/// terminates processes, demotes killed siblings, runs diagnostics and AI recovery,
/// and produces the final failure report and hooks.
pub(crate) async fn handle_run_failure_abort(
    completed_rule: &str,
    status: JobStatus,
    ctx: AbortContext<'_>,
) -> anyhow::Result<()> {
    // Kill in-flight rule processes before cancelling their tasks —
    // `abort_all()` alone orphans the OS children, which keep
    // consuming the machine after the run exits (issue #131).
    // Clear the pool's FIFO waiter queue first so cancelled waiters
    // cannot hold the line hostage (issue #123 100% guarantee).
    ctx.executor.clear_resource_waiters().await;
    // The grace poll inside kill_process_tree sleeps up to 10 s —
    // run the sweep on the blocking pool, not a runtime worker
    // (#524; the timeout path already follows this rule). The
    // snapshot also records WHICH rules the abort actually killed:
    // the #498 demotion below keys on that evidence, not on bare
    // signal deaths — a sibling the kernel OOM-killer shot carries
    // the same signal signature and must keep its failed record
    // (#525).
    let snapshot = ctx.executor.active_pids();
    let mut abort_killed: std::collections::HashSet<String> =
        snapshot.iter().map(|(r, _)| r.clone()).collect();
    tokio::task::spawn_blocking(move || {
        for (rule_name, pid) in snapshot {
            if let Err(e) = oxo_flow_core::executor::timeout::kill_process_tree(pid) {
                tracing::warn!(
                    rule = %rule_name,
                    pid,
                    error = %e,
                    "failed to signal in-flight rule process during abort"
                );
            }
        }
    })
    .await
    .ok();
    ctx.join_set.abort_all();
    // Re-snapshot and sweep again: a retry task could wake during
    // the first sweep and spawn its next attempt; those late
    // children escape the stale first snapshot (#524).
    let late = ctx.executor.active_pids();
    abort_killed.extend(late.iter().map(|(r, _)| r.clone()));
    if !late.is_empty() {
        tokio::task::spawn_blocking(move || {
            for (rule_name, pid) in late {
                if let Err(e) = oxo_flow_core::executor::timeout::kill_process_tree(pid) {
                    tracing::warn!(
                        rule = %rule_name,
                        pid,
                        error = %e,
                        "failed to signal late-spawned rule process during abort"
                    );
                }
            }
        })
        .await
        .ok();
    }

    // The killed siblings would otherwise surface as bare
    // "✗ rule 'X' failed" with no exit code and an empty stderr
    // tail: the process-tree kill makes their shells die by signal,
    // so their command loops record exit_code None and no
    // diagnostics. Record them as honest Cancelled entries instead
    // — never in failed_rules, they are collateral cancellations,
    // not failures — and narrate them like skips.
    {
        let verb = if status == oxo_flow_core::executor::JobStatus::Cancelled {
            "cancelled"
        } else {
            "failed"
        };
        // Sorted: this vec persists into checkpoint.json JobRecords via
        // record_run below — HashMap iteration order must not leak into
        // the persisted artifact.
        let mut killed: Vec<String> = ctx
            .task_rule
            .values()
            .filter(|name| *name != completed_rule)
            .cloned()
            .collect();
        killed.sort();
        if !killed.is_empty() {
            // A rule that already finished its closure (result still
            // unreaped in the JoinSet) has written its own record —
            // never overwrite a genuine Success/Failed. BUT a failure
            // whose only evidence is a signal death with no exit code
            // IS the abort's kill, not a self-caused failure: demote
            // it out of failed_rules/metrics (issue #498). Pure
            // signal deaths with no exit code are exactly what the
            // process-tree kill produces; a rule that failed on its
            // own (exit code, or a pre-abort signal like OOM-kill
            // whose result was reaped first) keeps its record.
            let already_failed = ctx.failed_rules_set.lock().await.clone();
            let mut ck = ctx.checkpoint.lock().await;
            for rule_name in &killed {
                if ck.is_completed(rule_name) {
                    continue;
                }
                // The kill-snapshot membership is the demotion
                // evidence (#525): a signal death alone cannot
                // distinguish the abort's kill from a kernel
                // OOM-kill that merely reaped around the same time.
                // A rule that already failed on its own and was NOT
                // in the kill snapshots keeps its genuine failure
                // record untouched — never demoted, never
                // overwritten with a cancellation.
                if already_failed.contains(rule_name) && !abort_killed.contains(rule_name) {
                    continue;
                }
                if already_failed.contains(rule_name) {
                    let skip_reason = format!(
                        "run aborted before this rule finished — required rule \
'{completed_rule}' {verb}"
                    );
                    if ck.demote_failed_to_cancelled(rule_name, skip_reason) {
                        let required = ctx.config.get_rule(rule_name).is_some_and(|r| r.required);
                        if required {
                            ctx.fail_count
                                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            ctx.non_required_fail_count
                                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        ctx.failures.lock().await.retain(|(n, _)| n != rule_name);
                        if !ctx.is_tty {
                            diagnostic_narrate(
                                format_args!(
                                    "  {} {} (run aborted — required rule '{}' {})",
                                    "⊝".dimmed(),
                                    rule_name,
                                    completed_rule,
                                    verb
                                ),
                                Some(ctx.run_log),
                            );
                        }
                    }
                    continue;
                }
                let record = oxo_flow_core::executor::JobRecord {
                    signal: None,
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Cancelled,
                    enqueued_at: None,
                    started_at: None,
                    finished_at: Some(chrono::Utc::now()),
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    skip_reason: Some(format!(
                        "run aborted before this rule finished — required rule \
'{completed_rule}' {verb}"
                    )),
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: None,
                };
                ck.record_run(&record, output_tail_bytes_from_env());
                if !ctx.is_tty {
                    diagnostic_narrate(
                        format_args!(
                            "  {} {} (run aborted — required rule '{}' {})",
                            "⊝".dimmed(),
                            rule_name,
                            completed_rule,
                            verb
                        ),
                        Some(ctx.run_log),
                    );
                }
            }
            if let Err(e) = ck.save_to_file_async(ctx.checkpoint_path).await {
                tracing::warn!(error = %e, "failed to save checkpoint after abort");
            }
        }
    }

    // Deterministic diagnostics & repair hint
    let mut has_repair_plan = false;
    {
        let failures_guard = ctx.failures.lock().await;
        let failure = failures_guard
            .iter()
            .find(|(name, _)| name == completed_rule)
            .or_else(|| failures_guard.first());
        if let Some((rule, error)) = failure {
            let exit_code = failure_exit_code(error);
            let code_opt = if exit_code == -1 {
                None
            } else {
                Some(exit_code)
            };
            let ctx = oxo_flow_core::diagnostics::DiagnosticContext::new(rule, error, code_opt);
            let diags =
                oxo_flow_core::diagnostics::DiagnosticsEngine::global().analyze_context(&ctx);
            if let Some(d) = diags.first() {
                narrate_console_line(format_args!(
                    "  {} Root Cause: {} ({:?})",
                    "🔍".bold(),
                    d.likely_cause.yellow(),
                    d.category
                ));
                for s in &d.suggestions {
                    narrate_console_line(format_args!("     {} {}", "💡 Suggestion:".cyan(), s));
                }
                has_repair_plan = d.repair_action.is_some();
            }
        }
    }

    // AI error recovery
    let should_recover =
        ctx.ai_recover || crate::commands::ai_template::should_use_ai(Some(ctx.workflow), false);
    // The engine produced a bounded deterministic repair plan, but no
    // interactive key handlers exist — the old "[R] Auto-scale & resume"
    // menu promised keys that were never wired (false affordance). Point
    // at the real automated path instead, and only when the AI recovery
    // block below won't run and report on its own.
    if has_repair_plan && !should_recover {
        narrate_console_line(format_args!(
            "     {} Repair plan available — rerun or resume with `--ai-recover` for automated recovery",
            "⚡".bold()
        ));
    }
    if should_recover {
        let failures_guard = ctx.failures.lock().await;
        // Diagnose the rule that actually aborted the run — a
        // non-required failure recorded earlier must not shadow it.
        let failure = failures_guard
            .iter()
            .find(|(name, _)| name == completed_rule)
            .or_else(|| failures_guard.first());
        if let Some((rule, error)) = failure
            && let Some(provider) =
                crate::commands::ai_template::try_resolve_ai(Some(ctx.workflow), true)
        {
            // `--ai-max-retries` bounds the recovery attempts: a
            // failed diagnosis call is retried up to the flag's
            // value, a successful one is never repeated.
            let attempts = ai_attempts(ctx.ai_max_retries);
            let mut result: Result<crate::commands::ai_recover::DiagnoseResult> =
                Err(anyhow::anyhow!("AI recovery did not run"));
            for attempt in 1..=attempts {
                // Feed the real exit code when the failure record has
                // one: the banner previously always showed -1, and the
                // prompt's exit-code→category triage (137=OOM,
                // 139=segfault, 127=command not found) needs the true
                // value to classify correctly. Panics/aborts keep -1.
                let exit_code = failure_exit_code(error);
                result = crate::commands::ai_recover::diagnose_failure(
                    ctx.workflow,
                    rule,
                    exit_code,
                    error,
                    &provider,
                )
                .await;
                if result.is_ok() || attempt == attempts {
                    break;
                }
                if let Err(e) = &result {
                    narrate_console_line(format_args!(
                        "  AI diagnosis attempt {attempt}/{attempts} failed: {e} — retrying"
                    ));
                }
            }
            match result {
                Ok(diag) => {
                    if let Some(ref toml) = diag.modified_toml {
                        // Layered authorization for writing AI-generated TOML
                        // (issue #1013 O1): the resolved `[ai] auto_fix` mode
                        // decides who may write — `never` never does, `ask`
                        // (the documented default) prompts on an interactive
                        // terminal and is denied elsewhere so a headless run
                        // can never rewrite the workflow silently, and
                        // `always` applies safe fixes automatically. In every
                        // mode the model's own safety verdict still gates the
                        // write, and the generated TOML must still parse as a
                        // valid workflow before it lands.
                        let mode = crate::commands::ai_recover::resolve_auto_fix_mode(ctx.workflow);
                        let interactive = {
                            use std::io::IsTerminal as _;
                            std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
                        };
                        let decision = crate::commands::ai_recover::auto_fix_decision(
                            mode,
                            diag.safe_to_auto_apply,
                        );
                        // Only an explicit `Apply` (or an interactive y/N
                        // confirmation) reaches the writer; every other
                        // outcome is narration, never a write.
                        let apply_ready = match decision {
                            crate::commands::ai_recover::AutoFixDecision::ShowOnly(reason) => {
                                narrate_console_line(format_args!(
                                    "  AI fix not auto-applied ({reason}): review the \
                                             suggestion manually."
                                ));
                                false
                            }
                            crate::commands::ai_recover::AutoFixDecision::Prompt => {
                                if !interactive {
                                    narrate_console_line(format_args!(
                                        "  AI fix not auto-applied: auto_fix = \"ask\" needs \
                                                 confirmation, but this session is not \
                                                 interactive. Review the suggestion manually or \
                                                 set auto_fix = \"always\"."
                                    ));
                                    false
                                } else {
                                    crate::commands::ai_recover::confirm_auto_fix_blocking()
                                }
                            }
                            crate::commands::ai_recover::AutoFixDecision::Apply => true,
                        };
                        if apply_ready && let Err(e) = oxo_flow_core::WorkflowConfig::parse(toml) {
                            narrate_console_line(format_args!(
                                "  AI fix not auto-applied: generated TOML failed \
                                         validation ({e}). Review the suggestion manually."
                            ));
                        } else if apply_ready
                            && let Err(e) = crate::commands::ai_recover::apply_fix(
                                ctx.workflow,
                                toml,
                                "recovery",
                            )
                        {
                            narrate_console_line(format_args!("  Failed to apply AI fix: {e}"));
                        }
                    }
                }
                Err(e) => narrate_console_line(format_args!("  AI diagnosis failed: {e}")),
            }
        }
    }

    // The plain-failure abort: emit the summary BEFORE returning,
    // mirroring the keep-going path's document (issue #142 H6).
    let ck = ctx.checkpoint.lock().await;
    ctx.json_summary.emit(
        "failed",
        &RunCounts {
            succeeded: ctx.success_count.load(std::sync::atomic::Ordering::Relaxed),
            skipped: ctx.skipped_count.load(std::sync::atomic::Ordering::Relaxed),
            failed: ctx.fail_count.load(std::sync::atomic::Ordering::Relaxed),
            non_required_failed: ctx
                .non_required_fail_count
                .load(std::sync::atomic::Ordering::Relaxed),
            blocked: ctx.blocked.lock().await.len(),
        },
        rule_resource_rows(&ck),
    );
    // on_error fires on the plain-failure abort too (issue #227).
    run_terminal_hook(
        ctx.config,
        ctx.workdir_actual,
        ctx.tmp_dir_resolved,
        ctx.success_count.load(std::sync::atomic::Ordering::Relaxed),
        ctx.fail_count.load(std::sync::atomic::Ordering::Relaxed),
        ctx.skipped_count.load(std::sync::atomic::Ordering::Relaxed),
    )
    .await;
    notify_webhook(
        ctx.config,
        oxo_flow_core::webhook::WebhookEvent::WorkflowFailed,
        Some((
            ctx.success_count.load(std::sync::atomic::Ordering::Relaxed),
            ctx.fail_count.load(std::sync::atomic::Ordering::Relaxed),
            ctx.skipped_count.load(std::sync::atomic::Ordering::Relaxed),
        )),
    )
    .await;
    Err(anyhow::anyhow!("workflow execution failed"))
}

/// Parameters needed to propagate failure transitively to unsubmitted dependent rules.
pub(crate) struct TransitiveFailureContext<'a> {
    pub dag: &'a WorkflowDag,
    pub rule_names: &'a [String],
    pub submitted: &'a mut HashSet<String>,
    pub failed_rules_set: &'a Mutex<HashSet<String>>,
    pub skipped_count: &'a AtomicUsize,
    pub blocked: &'a Mutex<Vec<(String, String)>>,
    pub sched: &'a mut oxo_flow_core::scheduler::SchedulerState,
    pub config: &'a WorkflowConfig,
    pub workdir_actual: &'a Path,
    pub is_tty: bool,
    pub progress: &'a indicatif::ProgressBar,
}

/// Propagate failures transitively: when continuing past failures (keep_going or non-required),
/// every unsubmitted rule that depends on a failed rule is marked as skipped and blocked.
pub(crate) async fn propagate_transitive_failures(ctx: TransitiveFailureContext<'_>) {
    // Seed the worklist with every rule that directly depends on a
    // failed rule and has not been submitted yet.
    let frs = ctx.failed_rules_set.lock().await;
    let mut worklist: Vec<String> = Vec::new();
    for rule_name in ctx.rule_names {
        if ctx.submitted.contains(rule_name) {
            continue;
        }
        let deps = ctx.dag.dependencies(rule_name).unwrap_or_default();
        if deps.iter().any(|d| frs.contains(d.as_str())) {
            worklist.push(rule_name.to_string());
        }
    }
    drop(frs);

    // Process transitively: when a rule is blocked, its own dependents
    // may also become blockable.
    let mut idx = 0;
    while idx < worklist.len() {
        let name = &worklist[idx];
        if ctx.submitted.contains(name.as_str()) {
            idx += 1;
            continue;
        }
        ctx.submitted.insert(name.clone());
        ctx.skipped_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut frs = ctx.failed_rules_set.lock().await;
            frs.insert(name.clone());
        }
        ctx.sched
            .mark_completed(oxo_flow_core::executor::JobRecord {
                signal: None,
                rule: name.clone(),
                status: oxo_flow_core::executor::JobStatus::Skipped,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: None,
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: Some("blocked by failed upstream dependency".into()),
                max_rss_mb: None,
                cpu_seconds: None,
                caption: ctx.config.get_rule(name).and_then(|r| {
                    oxo_flow_core::executor::process::rule_report_caption(r, ctx.workdir_actual)
                }),
            });
        ctx.progress.inc(1);
        if !ctx.is_tty {
            narrate_console_line(format_args!(
                "  {} {} (blocked by failed dependency)",
                "⊘".yellow(),
                name
            ));
        }

        // Find transitive dependents of this newly-blocked rule.
        {
            let frs = ctx.failed_rules_set.lock().await;
            for other in ctx.rule_names {
                if ctx.submitted.contains(other) || worklist.contains(other) {
                    continue;
                }
                let deps = ctx.dag.dependencies(other).unwrap_or_default();
                if deps.iter().any(|d| frs.contains(d.as_str())) {
                    worklist.push(other.to_string());
                }
            }
        }

        idx += 1;
    }

    // Record blocked rules for final summary
    {
        let frs = ctx.failed_rules_set.lock().await;
        for name in &worklist {
            let deps = ctx.dag.dependencies(name).unwrap_or_default();
            if let Some(dep) = deps.into_iter().find(|d| frs.contains(d.as_str())) {
                ctx.blocked.lock().await.push((name.clone(), dep));
            }
        }
    }
}
