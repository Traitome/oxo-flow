//! Team-mode RBAC for the audit surfaces (audit M5).
//!
//! Live probes verified that `GET /api/audit`, `GET /api/audit/chain/export`,
//! and `GET /api/audit/chain/verify` answer `403 ACCESS_DENIED` for both
//! `user` and `viewer` in team mode, but `audit_chain_api.rs` builds the
//! `personal` router (no roles at all) and never sends a non-admin request.
//! This binary pins the role gate: admin 200, user/viewer 403, anonymous
//! 401. One test per binary because `build_router` fixes the process mode.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const ADMIN_TOKEN: &str = "audit-rbac-admin-token";
const USER_TOKEN: &str = "audit-rbac-user-token";
const VIEWER_TOKEN: &str = "audit-rbac-viewer-token";

async fn seed_identity(id: &str, username: &str, role: &str, token: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, ?, ?, 'password', ?)",
    )
    .bind(id)
    .bind(username)
    .bind(role)
    .bind(&now)
    .execute(pool)
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
    )
    .bind(token)
    .bind(id)
    .bind(&now)
    .execute(pool)
    .await
    .expect("seed session");
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn audit_and_chain_routes_are_admin_only_in_team_mode() {
    common::ensure_db().await;
    seed_identity("audit-rbac-admin", "rbac-admin", "admin", ADMIN_TOKEN).await;
    seed_identity("audit-rbac-user", "rbac-user", "user", USER_TOKEN).await;
    seed_identity("audit-rbac-viewer", "rbac-viewer", "viewer", VIEWER_TOKEN).await;

    // Team mode is fixed on the first build_router call in this binary.
    let app = server::build_router("team");

    for uri in [
        "/api/audit",
        "/api/audit/chain/export",
        "/api/audit/chain/verify",
    ] {
        let (status, body) = get(&app, uri, Some(ADMIN_TOKEN)).await;
        assert_eq!(status, StatusCode::OK, "admin GET {uri}: {body}");

        for token in [USER_TOKEN, VIEWER_TOKEN] {
            let (status, body) = get(&app, uri, Some(token)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "non-admin GET {uri}: {body}");
            assert_eq!(body["code"], "ACCESS_DENIED", "{uri}: {body}");
        }

        let (status, body) = get(&app, uri, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "anonymous GET {uri}: {body}"
        );
        assert_eq!(body["code"], "AUTH_REQUIRED", "{uri}: {body}");
    }
}
