use chrono::Utc;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::workspace::get_run_directory;
use crate::{broadcast_event_for, db};

/// Locate the `oxo-flow` CLI binary.
///
/// Search order:
///   1. `OXO_FLOW_BIN` environment variable (explicit override)
///   2. `CARGO_BIN_EXE_oxo-flow` (set by cargo test when oxo-flow-cli is a dependency)
///   3. Next to the current executable (same target dir)
///   4. One level above the current executable (cargo test places test binaries in `deps/`)
///   5. Fall back to `"oxo-flow"` (PATH lookup)
pub(crate) fn find_oxo_flow_binary() -> PathBuf {
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

/// Extract the CLI dry-run's machine-readable preview from the mixed
/// stdout/stderr log. The banner and human sections interleave with the
/// JSON — scan from the first occurrence of `checkpoint_preview`, back up
/// to the enclosing `{`, and grow the slice until the document parses.
pub fn extract_dry_run_preview(log: &str) -> Option<serde_json::Value> {
    let start = log.find("\"checkpoint_preview\"")?;
    // Back up to the enclosing '{' of the JSON document.
    let object_start = log[..start].rfind('{')?;
    let tail = &log[object_start..];
    // Grow the slice until serde accepts it (handles nested braces).
    let mut end = 1;
    while end <= tail.len() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tail[..end])
            && value.get("checkpoint_preview").is_some()
        {
            return Some(value);
        }
        end += 1;
    }
    None
}

/// Map a subprocess exit status to the run status vocabulary.
fn final_status_from_exit(success: bool) -> &'static str {
    if success { "completed" } else { "failed" }
}

/// Wrapper script the CLI runs inside: `f="$1"; shift; "$@"; rc=$?;
/// printf '%s' "$rc" > "$f"; exit "$rc"`.
///
/// `$0` is a dummy, `$1` the exit-record path, and the remaining positional
/// parameters form the real command line. The record lets startup recovery
/// attribute a run honestly after a web-server crash — the same pattern
/// Nextflow uses (`.exitcode` files). Positional forwarding avoids any
/// shell-quoting of user-controlled arguments.
const EXIT_CODE_WRAPPER_SCRIPT: &str =
    r#"f="$1"; shift; "$@"; rc=$?; printf '%s' "$rc" > "$f"; exit "$rc""#;

/// Execution options carried from the run request to the CLI subprocess.
#[derive(Debug, Clone, Default)]
pub struct RunFlags {
    /// Preview mode: spawns `oxo-flow dry-run` instead of executing anything.
    pub dry_run: bool,
    /// Continue executing independent rules when one fails (`-k`).
    pub keep_going: bool,
    /// Explicitly requested parallelism (`-j`); `None` keeps the CLI default.
    pub max_jobs: Option<usize>,
    /// Sample filters (`--samples` filter semantics) — restrict execution
    /// to these samples. Supports names, `first:N`, and `ready`, matching
    /// the CLI. Unknown names warn; a selection matching nothing fails the
    /// run instead of executing a phantom sample. Empty = all samples.
    pub samples: Vec<String>,
    /// Explicit target rules (`-t <name>` each). Empty = engine default.
    pub targets: Vec<String>,
    /// Re-run only rules that failed in the previous run (`--resume-failed`).
    /// Used by the web retry path (issue #82 P0-3): plain `run` would skip
    /// failed rules as already-attempted and the retry would be a no-op.
    pub resume_failed: bool,
    /// Force execution ignoring up-to-date checks (`--rerun`). Paired with
    /// resume_failed, this makes the retry actually execute the failed set
    /// (their outputs exist, so freshness alone would skip them again).
    pub rerun: bool,
}

