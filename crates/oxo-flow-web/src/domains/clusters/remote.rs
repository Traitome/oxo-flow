//! Remote execution backend: the web server runs on host A, the workflow
//! executes on a cluster login node reachable over SSH.
//!
//! Protocol (no rsync required — tar streams over stdio):
//!   1. stage — the caller wrote `workflow.oxoflow` + local inputs into
//!      the run's LOCAL workdir; a tar of that directory is piped through
//!      `ssh … tar -xf -` into `{remote_dir}/runs/{run_id}`.
//!   2. launch — a per-run wrapper script (`run-{run_id}.sh`) runs the
//!      CLI remotely, detached in its own session (`setsid`, falling back
//!      to `nohup`); its exit code lands in `.exit-code` (same convention
//!      the local executor uses) and its process-group id in `.pgid`. The
//!      launch SSH call returns as soon as the wrapper is started — it
//!      never waits for the workflow.
//!   3. poll — the web server polls `.exit-code` over SSH every 5s.
//!   4. pull — on completion the whole remote workdir is tar-pulled back
//!      into the LOCAL workdir, so logs/files/preview/report endpoints
//!      keep working unchanged (they read the local path).
//!
//! Cancellation: the wrapper name is unique per run and its `.pgid` records
//! the whole run's process group, so cancel kills the group — the CLI and
//! every rule subprocess die with the wrapper instead of surviving it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::broadcast_event_for;
use crate::infra::db::models::ClusterRow;

use super::service::{SshIdentity, ssh_base_args, ssh_target_string};

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

/// Seconds between `.exit-code` polls.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Consecutive SSH poll failures tolerated before the run is failed
/// (one transient login-node restart must not kill a healthy run).
const POLL_ERROR_TOLERANCE: u32 = 5;

/// Marker printed by the poll command while `.exit-code` does not exist.
const RUNNING_MARKER: &str = "__RUNNING__";

/// Remote file recording the wrapper's process-group id (cancel target).
const PGID_FILE: &str = ".pgid";

/// Remote file collecting the wrapper's own stdout/stderr — kept separate
/// from the launch call's stdout, which carries only the `launched` marker.
const LAUNCH_LOG: &str = "launch.log";

/// An SSH invocation plus the temp key file (if any) it depends on.
///
/// The identity guard must outlive the spawned process: a key-material temp
/// file is removed on drop, so callers keep this struct alive until the
/// child is reaped.
struct SshCommand {
    cmd: tokio::process::Command,
    _identity: Option<SshIdentity>,
}

/// Build an SSH command for `cluster`. The stored `ssh_key` may be sealed
/// (OXO_FLOW_MASTER_KEY set, #517) and may be a path or key material; both
/// are resolved through [`SshIdentity`].
fn ssh(cluster: &ClusterRow) -> std::io::Result<SshCommand> {
    let mut cmd = tokio::process::Command::new("ssh");
    for arg in ssh_base_args(cluster.ssh_port as u16) {
        cmd.arg(arg);
    }
    let identity = cluster
        .ssh_key
        .as_deref()
        .map(crate::infra::crypto::open)
        .map(|key| SshIdentity::resolve(&key))
        .transpose()?;
    if let Some(identity) = &identity {
        cmd.arg("-i").arg(identity.path());
    }
    cmd.arg(ssh_target_string(
        cluster.ssh_user.as_deref(),
        &cluster.ssh_host,
    ));
    Ok(SshCommand {
        cmd,
        _identity: identity,
    })
}

/// Quote a value for a POSIX shell single-quoted context: `'` → `'\''`.
///
/// remote_dir and run_id are interpolated into remote shell commands; both
/// come from admin config / server-generated UUIDs, but escaping here keeps
/// any legacy or hand-edited value from becoming an injection primitive.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// One remote command result. The two streams are kept apart: exit-record
/// and marker parsing reads `stdout` only, because OpenSSH and login shells
/// write warnings to stderr — merging them made a successful run parse as
/// exit code 1 (the ssh-key-material warning) and report `failed`.
struct RemoteOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

impl RemoteOutput {
    /// Flattened, log-safe rendering (stdout first, stderr appended).
    fn for_log(&self) -> String {
        let mut rendered = self.stdout.trim().to_string();
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            if !rendered.is_empty() {
                rendered.push_str("; ");
            }
            rendered.push_str(stderr);
        }
        rendered
    }
}

