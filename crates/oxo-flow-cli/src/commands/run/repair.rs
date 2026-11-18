//! Verified AI-repair loop (issue #1164): after `apply_fix` succeeds, the
//! engine attempts ONE bounded verification rerun of the failed rules using
//! the UPDATED workflow, records the outcome in the checkpoint
//! (`repair_record`), and narrates stable machine-readable `REPAIR_*`
//! prefixes. The prefixes are a cross-agent contract consumed verbatim by
//! the eval harness — never reword them:
//!
//! - `REPAIR_APPLIED <session-id>` — the AI fix was written to the workflow
//! - `REPAIR_RERUN_SUCCEEDED <detail>` — the verification rerun exited 0
//! - `REPAIR_RERUN_FAILED <detail>` — the rerun exited non-zero
//! - `REPAIR_RERUN_SKIPPED <reason>` — no rerun was attempted

use super::*;

/// Environment variable marking a process as a verification-rerun child
/// (issue #1164 loop guard): a rerun never spawns another rerun. Set on the
/// child process by [`verification_rerun`]; the parent's failure handler
/// consults [`loop_guard_active`] before attempting a rerun.
pub(crate) const REPAIR_RERUN_ENV: &str = "OXO_FLOW_REPAIR_RERUN";

/// Outcome of one verification-rerun decision (issue #1164).
pub(crate) struct VerificationRerun {
    /// Whether the rerun was actually attempted (guards and launch failures
    /// leave this false).
    pub attempted: bool,
    /// Whether the child completed the workflow (exit 0). Meaningful only
    /// when `attempted`.
    pub succeeded: bool,
    /// Human-readable detail: skip reason or exit code.
    pub detail: String,
}

/// Whether the current process is itself a verification rerun (loop guard).
pub(crate) fn loop_guard_active() -> bool {
    std::env::var_os(REPAIR_RERUN_ENV).is_some()
}

/// Attempt the ONE bounded verification rerun (issue #1164 R1): resolve the
/// current binary, spawn `run <workflow> --resume-failed --no-ai-recover`
/// against the SAME checkpoint, and wait for it with inherited stdio so the
/// verification run's logs interleave with the parent's narration. The child
/// carries [`REPAIR_RERUN_ENV`] so it can never start a second rerun.
pub(crate) async fn verification_rerun(
    workflow: &Path,
    checkpoint_path: &Path,
) -> VerificationRerun {
    verification_rerun_checked(workflow, checkpoint_path, loop_guard_active()).await
}

/// [`verification_rerun`] with the loop guard injected — the explicit-guard
/// form exists so tests can exercise the skip branch without mutating the
/// process environment (edition-2024 `set_var` is unsafe and races parallel
/// tests).
pub(crate) async fn verification_rerun_checked(
    workflow: &Path,
    checkpoint_path: &Path,
    guard_active: bool,
) -> VerificationRerun {
    if guard_active {
        return VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: "already a verification rerun (loop guard)".into(),
        };
    }
    // Binary resolution: an explicit `OXO_FLOW_BIN` pin wins (the same knob
    // the web server/desktop use to spawn runs — reuse, never retype), then
    // the running executable itself.
    let exe = match std::env::var_os("OXO_FLOW_BIN") {
        Some(bin) if !bin.is_empty() => PathBuf::from(bin),
        _ => match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                return VerificationRerun {
                    attempted: false,
                    succeeded: false,
                    detail: format!("cannot resolve the current executable: {e}"),
                };
            }
        },
    };
    // Same checkpoint as the parent run: `--resume-failed` clears the failed
    // rules so they re-execute while succeeded rules stay skipped (resume
    // semantics), and the explicit `--checkpoint` pins the file a
    // `--checkpoint`-carrying parent run used. `--no-ai-recover` both
    // disables AI recovery inside the child (no recursion through the
    // recovery path) and, together with the env guard, makes a second
    // verification rerun impossible. The workflow path is absolutized against
    // the inherited cwd so the child resolves the same file even if a future
    // caller changes directory before the spawn.
    let workflow = if workflow.is_absolute() {
        workflow.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(workflow))
            .unwrap_or_else(|_| workflow.to_path_buf())
    };
    let status = tokio::process::Command::new(&exe)
        .arg("run")
        .arg(&workflow)
        .arg("--resume-failed")
        .arg("--no-ai-recover")
        .arg("--checkpoint")
        .arg(checkpoint_path)
        .env(REPAIR_RERUN_ENV, "1")
        .status()
        .await;
    match status {
        Ok(status) if status.success() => VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        },
        Ok(status) => VerificationRerun {
            attempted: true,
            succeeded: false,
            detail: format!(
                "verification rerun exited {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "by signal".into())
            ),
        },
        Err(e) => VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: format!("verification rerun could not be launched: {e}"),
        },
    }
}