/// Build the CLI argument vector for a run (pure — unit-tested).
pub fn build_cli_args(
    workflow_file: &std::path::Path,
    run_dir: &std::path::Path,
    flags: &RunFlags,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if flags.dry_run {
        // dry-run shares --samples/-t semantics with run (issue #79 P2:
        // the web preview dropped them) and emits a machine-readable
        // instance-level preview via --json.
        args.push("dry-run".into());
        args.push("--json".into());
    } else {
        args.push("run".into());
    }
    args.push(workflow_file.as_os_str().to_owned());
    args.push("--workdir".into());
    args.push(run_dir.as_os_str().to_owned());
    if flags.dry_run {
        if !flags.samples.is_empty() {
            // --samples filters to the named subset (the CLI also accepts
            // first:N / ready). --sample would APPEND phantom samples —
            // the issue #79 P1-07 mis-wiring.
            args.push("--samples".into());
            args.push(flags.samples.join(",").into());
        }
        for target in &flags.targets {
            args.push("-t".into());
            args.push(target.into());
        }
    } else {
        if flags.keep_going {
            args.push("--keep-going".into());
        }
        if flags.resume_failed {
            args.push("--resume-failed".into());
        }
        if flags.rerun {
            args.push("--rerun".into());
        }
        if let Some(jobs) = flags.max_jobs {
            args.push("-j".into());
            args.push(jobs.to_string().into());
        }
        if !flags.samples.is_empty() {
            args.push("--samples".into());
            args.push(flags.samples.join(",").into());
        }
        for target in &flags.targets {
            args.push("-t".into());
            args.push(target.into());
        }
    }
    args
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
    // The CLI invocation is derived from RunFlags + the workflow file the
    // caller wrote into the workdir.
    let run_dir = workdir
        .clone()
        .unwrap_or_else(|| get_run_directory(&username, &run_id));
    let args = build_cli_args(&run_dir.join("workflow.oxoflow"), &run_dir, &flags);
    spawn_background_run_with_args(run_id, username, auth_type, os_user, workdir, args);
}

