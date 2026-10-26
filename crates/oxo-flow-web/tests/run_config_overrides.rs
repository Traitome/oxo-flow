//! Run-level config-overrides contract (PR #930 NIT-3 + NIT-8).
//!
//! NIT-3: a 400 from include-key/override validation must never leave a
//! ghost `queued` runs row behind — the pure gates run before the INSERT.
//! NIT-8: a retry replays the original run's config_overrides instead of
//! re-executing with default configuration.
//!
//! This is a dedicated test binary on purpose: the run-creation rate
//! limiter (#213) is a process-wide sliding window, and the other router
//! test binaries already spend most of its five-per-minute allowance.
//! The four create-calls below would spend exactly that ceiling (zero
//! headroom for any future create-call), so the binary zeroes
//! `OXO_FLOW_RUNS_RATE_LIMIT` in `ensure_db` — the limiter's own behavior
//! stays pinned by the `handlers::run_rate_limit` unit tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

async fn ensure_db() {
    // The four `POST /api/runs` calls below spend exactly the default
    // five-per-minute allowance; disable the limiter so this binary never
    // flakes with 429 RUN_RATE_LIMITED (the limiter itself is pinned by
    // `handlers::run_rate_limit` unit tests and other router binaries).
    unsafe { std::env::set_var("OXO_FLOW_RUNS_RATE_LIMIT", "0") };
    common::ensure_db().await;
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{method} {uri}: body is not JSON ({e}): {bytes:?}"));
    (status, value)
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(app, "POST", uri, Some(body)).await
}

/// Engine-acceptable one-rule workflow whose name doubles as the unique
/// marker for ghost-row assertions (the name lands verbatim in the
/// pipeline_snapshot column).
fn marked_toml(marker: &str) -> String {
    format!(
        r#"
[workflow]
name = "{marker}"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {{input}} > {{output}}"
"#
    )
}

async fn ghost_run_count(marker: &str) -> i64 {
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("personal mode has SQLite");
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM runs WHERE pipeline_snapshot LIKE ?")
            .bind(format!("%{marker}%"))
            .fetch_one(pool)
            .await
            .expect("count runs");
    count
}

// ---------------------------------------------------------------------------
// NIT-3 — a rejected create must not persist anything
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_include_path_leaves_no_ghost_run() {
    ensure_db().await;
    let app = server::build_router("personal");
    let marker = "ghost-include-nit3";
    let body_toml = marked_toml(marker);

    // Key that would overwrite the staged main workflow.
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": body_toml,
            "dry_run": true,
            "include_files": {"workflow.oxoflow": "[workflow]\nname = \"evil\""}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_INCLUDE_PATH", "{body}");

    // Traversal key.
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": body_toml,
            "dry_run": true,
            "include_files": {"../escape.toml": "[env]\n"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_INCLUDE_PATH", "{body}");

    assert_eq!(
        ghost_run_count(marker).await,
        0,
        "rejected creates must not leave a ghost runs row"
    );
}

#[tokio::test]
async fn invalid_config_override_leaves_no_ghost_row() {
    ensure_db().await;
    let app = server::build_router("personal");
    let marker = "ghost-override-nit3";

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": marked_toml(marker),
            "dry_run": true,
            "config_overrides": ["noequals-sign"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_CONFIG_OVERRIDE", "{body}");

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": marked_toml(marker),
            "dry_run": true,
            "config_overrides": [""]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_CONFIG_OVERRIDE", "{body}");

    assert_eq!(
        ghost_run_count(marker).await,
        0,
        "rejected overrides must not leave a ghost runs row"
    );
}

// ---------------------------------------------------------------------------
// Persistence — the create path stores overrides for the retry path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_config_overrides() {
    ensure_db().await;
    let app = server::build_router("personal");
    let marker = "persist-overrides-nit3";

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": marked_toml(marker),
            "dry_run": true,
            "config_overrides": ["k=v"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run_id in response");

    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("personal mode has SQLite");
    let (stored,): (Option<String>,) =
        sqlx::query_as("SELECT config_overrides FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
    assert_eq!(
        stored.as_deref(),
        Some(r#"["k=v"]"#),
        "create must persist the overrides verbatim (order-preserving JSON array)"
    );
}

// ---------------------------------------------------------------------------
// NIT-8 — retry replays the original run's config overrides
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_replays_the_original_config_overrides() {
    ensure_db().await;
    let app = server::build_router("personal");
    let marker = "retry-overrides-nit8";
    let run_id = format!("retry-ov-{marker}");

    // A FAILED run with a parseable snapshot (the retry rebuilds the DAG
    // from it) and stored overrides. No real workdir is needed: with no
    // checkpoint the plan is empty and the handler still answers 200.
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("personal mode has SQLite");
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, ?, ?, 'failed', 'execution', 0, NULL, NULL, '[\"k=v\"]', NULL, NULL, '2026-10-27T00:00:00Z')",
    )
    .bind(&run_id)
    .bind(marked_toml(marker))
    .bind(marker)
    .execute(pool)
    .await
    .unwrap();

    let (status, body) = post(&app, &format!("/api/runs/{run_id}/retry"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let new_run_id = body["new_run_id"]
        .as_str()
        .expect("retry response carries new_run_id")
        .to_string();

    // The retry row INSERT is synchronous (it precedes the background
    // spawn), so the persisted overrides can be asserted immediately.
    let (stored,): (Option<String>,) =
        sqlx::query_as("SELECT config_overrides FROM runs WHERE id = ?")
            .bind(&new_run_id)
            .fetch_one(pool)
            .await
            .expect("retry row exists");
    assert_eq!(
        stored.as_deref(),
        Some(r#"["k=v"]"#),
        "retry must carry the original run's overrides forward"
    );
}
