//! Remote execution backend: the web server runs on host A, the workflow
//! executes on a cluster login node reachable over SSH.
//!
//! Protocol (no rsync required — tar streams over stdio):
//!   1. stage — the caller wrote `workflow.oxoflow` + local inputs into
//!      the run's LOCAL workdir; a tar of that directory is piped through
//!      `ssh … tar -xf -` into `{remote_dir}/runs/{run_id}`.
//!   2. launch — a per-run wrapper script (`run-{run_id}.sh`) runs the
//!      CLI remotely under nohup; its exit code lands in `.exit-code`
//!      (same convention the local executor uses).
//!   3. poll — the web server polls `.exit-code` over SSH every 5s.
//!   4. pull — on completion the whole remote workdir is tar-pulled back
//!      into the LOCAL workdir, so logs/files/preview/report endpoints
//!      keep working unchanged (they read the local path).
//!
//! Cancellation: `pkill -f run-{run_id}.sh` on the remote — the wrapper
//! name is unique per run.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::infra::db::models::ClusterRow;
use crate::{broadcast_event_for, db};

use super::service::ssh_base_args;

/// registry: run_id → cluster id, for cancellation of remote runs.
static REMOTE_RUNS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub fn register_remote(run_id: &str, cluster_id: &str) {
    if let Ok(mut map) = REMOTE_RUNS.lock() {
        map.insert(run_id.to_string(), cluster_id.to_string());
    }
}

pub fn unregister_remote(run_id: &str) {
    if let Ok(mut map) = REMOTE_RUNS.lock() {
        map.remove(run_id);
    }
}

pub fn remote_cluster_of(run_id: &str) -> Option<String> {
    REMOTE_RUNS
        .lock()
        .ok()
        .and_then(|map| map.get(run_id).cloned())
}

fn ssh(cluster: &ClusterRow) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("ssh");
    for arg in ssh_base_args(cluster.ssh_port as u16) {
        cmd.arg(arg);
    }
    if let Some(key) = cluster.ssh_key.as_deref() {
        cmd.arg("-i").arg(key);
    }
    cmd.arg(match cluster.ssh_user.as_deref() {
        Some(user) => format!("{user}@{}", cluster.ssh_host),
        None => cluster.ssh_host.clone(),
    });
    cmd
}

/// Run one remote shell command; returns (exit_code, combined output).
async fn remote_exec(cluster: &ClusterRow, remote_cmd: &str) -> std::io::Result<(i32, String)> {
    let output = ssh(cluster).arg(remote_cmd).output().await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.code().unwrap_or(255), combined))
}

/// Stage a local directory onto the remote host via tar-over-stdio.
async fn stage_directory(
    cluster: &ClusterRow,
    local_dir: &Path,
    remote_dir: &str,
) -> std::io::Result<()> {
    // tar -C local -cf - . | ssh host "mkdir -p remote && tar -C remote -xf -"
    let mut tar = tokio::process::Command::new("tar");
    tar.arg("-C").arg(local_dir).arg("-cf").arg("-").arg(".");
    tar.stdout(Stdio::piped());

    let mut ssh_cmd = ssh(cluster);
    ssh_cmd.arg(format!(
        "mkdir -p '{remote_dir}' && tar -C '{remote_dir}' -xf -"
    ));
    ssh_cmd.stdin(Stdio::piped());
    ssh_cmd.stdout(Stdio::null());
    ssh_cmd.stderr(Stdio::piped());

    let mut tar_child = tar.spawn()?;
    let tar_stdout = tar_child.stdout.take().expect("piped stdout");
    let mut ssh_child = ssh_cmd.spawn()?;
    let mut ssh_stdin = ssh_child.stdin.take().expect("piped stdin");
    tokio::io::copy(&mut tokio::io::BufReader::new(tar_stdout), &mut ssh_stdin).await?;
    drop(ssh_stdin);
    let tar_status = tar_child.wait().await?;
    let ssh_status = ssh_child.wait().await?;
    if !tar_status.success() || !ssh_status.success() {
        return Err(std::io::Error::other("staging failed"));
    }
    Ok(())
}

/// Pull a remote directory back over tar-over-stdio.
async fn pull_directory(
    cluster: &ClusterRow,
    remote_dir: &str,
    local_dir: &Path,
) -> std::io::Result<()> {
    let mut ssh_cmd = ssh(cluster);
    ssh_cmd.arg(format!("tar -C '{remote_dir}' -cf - ."));
    ssh_cmd.stdout(Stdio::piped());

    let mut tar = tokio::process::Command::new("tar");
    tar.arg("-C").arg(local_dir).arg("-xf").arg("-");
    tar.stdin(Stdio::piped());
    tar.stdout(Stdio::null());

    let mut ssh_child = ssh_cmd.spawn()?;
    let ssh_stdout = ssh_child.stdout.take().expect("piped stdout");
    let mut tar_child = tar.spawn()?;
    let mut tar_stdin = tar_child.stdin.take().expect("piped stdin");
    tokio::io::copy(&mut tokio::io::BufReader::new(ssh_stdout), &mut tar_stdin).await?;
    drop(tar_stdin);
    let ssh_status = ssh_child.wait().await?;
    let tar_status = tar_child.wait().await?;
    if !ssh_status.success() || !tar_status.success() {
        return Err(std::io::Error::other("pulling results failed"));
    }
    Ok(())
}