/// Spawn a background CLI run with an explicit argument vector (used by
/// the checkpoint-resume path, issue #81 web exposure).
pub fn spawn_background_run_with_args(
    run_id: String,
    username: String,
    auth_type: String,
    os_user: String,
    workdir: Option<PathBuf>,
    cli_args: Vec<std::ffi::OsString>,
) {
    tokio::spawn(async move {
        info!("Starting background run {} for user {}", run_id, username);

        // Update status to running
        let now = Utc::now();
        if let Err(e) = sqlx::query(
            "UPDATE runs SET status = 'running', phase = 'executing', started_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(&run_id)
        .execute(db::pool())
        .await
        {
            error!("Failed to update run {run_id} to running: {e}");
            return;
        }

        // Broadcast run start event, scoped to the run owner (issue #82
        // P0-5: SSE subscribers only receive their own runs' events).
        broadcast_event_for(
            "run_started",
            &serde_json::json!({
                "run_id": run_id,
                "username": username,
                "status": "running",
                "started_at": now.to_rfc3339(),
            }),
            Some(&username),
        );

        let run_dir = workdir.unwrap_or_else(|| get_run_directory(&username, &run_id));

        // Validate OS username to prevent injection in sudo mode
        let os_user_regex = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$")
            .expect("Static regex pattern should always compile");
        if auth_type == "sudo" && !os_user_regex.is_match(&os_user) {
            error!("Invalid OS username format: {os_user}");
            mark_run_failed(&run_id).await;
            return;
        }

        let oxo_bin = find_oxo_flow_binary();

        // The caller supplies the exact CLI arguments: normal runs build
        // them from RunFlags + the workflow file; checkpoint resumes pass
        // `resume <checkpoint>` directly (issue #81 web exposure).
        let oxo_args = cli_args;

        // Inner command: the CLI itself, optionally through sudo.
        let mut payload: Vec<std::ffi::OsString> = Vec::new();
        if auth_type == "sudo" && os_user != "oxo-flow" {
            payload.push("sudo".into());
            payload.push("-n".into());
            payload.push("-u".into());
            payload.push(os_user.into());
        }
        payload.push(oxo_bin.into_os_string());
        payload.extend(oxo_args);

        // Wrap the payload in `sh -c` so the CLI's exit code is persisted to
        // `<workdir>/.exit-code`. If the web server crashes before reaping
        // the child, startup recovery reads that record instead of blindly
        // marking the run failed (issue #79 P1-02).
        let exit_file = run_dir.join(".exit-code");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.args(payload);

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

                // Real resource telemetry (issue #82 P1-2): sample the
                // CLI's process tree every 5s into workdir/metrics.jsonl so
                // the monitor cards show measured memory/CPU, not defaults.
                if let Some(pid) = child.id() {
                    spawn_resource_sampler(run_id.clone(), pid, run_dir.clone());
                }
                spawn_log_tailer(run_id.clone(), run_dir.clone());

                // Wait for process completion
                match child.wait().await {
                    Ok(status) => {
                        // The exit record was only needed for crash recovery —
                        // this live path reaped the child directly.
                        let _ = tokio::fs::remove_file(&exit_file).await;
                        finalize_run(&run_id, status.code(), &log_file_path).await;
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

/// Persist the terminal state of a finished run and broadcast the SSE event.
///
/// Shared by the live wait path and crash-recovery monitoring so both
/// attribute runs with identical semantics. `exit_code` is the CLI's exit
/// code; `None` means the process was killed without leaving a record and is
/// attributed as failed. A run already marked 'cancelled' is left untouched:
/// the terminal write happened at cancel time and the kill fallout must not
/// overwrite it back to completed/failed.
pub(crate) async fn finalize_run(run_id: &str, exit_code: Option<i32>, log_path: &std::path::Path) {
    crate::process_control::unregister(run_id);
    let success = exit_code == Some(0);
    let final_state = final_status_from_exit(success);

    let cancelled: bool =
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE id = ? AND status = 'cancelled'")
            .bind(run_id)
            .fetch_one(db::pool())
            .await
            .map(|n: i64| n > 0)
            .unwrap_or(false);
    if cancelled {
        info!("Run {run_id} exited after cancel; keeping 'cancelled'");
        return;
    }

    let end = Utc::now();
    if let Err(e) =
        sqlx::query("UPDATE runs SET status = ?, phase = ?, finished_at = ? WHERE id = ?")
            .bind(final_state)
            .bind(final_state)
            .bind(end)
            .bind(run_id)
            .execute(db::pool())
            .await
    {
        error!("Failed to update final status for run {run_id}: {e}");
    }
    // Persist the dry-run preview (instance-level plan) next to the log so
    // /api/runs/{id}/preview can serve it without re-parsing the log.
    if let Ok(log) = std::fs::read_to_string(log_path)
        && let Some(preview) = extract_dry_run_preview(&log)
        && let Ok(json) = serde_json::to_string_pretty(&preview)
        && let Some(dir) = log_path.parent()
    {
        let _ = std::fs::write(dir.join("dry-run-preview.json"), json);
    }
    info!("Run {run_id} finished: {final_state}");

    // Broadcast the terminal event (documented in the SSE API):
    // run_completed on success, run_failed otherwise.
    let event = if success {
        "run_completed"
    } else {
        "run_failed"
    };
    // Surfacing the CLI's invalidation summary (issue #69): config changes,
    // rule-definition edits, and input-set changes that invalidated
    // checkpoint records this run.
    let summary = std::fs::read_to_string(log_path)
        .ok()
        .and_then(|log| extract_invalidation_summary(&log));
    // Scope the terminal event to the run owner (issue #82 P0-5).
    let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(db::pool())
        .await
        .unwrap_or(None);
    broadcast_event_for(
        event,
        &serde_json::json!({
            "run_id": run_id,
            "status": final_state,
            "finished_at": end.to_rfc3339(),
            "summary": summary,
        }),
        user_id.as_deref(),
    );

    // Release the run's quota reservation (issue #82 P1-9).
    if let Some((user_id, threads, memory_mb)) = crate::infra::quota::release(run_id) {
        crate::infra::quota::global_quota_tracker().record_complete(&user_id, threads, memory_mb);
    }

    // Configured webhooks fire on terminal states (issue #82 P1-12).
    crate::domains::observability::webhook::notify_terminal(run_id, final_state).await;
}

/// Monitor a run that was re-attached after a server restart (crash
/// recovery, issue #79 P1-02).
///
/// The web server is no longer the CLI's parent, so completion is detected
/// by polling the `.exit-code` record (written by the wrapper shell the
/// moment the CLI exits — the primary signal) and process liveness (the
/// fallback for a process killed without a record). Finalization goes
/// through [`finalize_run`] so semantics match the live wait path exactly.
pub fn resume_monitoring(run_id: String, pid: i32, workdir: PathBuf) {
    // Crash-recovery re-attach also samples resources and tails the log
    // (issue #82 P1-2 / P1-18).
    if pid > 0 {
        spawn_resource_sampler(run_id.clone(), pid as u32, workdir.clone());
    }
    spawn_log_tailer(run_id.clone(), workdir.clone());
    tokio::spawn(async move {
        let exit_file = workdir.join(".exit-code");
        let log_path = workdir.join("execution.log");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(content) = tokio::fs::read_to_string(&exit_file).await {
                let code = content.trim().parse::<i32>().ok();
                finalize_run(&run_id, code, &log_path).await;
                break;
            }
            if !crate::process_control::probe_alive(pid) {
                // Killed without leaving a record (SIGKILL to the group).
                finalize_run(&run_id, None, &log_path).await;
                break;
            }
        }
    });
}

/// Tail `workdir/execution.log` and broadcast per-rule SSE events
/// (issue #82 P1-18: previously only terminal events existed and the
/// frontend polled to fill the gap). Parses the CLI's stable log lines:
/// `Running: <rule>` / `✓ <rule>` / `✗ rule '<rule>' failed` /
/// `⊝ <rule>` (skipped).
fn spawn_log_tailer(run_id: String, workdir: PathBuf) {
    tokio::spawn(async move {
        let log_path = workdir.join("execution.log");
        let mut offset: u64 = 0;
        let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM runs WHERE id = ?")
            .bind(&run_id)
            .fetch_optional(db::pool())
            .await
            .unwrap_or(None);
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(1500));
        ticker.tick().await; // fire immediately
        loop {
            ticker.tick().await;
            let active: Option<String> = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
                .bind(&run_id)
                .fetch_optional(db::pool())
                .await
                .unwrap_or(None);
            match active.as_deref() {
                Some("running") | Some("paused") | Some("queued") => {}
                _ => break,
            }
            let Ok(meta) = std::fs::metadata(&log_path) else {
                continue;
            };
            let len = meta.len();
            if len < offset {
                offset = 0; // log rotated/truncated
            }
            if len == offset {
                continue;
            }
            let Ok(mut file) = std::fs::File::open(&log_path) else {
                continue;
            };
            use std::io::{Read, Seek, SeekFrom};
            if file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_err() {
                continue;
            }
            offset = len;
            for line in buf.lines() {
                let event = if let Some(rest) = line.strip_prefix("Running: ") {
                    Some(("rule_started", rest.trim().to_string()))
                } else if let Some(rest) = line.strip_prefix('✓') {
                    Some(("rule_completed", rest.trim().to_string()))
                } else if line.starts_with('⊝') {
                    Some((
                        "rule_skipped",
                        line.trim_start_matches('⊝').trim().to_string(),
                    ))
                } else if let Some(rule) = line
                    .strip_prefix("✗ rule '")
                    .and_then(|l| l.split_once("'"))
                    .map(|(r, _)| r)
                {
                    Some(("rule_failed", rule.to_string()))
                } else {
                    None
                };
                if let Some((event_type, rule)) = event {
                    if !rule.is_empty() {
                        broadcast_event_for(
                            event_type,
                            &serde_json::json!({"run_id": run_id, "rule": rule}),
                            user_id.as_deref(),
                        );
                    }
                }
            }
        }
    });
}

/// Sample the run's process-tree memory/CPU every 5 seconds into
/// `workdir/metrics.jsonl` (issue #82 P1-2: real telemetry behind the
/// monitor trend cards, replacing the fabricated defaults). Stops when the
/// run reaches a terminal DB state.
fn spawn_resource_sampler(run_id: String, cli_pid: u32, workdir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.tick().await; // fire immediately, then every 5s
        loop {
            ticker.tick().await;
            let active: Option<String> = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
                .bind(&run_id)
                .fetch_optional(db::pool())
                .await
                .unwrap_or(None);
            match active.as_deref() {
                Some("running") | Some("paused") | Some("queued") => {}
                _ => break,
            }
            if let Some((memory_mb, cpu_pct, processes)) = crate::sys::process_tree_usage(cli_pid) {
                let line = format!(
                    r#"{{"ts":"{}","memory_mb":{:.1},"cpu_pct":{:.1},"processes":{}}}"#,
                    Utc::now().to_rfc3339(),
                    memory_mb,
                    cpu_pct,
                    processes
                );
                append_metrics(&workdir, &line).await;
            }
        }
    });
}

/// Append one sample to `workdir/metrics.jsonl`, keeping at most the last
/// 2000 lines (≈2.7 h at 5 s ticks) so long runs cannot grow the file
/// unboundedly.
async fn append_metrics(workdir: &std::path::Path, line: &str) {
    const MAX_METRICS_LINES: usize = 2000;
    let path = workdir.join("metrics.jsonl");
    let mut content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    content.push_str(line);
    content.push('\n');
    let lines: Vec<&str> = content.lines().collect();
    let keep = if lines.len() > MAX_METRICS_LINES {
        &lines[lines.len() - MAX_METRICS_LINES..]
    } else {
        &lines[..]
    };
    let _ = tokio::fs::write(&path, format!("{}\n", keep.join("\n"))).await;
}

/// Mark a run as failed with the current timestamp.
async fn mark_run_failed(run_id: &str) {
    let end = Utc::now();
    if let Err(e) = sqlx::query(
        "UPDATE runs SET status = 'failed', phase = 'failed', finished_at = ? WHERE id = ?",
    )
    .bind(end)
    .bind(run_id)
    .execute(db::pool())
    .await
    {
        error!("Failed to mark run {run_id} as failed: {e}");
    }

    // Broadcast run failure event, scoped to the run owner (issue #82 P0-5).
    let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(db::pool())
        .await
        .unwrap_or(None);
    broadcast_event_for(
        "run_failed",
        &serde_json::json!({
            "run_id": run_id,
            "status": "failed",
            "finished_at": end.to_rfc3339(),
        }),
        user_id.as_deref(),
    );

    if let Some((user_id, threads, memory_mb)) = crate::infra::quota::release(run_id) {
        crate::infra::quota::global_quota_tracker().record_complete(&user_id, threads, memory_mb);
    }
    crate::domains::observability::webhook::notify_terminal(run_id, "failed").await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wrapper_script_records_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exit_file = dir.path().join(".exit-code");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.arg("/bin/sh").arg("-c").arg("exit 3");
        let status = cmd.status().await.expect("wrapper runs");
        assert_eq!(status.code(), Some(3));
        assert_eq!(
            std::fs::read_to_string(&exit_file).unwrap().trim(),
            "3",
            "exit record must carry the CLI's exit code"
        );
    }

    #[tokio::test]
    async fn wrapper_script_forwards_arguments_positionally() {
        // Metacharacters must survive the wrapper untouched — that is the
        // point of positional ("$@") forwarding.
        let dir = tempfile::tempdir().expect("tempdir");
        let exit_file = dir.path().join(".exit-code");
        let value = "a b'c\"d $HOME; rm -rf /";
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(EXIT_CODE_WRAPPER_SCRIPT);
        cmd.arg("sh");
        cmd.arg(&exit_file);
        cmd.arg("/bin/echo").arg(value);
        let out = cmd.output().await.expect("wrapper runs");
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), value);
        assert_eq!(std::fs::read_to_string(&exit_file).unwrap().trim(), "0");
    }

    #[test]
    fn build_cli_args_includes_samples_and_targets() {
        let flags = RunFlags {
            dry_run: false,
            resume_failed: false,
            rerun: false,
            keep_going: true,
            max_jobs: Some(4),
            samples: vec!["S1".into(), "S2".into()],
            targets: vec!["align".into()],
        };
        let args = build_cli_args(
            std::path::Path::new("wf.oxoflow"),
            std::path::Path::new("dir"),
            &flags,
        );
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "run");
        assert!(strs.contains(&"--keep-going".to_string()));
        assert!(strs.contains(&"--samples".to_string()));
        assert!(
            strs.contains(&"S1,S2".to_string()),
            "samples join into one --samples value: {strs:?}"
        );
        assert!(strs.contains(&"-t".to_string()));
        assert!(strs.contains(&"align".to_string()));
        // dry-run shares --samples/-t (issue #79 P2: the preview dropped
        // them) and asks for the machine-readable instance-level plan,
        // but omits execution-only flags (-j / --keep-going).
        let dry = RunFlags {
            dry_run: true,
            resume_failed: false,
            rerun: false,
            ..flags
        };
        let args = build_cli_args(std::path::Path::new("w"), std::path::Path::new("d"), &dry);
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(strs[0], "dry-run");
        assert!(strs.contains(&"--json".to_string()));
        assert!(strs.contains(&"--samples".to_string()));
        assert!(strs.contains(&"S1,S2".to_string()));
        assert!(strs.contains(&"-t".to_string()));
        assert!(strs.contains(&"align".to_string()));
        assert!(!strs.contains(&"--keep-going".to_string()));
        assert!(!strs.contains(&"-j".to_string()));
    }

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

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn extracts_preview_from_mixed_log() {
        // Banner + human lines interleave with the JSON document (the real
        // execution.log shape).
        let log = "oxo-flow v0.11.0 — banner line\n\
                   DAG: (dry-run) 2 rules would execute\n\
                   {\"checkpoint_preview\":{\"summary\":{\"will_run\":2,\"will_skip\":0},\"plan\":[{\"name\":\"gather_cohort_S1\",\"status\":\"run-never-completed\"},{\"name\":\"gather_cohort_S2\",\"status\":\"run-never-completed\"}]},\"execution_order\":[\"gather_cohort_S1\",\"gather_cohort_S2\"]}\n";
        let preview = extract_dry_run_preview(log).expect("preview must parse");
        assert_eq!(preview["checkpoint_preview"]["summary"]["will_run"], 2);
        let plan = preview["checkpoint_preview"]["plan"].as_array().unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0]["name"], "gather_cohort_S1");
    }

    #[test]
    fn returns_none_for_run_logs_without_preview() {
        let log = "DAG: 2 rules in execution order\nRunning: a\n✓ a (0.1s)\nDone: 2 succeeded\n";
        assert_eq!(extract_dry_run_preview(log), None);
    }

    #[test]
    fn handles_nested_objects_until_parse_succeeds() {
        let log = "{\"checkpoint_preview\":{\"summary\":{\"will_run\":1,\"will_skip\":0,\"protected_outside\":0},\"plan\":[{\"name\":\"a\",\"status\":\"run-never-completed\",\"cascaded_from\":null}],\"cascade_chains\":[]},\"command\":\"dry-run\",\"execution_order\":[\"a\"]}";
        let preview = extract_dry_run_preview(log).expect("nested preview must parse");
        assert_eq!(preview["command"], "dry-run");
    }
}
