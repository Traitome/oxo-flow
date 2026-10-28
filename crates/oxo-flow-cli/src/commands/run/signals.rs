//! Interrupt/termination plumbing: InterruptSignal, wait_for_termination_signal, terminate_on_signal teardown, reclaim_rule_outputs (#948 P2-5).

use super::*;

/// A termination signal `run` handles (audit C14), with the shell exit
/// convention `128 + signum`: SIGINT → 130, SIGTERM → 143, SIGHUP → 129.
#[derive(Debug, Clone, Copy)]
pub struct InterruptSignal {
    /// Signal name, for the operator-facing message.
    pub name: &'static str,
    /// Raw signal number.
    pub signum: i32,
}

impl InterruptSignal {
    /// Process exit status for a run stopped by this signal.
    pub fn exit_code(&self) -> i32 {
        128 + self.signum
    }
}

/// Resolve when a termination signal arrives (audit C14). Installs handlers
/// for SIGINT, SIGTERM, and SIGHUP; `None` where they cannot be installed.
pub(crate) async fn wait_for_termination_signal() -> Option<InterruptSignal> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).ok()?;
        let mut sigterm = signal(SignalKind::terminate()).ok()?;
        let mut sighup = signal(SignalKind::hangup()).ok()?;
        tokio::select! {
            _ = sigint.recv() => Some(InterruptSignal { name: "SIGINT", signum: 2 }),
            _ = sigterm.recv() => Some(InterruptSignal { name: "SIGTERM", signum: 15 }),
            _ = sighup.recv() => Some(InterruptSignal { name: "SIGHUP", signum: 1 }),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok()?;
        Some(InterruptSignal {
            name: "SIGINT",
            signum: 2,
        })
    }
}

/// Tear the run down on a termination signal (audit C14): signal every
/// in-flight rule's process tree (the default disposition would orphan them
/// with PPID 1), record the interrupted rules in the checkpoint, persist it,
/// and exit `128 + signum`. Never returns.
#[allow(clippy::too_many_arguments)] // the teardown needs the full teardown context
pub(crate) async fn terminate_on_signal(
    signal: InterruptSignal,
    executor: &LocalExecutor,
    join_set: &mut tokio::task::JoinSet<(
        String,
        oxo_flow_core::executor::JobStatus,
        oxo_flow_core::executor::JobRecord,
    )>,
    in_flight: impl Iterator<Item = &String>,
    checkpoint: &Arc<Mutex<CheckpointState>>,
    checkpoint_path: &Path,
    // #540: `std::process::exit` bypassed the RunJsonSummary Drop backstop
    // AND skipped the terminal webhook — --json consumers got zero bytes
    // and webhook subscribers never saw the run end. Emit both here.
    json_summary: &mut RunJsonSummary,
    config: &WorkflowConfig,
) -> ! {
    eprintln!(
        "\n{} {} received — stopping in-flight rules and saving the checkpoint",
        "Interrupted:".yellow().bold(),
        signal.name
    );
    // Same teardown as the first-failure abort (issue #131): release queued
    // resource waiters, then signal each live child's whole subtree.
    executor.clear_resource_waiters().await;
    // Blocking-pool sweep + a post-abort_all second pass — same shape and
    // rationale as the first-failure abort (#524).
    let snapshot = executor.active_pids();
    tokio::task::spawn_blocking(move || {
        for (rule, pid) in snapshot {
            if let Err(e) = oxo_flow_core::executor::timeout::kill_process_tree(pid) {
                tracing::warn!(rule = %rule, pid, error = %e, "failed to signal in-flight rule on {}", signal.name);
            }
        }
    })
    .await
    .ok();
    join_set.abort_all();
    let late = executor.active_pids();
    if !late.is_empty() {
        tokio::task::spawn_blocking(move || {
            for (rule, pid) in late {
                if let Err(e) = oxo_flow_core::executor::timeout::kill_process_tree(pid) {
                    tracing::warn!(rule = %rule, pid, error = %e, "failed to signal late-spawned rule on {}", signal.name);
                }
            }
        })
        .await
        .ok();
    }

    let mut ck = checkpoint.lock().await;
    for rule in in_flight {
        // #525: the engine killed this rule — record the signal evidence,
        // never a fabricated exit code. `exit_code: Some(130)` read as a
        // real process exit in rule_runs/reports (the exact inverse of the
        // #498 evidence model, and undemotable).
        let record = oxo_flow_core::executor::JobRecord {
            signal: Some(15), // SIGTERM — what kill_process_tree sends first
            rule: rule.clone(),
            status: oxo_flow_core::executor::JobStatus::Failed,
            enqueued_at: None,
            started_at: None,
            finished_at: Some(chrono::Utc::now()),
            exit_code: None,
            stdout: None,
            stderr: None,
            command: None,
            retries: 0,
            skip_reason: Some(format!("interrupted by {}", signal.name)),
            max_rss_mb: None,
            cpu_seconds: None,
            caption: None,
        };
        ck.record_run(&record, output_tail_bytes_from_env());
        ck.mark_failed(rule);
    }
    if let Err(e) = ck.save_to_file(checkpoint_path) {
        tracing::warn!(error = %e, "failed to save checkpoint after interruption");
    }
    drop(ck);
    json_summary.emit("failed", &RunCounts::default(), vec![]);
    notify_webhook(
        config,
        oxo_flow_core::webhook::WebhookEvent::WorkflowFailed,
        None,
    )
    .await;
    std::process::exit(signal.exit_code());
}

