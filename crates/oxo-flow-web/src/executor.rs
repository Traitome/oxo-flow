use chrono::Utc;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::workspace::get_run_directory;
use crate::{broadcast_event, db};

/// Locate the `oxo-flow` CLI binary.
///
/// Search order:
///   1. `OXO_FLOW_BIN` environment variable (explicit override)
///   2. `CARGO_BIN_EXE_oxo-flow` (set by cargo test when oxo-flow-cli is a dependency)
///   3. Next to the current executable (same target dir)
///   4. One level above the current executable (cargo test places test binaries in `deps/`)
///   5. Fall back to `"oxo-flow"` (PATH lookup)
fn find_oxo_flow_binary() -> PathBuf {
    if let Ok(path) = std::env::var("OXO_FLOW_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_oxo-flow")
        && std::path::Path::new(&path).exists()
    {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let sibling = exe_dir.join("oxo-flow");
        if sibling.exists() {
            return sibling;
        }
        // When running under `cargo test`, the test binary lives in
        // `target/debug/deps/`, so the actual binary is one level up.
        if let Some(parent) = exe_dir.parent() {
            let in_parent = parent.join("oxo-flow");
            if in_parent.exists() {
                return in_parent;
            }
        }
    }
    PathBuf::from("oxo-flow")
}

/// Extract the invalidation summary lines the CLI writes into execution.log
/// (issue #69): the config-change block, input-manifest invalidation lines,
/// and baseline adoption notes. Returns `None` when the log shows no
/// invalidation activity.
///
/// Substring matching is ANSI-escape tolerant: the colored prefixes wrap
/// whole words, so the literal text stays contiguous.
fn extract_invalidation_summary(log: &str) -> Option<String> {
    let mut summary: Vec<&str> = Vec::new();
    let mut in_config_block = false;
    for line in log.lines() {
        if line.contains("Config change:") {
            in_config_block = true;
            summary.push(line);
        } else if in_config_block {
            if line.contains("re-running") {
                // Final line of the block ("→ invalidated N …, re-running …").
                summary.push(line);
                in_config_block = false;
            } else if line.trim().is_empty() {
                // The block ended without a summary line.
                in_config_block = false;
            } else {
                // Key-level lines ("  key: old → new", "(new key)",
                // "rule definition changed: …", "→ invalidated N …").
                summary.push(line);
            }
        } else if line.contains("input changes invalidated")
            || line.contains("recorded baseline input manifests")
        {
            summary.push(line);
        }
    }
    if summary.is_empty() {
        None
    } else {
        Some(summary.join("\n"))
    }
}

/// Map a subprocess exit status to the run status vocabulary.
fn final_status_from_exit(success: bool) -> &'static str {
    if success { "completed" } else { "failed" }
}

/// Execution options carried from the run request to the CLI subprocess.
#[derive(Debug, Clone, Default)]
pub struct RunFlags {
    /// Preview mode: spawns `oxo-flow dry-run` instead of executing anything.
    pub dry_run: bool,
    /// Continue executing independent rules when one fails (`-k`).
    pub keep_going: bool,
    /// Explicitly requested parallelism (`-j`); `None` keeps the CLI default.
    pub max_jobs: Option<usize>,
}