/// The `REPAIR_APPLIED` narration line (issue #1164 contract): the AI fix
/// landed, carrying the diagnosis session id that links the archived session
/// and the before-fix workflow snapshot.
pub(crate) fn applied_narration(session_id: &str) -> String {
    format!("REPAIR_APPLIED session_id={session_id}")
}

/// The `REPAIR_RERUN_*` narration line for a rerun outcome (issue #1164
/// contract) — exactly one of SUCCEEDED / FAILED / SKIPPED.
pub(crate) fn rerun_narration(rerun: &VerificationRerun) -> String {
    if !rerun.attempted {
        format!("REPAIR_RERUN_SKIPPED {}", rerun.detail)
    } else if rerun.succeeded {
        format!("REPAIR_RERUN_SUCCEEDED {}", rerun.detail)
    } else {
        format!("REPAIR_RERUN_FAILED {}", rerun.detail)
    }
}

/// Emit one machine-readable repair-contract line: plain text on stderr
/// (never colored, never quiet-suppressed — the eval harness parses it) plus
/// a plain-text copy in the archived run log.
pub(crate) fn narrate_repair_line(line: &str, run_log: Option<&SharedRunLog>) {
    narrate_console_line(format_args!("{line}"));
    if let Some(log) = run_log {
        write_run_log_line(log, line);
    }
}