/// #843: reclaim one candidate's outputs mid-run — the runtime counterpart
/// of the post-run temporary-cleanup sweep. Same persist-before-unlink audit
/// rule (issue #315 F2): tombstone + checksum migration are persisted first,
/// and the unlink is skipped when persistence fails, so a crash can never
/// leave an output gone-but-not-tombstoned. Returns the number of unlinked
/// paths so the caller can narrate reclaimed bytes.
pub(crate) async fn reclaim_rule_outputs(
    candidate: &oxo_flow_core::scheduler::ReclaimCandidate,
    workdir: &std::path::Path,
    checkpoint: &Arc<Mutex<CheckpointState>>,
    checkpoint_path: &Path,
    run_log: &SharedRunLog,
) -> anyhow::Result<usize> {
    let mut paths = Vec::new();
    {
        let mut ck = checkpoint.lock().await;
        for path in &candidate.paths {
            let resolved = workdir.join(path);
            if !resolved.exists() {
                continue;
            }
            if let Some(sha) = ck.checksums.remove(path) {
                ck.record_cleaned_checksum(path, sha);
            }
            paths.push(path.clone());
        }
        if paths.is_empty() {
            return Ok(0);
        }
        ck.tombstones.insert(candidate.rule.clone(), paths.clone());
        if let Err(e) = ck.save_to_file_async(checkpoint_path).await {
            // Roll back the in-memory migration; the outputs stay on disk.
            ck.tombstones.remove(&candidate.rule);
            for path in &paths {
                if let Some(sha) = ck.cleaned_checksums.remove(path) {
                    ck.checksums.insert(path.clone(), sha);
                }
            }
            return Err(anyhow::anyhow!(
                "persisting tombstone failed: {e} — outputs kept"
            ));
        }
    }
    let mut unlinked = 0usize;
    for path in &paths {
        let resolved = workdir.join(path);
        // Symlink-aware classification (review): `is_dir` follows links, so
        // a symlinked output directory would hit remove_dir_all's
        // not-a-directory failure. A symlink is always unlinked as the link
        // itself — never the bytes it points at.
        let ok = match std::fs::symlink_metadata(&resolved) {
            Ok(md) if md.file_type().is_symlink() => std::fs::remove_file(&resolved).is_ok(),
            Ok(md) if md.is_dir() => std::fs::remove_dir_all(&resolved).is_ok(),
            Ok(_) => std::fs::remove_file(&resolved).is_ok(),
            Err(_) => false,
        };
        if ok {
            unlinked += 1;
        } else {
            diagnostic_narrate(
                format_args!(
                    "  {} reclaim unlink failed for '{}' path '{path}'",
                    "!".yellow(),
                    candidate.rule
                ),
                Some(run_log),
            );
        }
    }
    diagnostic_narrate(
        format_args!(
            "  {} reclaimed temporary outputs of '{}' ({}) — regenerated on demand via cascade-up",
            "⊘".dimmed(),
            candidate.rule,
            oxo_flow_core::plural(unlinked, "file", "files")
        ),
        Some(run_log),
    );
    Ok(unlinked)
}