/// Spawns a background task to execute the workflow in a sandboxed workspace.
///
/// `workdir` is the directory the CLI executes in: a persistent per-pipeline
/// directory for saved-pipeline runs (so checkpoint-driven invalidation
/// applies across re-runs, issue #69) or the per-run sandbox for ad-hoc runs.
/// The caller must have already written `workflow.oxoflow` there.
pub fn spawn_background_run(
    run_id: String,
    username: String,
    auth_type: String,
    os_user: String,
    workdir: Option<PathBuf>,
    flags: RunFlags,
) {
    tokio::spawn(async move {
        info!("Starting background run {} for user {}", run_id, username);

        // Update status to running
        let now = Utc::now();
        if let Err(e) =
            sqlx::query("UPDATE runs SET status = 'running', started_at = ? WHERE id = ?")
                .bind(now)
                .bind(&run_id)
                .execute(db::pool())
                .await
        {
            error!("Failed to update run {run_id} to running: {e}");
            return;
        }

        // Broadcast run start event
        broadcast_event(
            "run_started",
            &serde_json::json!({
                "run_id": run_id,
                "username": username,
                "status": "running",
                "started_at": now.to_rfc3339(),
            }),
        );

        let run_dir = workdir.unwrap_or_else(|| get_run_directory(&username, &run_id));
        let workflow_file = run_dir.join("workflow.oxoflow");

        // Validate OS username to prevent injection in sudo mode
        let os_user_regex = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$")
            .expect("Static regex pattern should always compile");
        if auth_type == "sudo" && !os_user_regex.is_match(&os_user) {
            error!("Invalid OS username format: {os_user}");
            mark_run_failed(&run_id).await;
            return;
        }

        let oxo_bin = find_oxo_flow_binary();

        // Issue #69 follow-up: honor the run request's execution flags.
        // `dry_run` spawns the preview subcommand (nothing executes);
        // max_jobs and keep_going map to -j / -k. Only explicitly requested
        // jobs are passed — the CLI default stays in charge otherwise.
        let mut oxo_args: Vec<std::ffi::OsString> = Vec::new();
        if flags.dry_run {
            oxo_args.push("dry-run".into());
        } else {
            oxo_args.push("run".into());
        }
        oxo_args.push(workflow_file.as_os_str().to_owned());
        oxo_args.push("--workdir".into());
        oxo_args.push(run_dir.as_os_str().to_owned());
        if !flags.dry_run {
            if flags.keep_going {
                oxo_args.push("--keep-going".into());
            }
            if let Some(jobs) = flags.max_jobs {
                oxo_args.push("-j".into());
                oxo_args.push(jobs.to_string().into());
            }
        }

        let mut cmd = if auth_type == "sudo" && os_user != "oxo-flow" {
            let mut c = Command::new("sudo");
            c.arg("-n").arg("-u").arg(&os_user).arg(&oxo_bin);
            c.args(&oxo_args);
            c
        } else {
            let mut c = Command::new(&oxo_bin);
            c.args(&oxo_args);
            c
        };

        // Capture output to files in the run directory
        let log_file_path = run_dir.join("execution.log");
        let log_file = match std::fs::File::create(&log_file_path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create log file for run {run_id}: {e}");
                mark_run_failed(&run_id).await;
                return;
            }
        };
        let err_file = match log_file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to clone log file handle for run {run_id}: {e}");
                mark_run_failed(&run_id).await;
                return;
            }
        };

        // New process group: signals from cancel/pause/resume handlers reach
        // the CLI and every rule subprocess it spawns (see process_control).
        #[cfg(unix)]
        cmd.process_group(0);

        cmd.stdout(Stdio::from(log_file));
        cmd.stderr(Stdio::from(err_file));

        match cmd.spawn() {
            Ok(mut child) => {
                // Record PID for cancellation support
                if let Some(pid) = child.id()
                    && let Err(e) = sqlx::query("UPDATE runs SET pid = ? WHERE id = ?")
                        .bind(pid as i64)
                        .bind(&run_id)
                        .execute(db::pool())
                        .await
                {
                    warn!("Failed to record PID for run {run_id}: {e}");
                }

                // Register the process group so cancel/pause/resume can signal it.
                if let Some(pid) = child.id() {
                    crate::process_control::register(&run_id, pid as i32);
                }

                // Wait for process completion
                match child.wait().await {
                    Ok(status) => {
                        crate::process_control::unregister(&run_id);
                        let final_state = final_status_from_exit(status.success());
                        let end = Utc::now();
                        // A concurrent cancel sets status='cancelled' and emits
                        // run_cancelled; the exit here is the SIGKILL fallout
                        // and must not overwrite it back to completed/failed.
                        let cancelled: bool = sqlx::query_scalar(
                            "SELECT COUNT(*) FROM runs WHERE id = ? AND status = 'cancelled'",
                        )
                        .bind(&run_id)
                        .fetch_one(db::pool())
                        .await
                        .map(|n: i64| n > 0)
                        .unwrap_or(false);
                        if !cancelled {
                            if let Err(e) =
                                sqlx::query("UPDATE runs SET status = ?, finished_at = ? WHERE id = ?")
                                    .bind(final_state)
                                    .bind(end)
                                    .bind(&run_id)
                                    .execute(db::pool())
                                    .await
                            {
                                error!("Failed to update final status for run {run_id}: {e}");
                            }
                            info!("Run {run_id} finished: {final_state}");

                            // Broadcast the terminal event (documented in the SSE
                            // API): run_completed on success, run_failed otherwise.
                            let event = if status.success() {
                                "run_completed"
                            } else {
                                "run_failed"
                            };
                            // Surfacing the CLI's invalidation summary (issue #69):
                            // config changes, rule-definition edits, and input-set
                            // changes that invalidated checkpoint records this run.
                            let summary = std::fs::read_to_string(&log_file_path)
                                .ok()
                                .and_then(|log| extract_invalidation_summary(&log));
                            broadcast_event(
                                event,
                                &serde_json::json!({
                                    "run_id": run_id,
                                    "status": final_state,
                                    "finished_at": end.to_rfc3339(),
                                    "summary": summary,
                                }),
                            );
                        } else {
                            info!("Run {run_id} exited after cancel; keeping 'cancelled'");
                        }
                    }
                    Err(e) => {
                        crate::process_control::unregister(&run_id);
                        error!("Failed to wait on child process for run {run_id}: {e}");
                        mark_run_failed(&run_id).await;
                    }
                }
            }
            Err(e) => {
                error!("Failed to spawn child process for run {run_id}: {e}");
                mark_run_failed(&run_id).await;
            }
        }
    });
}