/// Run one remote shell command, keeping stdout and stderr separate.
async fn remote_exec(cluster: &ClusterRow, remote_cmd: &str) -> std::io::Result<RemoteOutput> {
    let mut ssh_cmd = ssh(cluster)?;
    let output = ssh_cmd.cmd.arg(remote_cmd).output().await?;
    Ok(RemoteOutput {
        code: output.status.code().unwrap_or(255),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// The remote directory holding one run's staged tree and records.
fn remote_run_dir(cluster: &ClusterRow, run_id: &str) -> String {
    let root = cluster
        .remote_dir
        .clone()
        .unwrap_or_else(|| "oxo-flow-runs".into());
    format!("{}/runs/{}", root.trim_end_matches('/'), run_id)
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

    let mut ssh_cmd = ssh(cluster)?;
    let remote_dir_q = shell_quote(remote_dir);
    ssh_cmd.cmd.arg(format!(
        "mkdir -p {remote_dir_q} && tar -C {remote_dir_q} -xf -"
    ));
    ssh_cmd.cmd.stdin(Stdio::piped());
    ssh_cmd.cmd.stdout(Stdio::null());
    ssh_cmd.cmd.stderr(Stdio::piped());

    let mut tar_child = tar.spawn()?;
    let tar_stdout = tar_child.stdout.take().expect("piped stdout");
    let mut ssh_child = ssh_cmd.cmd.spawn()?;
    let mut ssh_stdin = ssh_child.stdin.take().expect("piped stdin");
    // Drain ssh stderr concurrently (a full pipe would block the child) so
    // a failed staging step reports WHY — an unusable ssh identity used to
    // surface only as "staging failed".
    let mut ssh_stderr = ssh_child.stderr.take().expect("piped stderr");
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut ssh_stderr, &mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });
    tokio::io::copy(&mut tokio::io::BufReader::new(tar_stdout), &mut ssh_stdin).await?;
    drop(ssh_stdin);
    let tar_status = tar_child.wait().await?;
    let ssh_status = ssh_child.wait().await?;
    let ssh_error = stderr_task.await.unwrap_or_default();
    if !tar_status.success() || !ssh_status.success() {
        return Err(std::io::Error::other(format!(
            "staging failed (tar {} / ssh {}): {}",
            tar_status.code().unwrap_or(-1),
            ssh_status.code().unwrap_or(-1),
            ssh_error.trim()
        )));
    }
    Ok(())
}

/// Pull a remote directory back over tar-over-stdio.
async fn pull_directory(
    cluster: &ClusterRow,
    remote_dir: &str,
    local_dir: &Path,
) -> std::io::Result<()> {
    let mut ssh_cmd = ssh(cluster)?;
    ssh_cmd
        .cmd
        .arg(format!("tar -C {} -cf - .", shell_quote(remote_dir)));
    ssh_cmd.cmd.stdout(Stdio::piped());

    let mut tar = tokio::process::Command::new("tar");
    tar.arg("-C").arg(local_dir).arg("-xf").arg("-");
    tar.stdin(Stdio::piped());
    tar.stdout(Stdio::null());

    let mut ssh_child = ssh_cmd.cmd.spawn()?;
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

/// The remote launcher for one run.
///
/// The wrapper records its process-group id (`.pgid`), runs the CLI, and
/// writes the exit code (`.exit-code`). It is started inside a subshell
/// **and** backgrounded there with every stdio stream redirected, so the
/// SSH channel closes immediately: without the inner subshell, `&` would
/// background the whole `cd && printf && …` list, leaving the wrapper in
/// the foreground of that list — ssh then waits for the run to finish
/// (the original defect). `setsid` puts the wrapper in its own session, so
/// the recorded id is a killable process group covering the CLI and every
/// rule subprocess; `nohup` is the fallback where setsid is unavailable.
fn launch_script(remote_dir: &str, wrapper: &str, run_cmd: &str) -> String {
    let remote_dir_q = shell_quote(remote_dir);
    let wrapper_q = shell_quote(wrapper);
    let run_cmd_q = shell_quote(run_cmd);
    // Written verbatim into the wrapper. Only recorded when the launcher
    // actually created a session: without setsid the shell's group is not
    // ours to kill, so cancel falls back to the wrapper-name match.
    let pgid_line =
        shell_quote("if [ -n \"$OXO_FLOW_TRACK_PGID\" ]; then printf '%s\\n' \"$$\" > .pgid; fi");
    let launch_log_q = shell_quote(LAUNCH_LOG);
    format!(
        "cd {remote_dir_q} && \
         printf '%s\\n' '#!/bin/sh' {pgid_line} {run_cmd_q} 'echo $? > .exit-code' > {wrapper_q} && \
         ( if command -v setsid >/dev/null 2>&1; then \
             OXO_FLOW_TRACK_PGID=1 setsid sh {wrapper_q} </dev/null >>{launch_log_q} 2>&1 & \
           else \
             nohup sh {wrapper_q} </dev/null >>{launch_log_q} 2>&1 & \
           fi ) && echo launched"
    )
}

/// Cancel a remote run: kill the wrapper's process group so the CLI and rule
/// subprocesses cannot survive as orphans, then fall back to a wrapper-exact
/// match (covers a cancel that raced the launch, before `.pgid` existed).
///
/// The match is `-x` (exact command line) on purpose: a plain `pkill -f`
/// pattern also matches the `bash -c '<this script>'` process running it —
/// the shell would TERM itself mid-script (the old `pkill -f` path died
/// with exit 255 before finishing).
fn cancel_script(remote_dir: &str, run_id: &str) -> String {
    let remote_dir_q = shell_quote(remote_dir);
    let pgid_file_q = shell_quote(PGID_FILE);
    // The wrapper is started as `sh run-<id>.sh` (relative, after `cd`).
    let wrapper_cmdline_q = shell_quote(&format!("sh run-{run_id}.sh"));
    format!(
        // No `--` before the negative pid: dash's kill builtin rejects it
        // ("Illegal number: -"), so the group kill must use the POSIX
        // negative-pid form directly.
        "cd {remote_dir_q} 2>/dev/null; \
         pgid=$(cat {pgid_file_q} 2>/dev/null); \
         case \"$pgid\" in \
           ''|*[!0-9]*) ;; \
           *) kill -TERM -\"$pgid\" 2>/dev/null; \
              sleep 1; \
              kill -KILL -\"$pgid\" 2>/dev/null ;; \
         esac; \
         pkill -TERM -xf {wrapper_cmdline_q} 2>/dev/null; \
         true"
    )
}

/// Cancel a remote run by killing its process group (or wrapper) remotely.
pub async fn cancel_remote(cluster: &ClusterRow, run_id: &str) -> std::io::Result<()> {
    let remote_dir = remote_run_dir(cluster, run_id);
    let output = remote_exec(cluster, &cancel_script(&remote_dir, run_id)).await?;
    tracing::info!(
        "remote cancel for {run_id}: exit {}, {}",
        output.code,
        output.for_log()
    );
    Ok(())
}

/// Best-effort remote cancel + registry cleanup for terminal paths.
async fn cancel_and_unregister(cluster: &ClusterRow, run_id: &str) {
    if let Err(e) = cancel_remote(cluster, run_id).await {
        tracing::warn!("remote cancel for {run_id} failed: {e}");
    }
    unregister_remote(run_id);
}

/// What one poll of `.exit-code` said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollOutcome {
    Running,
    Exited(i32),
}

/// Interpret the poll command's stdout.
///
/// Parsed line-wise from the end and only from stdout: a login shell may
/// print a banner before the record, and stderr is never part of this
/// stream (see [`RemoteOutput`]). Returns `None` for output that carries
/// neither the running marker nor an exit code, which the poll loop treats
/// as a transient failure rather than a run result.
fn parse_poll_stdout(stdout: &str) -> Option<PollOutcome> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line == RUNNING_MARKER {
            return Some(PollOutcome::Running);
        }
        if let Ok(code) = line.parse::<i32>() {
            return Some(PollOutcome::Exited(code));
        }
    }
    None
}

