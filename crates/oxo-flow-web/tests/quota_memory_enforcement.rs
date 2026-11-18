//! OS-enforced memory ceiling for web-spawned runs (RB-09 / issue #1133).
//!
//! The quota layer has always *counted* a workflow's declared memory; before
//! this evidence it never *enforced* it. `spawn_background_run_with_args`
//! now installs `RLIMIT_DATA` on the spawned CLI process tree at the run's
//! declared total memory (512 MB floor for engine overhead), so a rule that
//! exceeds its declaration dies on allocation failure and the run fails —
//! observable rejection, not an advisory count.
//!
//! Linux-gated: Darwin rejects `RLIMIT_DATA`/`RLIMIT_AS` changes (EINVAL), so
//! the executor degrades to advisory there (documented in the web API
//! reference). The test drives the REAL web path (`POST /api/runs` through
//! the production router) with a stub `oxo-flow` that tries to allocate
//! 520 MiB — above the 512 MiB declaration — in both configurations:
//! declared (must fail) and undeclared (no ceiling, must complete).

#![cfg(target_os = "linux")]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

/// One workflow, differing only in the memory declaration.
const DECLARED_WORKFLOW: &str = r#"
[workflow]
name = "quota-memory-declared"

[[rules]]
name = "hog"
output = ["out.txt"]
shell = "dd if=/dev/zero of=/dev/null bs=520M count=1"

[rules.resources]
memory = "512M"
"#;

const UNDECLARED_WORKFLOW: &str = r#"
[workflow]
name = "quota-memory-undeclared"

[[rules]]
name = "hog"
output = ["out.txt"]
shell = "dd if=/dev/zero of=/dev/null bs=520M count=1"
"#;

#[tokio::test]
async fn declared_memory_is_enforced_with_rlimit_and_rejection_is_observable() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("quota-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch root");
    let stub = root.join("oxo-flow-hog");
    write_stub(&stub);

    // This binary contains exactly one test, so mutating process-global
    // environment cannot race another thread.
    // SAFETY: `set_var` runs before the router exists and no other thread
    // reads these variables.
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", root.join("workspace"));
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var(
            "OXO_FLOW_AI_CONFIG",
            root.join("hermetic/.oxo-flow/ai_config.json"),
        );
    }
    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("personal");

    // ── Declared: the 520 MiB allocation exceeds the 512 MiB ceiling ────
    let (status, body) = post_run(&app, DECLARED_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "create accepts the file: {body}");
    let declared_run = body["run_id"].as_str().expect("run_id").to_string();
    let declared_status = wait_terminal(&declared_run).await;
    assert_eq!(
        declared_status, "failed",
        "a rule allocating past its declared memory must fail the run, not \
         complete with an advisory overage"
    );

    // ── Undeclared: no memory claim, so no ceiling — same stub succeeds ─
    let (status, body) = post_run(&app, UNDECLARED_WORKFLOW).await;
    assert_eq!(status, StatusCode::OK, "create accepts the file: {body}");
    let undeclared_run = body["run_id"].as_str().expect("run_id").to_string();
    let undeclared_status = wait_terminal(&undeclared_run).await;
    assert_eq!(
        undeclared_status, "completed",
        "without a declared memory there is no ceiling to enforce"
    );
}

/// A stub `oxo-flow` that tries to allocate 520 MiB (GNU dd allocates its
/// `bs` buffer), ignoring its arguments.
fn write_stub(stub: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = "#!/bin/sh\ndd if=/dev/zero of=/dev/null bs=520M count=1\n";
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

async fn post_run(app: &axum::Router, toml: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("content-type", "application/json")
                .body(Body::from(json!({"toml_content": toml}).to_string()))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn wait_terminal(run_id: &str) -> String {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
