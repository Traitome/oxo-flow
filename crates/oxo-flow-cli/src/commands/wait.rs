//! `oxo-flow wait`: block until a run finishes, then report its outcome
//! (issue #1078 F4).
//!
//! Scripted supervision used to be a poll-for-output-files shell loop. The
//! pair `oxo-flow run --background` + `oxo-flow wait --workdir DIR` makes
//! that native:
//!
//! - a run in progress is detected from the workdir lock (foreground runs)
//!   or from the background pid file (detached runs);
//! - completion is the moment neither is held/ alive anymore;
//! - the checkpoint's `<checkpoint>.status` file then supplies the run's own
//!   terminal outcome. It belongs to ONE checkpoint: activity from a
//!   different variant sharing the workdir cannot turn a finished run's
//!   succeeded status into a failure, and a status left `running` by a dead
//!   process is a failure;
//! - without a status file (legacy runs), the checkpoint supplies the
//!   structured verdict — failed rules, rules still marked in-flight (the
//!   process died), or success.
//!
//! Exit status: `0` success, `1` failed/incomplete, `3` timeout, `4` no run
//! found (no lock, no pid file, no checkpoint). `--json` emits the same
//! verdict as one document on stdout.

use anyhow::{Context, Result};
use oxo_flow_core::executor::timeout::process_exists;
use oxo_flow_core::executor::{CheckpointState, WorkdirLock};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The run finished but at least one rule failed or stayed in-flight.
pub const EXIT_FAILED: i32 = 1;
/// The wait deadline elapsed while the run was still active.
pub const EXIT_TIMEOUT: i32 = 3;
/// No run and no checkpoint were found in the workdir.
pub const EXIT_NO_RUN: i32 = 4;

/// `wait` invocation parameters.
pub struct WaitRequest {
    pub workdir: Option<PathBuf>,
    pub checkpoint: Option<PathBuf>,
    pub timeout_secs: u64,
    pub interval_secs: u64,
    pub json: bool,
}

/// Structured completion verdict derived from a checkpoint.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WaitSummary {
    pub failed: Vec<String>,
    pub running: Vec<String>,
    /// Rules whose failure record is stale because their `when` gate
    /// evaluated false (reported as skipped, not failed).
    pub skipped_when_false: Vec<String>,
}

impl WaitSummary {
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failed.is_empty() && self.running.is_empty()
    }
}

/// Build the completion verdict from a checkpoint.
#[must_use]
pub fn summarize(state: &CheckpointState) -> WaitSummary {
    let (failed, skipped_when_false) = crate::commands::run::status::reconcile_failed_rules(
        &state.failed_rules,
        &state.when_verdicts,
    );
    WaitSummary {
        failed: failed.into_iter().map(str::to_string).collect(),
        running: state.running.iter().cloned().collect(),
        skipped_when_false: skipped_when_false.into_iter().map(str::to_string).collect(),
    }
}

/// Read a pid file, tolerating whitespace and trailing newlines. Pid 0 and
/// values outside the `i32` range are rejected: `kill(0, …)` targets the
/// caller's process group and a wrapped negative pid targets every
/// process (issue #1078 M3).
#[must_use]
pub fn read_pid(path: &Path) -> Option<u32> {
    let pid = std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    (pid > 0 && pid <= i32::MAX as u32).then_some(pid)
}

