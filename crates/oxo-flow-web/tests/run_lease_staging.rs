//! The claim-time lease guard covers the pre-spawn staging window
//! (issue #1402 delta review, HIGH).
//!
//! The executor-scoped heartbeat alone was not enough: `POST /api/runs`
//! claims the queued row, then materializes the workflow sources and stages
//! uploaded inputs BEFORE the spawn task exists. A staging window longer
//! than the 90 s lease TTL (a large uploads volume) would let another
//! instance reap the queued row while the owning handler was still working
//! on it — the fix creates the `RunMonitorGuard` at claim time and moves it
//! into the spawn task.
//!
//! This test parks the handler inside the staging seam
//! (`OXO_FLOW_TEST_STAGE_WAIT_FILE`), backdates the row's heartbeat beyond
//! the TTL, and proves (a) one heartbeat tick renews the row while the
//! handler is still staging — so the guard was registered before the spawn
//! task, not inside it; (b) the reaper refuses to touch a row this process
//! is still monitoring; (c) releasing the staging lets the run spawn and
//! complete normally.
//!
//! One test per binary: `OXO_FLOW_BIN`, `OXO_FLOW_WORKSPACE`, and
//! `OXO_FLOW_TEST_STAGE_WAIT_FILE` are process-global.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "lease-staging"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// This binary contains exactly one test, so mutating process-global
/// environment cannot race another thread. SAFETY: `set_var` runs before any
/// other thread reads the value.
fn set_env(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

/// Write an executable stub `oxo-flow` that records its argv and exits 0.
fn write_stub(stub: &std::path::Path, record: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
        record.display()
    );
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

async fn post(app: axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn find_run() -> Option<(String, String, Option<String>)> {
    sqlx::query_as("SELECT id, status, workdir FROM runs WHERE workflow_name = 'lease-staging'")
        .fetch_optional(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("read staged run")
}

async fn run_status(run_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("read run status")
}

/// Deadline guard for the polling loops below.
fn fresh_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(30)
}

fn assert_before(deadline: std::time::Instant, what: &str) {
    assert!(
        std::time::Instant::now() < deadline,
        "timed out waiting for {what}"
    );
}

#[tokio::test]
async fn claim_time_guard_heartbeats_the_pre_spawn_staging_window() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("lease-staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch root");
    set_env(
        "OXO_FLOW_WORKSPACE",
        root.join("workspace").to_str().unwrap(),
    );
    // Hermetic AI config (repo test rule): a non-existent path containing
    // `.oxo-flow`.
    set_env(
        "OXO_FLOW_AI_CONFIG",
        root.join("hermetic/.oxo-flow/ai_config.json")
            .to_str()
            .unwrap(),
    );

    let stub = root.join("oxo-flow-stub");
    let argv_record = root.join("argv.txt");
    write_stub(&stub, &argv_record);
    set_env("OXO_FLOW_BIN", stub.to_str().unwrap());
    // Park the handler between workflow materialization and input staging.
    let stage_release = root.join("release-staging");
    set_env(
        "OXO_FLOW_TEST_STAGE_WAIT_FILE",
        stage_release.to_str().unwrap(),
    );

    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("personal");

    // Act — the create request blocks in the staging seam while holding the
    // claim-time guard.
    let post_task = tokio::spawn(post(app, json!({"toml_content": WORKFLOW})));
    let deadline = fresh_deadline();
    let (run_id, status, workdir) = loop {
        if let Some(row) = find_run().await {
            break row;
        }
        assert_before(deadline, "the queued run row");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert_eq!(status, "queued", "the handler is parked before spawn");
    assert!(
        !argv_record.exists(),
        "the CLI stub must not have been spawned while staging is parked"
    );

    let backend = oxo_flow_web::infra::db::global_backend().expect("global backend");
    let old = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    sqlx::query("UPDATE runs SET heartbeat_at = ? WHERE id = ?")
        .bind(&old)
        .bind(&run_id)
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("backdate the heartbeat");

    // The row now looks stale to every other instance.
    let stale = backend
        .stale_run_leases(&oxo_flow_web::infra::run_lease::stale_cutoff(), 100)
        .await
        .expect("stale leases");
    assert!(
        stale.iter().any(|row| row.id == run_id),
        "the parked row looks stale before renewal"
    );

    // One heartbeat tick renews it: the claim-time guard is registered while
    // the handler is still staging, before any spawn task exists.
    let touched = oxo_flow_web::infra::run_lease::heartbeat(backend.as_ref())
        .await
        .expect("heartbeat");
    assert!(touched >= 1, "the claim-time guard is heartbeated");
    let renewed: Option<String> = sqlx::query_scalar("SELECT heartbeat_at FROM runs WHERE id = ?")
        .bind(&run_id)
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("read heartbeat");
    assert_ne!(
        renewed.as_deref(),
        Some(old.as_str()),
        "the staging window is inside the lease, not outside it"
    );

    // The reaper refuses a row this process is still monitoring, even
    // though its timestamp has been backdated again.
    sqlx::query("UPDATE runs SET heartbeat_at = ? WHERE id = ?")
        .bind(&old)
        .bind(&run_id)
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("backdate again");
    let reaped = oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref())
        .await
        .expect("reap");
    assert_eq!(reaped, 0, "a monitored row is never reaped");
    assert_eq!(
        run_status(&run_id).await,
        "queued",
        "the parked run was not finalized behind the handler's back"
    );

    // Release the staging window; the handler finishes and spawns the stub.
    std::fs::write(&stage_release, b"go").expect("release staging");
    let (status, body) = post_task.await.expect("create task");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["run_id"], run_id, "{body}");

    let deadline = fresh_deadline();
    while !argv_record.exists() {
        assert_before(deadline, "the CLI stub invocation");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // The stub exits 0; the wrapper records `.exit-code`, the live wait
    // finalizes the row, and the record is cleared after the terminal write.
    let deadline = fresh_deadline();
    while run_status(&run_id).await != "completed" {
        assert_before(deadline, "the run to finish");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let workdir = std::path::PathBuf::from(workdir.expect("run workdir"));
    let deadline = fresh_deadline();
    while workdir.join(".exit-code").exists() {
        assert_before(deadline, "the exit record to be cleared");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
