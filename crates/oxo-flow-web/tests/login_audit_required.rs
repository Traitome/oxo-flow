//! WEB-15 / issue #1199: the successful-login audit row is REQUIRED.
//!
//! Authentication success is security-relevant (the WEB-15 classification
//! table lists auth login as REQUIRED), so a lost login row must fail the
//! request closed with the shared structured `500 AUDIT_WRITE_FAILED` — the
//! message stays honest that the session/token may already have been minted
//! because `create_session` ran before the audit write. The license-upload
//! metadata row stays best-effort: the middleware already writes the
//! REQUIRED row for that action.
//!
//! Storage failure is injected the way the storage layer actually fails and
//! exactly as `audit_durability.rs` does: the `audit_logs` table is dropped
//! after a control request proves the REQUIRED row is normally recorded. One
//! test function, one binary — the table drop is process-visible and must
//! not race other tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const LOGIN_USER_ID: &str = "web19-login-user-id";
const LOGIN_USERNAME: &str = "web19-login-user";
const LOGIN_PASSWORD: &str = "web19-login-secret";

async fn json_body(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// A DB-created account, so the control login exercises the real DB
/// fallback (bcrypt hash in `users.password_hash`, issue #79 P1-06) without
/// mutating process-wide env credentials.
async fn seed_login_user() {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    let hash = oxo_flow_web::domains::auth::service::hash_password(LOGIN_PASSWORD)
        .expect("bcrypt hashing succeeds");
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, password_hash, created_at) \
         VALUES (?, ?, 'user', 'password', ?, ?)",
    )
    .bind(LOGIN_USER_ID)
    .bind(LOGIN_USERNAME)
    .bind(&hash)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
}

async fn login(app: &axum::Router) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "username": LOGIN_USERNAME,
                "password": LOGIN_PASSWORD,
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp.into_body()).await)
}

#[tokio::test]
async fn successful_login_row_is_required_and_license_metadata_is_best_effort() {
    common::ensure_db().await;
    seed_login_user().await;
    let app = server::build_router("personal");

    // Control: with healthy storage the successful login answers 200 and the
    // handler records exactly one row for the canonical users.id. The
    // middleware must not double-record a successful sign-in.
    let (status, body) = login(&app).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["token"].as_str().is_some_and(|t| !t.is_empty()),
        "the control login mints a session token: {body}"
    );

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT user_id, result FROM audit_logs WHERE action = 'POST /api/auth/login'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the handler records the login row once (the middleware skips success)"
    );
    assert_eq!(
        rows[0].0, LOGIN_USER_ID,
        "the row carries the canonical user id"
    );
    assert_eq!(rows[0].1, "success");

    // Inject the storage failure every audit write will now hit.
    sqlx::query("DROP TABLE audit_logs")
        .execute(pool)
        .await
        .expect("drop the audit table to simulate storage failure");

    // REQUIRED: a successful authentication whose audit row cannot be
    // persisted is not acknowledged — the structured fail-closed 500 replaces
    // the otherwise computed 200 (WEB-15 / #1199).
    let (status, body) = login(&app).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a lost REQUIRED login row must fail the response closed: {body}"
    );
    assert_eq!(body["code"], "AUDIT_WRITE_FAILED", "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("POST /api/auth/login"),
        "the error names the affected action: {body}"
    );
    assert!(
        message.contains("may already have been applied"),
        "the fail-closed error must admit the action may have committed: {body}"
    );
    assert!(
        !message.contains("refused"),
        "the sign-in is not refused — it is unacknowledged: {body}"
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
    // Ordering honesty: create_session already ran, so the token/session may
    // exist even though the audit write (and thus the response) failed.
    assert!(
        suggestion.contains("session token") && suggestion.contains("minted"),
        "the sign-in failure must say a session token may already be minted: {body}"
    );
    assert!(
        body["detail"].is_string(),
        "the storage error is carried as detail: {body}"
    );

    // The claim above is not hypothetical: the session row was committed
    // before the audit write failed.
    let sessions: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE user_id = ?")
        .bind(LOGIN_USER_ID)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(
        sessions.0 >= 2,
        "the audit write failed AFTER the session was persisted: {sessions:?}"
    );

    // BEST-EFFORT: the license-upload metadata row is supplementary — the
    // middleware already writes the REQUIRED row for this action — so a lost
    // metadata row must leave the handler's response unchanged. The handler
    // router is driven directly because the full router's middleware would
    // (correctly) replace the 200 with its own REQUIRED-audit 500; that
    // middleware contract is pinned by `audit_durability.rs`.
    let handler_app = axum::Router::new().route(
        "/api/license/upload",
        axum::routing::post(oxo_flow_web::domains::auth::handlers::upload_license),
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/license/upload")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "license_data": "example-license-blob" }).to_string(),
        ))
        .unwrap();
    let resp = handler_app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = json_body(resp.into_body()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a lost BEST-EFFORT metadata row must not change the license response: {body}"
    );
    assert_eq!(
        body["valid"], true,
        "the normal license status is returned: {body}"
    );
}