/// Mark a run as failed with the current timestamp.
async fn mark_run_failed(run_id: &str) {
    let end = Utc::now();
    if let Err(e) = sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ?")
        .bind(end)
        .bind(run_id)
        .execute(db::pool())
        .await
    {
        error!("Failed to mark run {run_id} as failed: {e}");
    }

    // Broadcast run failure event
    broadcast_event(
        "run_failed",
        &serde_json::json!({
            "run_id": run_id,
            "status": "failed",
            "finished_at": end.to_rfc3339(),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_success_maps_to_completed() {
        assert_eq!(final_status_from_exit(true), "completed");
        assert_eq!(final_status_from_exit(false), "failed");
    }

    #[test]
    fn summary_extracts_config_change_block() {
        let log = "DAG: 2 rules in execution order\n\
                   Config change:\n\
                   \x20 min_quality: 20 → 30\n\
                   \x20 → invalidated 1 (1 directly affected), re-running 1/2 this run, skipping 1\n\
                   \x20 Running: affected\n\
                   \x20 ✓ affected (0.1s)\n";
        let summary = extract_invalidation_summary(log).expect("config change must be found");
        assert!(summary.contains("Config change:"));
        assert!(summary.contains("min_quality: 20 → 30"));
        assert!(summary.contains("re-running 1/2 this run, skipping 1"));
        assert!(!summary.contains("Running: affected"));
        assert!(!summary.contains("✓ affected"));
    }

    #[test]
    fn summary_extracts_input_manifest_invalidations() {
        let log = "  ↻ input changes invalidated 2 rule(s): gather, report\n\
                   Note: checkpoint predates input tracking: recorded baseline input manifests for 1 completed rule(s); future input changes will invalidate them automatically\n";
        let summary = extract_invalidation_summary(log).expect("invalidation lines must be found");
        assert!(summary.contains("input changes invalidated 2 rule(s): gather, report"));
        assert!(summary.contains("recorded baseline input manifests"));
    }

    #[test]
    fn summary_handles_ansi_colored_output() {
        // The CLI colors "Config change:" and the "↻" marker when stderr is a
        // tty; substring matching must survive the escape wrappers.
        let log = "\u{1b}[1;36mConfig change:\u{1b}[0m\n\
                   \x20 n_threads: 4 → 8\n\
                   \x20 → invalidated 2 (1 directly affected), re-running 2/3 this run, skipping 1\n\
                   \u{1b}[33m↻\u{1b}[0m input changes invalidated 1 rule(s): align\n";
        let summary = extract_invalidation_summary(log).expect("must parse ANSI-wrapped lines");
        assert!(summary.contains("Config change:"));
        assert!(summary.contains("n_threads: 4 → 8"));
        assert!(summary.contains("input changes invalidated 1 rule(s): align"));
    }

    #[test]
    fn summary_returns_none_without_invalidation_activity() {
        let log = "DAG: 2 rules in execution order\nRunning: a\n✓ a (0.1s)\nDone: 2 succeeded\n";
        assert_eq!(extract_invalidation_summary(log), None);
    }

    #[test]
    fn sudo_username_regex_accepts_valid() {
        let re = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").unwrap();
        assert!(re.is_match("admin"));
        assert!(re.is_match("user_001"));
        assert!(re.is_match("test-user"));
        assert!(re.is_match("bioinfo$"));
    }

    #[test]
    fn sudo_username_regex_rejects_injection() {
        let re = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").unwrap();
        assert!(!re.is_match("admin; rm -rf /"));
        assert!(!re.is_match("user$(whoami)"));
        assert!(!re.is_match("root /etc/passwd"));
        assert!(!re.is_match(""));
        assert!(!re.is_match("UPPERCASE"));
    }
}
