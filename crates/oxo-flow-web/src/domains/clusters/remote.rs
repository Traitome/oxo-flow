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

use std::io::Read;
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

/// The `ssh` argument list for `cluster` (options, optional identity, target)
/// plus the identity guard the caller must keep alive until ssh exits.
///
/// One definition serves both the async command builder ([`ssh`]) and the
/// blocking result pull — the two must issue byte-identical ssh invocations.
fn ssh_args(
    cluster: &ClusterRow,
) -> std::io::Result<(Vec<std::ffi::OsString>, Option<SshIdentity>)> {
    let mut args: Vec<std::ffi::OsString> = ssh_base_args(cluster.ssh_port as u16)
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect();
    let identity = cluster
        .ssh_key
        .as_deref()
        .map(crate::infra::crypto::open)
        .map(|key| SshIdentity::resolve(&key))
        .transpose()?;
    if let Some(identity) = &identity {
        args.push(std::ffi::OsString::from("-i"));
        args.push(identity.path().as_os_str().to_os_string());
    }
    args.push(std::ffi::OsString::from(ssh_target_string(
        cluster.ssh_user.as_deref(),
        &cluster.ssh_host,
    )));
    Ok((args, identity))
}

/// Build an SSH command for `cluster`. The stored `ssh_key` may be sealed
/// (OXO_FLOW_MASTER_KEY set, #517) and may be a path or key material; both
/// are resolved through [`SshIdentity`].
fn ssh(cluster: &ClusterRow) -> std::io::Result<SshCommand> {
    let (args, identity) = ssh_args(cluster)?;
    let mut cmd = tokio::process::Command::new("ssh");
    cmd.args(&args);
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

/// Run-specific completion record appended to the result stream by the
/// wrapper AFTER the whole result tree (#1157). Because the shell writes the
/// directory tar first and this segment second onto the same stdout stream, a
/// transfer truncated anywhere inside the tree cannot carry it — the record is
/// the proof that every tarred member arrived.
const RESULT_SENTINEL: &str = ".oxo-flow-pull-complete";

/// Members every completed remote run must pull back (issue #1157, audit
/// CL-04). The wrapper redirects the run into `execution.log`, records the CLI
/// exit code in `.exit-code`, and appends the completion record; a stream
/// missing any of them is a partial transfer and must fail the run, not
/// silently produce a run with no log and no exit record.
const REQUIRED_RESULT_MEMBERS: &[&str] = &["execution.log", ".exit-code", RESULT_SENTINEL];

/// Upper bound on captured ssh stderr. The stream is drained fully (or ssh
/// would block on a full pipe) but only the tail is retained — the same
/// 64 KiB window convention as the engine's rule output tails.
const MAX_CAPTURED_STDERR: usize = 64 * 1024;

/// Drain a stderr stream to EOF, retaining only the last [`MAX_CAPTURED_STDERR`]
/// bytes.
fn drain_stderr_capped<R: Read>(mut reader: R) -> String {
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                tail.extend_from_slice(&chunk[..n]);
                if tail.len() > MAX_CAPTURED_STDERR {
                    let excess = tail.len() - MAX_CAPTURED_STDERR;
                    tail.drain(..excess);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

/// The remote command that streams one run's result tree followed by its
/// completion record.
///
/// The directory tar and the sentinel tar are sequential writes to the same
/// stdout, so the sentinel can only arrive if the directory segment did. The
/// sentinel lives in a temp dir outside the result tree (never archived by
/// the first tar) and carries the run id, so a stale record left by an
/// earlier pull of the same persistent workdir cannot satisfy this run.
fn pull_command(remote_dir: &str, run_id: &str) -> String {
    let remote_dir_q = shell_quote(remote_dir);
    let run_id_q = shell_quote(run_id);
    let sentinel_q = shell_quote(RESULT_SENTINEL);
    format!(
        "d=$(mktemp -d) || exit 1; \
         printf '%s' {run_id_q} > \"$d\"/{sentinel_q} || exit 1; \
         tar -C {remote_dir_q} -cf - . ; rc=$?; \
         tar -C \"$d\" -cf - {sentinel_q} ; rm -rf \"$d\" ; exit $rc"
    )
}

/// Extract one pulled result stream into the local workdir through the shared
/// validated extractor: absolute/`..` names and escaping symlink or hard-link
/// targets are rejected by name, every member is verified on disk, and the
/// run records (including the run-id sentinel) must be present. The sentinel's
/// content is checked against `run_id` and the file removed afterwards.
fn extract_pulled_results<R: Read>(
    reader: R,
    local_dir: &Path,
    run_id: &str,
) -> std::io::Result<()> {
    oxo_flow_core::archive::extract_tar_requiring(reader, local_dir, REQUIRED_RESULT_MEMBERS)?;

    let sentinel_path = local_dir.join(RESULT_SENTINEL);
    let recorded = std::fs::read(&sentinel_path).map_err(|e| {
        std::io::Error::other(format!(
            "result stream completion record '{}' is unreadable: {e}",
            sentinel_path.display()
        ))
    })?;
    if recorded != run_id.as_bytes() {
        return Err(std::io::Error::other(format!(
            "result stream completion record does not belong to run {run_id} — \
             refusing a stale or forged pull"
        )));
    }
    // Bookkeeping only; the extracted tree is what the run serves.
    let _ = std::fs::remove_file(&sentinel_path);
    Ok(())
}

/// Pull a remote directory back over tar-over-stdio.
///
/// The tar stream is parsed and extracted by the shared pure-Rust extractor
/// (issue #1157): the previous implementation piped it into the host `tar`,
/// whose path-traversal handling is platform-dependent and unproven, and which
/// reports success for an archive that simply omits members. The trailing
/// run-id completion record makes a truncated stream detectable (#1189
/// review).
async fn pull_directory(
    cluster: &ClusterRow,
    remote_dir: &str,
    local_dir: &Path,
    run_id: &str,
) -> std::io::Result<()> {
    let (args, identity) = ssh_args(cluster)?;
    let remote_cmd = pull_command(remote_dir, run_id);
    let local_dir = local_dir.to_path_buf();
    let run_id = run_id.to_string();

    // The extraction is a blocking stream parse; run the whole ssh+extract in
    // a blocking task so stdout is consumed synchronously while ssh writes.
    tokio::task::spawn_blocking(move || {
        // Keep key material on disk until ssh has been reaped.
        let _identity = identity;
        let mut cmd = std::process::Command::new("ssh");
        cmd.args(&args)
            .arg(remote_cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        // Drain stderr concurrently: a full stderr pipe would otherwise block
        // ssh while the extractor waits on stdout. Only a bounded tail is
        // retained (a verbose or hostile remote cannot exhaust memory).
        let stderr_task = std::thread::spawn(move || drain_stderr_capped(stderr));

        let extracted = extract_pulled_results(stdout, &local_dir, &run_id);
        if extracted.is_err() {
            // Stop the transfer promptly; dropping stdout above already
            // closes the read end, but killing is deterministic.
            let _ = child.kill();
        }
        let status = child.wait()?;
        let ssh_stderr = stderr_task.join().unwrap_or_default();
        match extracted {
            Ok(()) if status.success() => Ok(()),
            Ok(()) => Err(std::io::Error::other(format!(
                "pulling results failed (ssh exit {}): {}",
                status.code().unwrap_or(-1),
                ssh_stderr.trim()
            ))),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| std::io::Error::other(format!("result pull task failed: {e}")))?
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

/// Terminal handling for a remote run whose `.exit-code` is known and whose
/// result pull has been attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteFinalize {
    /// Artifacts landed locally in full: hand the recorded exit code to the
    /// shared finalizer (which reports `completed` iff the code is 0).
    Finalize,
    /// The result pull failed, so the local workdir — the source for the
    /// run's logs, preview and report — is missing or stale. The run must
    /// NOT be finalized as success; the carried text is both the failure
    /// reason and the persisted artifact-incomplete note.
    FailArtifactsIncomplete(String),
}

/// Decide how a just-finished remote run reaches its terminal state.
///
/// Pure and unit-tested on purpose: audit RB-05 is the invariant that a
/// failed result pull must never let a run be reported as completed — the
/// local workdir is what every log/report endpoint reads, so a broken
/// transfer means the run's artifacts are incomplete and `status` alone
/// would lie. Kept separate from the SSH plumbing so the decision can be
/// pinned without standing up a fake cluster.
///
/// A clean pull yields [`RemoteFinalize::Finalize`] regardless of the exit
/// code: the shared finalizer (`final_status_from_exit`) is the single place
/// that maps code → `completed`/`failed`, so a non-zero remote exit still
/// stays a failure.
fn decide_remote_finalize(
    run_id: &str,
    exit_code: i32,
    pull_result: &std::io::Result<()>,
) -> RemoteFinalize {
    match pull_result {
        Ok(()) => RemoteFinalize::Finalize,
        Err(e) => RemoteFinalize::FailArtifactsIncomplete(format!(
            "pulling results for {run_id} failed (remote exit code {exit_code}): {e}"
        )),
    }
}

/// Run-launch parameters staged by `POST /api/runs`: the local staging
/// directory plus the CLI flag set forwarded to the remote wrapper script.
/// Retry is local-only and never builds one of these (see the retry handler's
/// create-only flag contract).
pub struct RunStaging {
    pub local_workdir: PathBuf,
    pub max_jobs: Option<usize>,
    pub config_overrides: Vec<String>,
    pub allow_protected_drift: bool,
    pub provenance: bool,
    /// Continue past a failed required rule (`--keep-going`). Create-only:
    /// retry never stages a remote run, so there is no replay path here.
    pub keep_going: bool,
}

/// Build the remote `oxo-flow run …` command line embedded in the wrapper
/// script. Pure and unit-tested so the remote flag set can never drift from
/// the local executor's (`RunFlags` -> `build_cli_args`): a missing flag
/// would silently change cluster-run semantics relative to a local run.
fn build_remote_run_cmd(
    max_jobs: Option<usize>,
    keep_going: bool,
    allow_protected_drift: bool,
    provenance: bool,
    config_overrides: &[String],
) -> String {
    let mut run_cmd = format!(
        "oxo-flow run workflow.oxoflow --workdir .{jobs}",
        jobs = match max_jobs {
            Some(j) => format!(" -j {j}"),
            None => String::new(),
        }
    );
    // Issue #1093 follow-up: forward the failure policy so a cluster run
    // behaves like a local one. Boolean flag (no value) — no shell quoting.
    if keep_going {
        run_cmd.push_str(" --keep-going");
    }
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
    for kv in config_overrides {
        run_cmd.push_str(" --arg=");
        run_cmd.push_str(&shell_quote(kv));
    }
    run_cmd.push_str(" > execution.log 2>&1");
    run_cmd
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
            keep_going,
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
        let run_cmd = build_remote_run_cmd(
            max_jobs,
            keep_going,
            allow_protected_drift,
            provenance,
            &config_overrides,
        );
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
                    let pull_result =
                        pull_directory(&cluster, &remote_dir, &local_workdir, &run_id).await;
                    match decide_remote_finalize(&run_id, code, &pull_result) {
                        RemoteFinalize::Finalize => {
                            crate::executor::finalize_run(
                                &run_id,
                                Some(code),
                                &local_workdir.join("execution.log"),
                            )
                            .await;
                            unregister_remote(&run_id);
                        }
                        RemoteFinalize::FailArtifactsIncomplete(reason) => {
                            // Persist the reason first (issue #1112 / audit
                            // RB-05), then fail the run through the shared
                            // non-success path. The note is written BEFORE
                            // the terminal transition on purpose:
                            // `record_artifacts_incomplete` does not touch
                            // `status`, so ordering here only matters if the
                            // terminal write short-circuits (row already
                            // cancelled) — in which case the persisted
                            // reason is exactly what explains the missing
                            // artifacts.
                            if let Some(b) = crate::infra::db::global_backend()
                                && let Err(e) =
                                    b.record_artifacts_incomplete(&run_id, &reason).await
                            {
                                tracing::error!(
                                    "failed to record incomplete-artifacts note for {run_id}: {e}"
                                );
                            }
                            mark_failed(&run_id, &reason).await;
                        }
                    }
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

    /// Issue #1093 follow-up: the remote wrapper must forward
    /// `--keep-going` exactly when the create request asked for it — parity
    /// with the local executor's `build_cli_args`.
    #[test]
    fn remote_run_cmd_carries_keep_going_only_when_enabled() {
        // Arrange / Act
        let abort = build_remote_run_cmd(Some(4), false, false, false, &[]);
        let keep = build_remote_run_cmd(Some(4), true, false, false, &[]);

        // Assert
        assert!(!abort.contains("--keep-going"), "got: {abort}");
        assert!(keep.contains("--keep-going"), "got: {keep}");
        assert!(
            keep.starts_with("oxo-flow run workflow.oxoflow --workdir ."),
            "got: {keep}"
        );
        assert!(keep.contains(" -j 4"), "got: {keep}");
        assert!(keep.ends_with(" > execution.log 2>&1"), "got: {keep}");
    }

    #[test]
    fn remote_run_cmd_carries_every_boolean_execution_flag() {
        // Arrange / Act — the full staged set arrives in one command.
        let cmd = build_remote_run_cmd(None, true, true, true, &["k=v".into(), "b=c d".into()]);

        // Assert — flags and single-quoted overrides, no `-j` when unset.
        assert!(!cmd.contains(" -j "), "got: {cmd}");
        assert!(cmd.contains(" --keep-going"), "got: {cmd}");
        assert!(cmd.contains(" --allow-protected-drift"), "got: {cmd}");
        assert!(cmd.contains(" --provenance"), "got: {cmd}");
        assert!(cmd.contains(" --arg='k=v'"), "got: {cmd}");
        assert!(cmd.contains(" --arg='b=c d'"), "got: {cmd}");
        assert!(cmd.ends_with(" > execution.log 2>&1"), "got: {cmd}");
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

    /// Audit RB-05 regression: a failed result pull must NOT finalize the
    /// run — the local workdir feeds every log/preview/report endpoint, so
    /// the run's artifacts are incomplete and must be reported as such.
    #[test]
    fn failed_pull_never_finalizes_as_success() {
        // Arrange
        let err = std::io::Error::other("pulling results failed");

        // Act
        let decision = decide_remote_finalize("run-1", 0, &Err(err));

        // Assert — even a zero remote exit code cannot be finalized, and the
        // pull error text is surfaced in the reason.
        match decision {
            RemoteFinalize::FailArtifactsIncomplete(reason) => {
                assert!(reason.contains("run-1"), "got: {reason}");
                assert!(reason.contains("pulling results"), "got: {reason}");
                assert!(reason.contains("pulling results failed"), "got: {reason}");
            }
            RemoteFinalize::Finalize => panic!("a failed pull must not be finalized"),
        }
    }

    /// The positive path is untouched: a clean pull hands the exit code to
    /// the shared finalizer, which owns the success/failure mapping.
    #[test]
    fn clean_pull_finalizes_with_the_recorded_exit_code() {
        assert_eq!(
            decide_remote_finalize("run-1", 0, &Ok(())),
            RemoteFinalize::Finalize
        );
    }

    /// Reuse-not-retype: a non-zero remote exit code stays a failure. The
    /// pull decision still Finalizes (the transfer worked), and the shared
    /// finalizer maps the code to `failed` — pinned here against that
    /// single definition, so a code cannot diverge between the local and
    /// remote paths.
    #[test]
    fn nonzero_remote_exit_stays_a_failure_after_a_clean_pull() {
        // Arrange / Act
        let decision = decide_remote_finalize("run-1", 7, &Ok(()));

        // Assert
        assert_eq!(decision, RemoteFinalize::Finalize);
        assert_eq!(crate::executor::final_status_from_exit(7 == 0), "failed");
    }

    /// Minimal raw-tar writer (the tar crate's `Builder` refuses `..` and
    /// absolute paths, so hostile archives must be built byte-for-byte).
    struct RawTar {
        bytes: Vec<u8>,
    }

    impl RawTar {
        fn new() -> Self {
            Self { bytes: Vec::new() }
        }

        fn add(&mut self, name: &str, typeflag: u8, linkname: &str, data: &[u8]) -> &mut Self {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[100..107].copy_from_slice(b"0000644");
            header[108..115].copy_from_slice(b"0000000");
            header[116..123].copy_from_slice(b"0000000");
            header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            header[136..147].copy_from_slice(b"00000000000");
            header[148..156].copy_from_slice(b"        ");
            header[156] = typeflag;
            if !linkname.is_empty() {
                header[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
            }
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
            header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
            self.bytes.extend_from_slice(&header);
            self.bytes.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            self.bytes.extend(std::iter::repeat_n(0u8, pad));
            self
        }

        fn finish(&mut self) -> Vec<u8> {
            self.bytes.extend(std::iter::repeat_n(0u8, 1024));
            std::mem::take(&mut self.bytes)
        }

        /// Member bytes with no zero terminator — mirrors a stream that stops
        /// exactly on a member boundary (the transfer-truncation shape).
        fn finish_unterminated(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.bytes)
        }
    }

    const TEST_RUN_ID: &str = "run-1";

    /// A result stream shaped like the remote wrapper's: the result tree,
    /// then a second tar segment carrying the run-id completion record.
    fn result_stream(include_output: bool, sentinel: Option<&str>) -> Vec<u8> {
        let mut tree = RawTar::new();
        tree.add("./execution.log", b'0', "", b"run log");
        tree.add("./.exit-code", b'0', "", b"0\n");
        if include_output {
            tree.add("./outputs/out.txt", b'0', "", b"result");
        }
        let mut bytes = tree.finish();
        if let Some(token) = sentinel {
            let mut tail = RawTar::new();
            tail.add("./.oxo-flow-pull-complete", b'0', "", token.as_bytes());
            bytes.extend_from_slice(&tail.finish());
        }
        bytes
    }

    /// Issue #1157 / audit CL-04: the result extraction must fail when the
    /// pull stream is missing a member every completed run writes — the old
    /// external-`tar` pull exited 0 and finalized the run anyway.
    #[test]
    fn result_extraction_fails_when_required_members_are_missing() {
        // Arrange — only one of the records a completed wrapper writes.
        let mut tar = RawTar::new();
        tar.add("./execution.log", b'0', "", b"run log");
        let bytes = tar.finish();
        let dir = tempfile::tempdir().unwrap();

        // Act
        let err = extract_pulled_results(bytes.as_slice(), dir.path(), TEST_RUN_ID)
            .expect_err("a missing required member must fail the pull");

        // Assert — the missing record is named, the run cannot finalize.
        assert!(err.to_string().contains(".exit-code"), "got: {err}");
        assert!(
            err.to_string().contains(RESULT_SENTINEL),
            "the absent completion record must be named: {err}"
        );
    }

    /// Review probe (#1189): a stream carrying only the two bookkeeping
    /// records — no declared output, no completion record — used to extract
    /// cleanly and finalize the run as `completed` with `outputs/` absent.
    /// The completion record is written only after the whole tree, so a
    /// stream that stops before the output cannot carry it.
    #[test]
    fn result_extraction_fails_when_a_declared_output_is_missing() {
        // Arrange — exactly the probe: those two members and no terminator.
        let mut tree = RawTar::new();
        tree.add("./execution.log", b'0', "", b"run log");
        tree.add("./.exit-code", b'0', "", b"0\n");
        let bytes = tree.finish_unterminated();
        let dir = tempfile::tempdir().unwrap();

        // Act
        let pulled = extract_pulled_results(bytes.as_slice(), dir.path(), TEST_RUN_ID);

        // Assert — the pull fails, the declared output is absent, and the
        // existing decision machinery cannot finalize the run as completed.
        let err = pulled.expect_err("a truncated stream must fail the pull");
        assert!(err.to_string().contains(RESULT_SENTINEL), "got: {err}");
        assert!(
            !dir.path().join("outputs/out.txt").exists(),
            "the declared output is absent from the extracted tree"
        );
        match decide_remote_finalize(TEST_RUN_ID, 0, &Err(err)) {
            RemoteFinalize::FailArtifactsIncomplete(reason) => {
                assert!(reason.contains(RESULT_SENTINEL), "got: {reason}");
            }
            RemoteFinalize::Finalize => panic!("a truncated pull must not finalize"),
        }
    }

    /// A stale completion record (left by an earlier pull into the same
    /// persistent workdir, or forged) must not satisfy this run.
    #[test]
    fn result_extraction_rejects_a_stale_completion_record() {
        let bytes = result_stream(true, Some("some-other-run"));
        let dir = tempfile::tempdir().unwrap();

        let err = extract_pulled_results(bytes.as_slice(), dir.path(), TEST_RUN_ID)
            .expect_err("a completion record for another run must be rejected");
        assert!(
            err.to_string().contains("does not belong to run run-1"),
            "got: {err}"
        );
    }

    /// A clean stream with both records and the run-id completion record
    /// extracts and is accepted by the existing finalize decision; the
    /// bookkeeping sentinel is removed from the served tree.
    #[test]
    fn clean_result_archive_extracts_and_finalizes() {
        let bytes = result_stream(true, Some(TEST_RUN_ID));
        let dir = tempfile::tempdir().unwrap();

        let pulled = extract_pulled_results(bytes.as_slice(), dir.path(), TEST_RUN_ID);

        assert!(pulled.is_ok(), "clean pull failed: {pulled:?}");
        assert!(dir.path().join("execution.log").is_file());
        assert!(dir.path().join(".exit-code").is_file());
        assert!(dir.path().join("outputs/out.txt").is_file());
        assert!(
            !dir.path().join(RESULT_SENTINEL).exists(),
            "the completion record is bookkeeping, not a served artifact"
        );
        assert_eq!(
            decide_remote_finalize(TEST_RUN_ID, 0, &pulled),
            RemoteFinalize::Finalize
        );
    }

    /// A hostile stream is rejected by name and never writes outside the
    /// local workdir — the check replaced the platform-dependent host `tar`.
    #[test]
    fn result_extraction_rejects_parent_dir_members() {
        // Arrange
        let mut tar = RawTar::new();
        tar.add("./execution.log", b'0', "", b"run log");
        tar.add("./.exit-code", b'0', "", b"0\n");
        tar.add("../escape.txt", b'0', "", b"pwned");
        let bytes = tar.finish();
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("workdir");
        std::fs::create_dir_all(&dir).unwrap();

        // Act
        let err = extract_pulled_results(bytes.as_slice(), &dir, TEST_RUN_ID)
            .expect_err("a '..' member must be rejected");

        // Assert
        assert!(err.to_string().contains("../escape.txt"), "got: {err}");
        assert!(!parent.path().join("escape.txt").exists());
    }

    /// The pull command must emit the completion record strictly after the
    /// result-tree tar on the same stream, and stay valid POSIX shell.
    #[test]
    fn pull_command_appends_the_completion_record_after_the_tree() {
        let script = pull_command("oxo-flow-runs/runs/run-1", "run-1");

        let tree_at = script
            .find("tar -C 'oxo-flow-runs/runs/run-1' -cf - .")
            .expect("tree tar must be present");
        let sentinel_at = script
            .find("tar -C \"$d\" -cf - '.oxo-flow-pull-complete'")
            .expect("sentinel tar must be present");
        assert!(
            tree_at < sentinel_at,
            "the completion record must be tarred after the tree: {script}"
        );
        assert!(script.contains("printf '%s' 'run-1'"), "got: {script}");
        assert!(script.contains("exit $rc"), "the tree tar status must win");
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", &script])
            .status()
            .expect("sh runs");
        assert!(status.success(), "invalid POSIX shell: {script}");
    }

    /// Review finding (#1189): stderr capture must stay bounded while the
    /// pipe is still fully drained.
    #[test]
    fn stderr_capture_keeps_only_a_bounded_tail() {
        // Arrange — 4× the cap, ends with a recognizable marker.
        let mut payload: Vec<u8> = vec![b'x'; MAX_CAPTURED_STDERR * 4];
        payload.extend_from_slice(b"TAIL-MARKER");
        let expected_tail = payload[payload.len() - MAX_CAPTURED_STDERR..].to_vec();

        // Act
        let captured = drain_stderr_capped(std::io::Cursor::new(payload));

        // Assert — bounded, and the retained window is the tail.
        assert_eq!(captured.len(), MAX_CAPTURED_STDERR);
        assert_eq!(captured.as_bytes(), expected_tail.as_slice());
        assert!(captured.ends_with("TAIL-MARKER"));
        // A small stream passes through untouched.
        assert_eq!(drain_stderr_capped(&b"short"[..]), "short");
    }
}
