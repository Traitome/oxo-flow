//! Real process control: cancel/pause/resume must signal the CLI subprocess.
//!
//! The executor's CLI lookup honors OXO_FLOW_BIN first, but these tests
//! bypass spawning entirely: they register a `sleep` child's pgid under a
//! run id the same way the executor does, then drive the HTTP handlers.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxo_flow_web::{process_control, server};
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
