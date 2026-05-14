use chrono::Utc;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info};
use regex::Regex;

use crate::db;
use crate::workspace::get_run_directory;

/// Spawns a background task to execute the workflow sandbox.
pub fn spawn_background_run(
    run_id: String,
    username: String,
    auth_type: String,
    os_user: String,
) {
    tokio::spawn(async move {
        info!("Starting background run {} for user {}", run_id, username);

        // Update status to running
        let now = Utc::now();
        let _ = sqlx::query("UPDATE runs SET status = 'running', started_at = ? WHERE id = ?")
            .bind(now)
            .bind(&run_id)
            .execute(db::pool())
            .await;

        let run_dir = get_run_directory(&username, &run_id);
        let workflow_file = run_dir.join("workflow.oxoflow");

        // Validate OS User to prevent injection in sudo
        let os_user_regex = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").unwrap();
        if auth_type == "sudo" && !os_user_regex.is_match(&os_user) {
            error!("Invalid OS username format: {}", os_user);
            let end = Utc::now();
            let _ = sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ?")
                .bind(end)
                .bind(&run_id)
                .execute(db::pool())
                .await;
            return;
        }

        let mut cmd = if auth_type == "sudo" && os_user != "oxo-flow" {
            // Sudo mode
            let mut c = Command::new("sudo");
            c.arg("-n") // non-interactive
                .arg("-u")
                .arg(&os_user)
                .arg("oxo-flow")
                .arg("run")
                .arg(workflow_file)
                .arg("--workdir")
                .arg(&run_dir);
            c
        } else {
            // Local mode
            let mut c = Command::new("oxo-flow");
            c.arg("run")
                .arg(workflow_file)
                .arg("--workdir")
                .arg(&run_dir);
            c
        };

        // Capture output to files in the run directory
        let log_file_path = run_dir.join("execution.log");
        let log_file = match std::fs::File::create(&log_file_path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create log file: {}", e);
                let end = Utc::now();
                let _ = sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ?")
                    .bind(end)
                    .bind(&run_id)
                    .execute(db::pool())
                    .await;
                return;
            }
        };
        let err_file = log_file.try_clone().expect("Failed to clone log file handle");

        cmd.stdout(Stdio::from(log_file));
        cmd.stderr(Stdio::from(err_file));

        match cmd.spawn() {
            Ok(mut child) => {
                // Record PID
                if let Some(pid) = child.id() {
                    let _ = sqlx::query("UPDATE runs SET pid = ? WHERE id = ?")
                        .bind(pid as i64)
                        .bind(&run_id)
                        .execute(db::pool())
                        .await;
                }

                // Wait for completion
                match child.wait().await {
                    Ok(status) => {
                        let final_state = if status.success() { "success" } else { "failed" };
                        let end = Utc::now();
                        let _ = sqlx::query("UPDATE runs SET status = ?, finished_at = ? WHERE id = ?")
                            .bind(final_state)
                            .bind(end)
                            .bind(&run_id)
                            .execute(db::pool())
                            .await;
                        info!("Run {} finished with status: {}", run_id, final_state);
                    }
                    Err(e) => {
                        error!("Failed to wait on child process for run {}: {}", run_id, e);
                        let end = Utc::now();
                        let _ = sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ?")
                            .bind(end)
                            .bind(&run_id)
                            .execute(db::pool())
                            .await;
                    }
                }
            }
            Err(e) => {
                error!("Failed to spawn child process for run {}: {}", run_id, e);
                let end = Utc::now();
                let _ = sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ?")
                    .bind(end)
                    .bind(&run_id)
                    .execute(db::pool())
                    .await;
            }
        }
    });
}