/// Block until the run in `workdir` finishes, then print the verdict.
///
/// Returns normally on success and calls [`std::process::exit`] with the
/// documented status on failure, timeout, or a missing run.
pub async fn wait_command(request: WaitRequest) -> Result<()> {
    let interval = Duration::from_secs(request.interval_secs.max(1));
    let deadline = (request.timeout_secs > 0).then(|| Duration::from_secs(request.timeout_secs));
    let started = Instant::now();
    let mut checkpoint_arg = request.checkpoint.clone();
    // Resolve the run directory. With only `--checkpoint`, the checkpoint
    // itself names the workdir; before it is written, poll for it — NEVER
    // fall back to the caller's cwd, whose unrelated run status could turn
    // an active run into a false success (issue #1078 R1). An empty or
    // status-only checkpoint is not a finished run: a `failed` status is
    // reported immediately (the run failed before its first save, R11) and
    // anything else keeps polling (R12).
    let workdir = if let Some(dir) = &request.workdir {
        dir.clone()
    } else if let Some(path) = checkpoint_arg.as_ref() {
        loop {
            match CheckpointState::load_from_file(path) {
                Ok(state) => {
                    if let Some(dir) = state.workdir {
                        break PathBuf::from(dir);
                    }
                    if checkpoint_has_state(&state) {
                        tracing::warn!(
                            checkpoint = %path.display(),
                            "checkpoint records no workdir — using the current directory"
                        );
                        break std::env::current_dir()
                            .context("cannot resolve the current directory")?;
                    }
                    await_unwritten_checkpoint(path, &request, started, deadline, interval).await?;
                }
                Err(_error) if !path.exists() => {
                    await_unwritten_checkpoint(path, &request, started, deadline, interval).await?;
                }
                Err(error) => {
                    if request.json {
                        crate::commands::json_error::emit(
                            "wait",
                            "CHECKPOINT",
                            &format!("{error:#}"),
                            serde_json::json!({"checkpoint": path.display().to_string()}),
                        );
                    }
                    return Err(error).with_context(|| {
                        format!(
                            "failed to load checkpoint from '{}' — the run may have been \
                             killed before it could flush",
                            path.display()
                        )
                    });
                }
            }
        }
    } else {
        std::env::current_dir().context("cannot resolve the current directory")?
    };
    let checkpoint = checkpoint_arg
        .take()
        .unwrap_or_else(|| CheckpointState::default_path(&workdir));
    let pid_file = workdir.join(".oxo-flow").join("background.pid");
    let mut saw_activity = false;

    loop {
        let pid = read_pid(&pid_file);
        let child_alive = pid.is_some_and(process_exists);
        // The workdir lock covers foreground runs (and the window before a
        // background child writes its first checkpoint).
        let locked = WorkdirLock::is_locked(&workdir);
        if !child_alive && !locked {
            // A pid file left behind by a dead process is evidence of a run
            // that never recorded a terminal status (killed before its
            // handler installed); never fall back to an older checkpoint.
            saw_activity |= pid.is_some();
            break;
        }
        saw_activity = true;
        if let Some(limit) = deadline
            && started.elapsed() >= limit
        {
            let elapsed = started.elapsed().as_secs();
            if request.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "command": "wait",
                        "status": "timeout",
                        "checkpoint": checkpoint.display().to_string(),
                        "elapsed_seconds": elapsed,
                        "timeout_seconds": request.timeout_secs,
                    })
                );
            } else {
                eprintln!(
                    "wait: timed out after {elapsed}s — the run is still active; \
                     it was NOT cancelled (checkpoint: {})",
                    checkpoint.display()
                );
            }
            std::process::exit(EXIT_TIMEOUT);
        }
        tokio::time::sleep(interval).await;
    }

    // The run-status file is the run process's own terminal outcome and it
    // belongs to THIS checkpoint. Activity from another variant sharing the
    // workdir must not disqualify it (issue #1078 R10); a status left
    // `running` is only a failure once its process is gone. The wrapper
    // writes the terminal status immediately after releasing the lock, so a
    // still-`running` status whose process is alive gets one polling
    // interval to land before it counts as a killed run (R13).
    let mut status = crate::run_status::read_status(&checkpoint);
    if let Some(current) = status.as_ref()
        && current.state == crate::run_status::RunState::Running
        && process_exists(current.pid)
    {
        tokio::time::sleep(interval).await;
        status = crate::run_status::read_status(&checkpoint).or(status);
    }
    let elapsed = started.elapsed().as_secs();
    if let Some(status) = status.as_ref() {
        match status.state {
            crate::run_status::RunState::Running => {
                if request.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "command": "wait",
                            "status": "failed",
                            "run_state": "running",
                            "checkpoint": checkpoint.display().to_string(),
                            "elapsed_seconds": elapsed,
                            "failed": [],
                            "running": [],
                        })
                    );
                } else {
                    eprintln!(
                        "wait: the run process exited while its status was still 'running' — \
                         it was killed or crashed before recording a terminal outcome \
                         (checkpoint: {})",
                        checkpoint.display()
                    );
                }
                std::process::exit(EXIT_FAILED);
            }
            crate::run_status::RunState::Failed => {
                exit_status_failure(&checkpoint, status, elapsed, request.json, None);
            }
            crate::run_status::RunState::Succeeded => {
                // The run's own terminal status is AUTHORITATIVE: a
                // `failed_rules` entry left by an earlier run (a removed or
                // renamed rule, a `--target` subset) must not turn this
                // success into a failure (issue #1078 R4).
                let stale = if checkpoint.exists() {
                    CheckpointState::load_from_file(&checkpoint)
                        .ok()
                        .map(|state| summarize(&state))
                        .unwrap_or_default()
                } else {
                    WaitSummary::default()
                };
                if request.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "command": "wait",
                            "status": "succeeded",
                            "run_state": "succeeded",
                            "checkpoint": checkpoint.display().to_string(),
                            "elapsed_seconds": elapsed,
                            "failed": [],
                            "running": [],
                            "skipped_when_false": stale.skipped_when_false,
                            "stale_failed": stale.failed,
                        })
                    );
                } else {
                    eprintln!(
                        "wait: run finished successfully in {elapsed}s (checkpoint: {})",
                        checkpoint.display()
                    );
                    if !stale.skipped_when_false.is_empty() {
                        eprintln!(
                            "wait: {} skipped by a false when-condition: {}",
                            stale.skipped_when_false.len(),
                            stale.skipped_when_false.join(", ")
                        );
                    }
                    if !stale.failed.is_empty() {
                        eprintln!(
                            "wait: note — the checkpoint retains {} stale failure record(s) \
                             from an earlier run: {} (the current run succeeded)",
                            stale.failed.len(),
                            stale.failed.join(", ")
                        );
                    }
                }
                return Ok(());
            }
        }
    } else if saw_activity {
        // We watched a run end but it never recorded a terminal status:
        // killed, crashed, or it failed before arming. Never success.
        if request.json {
            println!(
                "{}",
                serde_json::json!({
                    "command": "wait",
                    "status": "failed",
                    "run_state": "unknown",
                    "checkpoint": checkpoint.display().to_string(),
                    "elapsed_seconds": elapsed,
                    "failed": [],
                    "running": [],
                })
            );
        } else {
            eprintln!(
                "wait: the run ended without recording a terminal status (killed, crashed, \
                 or failed before startup) — check the run log; checkpoint: {}",
                checkpoint.display()
            );
        }
        std::process::exit(EXIT_FAILED);
    }

    if !checkpoint.exists() {
        if request.json {
            println!(
                "{}",
                serde_json::json!({
                    "command": "wait",
                    "status": "no-run",
                    "checkpoint": checkpoint.display().to_string(),
                    "elapsed_seconds": elapsed,
                    "saw_activity": saw_activity,
                })
            );
        } else if saw_activity {
            eprintln!(
                "wait: the run exited before writing a checkpoint at '{}' — it most likely \
                 failed preflight; check the run log and `oxo-flow status`",
                checkpoint.display()
            );
        } else {
            eprintln!(
                "wait: no active run and no checkpoint at '{}' — start the run with \
                 `oxo-flow run … --background` (or from another terminal) and retry",
                checkpoint.display()
            );
        }
        std::process::exit(EXIT_NO_RUN);
    }

    let state = match CheckpointState::load_from_file(&checkpoint) {
        Ok(state) => state,
        Err(error) => {
            // `--json` is one-document-in / one-document-out: a malformed
            // checkpoint must not exit with empty stdout.
            if request.json {
                crate::commands::json_error::emit(
                    "wait",
                    "CHECKPOINT",
                    &format!("{error:#}"),
                    serde_json::json!({"checkpoint": checkpoint.display().to_string()}),
                );
            }
            return Err(error).with_context(|| {
                format!(
                    "failed to load checkpoint from '{}' — the run may have been killed \
                     before it could flush",
                    checkpoint.display()
                )
            });
        }
    };
    if !checkpoint_has_state(&state) && state.workdir.is_none() {
        // A zero-byte or whitespace-only file parses as an empty state;
        // calling that a success would report a run that never existed
        // (issue #1078 R12).
        if request.json {
            println!(
                "{}",
                serde_json::json!({
                    "command": "wait",
                    "status": "no-run",
                    "checkpoint": checkpoint.display().to_string(),
                    "elapsed_seconds": elapsed,
                })
            );
        } else {
            eprintln!(
                "wait: the checkpoint at '{}' is empty — no run has written its state \
                 there; start the run and retry",
                checkpoint.display()
            );
        }
        std::process::exit(EXIT_NO_RUN);
    }
    let summary = summarize(&state);
    if request.json {
        println!(
            "{}",
            serde_json::json!({
                "command": "wait",
                "status": if summary.is_success() { "succeeded" } else { "failed" },
                "checkpoint": checkpoint.display().to_string(),
                "elapsed_seconds": elapsed,
                "failed": summary.failed,
                "running": summary.running,
                "skipped_when_false": summary.skipped_when_false,
            })
        );
    } else if summary.is_success() {
        eprintln!(
            "wait: run finished successfully in {elapsed}s (checkpoint: {})",
            checkpoint.display()
        );
        if !summary.skipped_when_false.is_empty() {
            eprintln!(
                "wait: {} skipped by a false when-condition: {}",
                summary.skipped_when_false.len(),
                summary.skipped_when_false.join(", ")
            );
        }
    } else {
        if !summary.failed.is_empty() {
            eprintln!(
                "wait: {} rule(s) failed: {}",
                summary.failed.len(),
                summary.failed.join(", ")
            );
        }
        if !summary.running.is_empty() {
            eprintln!(
                "wait: {} rule(s) were still marked running — the run process exited \
                 before completing them (killed or crashed): {}",
                summary.running.len(),
                summary.running.join(", ")
            );
        }
        if !summary.skipped_when_false.is_empty() {
            eprintln!(
                "wait: {} skipped by a false when-condition: {}",
                summary.skipped_when_false.len(),
                summary.skipped_when_false.join(", ")
            );
        }
    }
    if summary.is_success() {
        Ok(())
    } else {
        std::process::exit(EXIT_FAILED);
    }
}