/// Persist the repair record after the fix is written (issue #1164 R2):
/// `applied = true`, rerun outcome still pending.
pub(crate) async fn record_repair_applied(
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    session_id: &str,
) {
    let record = oxo_flow_core::executor::checkpoint::RepairRecord {
        applied: true,
        session_id: session_id.to_string(),
        rerun_attempted: false,
        rerun_succeeded: false,
        detail: "AI fix applied; verification rerun pending".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let mut ck = checkpoint.lock().await;
    ck.record_repair(record);
    if let Err(e) = ck.save_to_file_async(checkpoint_path).await {
        tracing::warn!(error = %e, "failed to persist repair_record after applying the fix");
    }
}

/// Persist the rerun outcome into the repair record (issue #1164 R2).
/// Overwrites the pending record written by [`record_repair_applied`]; the
/// session id is passed through so the record stays linked even if the
/// apply-time save was lost.
pub(crate) async fn record_repair_rerun(
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    session_id: &str,
    rerun: &VerificationRerun,
) {
    let record = oxo_flow_core::executor::checkpoint::RepairRecord {
        applied: true,
        session_id: session_id.to_string(),
        rerun_attempted: rerun.attempted,
        rerun_succeeded: rerun.succeeded,
        detail: rerun.detail.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let mut ck = checkpoint.lock().await;
    ck.record_repair(record);
    if let Err(e) = ck.save_to_file_async(checkpoint_path).await {
        tracing::warn!(error = %e, "failed to persist repair_record after the verification rerun");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The narration prefixes are a cross-agent contract (eval harness);
    /// pin them verbatim.
    #[test]
    fn narration_prefixes_are_stable() {
        assert_eq!(
            applied_narration("sess-1"),
            "REPAIR_APPLIED session_id=sess-1"
        );
        let ok = VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        };
        assert!(rerun_narration(&ok).starts_with("REPAIR_RERUN_SUCCEEDED "));
        let failed = VerificationRerun {
            attempted: true,
            succeeded: false,
            detail: "verification rerun exited 3".into(),
        };
        assert_eq!(
            rerun_narration(&failed),
            "REPAIR_RERUN_FAILED verification rerun exited 3"
        );
        let skipped = VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: "already a verification rerun (loop guard)".into(),
        };
        assert_eq!(
            rerun_narration(&skipped),
            "REPAIR_RERUN_SKIPPED already a verification rerun (loop guard)"
        );
    }

    /// The loop guard must short-circuit before any spawn: with the guard
    /// active, the rerun is skipped and no child process is started.
    #[tokio::test]
    async fn loop_guard_blocks_a_second_rerun() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = tmp.path().join("wf.oxoflow");
        std::fs::write(&wf, "[workflow]\nname = \"t\"\n").unwrap();
        let ck_path = tmp.path().join("checkpoint.json");

        let rerun = verification_rerun_checked(&wf, &ck_path, true).await;
        assert!(!rerun.attempted, "guarded rerun must not spawn a child");
        assert!(!rerun.succeeded);
        assert!(rerun.detail.contains("loop guard"));
        // The skip must be narratable as the contract's SKIPPED line.
        assert!(rerun_narration(&rerun).starts_with("REPAIR_RERUN_SKIPPED "));
    }

    /// Decision → apply → rerun plumbing without a live provider (issue
    /// #1164 R5a): the `auto_fix = "always"` + safe verdict decision
    /// authorizes the write, `apply_fix` rewrites the poisoned workflow with
    /// the fixed TOML, and the rerun layer is then reached with the UPDATED
    /// workflow. The rerun itself fails to launch here (the test process is
    /// not the CLI binary), which the record must carry honestly as an
    /// attempted-but-failed rerun — the failed-workflow outcome semantics.
    #[test]
    fn decision_apply_then_rerun_reaches_the_updated_workflow() {
        use crate::commands::ai_recover::{apply_fix, auto_fix_decision};
        use oxo_flow_ai::config::AutoFixMode;

        let _cwd = crate::commands::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::Builder::new()
            .prefix("oxo-repair-loop-")
            .tempdir()
            .unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let _restore = crate::commands::RestoreCwd(prev);

        let poisoned =
            "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"boom\"\nshell = \"exit 1\"\n";
        let fixed =
            "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"boom\"\nshell = \"echo fixed\"\n";
        std::fs::write("wf.oxoflow", poisoned).unwrap();

        // Decision plumbing: `always` + a safe verdict = Apply (never
        // ShowOnly) — the only branch that reaches the writer.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Always, true),
            crate::commands::ai_recover::AutoFixDecision::Apply
        );
        apply_fix(Path::new("wf.oxoflow"), fixed, "sess-1164").unwrap();
        assert_eq!(
            std::fs::read_to_string("wf.oxoflow").unwrap(),
            fixed,
            "the fix must be written before the rerun is attempted"
        );

        // The rerun layer receives the UPDATED workflow. The guard is off,
        // so the spawn IS attempted; the child here is the test harness
        // binary, which rejects the `run` argv — asserted as an attempted,
        // failed rerun (exit code in the detail), not a skip.
        let rerun = futures_block_on(verification_rerun_checked(
            Path::new("wf.oxoflow"),
            &tmp.path().join("checkpoint.json"),
            false,
        ));
        assert!(rerun.attempted, "unguarded rerun must be attempted");
        assert!(!rerun.succeeded);
        assert!(rerun.detail.contains("exited"), "detail: {}", rerun.detail);
        assert!(rerun_narration(&rerun).starts_with("REPAIR_RERUN_FAILED "));
    }

    /// Block on an async call from a sync test without a full runtime
    /// dependency: the CLI tests already link tokio.
    fn futures_block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// The repair record round-trips through the shared checkpoint
    /// persistence: apply-then-rerun overwrites the pending record and the
    /// file on disk carries the final outcome.
    #[tokio::test]
    async fn repair_record_persists_apply_then_rerun() {
        let tmp = tempfile::tempdir().unwrap();
        let ck_path = tmp.path().join("checkpoint.json");
        let checkpoint = Mutex::new(CheckpointState::new());

        record_repair_applied(&checkpoint, &ck_path, "sess-persist").await;
        {
            let ck = checkpoint.lock().await;
            let record = ck.repair_record.as_ref().expect("applied record set");
            assert!(record.applied);
            assert!(!record.rerun_attempted);
            // The apply-time save landed on disk.
            let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
            assert!(loaded.repair_record.as_ref().expect("persisted").applied);
        }

        let rerun = VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        };
        record_repair_rerun(&checkpoint, &ck_path, "sess-persist", &rerun).await;
        let ck = checkpoint.lock().await;
        let record = ck.repair_record.as_ref().expect("rerun record set");
        assert_eq!(record.session_id, "sess-persist");
        assert!(record.rerun_attempted);
        assert!(record.rerun_succeeded);
        assert_eq!(record.detail, "verification rerun completed the workflow");
        drop(ck);
        let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
        let record = loaded.repair_record.expect("rerun outcome persisted");
        assert!(record.rerun_attempted && record.rerun_succeeded);
    }
}