/// Cancel a remote run by killing its wrapper script on the remote host.
pub async fn cancel_remote(cluster: &ClusterRow, run_id: &str) -> std::io::Result<()> {
    let (code, out) = remote_exec(
        cluster,
        &format!("pkill -TERM -f 'run-{run_id}.sh' || true"),
    )
    .await?;
    tracing::info!("remote cancel for {run_id}: exit {code}, {out}");
    Ok(())
}

/// Spawn a remote run: stage → launch → poll → pull → finalize. Mirrors
/// spawn_background_run's DB transitions (running on launch, terminal
/// write + SSE at the end).
pub fn spawn_remote_run(
    run_id: String,
    username: String,
    cluster: ClusterRow,
    local_workdir: PathBuf,
    max_jobs: Option<usize>,
) {
    register_remote(&run_id, &cluster.id);
    tokio::spawn(async move {
        let remote_root = cluster
            .remote_dir
            .clone()
            .unwrap_or_else(|| "oxo-flow-runs".into());
        let remote_dir = format!("{}/runs/{}", remote_root.trim_end_matches('/'), run_id);

        let started = chrono::Utc::now();
        if let Err(e) = sqlx::query(
            "UPDATE runs SET status = 'running', phase = 'executing', started_at = ? WHERE id = ?",
        )
        .bind(started)
        .bind(&run_id)
        .execute(db::pool())
        .await
        {
            tracing::error!("Failed to update remote run {run_id} to running: {e}");
        }
        broadcast_event_for(
            "run_started",
            &serde_json::json!({"run_id": run_id, "username": username, "status": "running", "started_at": started.to_rfc3339()}),
            Some(&username),
        );

        // 1. stage: workflow + local inputs over tar-over-ssh.
        if let Err(e) = stage_directory(&cluster, &local_workdir, &remote_dir).await {
            mark_failed(
                &run_id,
                &format!("staging to {} failed: {e}", cluster.ssh_host),
            )
            .await;
            return;
        }

        // 2. write the per-run wrapper into the remote dir and launch under
        //    nohup. The wrapper name is unique (pkill target).
        let wrapper = format!("run-{run_id}.sh");
        let launch_script = format!(
            "cd '{remote_dir}' && printf '%s\\n' '#!/bin/sh' 'oxo-flow run workflow.oxoflow --workdir .{jobs} > execution.log 2>&1' 'echo $? > .exit-code' > {wrapper} && nohup sh {wrapper} >/dev/null 2>&1 & echo launched",
            jobs = match max_jobs {
                Some(j) => format!(" -j {j}"),
                None => String::new(),
            },
        );
        match remote_exec(&cluster, &launch_script).await {
            Ok((0, out)) => tracing::info!("remote launch for {run_id}: {out}"),
            Ok((code, out)) => {
                mark_failed(&run_id, &format!("remote launch failed ({code}): {out}")).await;
                return;
            }
            Err(e) => {
                mark_failed(&run_id, &format!("remote launch ssh failed: {e}")).await;
                return;
            }
        }

        // 3. poll .exit-code every 5s.
        let poll_cmd = format!("cat '{remote_dir}/.exit-code' 2>/dev/null || echo __RUNNING__");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let cancelled: Option<String> =
                sqlx::query_scalar("SELECT status FROM runs WHERE id = ? AND status = 'cancelled'")
                    .bind(&run_id)
                    .fetch_one(db::pool())
                    .await
                    .ok();
            if cancelled.is_some() {
                unregister_remote(&run_id);
                return;
            }
            match remote_exec(&cluster, &poll_cmd).await {
                Ok((_, out)) if out.trim() == "__RUNNING__" => continue,
                Ok((_, out)) => {
                    let code = out.trim().parse::<i32>().unwrap_or(1);
                    // 4. pull results back so the local file layer serves
                    //    them like any other run.
                    if let Err(e) = pull_directory(&cluster, &remote_dir, &local_workdir).await {
                        tracing::warn!("pulling results for {run_id} failed: {e}");
                    }
                    crate::executor::finalize_run(
                        &run_id,
                        Some(code),
                        &local_workdir.join("execution.log"),
                    )
                    .await;
                    unregister_remote(&run_id);
                    return;
                }
                Err(e) => {
                    mark_failed(&run_id, &format!("remote poll failed: {e}")).await;
                    return;
                }
            }
        }
    });
}

async fn mark_failed(run_id: &str, message: &str) {
    tracing::error!("{message}");
    let end = chrono::Utc::now();
    let _ = sqlx::query(
        "UPDATE runs SET status = 'failed', phase = 'failed', finished_at = ? WHERE id = ?",
    )
    .bind(end)
    .bind(run_id)
    .execute(db::pool())
    .await;
    let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(db::pool())
        .await
        .unwrap_or(None);
    broadcast_event_for(
        "run_failed",
        &serde_json::json!({"run_id": run_id, "status": "failed", "finished_at": end.to_rfc3339(), "error": message}),
        user_id.as_deref(),
    );
    unregister_remote(run_id);
}