/// Run-launch parameters staged by `POST /api/runs` and replayed by retry:
/// the local staging directory plus the CLI flag set forwarded to the
/// remote wrapper script.
pub struct RunStaging {
    pub local_workdir: PathBuf,
    pub max_jobs: Option<usize>,
    pub config_overrides: Vec<String>,
    pub allow_protected_drift: bool,
    pub provenance: bool,
}

/// Spawn a remote run: stage → launch → poll → pull → finalize. Mirrors
/// spawn_background_run's DB transitions (running on launch, terminal
/// write + SSE at the end). Callers stage a `RunStaging` bundle rather
/// than passing loose flags (clippy too_many_arguments at 8).
pub fn spawn_remote_run(run_id: String, username: String, cluster: ClusterRow, staged: RunStaging) {
    register_remote(&run_id, &cluster.id);
    tokio::spawn(async move {
        let remote_dir = remote_run_dir(&cluster, &run_id);
        let RunStaging {
            local_workdir,
            max_jobs,
            config_overrides,
            allow_protected_drift,
            provenance,
        } = staged;

        // All bookkeeping below uses the storage backend, which resolves to
        // the SQLite pool on SQLite deployments (see infra::db).
        let Some(backend) = crate::infra::db::global_backend() else {
            tracing::error!("Storage backend unavailable — cannot track remote run {run_id}");
            unregister_remote(&run_id);
            return;
        };
        let backend = &backend;

        // Only a still-queued run may transition to running: a cancel that
        // lands between create and launch must WIN, not be overwritten back
        // to running (audit #659; mirrors the local executor's
        // `AND status = 'queued'` predicate).
        let started = chrono::Utc::now();
        let started_str = started.to_rfc3339();
        match backend.mark_run_running(&run_id, &started_str).await {
            Ok(false) => {
                tracing::info!("remote run {run_id} was cancelled before launch — not starting");
                unregister_remote(&run_id);
                return;
            }
            Err(e) => {
                tracing::error!("Failed to update remote run {run_id} to running: {e}");
            }
            Ok(true) => {}
        }
        // Wire-shape note (#914 review NIT): `data.username` carries the
        // canonical users.id (kept for wire compatibility, mirroring
        // executor.rs); new consumers should read the envelope `user`.
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

        // A cancel landing DURING staging must win too: re-check before the
        // wrapper is written and launched, or the cancel that already
        // returned 200 silently executes anyway (audit #659).
        let cancelled_during_staging = backend.is_run_cancelled(&run_id).await.unwrap_or(false);
        if cancelled_during_staging {
            tracing::info!("remote run {run_id} was cancelled during staging — not launching");
            unregister_remote(&run_id);
            return;
        }

        // 2. write the per-run wrapper into the remote dir and launch it
        //    detached (see launch_script). Config overrides ride
        //    `--arg=KEY=VALUE` in equals form (single-quoted per token by
        //    shell_quote) so the remote run sees the same values the web
        //    caller passed — equals form survives hyphen-leading values, and
        //    the CLI's declared-key gate still applies remotely.
        let wrapper = format!("run-{run_id}.sh");
        let mut run_cmd = format!(
            "oxo-flow run workflow.oxoflow --workdir .{jobs}",
            jobs = match max_jobs {
                Some(j) => format!(" -j {j}"),
                None => String::new(),
            }
        );
        // Issue #935: forward the drift-gate escape hatch so a cluster run
        // behaves like a local one — without it, the API answers 200 queued
        // and the remote CLI gate then refuses the run on drift. The flag is
        // boolean (no value), so it needs no shell quoting.
        if allow_protected_drift {
            run_cmd.push_str(" --allow-protected-drift");
        }
        // Record output checksums remotely too (issue #276): a cluster run
        // with provenance enabled behaves like a local one — `provenance
        // verify` can audit its outputs after the pull. Boolean flag, no
        // shell quoting.
        if provenance {
            run_cmd.push_str(" --provenance");
        }
        for kv in &config_overrides {
            run_cmd.push_str(" --arg=");
            run_cmd.push_str(&shell_quote(kv));
        }
        run_cmd.push_str(" > execution.log 2>&1");
        match remote_exec(&cluster, &launch_script(&remote_dir, &wrapper, &run_cmd)).await {
            Ok(out) if out.code == 0 => {
                tracing::info!("remote launch for {run_id} (detached): {}", out.for_log())
            }
            Ok(out) => {
                mark_failed(
                    &run_id,
                    &format!("remote launch failed ({}): {}", out.code, out.for_log()),
                )
                .await;
                return;
            }
            Err(e) => {
                mark_failed(&run_id, &format!("remote launch ssh failed: {e}")).await;
                return;
            }
        }

        // 3. poll .exit-code every 5s. One transient SSH error (login-node
        // restart, network blip) must not permanently fail the run while
        // the wrapper keeps executing remotely — tolerate a few consecutive
        // errors before giving up (audit #662).
        let mut poll_errors: u32 = 0;
        let poll_cmd = format!(
            "cat {}/.exit-code 2>/dev/null || echo {RUNNING_MARKER}",
            shell_quote(&remote_dir)
        );
        loop {
            // Checked before the first sleep too: a cancel that landed
            // while the launch call was in flight is killed immediately,
            // not after the first poll interval.
            if backend.is_run_cancelled(&run_id).await.unwrap_or(false) {
                cancel_and_unregister(&cluster, &run_id).await;
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;

            let poll_result = remote_exec(&cluster, &poll_cmd).await;
            let outcome = match &poll_result {
                Ok(out) => parse_poll_stdout(&out.stdout),
                Err(_) => None,
            };
            match outcome {
                Some(PollOutcome::Running) => {
                    poll_errors = 0;
                }
                Some(PollOutcome::Exited(code)) => {
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
                None => {
                    poll_errors += 1;
                    let detail = match &poll_result {
                        Ok(out) => format!(
                            "unrecognized poll output {:?} (stderr: {:?})",
                            out.stdout.trim(),
                            out.stderr.trim()
                        ),
                        Err(e) => e.to_string(),
                    };
                    if poll_errors < POLL_ERROR_TOLERANCE {
                        tracing::warn!(
                            "remote poll for {run_id} failed ({poll_errors}/{POLL_ERROR_TOLERANCE} consecutive): {detail}"
                        );
                        continue;
                    }
                    // Give up — but do not orphan the remote workflow: kill
                    // the wrapper before failing the row, so a retry cannot
                    // collide with the still-live first invocation (#662).
                    cancel_and_unregister(&cluster, &run_id).await;
                    mark_failed(
                        &run_id,
                        &format!(
                            "remote poll failed {poll_errors} times in a row (last error: {detail})"
                        ),
                    )
                    .await;
                    return;
                }
            }
        }
    });
}

async fn mark_failed(run_id: &str, message: &str) {
    tracing::error!("{message}");
    // Quota release must happen even without the SQLite pool — it is
    // in-memory state and leaking it pins resources until restart.
    crate::infra::quota::release_and_complete(run_id);
    let end = chrono::Utc::now();
    let end_str = end.to_rfc3339();
    // One backend for the whole helper (mirrors executor::mark_run_failed):
    // the unavailable path releases the reservation above, so a missing
    // pool can't pin resources until restart.
    let Some(backend) = crate::infra::db::global_backend() else {
        tracing::error!(
            "Storage backend unavailable — cannot persist remote-run failure for {run_id}"
        );
        return;
    };
    // Terminal states are final (first terminal write wins at the storage
    // layer): a late launch/poll failure must never overwrite the
    // 'cancelled' state a user's cancel already persisted — nor broadcast a
    // failure the run did not have.
    match backend
        .record_run_terminal(run_id, "failed", "failed", &end_str)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                "remote run {run_id} is already terminal — not overwriting it with: {message}"
            );
            unregister_remote(run_id);
            return;
        }
        Err(e) => tracing::error!("Failed to record remote run failure for {run_id}: {e}"),
    }
    // Remote failure is a terminal path that bypasses finalize_run — the
    // quota reservation was released above before the pool guard.
    let user_id: Option<String> = backend
        .get_run(run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.user_id);
    broadcast_event_for(
        "run_failed",
        &serde_json::json!({"run_id": run_id, "status": "failed", "finished_at": end.to_rfc3339(), "error": message}),
        user_id.as_deref(),
    );
    crate::domains::observability::webhook::notify_terminal(run_id, "failed").await;
    unregister_remote(run_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_script_backgrounds_only_the_wrapper_and_detaches_stdio() {
        // Arrange / Act
        let script = launch_script(
            "oxo-flow-runs/runs/run-1",
            "run-run-1.sh",
            "oxo-flow run workflow.oxoflow --workdir . > execution.log 2>&1",
        );

        // Assert — the wrapper is started in a subshell that redirects all
        // three streams, so the ssh channel is not held open by the run.
        assert!(script.contains("( if command -v setsid"), "got: {script}");
        assert!(
            script.contains(
                "OXO_FLOW_TRACK_PGID=1 setsid sh 'run-run-1.sh' </dev/null >>'launch.log' 2>&1 &"
            ),
            "got: {script}"
        );
        assert!(
            script.contains("nohup sh 'run-run-1.sh' </dev/null >>'launch.log' 2>&1 &"),
            "fallback for hosts without setsid, got: {script}"
        );
        // The background marker must sit INSIDE the subshell; the buggy
        // shape backgrounded the whole `cd && printf && nohup` list and
        // left ssh waiting for the workflow.
        assert!(script.contains(" 2>&1 & fi )"), "got: {script}");
        assert!(!script.contains(">/dev/null 2>&1 & echo launched"));
    }

    /// The generated commands must be syntactically valid POSIX shell —
    /// the quoting in `launch_script`/`cancel_script` is easy to break and
    /// a broken launcher fails every remote run.
    #[test]
    fn generated_scripts_parse_as_posix_shell() {
        // Arrange
        let scripts = [
            launch_script(
                "oxo-flow-runs/runs/run-1",
                "run-run-1.sh",
                "oxo-flow run workflow.oxoflow --workdir . > execution.log 2>&1",
            ),
            cancel_script("oxo-flow-runs/runs/run-1", "run-1"),
        ];

        // Act / Assert
        for script in scripts {
            let status = std::process::Command::new("sh")
                .args(["-n", "-c", &script])
                .status()
                .expect("sh runs");
            assert!(status.success(), "invalid POSIX shell: {script}");
        }
    }

    #[test]
    fn launch_script_records_pgid_and_exit_code_in_wrapper() {
        // Arrange / Act
        let script = launch_script("remote", "run-abc.sh", "oxo-flow run wf");

        // Assert — the wrapper writes both records the poll/cancel paths
        // depend on, and keeps the CLI's log separate from the launch log.
        assert!(script.contains("> .pgid"), "got: {script}");
        assert!(script.contains("'echo $? > .exit-code'"), "got: {script}");
        assert!(script.contains("'oxo-flow run wf'"), "got: {script}");
        assert!(script.contains("echo launched"), "got: {script}");
    }

    #[test]
    fn cancel_script_kills_the_recorded_process_group_with_fallback() {
        // Arrange / Act
        let script = cancel_script("oxo-flow-runs/runs/run-1", "run-1");

        // Assert — numeric-only pgid guard, TERM then KILL escalation, and
        // the wrapper-name fallback when `.pgid` does not exist yet.
        assert!(script.contains("cat '.pgid'"), "got: {script}");
        assert!(script.contains("*[!0-9]*"), "got: {script}");
        assert!(script.contains("kill -TERM -\"$pgid\""), "got: {script}");
        assert!(script.contains("kill -KILL -\"$pgid\""), "got: {script}");
        // dash's kill builtin rejects `--` before a negative pid
        // ("Illegal number: -"), which would silently skip the group kill.
        assert!(!script.contains("-- -"), "got: {script}");
        assert!(
            script.contains("pkill -TERM -xf 'sh run-run-1.sh'"),
            "exact command-line match only — a loose -f pattern would TERM \
             the shell running the cancel script, got: {script}"
        );
    }

    #[test]
    fn poll_parsing_recognizes_marker_and_exit_codes() {
        // Arrange / Act / Assert
        assert_eq!(parse_poll_stdout("0"), Some(PollOutcome::Exited(0)));
        assert_eq!(parse_poll_stdout("137\n"), Some(PollOutcome::Exited(137)));
        assert_eq!(
            parse_poll_stdout("__RUNNING__\n"),
            Some(PollOutcome::Running)
        );
        assert_eq!(parse_poll_stdout(""), None);
        assert_eq!(parse_poll_stdout("garbage\n"), None);
    }

    #[test]
    fn poll_parsing_skips_login_banner_noise() {
        // Arrange — a login shell may print a banner on stdout before the
        // record the poll command emits.
        let banner_then_running = "Welcome to the cluster\nLast login: today\n__RUNNING__\n";

        // Act / Assert
        assert_eq!(
            parse_poll_stdout(banner_then_running),
            Some(PollOutcome::Running)
        );
        assert_eq!(
            parse_poll_stdout("Welcome\n42\n"),
            Some(PollOutcome::Exited(42))
        );
    }

    #[test]
    fn stderr_noise_never_reaches_exit_code_parsing() {
        // Arrange — the exact shape of the ssh-key-material defect: ssh
        // writes a warning to stderr while the exit record on stdout is 0.
        let output = RemoteOutput {
            code: 0,
            stdout: "0".into(),
            stderr: "Warning: Identity file -----BEGIN OPENSSH PRIVATE KEY----- not accessible"
                .into(),
        };

        // Act / Assert
        assert_eq!(
            parse_poll_stdout(&output.stdout),
            Some(PollOutcome::Exited(0)),
            "stderr must never contaminate the exit-code stream"
        );
        assert!(output.for_log().contains("Warning: Identity file"));
    }
}
