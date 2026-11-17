//! `POST /api/runs` forwards `keep_going` to the spawned CLI
//! (issue #1093 follow-up).
//!
//! The request field is a one-line mapping into `RunFlags`, and the argv
//! builder is unit-tested in `executor.rs` — but only an end-to-end create
//! proves the handler actually reads the wire field. This binary points
//! `OXO_FLOW_BIN` at a stub that records its argv and exits 0, then asserts
//! the recorded command carries `--keep-going` only when the request asked
//! for it.
//!
//! One test per binary on purpose: `OXO_FLOW_BIN` and `OXO_FLOW_WORKSPACE`
//! are process-global, and tests within a binary run on parallel threads.
//! The stub is swapped between the two creates sequentially, after the first
//! run has reached a terminal state, so no spawn can resolve the wrong pin.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "keep-going-spawn"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// This binary contains exactly one test, so mutating process-global
/// environment cannot race another thread. SAFETY: `set_var` runs before any
/// other thread can read the value.
fn set_env(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

/// Write an executable stub `oxo-flow` that records its argv (one entry per
/// line) to `record` and exits 0 — the executor's happy path.
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

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Wait for the stub's argv record to exist, then return it line by line.
async fn recorded_argv(record: &std::path::Path) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(record) {
            return text.lines().map(str::to_string).collect();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the spawned CLI never ran (no argv record at {})",
            record.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Wait until the run is neither queued nor running — the executor has
/// finished spawning and reaping, so the stub pin is safe to swap.
async fn wait_terminal(run_id: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn create_run_forwards_keep_going_to_the_spawned_cli() {
    // Arrange — isolated workspace + DB; one stub per policy.
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root =
        std::path::PathBuf::from(tmp).join(format!("keep-going-spawn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch root");
    set_env(
        "OXO_FLOW_WORKSPACE",
        root.join("workspace").to_str().unwrap(),
    );

    let keep_stub = root.join("oxo-flow-keep");
    let keep_record = root.join("argv-keep-going.txt");
    let abort_stub = root.join("oxo-flow-abort");
    let abort_record = root.join("argv-default.txt");

    common::ensure_db().await;
    // Hermetic AI config (repo test rule): a non-existent path containing
    // `.oxo-flow`. This binary never reads or writes provider config, and
    // the single test owns the process (no env races).
    set_env(
        "OXO_FLOW_AI_CONFIG",
        root.join("hermetic/.oxo-flow/ai_config.json")
            .to_str()
            .unwrap(),
    );
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("personal");

    // Act 1 — `keep_going: true` must reach the CLI as `--keep-going`.
    write_stub(&keep_stub, &keep_record);
    set_env("OXO_FLOW_BIN", keep_stub.to_str().unwrap());
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({"toml_content": WORKFLOW, "keep_going": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let keep_run = body["run_id"].as_str().expect("run_id").to_string();
    let argv = recorded_argv(&keep_record).await;

    // Assert 1 — the flag rides the real spawn, next to the standard args.
    assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
    assert!(
        argv.iter().any(|a| a == "--keep-going"),
        "keep_going: true must forward --keep-going: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--workdir"),
        "the run argv must still carry the workdir: {argv:?}"
    );
    wait_terminal(&keep_run).await;

    // Act 2 — an omitted field keeps the historical abort-on-failure policy.
    write_stub(&abort_stub, &abort_record);
    set_env("OXO_FLOW_BIN", abort_stub.to_str().unwrap());
    let (status, body) = post(&app, "/api/runs", json!({"toml_content": WORKFLOW})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let abort_run = body["run_id"].as_str().expect("run_id").to_string();
    let argv = recorded_argv(&abort_record).await;

    // Assert 2 — no implicit flag from the default (or from the previous
    // request's value).
    assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
    assert!(
        !argv.iter().any(|a| a == "--keep-going"),
        "absent keep_going must not forward --keep-going: {argv:?}"
    );
    wait_terminal(&abort_run).await;
}