/// Whether a checkpoint carries real run state (as opposed to a zero-byte
/// pre-created file, which `load_from_file` tolerates as a default,
/// issue #1078 R12).
fn checkpoint_has_state(state: &CheckpointState) -> bool {
    state.workflow_path.is_some()
        || !state.completed_rules.is_empty()
        || !state.failed_rules.is_empty()
        || !state.running.is_empty()
        || !state.benchmarks.is_empty()
}

/// Report a run's `failed` status (exit 1). The status message names the
/// top-level failure (a preflight refusal, for example); the checkpoint
/// names the failed RULES when the run got that far.
fn exit_status_failure(
    checkpoint: &Path,
    status: &crate::run_status::RunStatus,
    elapsed: u64,
    json: bool,
    fallback_message: Option<&str>,
) -> ! {
    let summary = CheckpointState::load_from_file(checkpoint)
        .ok()
        .map(|state| summarize(&state))
        .unwrap_or_default();
    let message = status
        .message
        .clone()
        .or_else(|| fallback_message.map(str::to_string))
        .unwrap_or_default();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "command": "wait",
                "status": "failed",
                "run_state": "failed",
                "checkpoint": checkpoint.display().to_string(),
                "elapsed_seconds": elapsed,
                "message": message,
                "failed": summary.failed,
                "running": summary.running,
                "skipped_when_false": summary.skipped_when_false,
            })
        );
    } else {
        eprintln!(
            "wait: the run failed: {message}\n  (checkpoint: {})",
            checkpoint.display()
        );
        if !summary.failed.is_empty() {
            eprintln!(
                "wait: {} rule(s) failed: {}",
                summary.failed.len(),
                summary.failed.join(", ")
            );
        }
    }
    std::process::exit(EXIT_FAILED);
}

