//! Scheduled task execution: executes a single rule within the scheduler worker pool,
//! records execution metrics, manages input/output manifests and provenance checksums,
//! and updates checkpoint state.

use super::*;
use colored::Colorize;
use oxo_flow_core::executor::{JobRecord, JobStatus, LocalExecutor, output_tail_bytes_from_env};
use oxo_flow_core::rule::Rule;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Mutex;

/// Render a BenchmarkRecord as JSON for the rule's declared benchmark file.
pub(crate) fn render_benchmark_json(
    b: &oxo_flow_core::executor::checkpoint::BenchmarkRecord,
) -> String {
    serde_json::json!({
        "rule": b.rule,
        "wall_time_secs": b.wall_time_secs,
        "max_memory_mb": b.max_memory_mb,
        "memory_limit_mb": b.memory_limit_mb,
        "cpu_seconds": b.cpu_seconds,
        "retries": b.retries,
    })
    .to_string()
}

/// Shared execution context threaded to each scheduled rule task.
pub(crate) struct TaskContext {
    pub executor: Arc<LocalExecutor>,
    pub checkpoint: Arc<Mutex<CheckpointState>>,
    pub checkpoint_path: PathBuf,
    pub workdir_actual: Arc<PathBuf>,
    pub task_run_log: SharedRunLog,
    pub sensitive_values: Vec<String>,
    pub failures: Arc<Mutex<Vec<(String, String)>>>,
    pub failed_rules_set: Arc<Mutex<HashSet<String>>>,
    pub skipped_no_output: Arc<Mutex<HashSet<String>>>,
    pub success_count: Arc<AtomicUsize>,
    pub fail_count: Arc<AtomicUsize>,
    pub non_required_fail_count: Arc<AtomicUsize>,
    pub skipped_count: Arc<AtomicUsize>,
    pub provenance_recorded: Arc<AtomicUsize>,
    pub provenance: bool,
    pub keep_going: bool,
    pub is_tty: bool,
    pub semaphore: Arc<tokio::sync::Semaphore>,
}

/// Parameters specific to one scheduled rule invocation.
pub(crate) struct TaskInvocation {
    pub rule: Rule,
    pub rule_caption: Option<String>,
    pub when_gated_off: bool,
    pub instance_bindings: HashMap<String, String>,
    pub wildcard_values: Arc<HashMap<String, String>>,
    pub typed_config: HashMap<String, toml::Value>,
}

