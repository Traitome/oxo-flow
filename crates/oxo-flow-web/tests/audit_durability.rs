//! WEB-15 / issue #1140: explicit audit durability.
//!
//! REQUIRED audit rows (security-relevant actions) fail the request closed
//! with a structured `500 AUDIT_WRITE_FAILED` when the write cannot be
//! persisted; BEST-EFFORT rows log a warning and the response is returned
//! unchanged.
//!
//! The failure is injected the way the storage layer actually fails: the
//! `audit_logs` table is dropped from the test database after a control
//! request proves the REQUIRED row is normally recorded. One test function,
//! one binary — the table drop is deliberately process-visible and must not
//! race other tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const ADMIN_TOKEN: &str = "web15-audit-admin-token";

async fn json_body(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn seed_admin_session() {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES ('web15-audit-admin-id', 'web15-audit-admin', 'admin', 'password', ?)",
    )
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, 'web15-audit-admin-id', ?, '2999-01-01T00:00:00Z')",
    )
    .bind(ADMIN_TOKEN)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
}

async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {ADMIN_TOKEN}"));
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
    (status, json_body(resp.into_body()).await)
}

#[tokio::test]
async fn required_audit_failures_fail_closed_and_best_effort_do_not() {
    common::ensure_db().await;
    seed_admin_session().await;
    let app = server::build_router("team");

    // Control: the REQUIRED route records its row while storage is healthy.
    // (The pipeline does not exist, so the action's own answer is 404 — the
    // audit row is still written, and that is what this assertion pins.)
    let (status, body) = request(&app, "DELETE", "/api/pipelines/web15-missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let recorded: (String,) =
        sqlx::query_as("SELECT result FROM audit_logs WHERE action = ? LIMIT 1")
            .bind("DELETE /api/pipelines/web15-missing")
            .fetch_one(pool)
            .await
            .expect("the REQUIRED audit row must be recorded");
    assert_eq!(recorded.0, "failure", "the 404 outcome is recorded");

    // Inject the storage failure every audit write will now hit.
    sqlx::query("DROP TABLE audit_logs")
        .execute(pool)
        .await
        .expect("drop the audit table to simulate storage failure");

    // REQUIRED: a security-relevant action is refused with the structured
    // fail-closed error instead of the handler's (otherwise computed) 404.
    let (status, body) = request(&app, "DELETE", "/api/pipelines/web15-missing", None).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a lost REQUIRED audit row must fail the request closed: {body}"
    );
    assert_eq!(body["code"], "AUDIT_WRITE_FAILED", "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("DELETE /api/pipelines/web15-missing"),
        "the error names the affected action: {body}"
    );
    // D1: the mutation already ran — the message must not claim it was
    // refused/rolled back, and must not invite a blind retry.
    assert!(
        message.contains("may already have been applied"),
        "the fail-closed error must admit the action may have committed: {body}"
    );
    assert!(
        !message.contains("refused"),
        "the mutation is not refused — it is unacknowledged: {body}"
    );
    let suggestion = body["suggestion"].as_str().unwrap_or_default();
    assert!(
        suggestion.contains("Verify the current state"),
        "the caller is told to inspect state before retrying: {body}"
    );
    assert!(
        suggestion.contains("duplicate"),
        "the caller is warned that a retry can duplicate the effect: {body}"
    );
    assert!(
        body["detail"].is_string(),
        "the storage error is carried as detail: {body}"
    );

    // BEST-EFFORT: an informational mutation still answers normally — the
    // handler's 400 parse error, never the audit failure's 500.
    let (status, body) = request(
        &app,
        "POST",
        "/api/pipelines/parse",
        Some(serde_json::json!({ "toml_content": "this is not TOML" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a lost BEST-EFFORT audit row must not change the response: {body}"
    );
    assert_eq!(body["code"], "PARSE_ERROR", "{body}");

    // 5xx PRESERVATION BOUNDARY: a REQUIRED route whose handler already
    // answered 5xx keeps that answer. This is what lets the documented
    // `503 RUNS_REQUIRE_STORAGE_BACKEND` boundary survive a no-backend
    // deployment instead of being overwritten by AUDIT_WRITE_FAILED. The
    // synthetic route is classified REQUIRED (`/api/runs/{id}/cancel`) and
    // the audit table is still dropped above, so the write inside the
    // middleware genuinely fails.
    let synthetic = axum::Router::new()
        .route(
            "/api/runs/synthetic/cancel",
            axum::routing::post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .layer(axum::middleware::from_fn(
            oxo_flow_web::audit::audit_middleware,
        ));
    let resp = synthetic
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs/synthetic/cancel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an already-5xx response must be preserved when the REQUIRED audit write fails"
    );
}
