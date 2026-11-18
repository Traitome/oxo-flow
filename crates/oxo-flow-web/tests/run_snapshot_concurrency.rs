//! Admission of concurrent saved-pipeline runs (issue #1130, audit RB-02).
//!
//! Two simultaneous `POST /api/runs` for one saved pipeline must admit
//! exactly one run and answer the other `409 RUN_NOT_TERMINAL`. The admitted
//! runs now get unique workdirs, so a workdir-keyed guard can no longer
//! express this policy — the guarded insert is keyed on `pipeline_id`.
//!
//! One test per binary: `OXO_FLOW_BIN` and `OXO_FLOW_WORKSPACE` are
//! process-global, and the stub must stay alive long enough for the second
//! request to observe the first run as live.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "rb02-concurrent"
version = "1.0.0"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// SAFETY: this binary contains exactly one test; `set_var` runs before any
/// workspace read and no other thread can race it.
fn init_env() -> PathBuf {
    let base = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let root = base.join(format!("run-snapshot-concurrency-{}", std::process::id()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace root");

    let stub = root.join("oxo-flow-sleep-stub");
    write_sleep_stub(&stub);
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var(
            "OXO_FLOW_AI_CONFIG",
            root.join("hermetic/.oxo-flow/ai_config.json"),
        );
    }
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    workspace
}

/// Stay alive long enough that the competing request cannot observe the
/// winner as terminal, but short enough that a leaked child is harmless.
fn write_sleep_stub(stub: &Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(stub, "#!/bin/sh\nsleep 5\nexit 0\n").expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

async fn wait_terminal(run_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool())
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
async fn concurrent_creates_from_one_saved_pipeline_admit_exactly_one() {
    let workspace = init_env();
    common::ensure_db().await;

    let (status, body) = send(
        &app(),
        "POST",
        "/api/pipelines",
        Some(json!({"name": "rb02-concurrent", "toml_content": WORKFLOW})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pipeline = body["id"].as_str().expect("pipeline id").to_string();

    let request = || {
        let app = app();
        let pipeline = pipeline.clone();
        async move {
            send(
                &app,
                "POST",
                "/api/runs",
                Some(json!({
                    "toml_content": WORKFLOW,
                    "pipeline_id": pipeline,
                    "max_jobs": 1,
                })),
            )
            .await
        }
    };
    let (a, b) = tokio::join!(request(), request());
    let responses = [a, b];

    let admitted: Vec<&(StatusCode, Value)> = responses
        .iter()
        .filter(|(status, _)| *status == StatusCode::OK)
        .collect();
    let rejected: Vec<&(StatusCode, Value)> = responses
        .iter()
        .filter(|(status, _)| *status == StatusCode::CONFLICT)
        .collect();
    assert_eq!(
        (admitted.len(), rejected.len()),
        (1, 1),
        "exactly one create must be admitted and one rejected: {responses:?}"
    );
    assert_eq!(
        rejected[0].1["code"], "RUN_NOT_TERMINAL",
        "the loser must report the documented conflict: {responses:?}"
    );

    let run_id = admitted[0].1["run_id"]
        .as_str()
        .expect("run id")
        .to_string();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE pipeline_id = ?")
        .bind(&pipeline)
        .fetch_one(pool())
        .await
        .expect("count runs");
    assert_eq!(count, 1, "only the admitted request may persist a run row");

    // Exactly one run directory exists; the rejected request left nothing.
    let runs_dir = workspace.join("users/default/runs");
    let dirs: Vec<_> = std::fs::read_dir(&runs_dir)
        .expect("runs dir exists")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    assert_eq!(
        dirs.len(),
        1,
        "exactly one run workdir may remain: {dirs:?}"
    );
    assert!(dirs[0].path().ends_with(&run_id));

    // Release the guard: the run is cancelled, then a new create is admitted.
    let (status, body) = send(&app(), "POST", &format!("/api/runs/{run_id}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_terminal(&run_id).await;

    let (status, body) = send(
        &app(),
        "POST",
        "/api/runs",
        Some(json!({
            "toml_content": WORKFLOW,
            "pipeline_id": pipeline,
            "max_jobs": 1,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a terminal run must release the pipeline admission guard: {body}"
    );
}