/// Poll while a checkpoint has not been written yet (issue #1078 R11/R12).
///
/// The run arms `<checkpoint>.status` before any preflight, so a `failed`
/// status is already the answer even though the checkpoint never appears;
/// a `running` status whose process is gone is a killed run. Anything else
/// keeps polling, bounded by `--timeout`.
async fn await_unwritten_checkpoint(
    checkpoint: &Path,
    request: &WaitRequest,
    started: Instant,
    deadline: Option<Duration>,
    interval: Duration,
) -> Result<()> {
    if let Some(status) = crate::run_status::read_status(checkpoint) {
        match status.state {
            crate::run_status::RunState::Failed => {
                exit_status_failure(
                    checkpoint,
                    &status,
                    started.elapsed().as_secs(),
                    request.json,
                    None,
                );
            }
            crate::run_status::RunState::Running if !process_exists(status.pid) => {
                exit_status_failure(
                    checkpoint,
                    &status,
                    started.elapsed().as_secs(),
                    request.json,
                    Some("the run process exited before writing its checkpoint"),
                );
            }
            _ => {}
        }
    }
    if let Some(limit) = deadline
        && started.elapsed() >= limit
    {
        let elapsed = started.elapsed().as_secs();
        if request.json {
            println!(
                "{}",
                serde_json::json!({
                    "command": "wait",
                    "status": "no-run",
                    "checkpoint": checkpoint.display().to_string(),
                    "elapsed_seconds": elapsed,
                })
            );
        } else {
            eprintln!(
                "wait: no checkpoint appeared at '{}' within {elapsed}s — \
                 pass --workdir for an immediate wait, or check that the run \
                 is using this checkpoint",
                checkpoint.display()
            );
        }
        std::process::exit(EXIT_NO_RUN);
    }
    tokio::time::sleep(interval).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn state() -> CheckpointState {
        CheckpointState::default()
    }

    #[test]
    fn read_pid_trims_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.pid");
        std::fs::write(&path, " 4321\n").unwrap();
        assert_eq!(read_pid(&path), Some(4321));
    }

    #[test]
    fn read_pid_rejects_garbage_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        assert_eq!(read_pid(&path), None);
        assert_eq!(read_pid(&dir.path().join("absent.pid")), None);
    }

    #[test]
    fn read_pid_rejects_zero_and_overflowing_pids() {
        // `kill(0, 0)` probes the caller's process group and a wrapped
        // negative pid probes every process (issue #1078 M3).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.pid");
        for raw in ["0", "4294967295", "2147483648"] {
            std::fs::write(&path, raw).unwrap();
            assert_eq!(read_pid(&path), None, "{raw}");
        }
        std::fs::write(&path, (i32::MAX as u32).to_string()).unwrap();
        assert_eq!(read_pid(&path), Some(i32::MAX as u32));
    }

    #[test]
    fn an_empty_checkpoint_is_not_run_state() {
        // Issue #1078 R12: a pre-created zero-byte checkpoint parses as a
        // default state; that must not summarize as success.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ck.json");
        std::fs::write(&path, "").unwrap();
        let state = CheckpointState::load_from_file(&path).unwrap();
        assert!(!checkpoint_has_state(&state));
        assert!(state.workdir.is_none());
    }

    #[test]
    fn checkpoint_state_detects_real_run_state() {
        let mut state = state();
        assert!(!checkpoint_has_state(&state));
        state.completed_rules.insert("trim".to_string());
        assert!(checkpoint_has_state(&state));
    }

    #[test]
    fn summarize_success_is_clean() {
        let summary = summarize(&state());
        assert!(summary.is_success());
        assert!(summary.failed.is_empty());
    }

    #[test]
    fn summarize_reports_failures_and_in_flight_rules() {
        let mut state = state();
        state.failed_rules = BTreeSet::from(["trim".to_string(), "align".to_string()]);
        state.running = BTreeSet::from(["call".to_string()]);
        let summary = summarize(&state);
        assert!(!summary.is_success());
        assert_eq!(summary.failed, vec!["align", "trim"]);
        assert_eq!(summary.running, vec!["call"]);
    }

    #[test]
    fn summarize_treats_when_false_records_as_skipped_not_failed() {
        let mut state = state();
        state.failed_rules = BTreeSet::from(["optional_qc".to_string()]);
        state.when_verdicts = BTreeMap::from([("optional_qc".to_string(), false)]);
        let summary = summarize(&state);
        assert!(summary.is_success());
        assert_eq!(summary.skipped_when_false, vec!["optional_qc"]);
    }
}
