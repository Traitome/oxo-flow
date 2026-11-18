//! A pre-spawn staging failure must fail closed (RB-03 / #1131).
//!
//! `POST /api/runs` claims the run row, materializes the workflow source,
//! stages uploaded inputs, and only then spawns the CLI. Before RB-03 the
//! materialize and input-staging steps logged and returned 2xx anyway, so a
//! client could receive success for a run whose workdir was missing its
//! workflow or uploads — and a `queued` row with no work behind it.
//!
//! This binary points `OXO_FLOW_BIN` at a stub that records its argv, then
//! forces each pre-spawn step to fail through `OXO_FLOW_TEST_FAIL_PRE_SPAWN`
//! and asserts the response is non-2xx, the stub was NEVER invoked, and no
//! ghost `queued` row survives. A final clean create proves the stub itself
//! works, so the "never invoked" assertions are not vacuous.
//!
//! One test per binary on purpose: `OXO_FLOW_BIN`, `OXO_FLOW_WORKSPACE`, and
//! `OXO_FLOW_TEST_FAIL_PRE_SPAWN` are process-global, and tests within a
//! binary run on parallel threads.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "pre-spawn-fault"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// This binary contains exactly one test, so mutating process-global
/// environment cannot race another thread. SAFETY: `set_var`/`remove_var`
/// run before any other thread can read the value.
fn set_env(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

fn unset_env(key: &str) {
    unsafe { std::env::remove_var(key) };
}

/// Write an executable stub `oxo-flow` that records its argv (one entry per
/// line) to `record` and exits 0.
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

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
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

/// Total rows in `runs`. A compensation that leaks the row would leave a
/// ghost `queued` entry behind a non-2xx answer.
async fn run_count() -> i64 {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(pool)
        .await
        .expect("count runs")
}

/// Give the background spawn a moment to (not) happen, then assert the stub
/// never recorded an invocation.
async fn assert_never_spawned(record: &std::path::Path) {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        !record.exists(),
        "the CLI stub was invoked despite a pre-spawn failure: {}",
        record.display()
    );
}

#[tokio::test]
async fn pre_spawn_staging_failure_fails_closed_without_spawning() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root =
        std::path::PathBuf::from(tmp).join(format!("pre-spawn-fault-{}", std::process::id()));
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
    let record = root.join("argv.txt");
    write_stub(&stub, &record);
    set_env("OXO_FLOW_BIN", stub.to_str().unwrap());

    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("personal");

    // Fault 1 — the workflow source cannot be materialized.
    let before = run_count().await;
    set_env("OXO_FLOW_TEST_FAIL_PRE_SPAWN", "materialize");
    let (status, body) = post(&app, json!({"toml_content": WORKFLOW})).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "a materialize failure must answer non-2xx, got {status}: {body}"
    );
    assert_eq!(body["code"], "RUN_ERROR", "{body}");
    assert_eq!(
        run_count().await,
        before,
        "a ghost queued row survived a materialize failure"
    );
    assert_never_spawned(&record).await;

    // Fault 2 — uploaded inputs cannot be staged.
    let before = run_count().await;
    set_env("OXO_FLOW_TEST_FAIL_PRE_SPAWN", "inputs");
    let (status, body) = post(&app, json!({"toml_content": WORKFLOW})).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "an input-staging failure must answer non-2xx, got {status}: {body}"
    );
    assert_eq!(body["code"], "RUN_ERROR", "{body}");
    assert_eq!(
        run_count().await,
        before,
        "a ghost queued row survived an input-staging failure"
    );
    assert_never_spawned(&record).await;

    // Fault 3 — unknown cluster must not leave a ghost queued row.
    unset_env("OXO_FLOW_TEST_FAIL_PRE_SPAWN");
    let before = run_count().await;
    let body = json!({"toml_content": WORKFLOW, "cluster_id": "no-such-cluster"});
    let (status, resp) = post(&app, body).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "unknown cluster must answer non-2xx, got {status}: {resp}"
    );
    assert_eq!(
        run_count().await,
        before,
        "a ghost queued row survived the cluster error path"
    );
    assert_never_spawned(&record).await;

    // Control — with the fault cleared the clean create still spawns, so the
    // "never invoked" assertions above are meaningful.
    unset_env("OXO_FLOW_TEST_FAIL_PRE_SPAWN");
    let (status, body) = post(&app, json!({"toml_content": WORKFLOW})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !record.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the control create never spawned the stub"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
