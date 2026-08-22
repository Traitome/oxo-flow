//! Real process control: cancel/pause/resume must signal the CLI subprocess.
//!
//! The executor's CLI lookup honors OXO_FLOW_BIN first, but most of these
//! tests bypass spawning entirely: they register a `sleep` child's pgid
//! under a run id the same way the executor does, then drive the HTTP
//! handlers. Two tests exercise the full executor spawn path (the EAGAIN
//! retry contract) and one runs the real CLI binary (the orphaned-group
//! cancel window) — those need `cargo build --workspace` first.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxo_flow_web::{executor, process_control, server};
use serde_json::json;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tower::ServiceExt;

mod common;

/// Spawn a `sleep` child in its own process group (like the executor does)
/// and register it under `run_id`.
fn spawn_and_register(run_id: &str) -> Child {
    let child = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    process_control::register(run_id, child.id() as i32);
    child
}

async fn insert_run_row(run_id: &str, status: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query(
        "INSERT INTO runs (id, user_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, created_at)
         VALUES (?, 'default', '', 'control-test', ?, 'executing', NULL, '', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(run_id)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

/// Locate a workspace binary from the target directory. The CLI is not a
/// dependency of the web crate, so `CARGO_BIN_EXE_oxo-flow` is unset for
/// these tests; the sibling-path lookup mirrors tests/web_integration.rs.
/// Requires `cargo build --workspace` first (CI does this).
fn workspace_bin(name: &str) -> std::path::PathBuf {
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

/// Serializes the `OXO_FLOW_TEST_SPAWN_FAIL`-dependent tests: env-var
/// mutation is process-global and integration tests run on parallel
/// threads. No other test reads the variable.
static SPAWN_FAIL_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard: set the executor's spawn-failure test seam, restore on drop.
struct SpawnFailEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for SpawnFailEnv {
    fn drop(&mut self) {
        // Soundness: SPAWN_FAIL_ENV serializes every writer and reader of
        // this variable inside the test binary.
        unsafe {
            std::env::remove_var("OXO_FLOW_TEST_SPAWN_FAIL");
        }
    }
}

fn set_spawn_fail(n: u32) -> SpawnFailEnv {
    // Poison recovery: a failing test unwinds while holding the guard, and
    // the next test must still be able to take the lock.
    let guard = SPAWN_FAIL_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Soundness: SPAWN_FAIL_ENV serializes every writer and reader of this
    // variable inside the test binary.
    unsafe {
        std::env::set_var("OXO_FLOW_TEST_SPAWN_FAIL", n.to_string());
    }
    SpawnFailEnv { _guard: guard }
}

/// True while any live process command line contains `needle` (used to
/// prove a late-spawned CLI did not survive a cancel).
fn process_alive_with(needle: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-e", "-o", "args="])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.contains(needle))
        })
        .unwrap_or(false)
}

async fn run_status(run_id: &str) -> Option<String> {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

#[tokio::test]
async fn cancel_signals_the_process_group() {
    common::ensure_db().await;
    insert_run_row("pc-cancel", "running").await;
    let mut child = spawn_and_register("pc-cancel");

    let (status, body) = post_json("/api/runs/pc-cancel/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body:?}");
    assert_eq!(body["status"], "cancelled");

    // SIGTERM kills `sleep` almost immediately; the grace window must not
    // keep the request hanging for the full 5 s. Poll the child.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        if child.try_wait().expect("try_wait").is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(exited, "cancelled child process must actually die");
    // Reap the child so no zombie outlives the test.
    let _ = child.wait();

    assert_eq!(run_status("pc-cancel").await.as_deref(), Some("cancelled"));
    assert!(process_control::pgid("pc-cancel").is_none());
}

/// Regression (issue #120): cancel must not rely on the in-memory registry
/// alone. finalize_run unregisters the moment the CLI wrapper is reaped, so
/// a cancel landing after that point must still find the group via the pid
/// recorded in the runs table and actually kill it.
#[tokio::test]
async fn cancel_falls_back_to_db_pid_when_registry_is_empty() {
    common::ensure_db().await;
    insert_run_row("pc-fallback", "running").await;
    let mut child = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    let pid = child.id() as i64;
    // Record the pid like the executor does, but never register in-memory —
    // the post-finalize window this test reproduces.
    sqlx::query("UPDATE runs SET pid = ? WHERE id = ?")
        .bind(pid)
        .bind("pc-fallback")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    assert!(process_control::pgid("pc-fallback").is_none());

    // looks_like_oxo_flow guards pid reuse — a bare `sleep` fails the probe,
    // so cancel must report the nothing-to-signal path, not kill a stranger.
    let (status, _) = post_json("/api/runs/pc-fallback/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "a process failing the identity probe must not be signalled"
    );
    process_control::signal_group(child.id() as i32, process_control::SIGKILL).expect("cleanup");
    child.wait().expect("wait after cleanup");
}

/// Regression (issue #136 tier-2): the DB-pid fallback must signal the
/// process GROUP, not just probe the recorded wrapper pid.
///
/// The window: finalize_run unregisters the moment the CLI wrapper leader
/// is reaped, while orphaned group members (the CLI itself and the running
/// rule) keep executing under the same pgid. A cancel landing then finds
/// no registry entry; the recorded pid probes dead (the leader WAS reaped)
/// — but `killpg(pgid, 0)` still answers. The fallback must probe group
/// liveness and, guarded by the identity check against the group's
/// surviving members, signal the group.
///
/// Simulation gap: the real trigger (OOM/load killing the wrapper or the
/// unregister-before-status-update race) is not reproducible on demand, so
/// the test kills the wrapper leader directly after the rule starts — the
/// resulting state (leader reaped, group alive, registry empty) is
/// byte-for-byte the window the fix targets.
#[tokio::test]
async fn cancel_kills_orphaned_group_when_wrapper_leader_is_gone() {
    common::ensure_db().await;
    insert_run_row("pc-orphan", "running").await;

    // Real CLI running a long rule, wrapped exactly as the executor wraps
    // it (`sh -c <exit-record script> sh <exitfile> <binary> run …`).
    let dir = tempfile::tempdir().expect("tempdir");
    let workdir = dir.path();
    let workflow = workdir.join("workflow.oxoflow");
    std::fs::write(
        &workflow,
        "[workflow]\nname = \"sleepy\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"sleepy\"\nshell = \"sleep 30\"\n",
    )
    .expect("write workflow");

    let bin = workspace_bin("oxo-flow");
    let exit_file = workdir.join(".exit-code");
    let log_file = std::fs::File::create(workdir.join("execution.log")).expect("create log");
    let mut wrapper = Command::new("sh")
        .arg("-c")
        .arg(executor::EXIT_CODE_WRAPPER_SCRIPT)
        .arg("sh")
        .arg(&exit_file)
        .arg(&bin)
        .arg("run")
        .arg(&workflow)
        .arg("--workdir")
        .arg(workdir)
        .process_group(0)
        .stdout(Stdio::from(log_file.try_clone().expect("clone log")))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("spawn wrapper");
    let wrapper_pid = wrapper.id() as i32;
    // Record the wrapper pid like the executor does — but never register
    // in-memory: the finalize_run window this reproduces has already run
    // unregister (the wrapper was reaped by the executor).
    sqlx::query("UPDATE runs SET pid = ? WHERE id = ?")
        .bind(wrapper_pid as i64)
        .bind("pc-orphan")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    assert!(process_control::pgid("pc-orphan").is_none());

    // Wait for the CLI to start the rule (execution.log gains the
    // "Running:" progress line).
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut started = false;
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(workdir.join("execution.log"))
            .map(|log| log.contains("Running:"))
            .unwrap_or(false)
        {
            started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(started, "CLI must start the rule before the crash window");

    // Simulate the crash window: the wrapper LEADER dies while its group —
    // the CLI and the running rule — keeps executing as orphans. kill()
    // targets a single process; the group survives.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(wrapper_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill wrapper leader");
    wrapper.wait().expect("reap wrapper");
    assert!(
        process_control::group_alive(wrapper_pid),
        "the orphaned group must outlive its wrapper leader"
    );

    // Cancel: the registry is empty and the recorded pid probes dead —
    // only the group-liveness + group-identity fallback can find the run.
    let (status, body) = post_json("/api/runs/pc-orphan/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body:?}");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process_control::group_alive(wrapper_pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !process_control::group_alive(wrapper_pid),
        "the orphaned group (CLI + rule) must be dead after cancel"
    );
    assert_eq!(run_status("pc-orphan").await.as_deref(), Some("cancelled"));
}

/// Regression (issue #136 tier-2): the grace→SIGKILL escalation must
/// actually run. A rule that traps (ignores) SIGTERM cannot be stopped by
/// the graceful signal; only the SIGKILL escalation kills its group, so the
/// group's death within the grace+verify window proves the escalation fired.
#[tokio::test]
async fn cancel_escalates_to_sigkill_when_sigterm_is_ignored() {
    common::ensure_db().await;
    insert_run_row("pc-escalate", "running").await;
    // `trap '' TERM` makes the shell ignore SIGTERM; `sleep` exec'd from it
    // inherits the ignored disposition. Neither member can be killed by
    // SIGTERM — only SIGKILL ends the group.
    let mut child = Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 30"])
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn TERM-immune group");
    let pgid = child.id() as i32;
    process_control::register("pc-escalate", pgid);

    let (status, body) = post_json("/api/runs/pc-escalate/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body:?}");

    // The handler waits out the full 5 s grace window (nothing dies on
    // SIGTERM), then SIGKILLs. Poll past grace + verify + margin.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        if child.try_wait().expect("try_wait").is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        exited,
        "TERM-immune group must be killed by the SIGKILL escalation"
    );
    let _ = child.wait();
    assert!(
        !process_control::group_alive(pgid),
        "whole group must be dead after the escalation"
    );
    assert_eq!(
        run_status("pc-escalate").await.as_deref(),
        Some("cancelled")
    );
}

/// Spawn a background run through the real executor with a workdir and CLI
/// args derived from a tempdir (the EAGAIN retry-contract tests below).
///
/// The workflow contains a long rule so a late-spawned CLI stays alive long
/// enough for the tests to observe it (a fast-exiting CLI would make the
/// "no survivor" assertion pass vacuously).
fn spawn_real_run(run_id: &str, workdir: &std::path::Path) {
    let workflow = workdir.join("workflow.oxoflow");
    std::fs::write(
        &workflow,
        "[workflow]\nname = \"sleepy\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"sleepy\"\nshell = \"sleep 30\"\n",
    )
    .expect("write workflow");
    let args: Vec<std::ffi::OsString> = [
        "run",
        workflow.to_str().unwrap(),
        "--workdir",
        workdir.to_str().unwrap(),
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .collect();
    executor::spawn_background_run_with_args(
        run_id.to_string(),
        "default".to_string(),
        "none".to_string(),
        "local".to_string(),
        Some(workdir.to_path_buf()),
        args,
    );
}

/// The spawn EAGAIN retry contract (issue #136 tier-2): when every retry is
/// exhausted the run must fail loudly — never dangle as 'running' with a
/// NULL pid.
#[tokio::test]
async fn eagain_retry_marks_run_failed_when_exhausted() {
    common::ensure_db().await;
    let _env = set_spawn_fail(100);
    let dir = tempfile::tempdir().expect("tempdir");
    insert_run_row("pc-eagain-fail", "queued").await;
    spawn_real_run("pc-eagain-fail", dir.path());

    // All 3 retries fail (test seam), the spawn gives up, and the run is
    // marked failed — loudly, without a lingering 'running' row.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while run_status("pc-eagain-fail").await.as_deref() != Some("failed")
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        run_status("pc-eagain-fail").await.as_deref(),
        Some("failed"),
        "retry exhaustion must mark the run failed"
    );
}

/// Regression (issue #136 tier-2): a cancel landing while the spawn retry
/// window is open must stick — the exhaustion path must not overwrite the
/// 'cancelled' terminal state with 'failed'.
#[tokio::test]
async fn eagain_retry_cancel_keeps_cancelled_on_exhaustion() {
    common::ensure_db().await;
    let _env = set_spawn_fail(100);
    let dir = tempfile::tempdir().expect("tempdir");
    insert_run_row("pc-eagain-cancel", "queued").await;
    spawn_real_run("pc-eagain-cancel", dir.path());

    // Wait for the executor to pick the run up (queued → running), then
    // cancel inside the retry window (3 × 250 ms of sleeps).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while run_status("pc-eagain-cancel").await.as_deref() != Some("running")
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (status, body) = post_json("/api/runs/pc-eagain-cancel/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body:?}");

    // Give the executor time to exhaust all retries; the row must stay
    // 'cancelled' (marking it failed would lie about the user's intent).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        run_status("pc-eagain-cancel").await.as_deref(),
        Some("cancelled"),
        "retry exhaustion must not overwrite a cancelled run"
    );
}

/// Regression (issue #136 tier-2): a cancel that lands while the spawn
/// retry window is open must win — the CLI must not start executing a
/// workflow the user already stopped.
#[tokio::test]
async fn eagain_retry_cancel_aborts_late_spawn() {
    common::ensure_db().await;
    let _env = set_spawn_fail(3); // 3 failures, then the 4th attempt is real
    let dir = tempfile::tempdir().expect("tempdir");
    let workdir_str = dir.path().to_string_lossy().into_owned();
    insert_run_row("pc-eagain-abort", "queued").await;
    spawn_real_run("pc-eagain-abort", dir.path());

    // Wait for the executor to pick the run up, then cancel inside the
    // retry window.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while run_status("pc-eagain-abort").await.as_deref() != Some("running")
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (status, body) = post_json("/api/runs/pc-eagain-abort/cancel", json!({})).await;
    assert_eq!(status, StatusCode::OK, "cancel response: {body:?}");

    // A late-spawned CLI would carry the workdir path on its command line.
    // First wait PAST the full retry window (3 × 250 ms + margin) so a
    // late spawn has time to appear, then watch for a survivor — the CLI
    // must never start executing the cancelled workflow.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        assert!(
            !process_alive_with(&workdir_str),
            "a late-spawned CLI must not survive the cancel"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        run_status("pc-eagain-abort").await.as_deref(),
        Some("cancelled"),
        "cancelled run must stay cancelled"
    );
}

#[tokio::test]
async fn pause_freezes_then_resume_continues() {
    common::ensure_db().await;
    insert_run_row("pc-pause", "running").await;
    let mut child = spawn_and_register("pc-pause");

    let (status, _body) = post_json("/api/runs/pc-pause/pause", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run_status("pc-pause").await.as_deref(), Some("paused"));
    // Stopped process must not exit while paused.
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "paused child must not exit"
    );

    let (status, _body) = post_json("/api/runs/pc-pause/resume", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run_status("pc-pause").await.as_deref(), Some("running"));

    // Cleanup: kill the still-sleeping child directly through the registry.
    process_control::signal_group(child.id() as i32, process_control::SIGKILL)
        .expect("cleanup kill");
    child.wait().expect("wait after cleanup");
    process_control::unregister("pc-pause");
}

#[tokio::test]
async fn cancel_unknown_run_is_404() {
    common::ensure_db().await;
    let (status, body) = post_json("/api/runs/does-not-exist/cancel", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn cancel_terminal_run_is_rejected() {
    common::ensure_db().await;
    insert_run_row("pc-done", "completed").await;
    let (status, body) = post_json("/api/runs/pc-done/cancel", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "RUN_NOT_ACTIVE");
    assert_eq!(run_status("pc-done").await.as_deref(), Some("completed"));
}
