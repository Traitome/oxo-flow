use chrono::Utc;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::db;
use crate::workspace::get_run_directory;

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

/// Spawns a background task to execute the workflow in a sandboxed workspace.
pub fn spawn_background_run(run_id: String, username: String, auth_type: String, os_user: String) {
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

        let run_dir = get_run_directory(&username, &run_id);
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

        let mut cmd = if auth_type == "sudo" && os_user != "oxo-flow" {
            let mut c = Command::new("sudo");
            c.arg("-n")
                .arg("-u")
                .arg(&os_user)
                .arg(&oxo_bin)
                .arg("run")
                .arg(&workflow_file)
                .arg("--workdir")
                .arg(&run_dir);
            c
        } else {
            let mut c = Command::new(&oxo_bin);
            c.arg("run")
                .arg(&workflow_file)
                .arg("--workdir")
                .arg(&run_dir);
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

                // Wait for process completion
                match child.wait().await {
                    Ok(status) => {
                        let final_state = if status.success() {
                            "success"
                        } else {
                            "failed"
                        };
                        let end = Utc::now();
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
                    }
                    Err(e) => {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