/// Execute a single rule task asynchronously within the scheduler pool.
pub(crate) async fn execute_rule_task(
    inv: TaskInvocation,
    ctx: Arc<TaskContext>,
) -> (String, JobStatus, JobRecord) {
    let rule = inv.rule;
    let rule_name = rule.name.clone();
    let rule_caption = inv.rule_caption;
    let when_gated_off = inv.when_gated_off;
    let instance_bindings = inv.instance_bindings;
    let wildcard_values = inv.wildcard_values;
    let typed_config = inv.typed_config;

    let _permit = ctx.semaphore.acquire().await;

    // Snapshot the input file set BEFORE execution (issue #72):
    // the manifest records what the rule is about to consume, so
    // files added mid-run are not silently baked into the
    // recorded baseline.
    // Remote inputs record (scheme, key, size, etag) when a cloud
    // backend is registered (issue #78 P2); without one they
    // degrade gracefully (warning + entry skipped).
    // A failed snapshot is narrated, not swallowed (issue #633):
    // with no manifest recorded, every later run re-invalidates
    // this rule — the least the user can expect is to be told.
    let input_manifest = if when_gated_off {
        // Gated-off rule: it will be skipped this run — its
        // inputs are absent by design, and snapshotting them
        // only emits spurious warnings (#757 part 3).
        None
    } else {
        match oxo_flow_core::executor::checkpoint::snapshot_input_manifest_async(
            &rule,
            ctx.workdir_actual.as_ref(),
            &wildcard_values,
            &crate::commands::run_preview::storage_resolver(),
        )
        .await
        {
            Ok(manifest) => manifest,
            Err(e) => {
                let msg = format!("could not snapshot inputs: {e}");
                diagnostic_narrate(
                    format_args!(
                        "  {} could not snapshot inputs of rule '{}' — {} (no manifest recorded; later runs may re-execute it)",
                        "⚠".yellow(),
                        rule_name,
                        oxo_flow_core::executor::process::mask_sensitive(
                            &msg,
                            &ctx.sensitive_values,
                        )
                    ),
                    Some(&ctx.task_run_log),
                );
                None
            }
        }
    };

    let result = ctx
        .executor
        .execute_rule_with_bindings(&rule, &wildcard_values, &typed_config, &instance_bindings)
        .await;

    match result {
        Ok(mut record) => {
            let duration = record
                .finished_at
                .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
                .map(|d| d.num_milliseconds() as f64 / 1000.0)
                .unwrap_or(0.0);

            if record.status == oxo_flow_core::executor::JobStatus::Success {
                ctx.success_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleCompleted {
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Success,
                    duration_ms: (duration * 1000.0) as u64,
                });
                if !ctx.is_tty {
                    if record.skip_reason.as_deref() == Some("restored from content cache") {
                        progress_narrate(
                            format_args!(
                                "  {} {} (restored from content cache)",
                                "↺".cyan(),
                                rule_name,
                            ),
                            &ctx.task_run_log,
                        );
                    } else {
                        progress_narrate(
                            format_args!("  {} {} ({:.1}s)", "✓".green(), rule_name, duration),
                            &ctx.task_run_log,
                        );
                    }
                }
                // Issue #832: a succeeded rule whose declared
                // `log` path stayed empty (0-byte file or empty
                // dir) while BOTH captured tails are empty too
                // almost certainly self-logs somewhere the
                // workflow did not declare (dnbc4tools writes
                // `scrna/count/Pt01/logs/<date>.txt`). One Info
                // hint, and only when all three are empty — a
                // quiet-but-correct tool with content in any of
                // them stays silent. The hint never gates the
                // rule; captured tails are persisted in the
                // checkpoint job records regardless (issue #692).
                if let Some(log_pattern) = &rule.log {
                    let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                        log_pattern,
                        &wildcard_values,
                    );
                    if !expanded.contains('{') {
                        let log_path = ctx.workdir_actual.as_ref().join(&expanded);
                        let log_empty = if log_path.is_dir() {
                            std::fs::read_dir(&log_path)
                                .map(|mut entries| entries.next().is_none())
                                .unwrap_or(false)
                        } else if log_path.is_file() {
                            std::fs::metadata(&log_path)
                                .map(|m| m.len() == 0)
                                .unwrap_or(false)
                        } else {
                            // Never created: the rule's `2> {log}`
                            // redirect never ran — a different
                            // signal than "declared log stayed
                            // empty", so stay silent here.
                            false
                        };
                        let tails_empty = record
                            .stdout
                            .as_deref()
                            .map(str::trim)
                            .unwrap_or("")
                            .is_empty()
                            && record
                                .stderr
                                .as_deref()
                                .map(str::trim)
                                .unwrap_or("")
                                .is_empty();
                        if log_empty && tails_empty {
                            diagnostic_narrate(
                                format_args!(
                                    "  {} rule '{}' succeeded but its declared log '{}' is empty — the tool likely writes its own log elsewhere (declare that path in `log`, or check the tool's default log location); captured stdout/stderr tails are persisted in the checkpoint job records regardless",
                                    "ℹ".dimmed(),
                                    rule_name,
                                    log_path.display()
                                ),
                                Some(&ctx.task_run_log),
                            );
                        }
                    }
                }
                let benchmark = oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                    rule: rule_name.clone(),
                    wall_time_secs: duration,
                    max_memory_mb: record.max_rss_mb,
                    memory_limit_mb: rule
                        .effective_memory()
                        .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                    cpu_seconds: record.cpu_seconds,
                    retries: record.retries,
                    recorded_as: None,
                };
                // `benchmark` declaration (issue #469): write the
                // engine-sampled metrics to the declared path,
                // per instance (the declaration may carry
                // `{sample}`-style wildcards). Best-effort — a
                // benchmark write failure must never fail a
                // completed rule.
                if let Some(bench_decl) = &rule.benchmark {
                    let bench_path = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                        bench_decl,
                        &wildcard_values,
                    );
                    if !bench_path.contains('{') {
                        let rendered = render_benchmark_json(&benchmark);
                        let dest = ctx.workdir_actual.as_ref().join(&bench_path);
                        if let Some(parent) = dest.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(e) = std::fs::write(&dest, rendered) {
                            tracing::warn!(
                                rule = %rule_name,
                                path = %dest.display(),
                                error = %e,
                                "failed to write benchmark file"
                            );
                        }
                    }
                }
                // Post-run manifest re-verification
                // (issue #194 §2.10): an external writer
                // touching an input mid-run is invisible to the
                // pre-run snapshot alone. Re-snapshot now; on
                // any change, warn loudly and keep the PRE-run
                // manifest recorded — the next run's comparison
                // then sees the mismatch and re-executes this
                // rule instead of serving outputs built from a
                // mixed file state.
                let post_result =
                    oxo_flow_core::executor::checkpoint::snapshot_input_manifest_async(
                        &rule,
                        ctx.workdir_actual.as_ref(),
                        &wildcard_values,
                        &crate::commands::run_preview::storage_resolver(),
                    )
                    .await;
                let post_manifest = match post_result {
                    Ok(manifest) => manifest,
                    Err(e) => {
                        let msg = format!("could not re-snapshot inputs: {e}");
                        diagnostic_narrate(
                            format_args!(
                                "  {} could not re-snapshot inputs of rule '{}' — {} (mid-run input changes cannot be detected for it)",
                                "⚠".yellow(),
                                rule_name,
                                oxo_flow_core::executor::process::mask_sensitive(
                                    &msg,
                                    &ctx.sensitive_values,
                                )
                            ),
                            Some(&ctx.task_run_log),
                        );
                        None
                    }
                };
                if let (Some(pre), Some(post)) = (&input_manifest, &post_manifest) {
                    let changes = oxo_flow_core::executor::checkpoint::manifest_changes(pre, post);
                    if !changes.is_empty() {
                        let described: Vec<String> = changes.iter().map(|c| c.describe()).collect();
                        let note = format!(
                            "\n[oxo-flow] WARNING: an input changed while this rule was running ({}); outputs may be inconsistent — the next run will re-execute this rule",
                            described.join(", ")
                        );
                        tracing::warn!(
                            rule = %rule_name,
                            changes = %described.join(", "),
                            "input changed while the rule was running; outputs may be inconsistent"
                        );
                        if let Some(ref mut stderr) = record.stderr {
                            stderr.push_str(&oxo_flow_core::executor::process::mask_sensitive(
                                &note,
                                &ctx.sensitive_values,
                            ));
                        } else {
                            record.stderr = Some(oxo_flow_core::executor::process::mask_sensitive(
                                &note,
                                &ctx.sensitive_values,
                            ));
                        }
                    }
                }

                let mut ck = ctx.checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_completed(&rule_name, benchmark);
                // #843: a rule re-executed via cascade-up has
                // just regenerated its outputs — the old
                // tombstone (from reclaim or the post-run sweep)
                // is stale and would make the post-run sweep
                // skip re-deleting them. Clear it here; the
                // sweep re-tombstones below.
                ck.tombstones.remove(&rule_name);
                if let Some(ref manifest) = input_manifest {
                    ck.record_input_manifest(&rule_name, manifest.clone());
                }
                if ctx.provenance {
                    // Expand `{config.*}` (and wildcards) exactly
                    // like the output-verification pass does —
                    // recording the UNEXPANDED template stored a
                    // path that never existed, so `--provenance`
                    // silently recorded nothing on the default
                    // `init` scaffold and any templated-output
                    // workflow (issue #276). Keys stay consistent
                    // with `should_skip_rule_with_checksums`,
                    // which looks up EXPANDED paths.
                    let mut recorded_here = 0usize;
                    for output in &rule.output {
                        let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            output,
                            &wildcard_values,
                        );
                        if expanded.contains('{') {
                            continue; // per-instance wildcard: resolved only at fan-out
                        }
                        let output_path = ctx.workdir_actual.join(&expanded);
                        if output_path.exists()
                            && let Ok(checksum) =
                                oxo_flow_core::executor::checkpoint::compute_file_checksum(
                                    &output_path,
                                )
                        {
                            ck.record_checksum(&expanded, checksum);
                            recorded_here += 1;
                        }
                    }
                    ctx.provenance_recorded
                        .fetch_add(recorded_here, std::sync::atomic::Ordering::Relaxed);
                }
                if let Err(e) = ck.save_to_file_async(&ctx.checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {e}");
                }
                (
                    rule_name,
                    oxo_flow_core::executor::JobStatus::Success,
                    record,
                )
            } else if record.status == oxo_flow_core::executor::JobStatus::Skipped {
                ctx.skipped_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Issue #747: a Skipped record is terminal. Drop
                // the spawn-time running mark regardless of which
                // skip path produced it (when-gate false skips it
                // in the executor; the other terminal skip
                // reasons clear it here). Otherwise the persisted
                // running set outlives the run and the web status
                // poll renders phantom Running nodes.
                if record.skip_reason.as_deref() != Some("outputs up-to-date") {
                    let mut ck = ctx.checkpoint.lock().await;
                    ck.clear_running(&rule_name);
                    if let Err(e) = ck.save_to_file_async(&ctx.checkpoint_path).await {
                        tracing::warn!("Failed to save checkpoint (skip): {e}");
                    }
                }
                if record.skip_reason.as_deref() == Some("optional inputs missing") {
                    // No outputs will exist — block non-optional
                    // dependents instead of letting them run into
                    // a missing-file shell error (issue #200).
                    let mut sno = ctx.skipped_no_output.lock().await;
                    sno.insert(rule_name.clone());
                }
                if record.skip_reason.as_deref() == Some("outputs up-to-date") {
                    // Up-to-date outputs are valid completions
                    // (issue #324 F-1): a rule interrupted
                    // mid-run can leave outputs behind (its
                    // shell kept running after a SIGKILL), and
                    // resume verdicts it up-to-date. Recording
                    // it prevents re-submission on every later
                    // run and keeps the audit trail aligned
                    // with the disk.
                    // Record a synthetic benchmark only when the
                    // rule has no real one (e.g. first adoption
                    // of a pre-existing run). A zero-wall
                    // placeholder must never overwrite measured
                    // wall-time history — the Dashboard's
                    // "Total Runtime" and Prometheus
                    // oxo_flow_rule_duration_seconds sum these
                    // records (issue #481).
                    let mut ck = ctx.checkpoint.lock().await;
                    if !ck.benchmarks.contains_key(&rule_name) {
                        let benchmark = oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                            rule: rule_name.clone(),
                            wall_time_secs: 0.0,
                            max_memory_mb: None,
                            memory_limit_mb: rule
                                .effective_memory()
                                .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                            cpu_seconds: None,
                            retries: 0,
                            recorded_as: Some("outputs up-to-date".to_string()),
                        };
                        ck.mark_completed(&rule_name, benchmark);
                    } else {
                        ck.mark_completed_quiet(&rule_name);
                    }
                    if let Err(e) = ck.save_to_file_async(&ctx.checkpoint_path).await {
                        tracing::warn!("Failed to save checkpoint: {e}");
                    }
                }
                // Surface the executor's skip reason (condition
                // false, optional inputs missing, outputs
                // up-to-date) — otherwise a submitted rule that
                // never executes looks indistinguishable from one
                // that ran.
                if !ctx.is_tty {
                    let reason = record.skip_reason.as_deref().unwrap_or("skipped");
                    narrate_console_line(format_args!(
                        "  {} {} ({reason})",
                        "⊝".dimmed(),
                        rule_name
                    ));
                }
                (
                    rule_name,
                    oxo_flow_core::executor::JobStatus::Skipped,
                    record,
                )
            } else {
                // A failed `required = false` rule is recorded and
                // blocks its dependents, but does not fail the
                // run (issue #99 B2).
                if rule.required {
                    ctx.fail_count
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    ctx.non_required_fail_count
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                {
                    let mut frs = ctx.failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                let mut ck = ctx.checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&rule_name);
                if let Err(e) = ck.save_to_file_async(&ctx.checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {e}");
                }
                let mut err_msg = format!("rule '{}' failed", rule_name);
                if let Some(ref stderr) = record.stderr {
                    let trimmed = stderr.trim();
                    if !trimmed.is_empty() {
                        err_msg.push_str(&format!("\nstderr: {}", trimmed));
                    }
                }
                if let Some(code) = record.exit_code {
                    err_msg.push_str(&format!("\n  exit code: {}", code));
                }
                progress_narrate(
                    format_args!("  {} {}", "✗".red(), err_msg),
                    &ctx.task_run_log,
                );
                // Issue #856: rule failures must be greppable as
                // machine events too — the pretty ✗ line alone
                // made `grep rule_failed run.launch.log` return
                // 0 while 19 rules had died.
                emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleFailed {
                    rule: rule_name.clone(),
                    exit_code: record.exit_code,
                    signal: record.signal,
                    retries: record.retries,
                    duration_ms: (duration * 1000.0) as u64,
                });
                // Always record: keep_going needs the final
                // listing, non-required failures are listed in
                // either mode, and the AI recovery path must be
                // able to find the ABORTING rule by name.
                {
                    let mut reason = String::new();
                    if let Some(code) = record.exit_code {
                        reason.push_str(&format!("exit code {}", code));
                    }
                    if let Some(ref stderr) = record.stderr
                        && let Some(last) =
                            stderr.trim().lines().next_back().filter(|l| !l.is_empty())
                    {
                        if !reason.is_empty() {
                            reason.push_str(" — ");
                        }
                        reason.push_str(last);
                    }
                    if reason.is_empty() {
                        reason.push_str("failed");
                    }
                    // Retry visibility (audit #276 P4-3): the
                    // executor retries silently by design, but the
                    // failure summary must say the rule was
                    // retried N times so automation and humans can
                    // distinguish flaky from immediate failures.
                    if record.retries > 0 {
                        reason.push_str(&format!(
                            " (after {} retr{})",
                            record.retries,
                            if record.retries == 1 { "y" } else { "ies" }
                        ));
                    }
                    if !rule.required {
                        reason.push_str(" (non-required)");
                    }
                    let mut f = ctx.failures.lock().await;
                    f.push((rule_name.clone(), reason));
                }
                (
                    rule_name,
                    record.status, // Failed or TimedOut
                    record,
                )
            }
        }
        Err(e) => {
            if rule.required {
                ctx.fail_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                ctx.non_required_fail_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            {
                let mut frs = ctx.failed_rules_set.lock().await;
                frs.insert(rule_name.clone());
            }
            // Build the failure record first so the checkpoint
            // captures the same diagnostics the report will show
            // (issue #83 WS2).
            let record = oxo_flow_core::executor::JobRecord {
                signal: None,
                rule: rule_name.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: Some(-1),
                stdout: None,
                // Staging/security errors embed config-expanded
                // URIs and full commands — mask them like every
                // other captured surface (issue #99 B1).
                stderr: Some(oxo_flow_core::executor::process::mask_sensitive(
                    &e.to_string(),
                    &ctx.sensitive_values,
                )),
                command: None,
                retries: 0,
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
                caption: rule_caption,
            };
            // Same #856 greppability contract as the executed-failure
            // branch above: a rule that dies at spawn/staging is a
            // FAILED rule, not a completion — emitting RuleCompleted
            // here made `grep -c rule_failed` undercount by one.
            emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleFailed {
                rule: rule_name.clone(),
                exit_code: record.exit_code,
                signal: record.signal,
                retries: record.retries,
                duration_ms: 0,
            });
            let mut ck = ctx.checkpoint.lock().await;
            ck.record_run(&record, output_tail_bytes_from_env());
            ck.mark_failed(&rule_name);
            if let Err(e) = ck.save_to_file_async(&ctx.checkpoint_path).await {
                tracing::warn!("Failed to save checkpoint: {e}");
            }
            if !ctx.keep_going {
                progress_narrate(
                    format_args!("  {} rule '{}' failed: {}", "✗".red(), rule_name, e),
                    &ctx.task_run_log,
                );
            }
            // Always record: keep_going needs the final
            // listing, non-required failures are listed in
            // either mode, and the AI recovery path must be able
            // to find the ABORTING rule by name (a non-required
            // failure recorded earlier must not shadow it).
            let mut f = ctx.failures.lock().await;
            let reason = if rule.required {
                e.to_string()
            } else {
                format!("{} (non-required)", e)
            };
            f.push((rule_name.clone(), reason));
            (
                rule_name,
                oxo_flow_core::executor::JobStatus::Failed,
                record,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_benchmark_json_carries_sampled_fields() {
        let b = oxo_flow_core::executor::checkpoint::BenchmarkRecord {
            rule: "fastqc".into(),
            wall_time_secs: 2.75,
            max_memory_mb: Some(512),
            memory_limit_mb: Some(1024),
            cpu_seconds: Some(2.1),
            retries: 0,
            recorded_as: None,
        };
        let json = render_benchmark_json(&b);
        assert!(json.contains(r#""rule":"fastqc""#), "{json}");
        assert!(json.contains(r#""wall_time_secs":2.75"#), "{json}");
        assert!(json.contains(r#""max_memory_mb":512"#), "{json}");
    }
}
