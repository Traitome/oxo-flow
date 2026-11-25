//! WEB-15 / issue #1140: AI provider metadata is role-gated.
//!
//! Redaction policy (documented in `domains/ai/handlers.rs`):
//!
//! - `GET /api/ai/config` is public for feature discoverability, so it is
//!   the strictest surface: outside personal mode every non-admin —
//!   anonymous included — sees only `is_configured` (provider/model/url are
//!   masked).
//! - `GET /api/ai/config/effective` is behind authentication. Non-admin
//!   `user`/`viewer` callers learn the provider NAME and enabled state but
//!   not the endpoint URL or model id (internal proxy/gateway shape).
//! - `GET /api/ai/config/user` returns a row the caller saved themselves in
//!   full; with no per-user row the mirrored SHARED runtime metadata is
//!   redacted for non-admins the same way.
//! - `GET /api/ai/config/server` stays admin-only (existing gate).
//!
//! One test per binary: `build_router("team")` and the AI registry are
//! process globals, so the role matrix is exercised sequentially in a single
//! test function.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const URL: &str = "https://internal-ai-proxy.example.invalid/v1";
const MODEL: &str = "journey-model";

async fn json_body(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Seed one account per role plus a session token for each.
async fn seed_role_sessions() {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    for role in ["admin", "user", "viewer"] {
        sqlx::query(
            "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
             VALUES (?, ?, ?, 'password', ?)",
        )
        .bind(format!("web15-{role}-id"))
        .bind(format!("web15-{role}"))
        .bind(role)
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
             VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
        )
        .bind(format!("web15-{role}-token"))
        .bind(format!("web15-{role}-id"))
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn get(app: &axum::Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(path);
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
async fn provider_metadata_is_role_gated() {
    // A developer shell may export the provider pin; this test owns its
    // process environment (one test per binary), so clear it before the AI
    // registry initializes. HOME is redirected so the config persistence
    // cannot rewrite the operator's real `~/.oxo-flow/ai_config.json`.
    // SAFETY: single-threaded test setup in a dedicated test binary; no
    // other thread reads these variables before they are changed.
    let home =
        std::env::temp_dir().join(format!("oxo-flow-web15-role-gate-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    unsafe {
        std::env::remove_var("OXO_FLOW_AI_PROVIDER");
        std::env::remove_var("OXO_FLOW_AI_MODEL");
        std::env::remove_var("OXO_FLOW_AI_API_URL");
        std::env::remove_var("OXO_FLOW_AI_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("CLAUDE_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OLLAMA_HOST");
        std::env::set_var("HOME", &home);
    }

    common::ensure_db().await;
    seed_role_sessions().await;
    oxo_flow_web::ai_provider::AiProviderRegistry::global()
        .reconfigure(
            "openai",
            Some("sk-web15-test".to_string()),
            Some(URL.to_string()),
            Some(MODEL.to_string()),
        )
        .expect("test provider configures");
    // The server TIER must exist as a DB row: `tiers.server_*` is read from
    // the `user_id IS NULL` row, never from the active runtime (Task 7b/I2).
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, NULL, 'openai', 'sk-web15-test', ?, ?, 1, 1, 0, 3, \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(URL)
    .bind(MODEL)
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();

    let app = server::build_router("team");
    let admin = Some("web15-admin-token");
    let user = Some("web15-user-token");
    let viewer = Some("web15-viewer-token");

    // ── GET /api/ai/config/effective (authenticated surface) ──────────────
    // Unauthenticated callers are rejected by the existing auth gate.
    let (status, body) = get(&app, "/api/ai/config/effective", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "anonymous callers must not reach provider metadata: {body}"
    );

    // Admin sees the full resolved configuration.
    let (status, body) = get(&app, "/api/ai/config/effective", admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effective"]["provider"], "openai", "{body}");
    assert_eq!(body["effective"]["model"], MODEL, "{body}");
    assert_eq!(body["effective"]["api_url"], URL, "{body}");
    assert_eq!(body["tiers"]["server_model"], MODEL, "{body}");
    assert_eq!(body["tiers"]["env_url"], Value::Null, "{body}");
    assert_eq!(
        body["tiers"]["server_provider"], "openai",
        "admin keeps the full resolution chain: {body}"
    );

    // Every non-admin role gets the redacted shape.
    for (token, role) in [(user, "user"), (viewer, "viewer")] {
        let (status, body) = get(&app, "/api/ai/config/effective", token).await;
        assert_eq!(status, StatusCode::OK, "{role}: {body}");
        assert_eq!(
            body["effective"]["provider"], "openai",
            "{role} keeps the provider name: {body}"
        );
        assert_eq!(
            body["effective"]["is_configured"], true,
            "{role} keeps the enabled state: {body}"
        );
        assert_eq!(
            body["effective"]["model"],
            Value::Null,
            "{role} must not learn the model id: {body}"
        );
        assert_eq!(
            body["effective"]["api_url"],
            Value::Null,
            "{role} must not learn the endpoint URL: {body}"
        );
        for tier in [
            "env_model",
            "env_url",
            "server_model",
            "user_model",
            "user_api_url",
        ] {
            assert_eq!(
                body["tiers"][tier],
                Value::Null,
                "{role} must not see tiers.{tier}: {body}"
            );
        }
        assert_eq!(
            body["tiers"]["server_provider"], "openai",
            "{role} may see the provider name: {body}"
        );
    }

    // ── GET /api/ai/config (public discoverability surface) ───────────────
    let (status, body) = get(&app, "/api/ai/config", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"], "", "anonymous stays fully masked: {body}");
    assert_eq!(body["is_configured"], true, "{body}");

    for (token, role) in [(user, "user"), (viewer, "viewer")] {
        let (status, body) = get(&app, "/api/ai/config", token).await;
        assert_eq!(status, StatusCode::OK, "{role}: {body}");
        assert_eq!(
            body["provider"], "",
            "{role} gets no provider metadata from the public route: {body}"
        );
        assert_eq!(body["model"], Value::Null, "{role}: {body}");
        assert_eq!(body["api_url"], Value::Null, "{role}: {body}");
        assert_eq!(body["is_configured"], true, "{role}: {body}");
    }

    let (status, body) = get(&app, "/api/ai/config", admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"], "openai", "{body}");
    assert_eq!(body["model"], MODEL, "{body}");
    assert_eq!(body["api_url"], URL, "{body}");

    // ── GET /api/ai/config/user (mirrored shared runtime) ─────────────────
    // No per-user row exists: the fallback mirrors the shared runtime.
    let (status, body) = get(&app, "/api/ai/config/user", user).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["user_config"]["provider"], "openai",
        "the provider name stays visible: {body}"
    );
    assert_eq!(
        body["user_config"]["api_url"],
        Value::Null,
        "a non-admin must not see the shared endpoint: {body}"
    );
    assert_eq!(
        body["user_config"]["model"],
        Value::Null,
        "a non-admin must not see the shared model id: {body}"
    );
    assert_eq!(body["configured"], true, "{body}");

    let (status, body) = get(&app, "/api/ai/config/user", admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["user_config"]["api_url"], URL,
        "an admin sees the shared fallback in full: {body}"
    );
    assert_eq!(body["user_config"]["model"], MODEL, "{body}");

    // Own-row branch: a non-admin's SAVED row is their own data and is
    // returned in full — the redaction above applies only to the mirrored
    // SHARED runtime. PUT is allowed for the acting user (not admin-gated);
    // outside personal mode a non-admin save persists but is not applied to
    // the shared runtime.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/ai/config/user")
                .header("authorization", "Bearer web15-user-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "provider": "deepseek",
                        "api_url": "https://user-own.example.invalid/v1",
                        "model": "user-own-model",
                        "api_key": "sk-user-own",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a non-admin may save their own user-tier config"
    );
    let saved: Value = json_body(resp.into_body()).await;
    assert_eq!(
        saved["applied_to_runtime"], false,
        "a non-admin save must not reconfigure the shared runtime: {saved}"
    );

    let (status, body) = get(&app, "/api/ai/config/user", user).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["user_config"]["provider"], "deepseek",
        "the caller's own saved row is visible: {body}"
    );
    assert_eq!(
        body["user_config"]["api_url"], "https://user-own.example.invalid/v1",
        "the caller's own endpoint is their own data: {body}"
    );
    assert_eq!(
        body["user_config"]["model"], "user-own-model",
        "the caller's own model id is their own data: {body}"
    );
    assert_eq!(
        body["user_config"]["api_key_set"], true,
        "the key is reported as set, never echoed: {body}"
    );

    // ── GET /api/ai/config/server (existing admin-only gate) ──────────────
    for (token, role) in [(user, "user"), (viewer, "viewer")] {
        let (status, body) = get(&app, "/api/ai/config/server", token).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{role} must not read the server config: {body}"
        );
        assert_eq!(body["code"], "ACCESS_DENIED", "{role}: {body}");
    }
    // The shared server row exists (seeded above); the point here is the
    // gate, not the row shape.
    let (status, body) = get(&app, "/api/ai/config/server", admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.get("server_config").is_some(),
        "the admin payload carries the server tier key: {body}"
    );
    assert_eq!(body["server_config"]["provider"], "openai", "{body}");
    assert_eq!(body["disabled_by_env"], false, "{body}");
}
