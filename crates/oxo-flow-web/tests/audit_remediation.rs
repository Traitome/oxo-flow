//! Regression tests for the audit-remediation fixes on
//! `fix/audit-remediation`: pause/resume/retry state integrity, share
//! visibility validation, and rollback parse validation.
//!
//! All tests drive the production router (`build_router("personal")`) against
//! the shared file-backed DB harness (`common::ensure_db`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn json_body(body: axum::body::Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn post(uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp.into_body()).await)
}

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

/// Seed a run owned by the personal-mode pseudo-user.
async fn seed_run(id: &str, status: &str) {
    sqlx::query(
        "INSERT OR REPLACE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, \
         workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, '', NULL, ?, 'executing', NULL, NULL, NULL, NULL, ?)",
    )
    .bind(id)
    .bind(status)
    .bind("2026-01-01T00:00:00Z")
    .execute(pool())
    .await
    .expect("seed run");
}

async fn run_status(id: &str) -> String {
    let row: (String,) = sqlx::query_as("SELECT status FROM runs WHERE id = ?")
        .bind(id)
        .fetch_one(pool())
        .await
        .expect("run row");
    row.0
}

/// A queued run has no process group; pausing it used to flip the row to
/// 'paused' with nothing to ever unfreeze it (stuck row + leaked quota).
#[tokio::test]
async fn pause_refuses_a_run_without_a_process_group() {
    common::ensure_db().await;
    seed_run("run-pause-queued", "queued").await;

    let (status, body) = post("/api/runs/run-pause-queued/pause", json!({})).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "NO_PROCESS_GROUP", "{body}");
    assert_eq!(
        run_status("run-pause-queued").await,
        "queued",
        "a refused pause must not strand the row in 'paused'"
    );
}

/// `from_rule` on retry is documented but the engine cannot honour it; it
/// must be rejected, not echoed back as an applied option.
#[tokio::test]
async fn retry_rejects_the_unsupported_from_rule() {
    common::ensure_db().await;
    seed_run("run-retry", "failed").await;

    let (status, body) = post("/api/runs/run-retry/retry", json!({"from_rule": "fastqc"})).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "UNSUPPORTED_FIELD", "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("from_rule"),
        "the error must name the unsupported field: {body}"
    );
}

/// Resuming unfreezes a process in place — a `from_rule` origin cannot
/// change that plan either.
#[tokio::test]
async fn resume_rejects_the_unsupported_from_rule() {
    common::ensure_db().await;
    seed_run("run-resume", "paused").await;

    let (status, body) = post(
        "/api/runs/run-resume/resume",
        json!({"from_rule": "fastqc"}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "UNSUPPORTED_FIELD", "{body}");
    assert_eq!(
        run_status("run-resume").await,
        "paused",
        "a rejected resume must not unfreeze the row"
    );
}

/// Share links are anonymous read-only pages, so only "link"/"public" are
/// real: any other visibility would be stored but never enforced.
#[tokio::test]
async fn share_rejects_a_visibility_it_cannot_enforce() {
    common::ensure_db().await;
    sqlx::query(
        "INSERT OR REPLACE INTO pipelines (id, user_id, name, version, toml_content, \
         rules_count, visibility, created_at, updated_at) \
         VALUES ('pl-share', 'default', 'p', '1.0.0', '[workflow]\nname = \"p\"', 0, \
         'private', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .execute(pool())
    .await
    .expect("seed pipeline");

    let (status, body) = post(
        "/api/pipelines/pl-share/share",
        json!({"visibility": "private"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "UNSUPPORTED_VISIBILITY", "{body}");

    let (status, body) = post(
        "/api/pipelines/pl-share/share",
        json!({"visibility": "link", "expires_in_days": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["access_token"].is_string(), "{body}");
}

/// Rolling back to a revision the engine cannot parse used to keep the
/// current pipeline's stale rules_count (save/update reject such content).
#[tokio::test]
async fn rollback_rejects_an_unparsable_revision() {
    common::ensure_db().await;
    const VALID: &str = "[workflow]\nname = \"p\"\n\n[[rules]]\nname = \"a\"\noutput = [\"a.txt\"]\nshell = \"echo a > a.txt\"\n";
    sqlx::query(
        "INSERT OR REPLACE INTO pipelines (id, user_id, name, version, toml_content, \
         rules_count, visibility, created_at, updated_at) \
         VALUES ('pl-roll', 'default', 'p', '1.0.0', ?, 1, 'private', \
         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(VALID)
    .execute(pool())
    .await
    .expect("seed pipeline");
    sqlx::query(
        "INSERT OR REPLACE INTO pipeline_revisions (id, pipeline_id, user_id, version, \
         toml_content, created_at) VALUES ('rev-bad', 'pl-roll', 'default', '0.9.0', \
         'this is not toml', '2026-01-01T00:00:00Z')",
    )
    .execute(pool())
    .await
    .expect("seed revision");

    let (status, body) = post(
        "/api/pipelines/pl-roll/rollback",
        json!({"revision_id": "rev-bad"}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "PARSE_ERROR", "{body}");
    let (rules_count, content): (i64, String) =
        sqlx::query_as("SELECT rules_count, toml_content FROM pipelines WHERE id = 'pl-roll'")
            .fetch_one(pool())
            .await
            .expect("pipeline row");
    assert_eq!(rules_count, 1, "rules_count must not go stale");
    assert_eq!(
        content, VALID,
        "a rejected rollback must not rewrite the TOML"
    );
}
