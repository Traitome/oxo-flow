//! OAuth CSRF-state verification (B8).
//!
//! The callback must reject any state that this server did not issue, BEFORE
//! any token exchange. The invalid-provider trick keeps the tests offline:
//! with an unknown provider the exchange itself errors without network I/O,
//! so any non-INVALID_STATE response proves the state gate did not fire.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tower::ServiceExt;

static DB_URL: OnceLock<String> = OnceLock::new();

fn db_url() -> &'static str {
    DB_URL.get_or_init(|| {
        let dir = std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let path = format!("{dir}/oauth-test.db");
        let _ = std::fs::remove_file(&path);
        format!("sqlite:{path}?mode=rwc")
    })
}

async fn ensure_db() {
    let url = db_url();
    oxo_flow_web::db::init_db(url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(url).await;
}

async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = server::build_router("team")
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
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

#[tokio::test]
async fn callback_rejects_unissued_state() {
    ensure_db().await;
    // Unknown provider: the exchange would fail fast without network, so a
    // non-INVALID_STATE code proves the state gate fired (or did not).
    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "attacker-state"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_INVALID_STATE", "{body:?}");
}

#[tokio::test]
async fn issued_state_is_single_use() {
    // Storage/verification helper round-trip: issued states verify once.
    let backend = SqliteBackend::new(db_url()).await.expect("backend");
    backend.init().await.expect("schema");

    oxo_flow_web::domains::auth::service::store_pending_state("state-1")
        .await
        .expect("store state");
    oxo_flow_web::domains::auth::service::verify_and_consume_state("state-1")
        .await
        .expect("first use verifies");
    let second = oxo_flow_web::domains::auth::service::verify_and_consume_state("state-1").await;
    assert!(second.is_err(), "state must be single-use");
    let unknown =
        oxo_flow_web::domains::auth::service::verify_and_consume_state("never-issued").await;
    assert!(unknown.is_err(), "unissued state must fail");
}
