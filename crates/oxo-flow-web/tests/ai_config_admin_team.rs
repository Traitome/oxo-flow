//! Team-mode `GET /api/ai/config` personalizes when credentials are present.
//!
//! The route is public for anonymous discoverability, but presenting a valid
//! credential used to be ignored: the middleware skipped public paths before
//! resolving the session, so an admin saw the same masked payload as an
//! anonymous caller (journey-power P3). This binary pins the contract:
//! anonymous stays masked, a valid admin token sees provider/model, and an
//! invalid token is rejected instead of silently downgraded.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const ADMIN_TOKEN: &str = "journey-power-admin-token";

async fn json_body(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn seed_admin_session() {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES ('journey-admin-id', 'journey-admin', 'admin', 'password', ?)",
    )
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, 'journey-admin-id', ?, '2999-01-01T00:00:00Z')",
    )
    .bind(ADMIN_TOKEN)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
}

async fn get(app: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri("/api/ai/config");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp.into_body()).await)
}

#[tokio::test]
async fn team_mode_ai_config_authenticates_when_credentials_are_present() {
    // A developer shell may export the provider pin; this test owns its
    // process environment (one test per binary), so clear it before the AI
    // registry initializes. HOME is redirected so `reconfigure`'s config
    // persistence can never rewrite the operator's real
    // `~/.oxo-flow/ai_config.json`.
    // SAFETY: single-threaded test setup in a dedicated test binary; no
    // other thread reads these variables before they are changed.
    let home = std::env::temp_dir().join(format!(
        "oxo-flow-ai-config-admin-team-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&home).unwrap();
    unsafe {
        std::env::remove_var("OXO_FLOW_AI_PROVIDER");
        std::env::set_var("HOME", &home);
    }

    common::ensure_db().await;
    seed_admin_session().await;
    oxo_flow_web::ai_provider::AiProviderRegistry::global()
        .reconfigure(
            "openai",
            Some("sk-journey-test".to_string()),
            Some("https://example.invalid/v1".to_string()),
            Some("journey-model".to_string()),
        )
        .expect("test provider configures");

    // Team mode is set on the first `build_router` call in this binary.
    let app = server::build_router("team");

    let (status, anon) = get(&app, None).await;
    assert_eq!(status, StatusCode::OK, "{anon}");
    assert_eq!(
        anon["provider"], "",
        "anonymous callers keep the masked payload: {anon}"
    );

    let (status, admin) = get(&app, Some(ADMIN_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(
        admin["provider"], "openai",
        "an authenticated admin sees the configured provider: {admin}"
    );
    assert_eq!(admin["model"], "journey-model", "{admin}");
    assert_eq!(
        admin["is_configured"], true,
        "a configured provider must report configured: {admin}"
    );

    // Presenting an unknown token must fail authentication, not fall back to
    // the anonymous payload.
    let (status, body) = get(&app, Some("not-a-session")).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "invalid credentials must not be silently downgraded: {body}"
    );
}
