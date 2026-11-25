//! `GET /api/ai/config/effective` tier attribution (Task 7b / I2).
//!
//! The endpoint used to fill `tiers.server_*` from the ACTIVE RUNTIME (which
//! includes env and per-user overrides) and read only `OXO_FLOW_AI_MODEL`
//! for the env tier. This binary pins the corrected contract: the server
//! tier comes from the `user_id IS NULL` DB row, the env tier from the same
//! provider-specific environment resolution the runtime uses, and the
//! effective tier from the active backend.
//!
//! One test in a dedicated binary: env mutation is process-global and the
//! storage backend / AI registry are process-wide.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request_body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(request_body).unwrap())
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
async fn effective_config_reports_each_tier_from_its_own_source() {
    common::ensure_db().await;

    // Deterministic environment: the test process owns it (dedicated
    // binary). Redirect HOME so the AI config writer never touches the
    // operator's real config, and clear every provider variable before
    // installing the OpenAI-specific tier under test.
    let home =
        std::env::temp_dir().join(format!("oxo-flow-effective-tiers-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temporary HOME");
    // SAFETY: single test in a dedicated test binary; no other thread reads
    // these variables concurrently.
    unsafe {
        std::env::set_var("HOME", &home);
        for name in [
            "OXO_FLOW_AI_PROVIDER",
            "OXO_FLOW_AI_MODEL",
            "OXO_FLOW_AI_API_URL",
            "OXO_FLOW_AI_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_API_KEY",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_BASE_URL",
            "OPENAI_API_KEY",
            "OPENAI_MODEL",
            "OPENAI_BASE_URL",
            "DEEPSEEK_API_KEY",
            "DEEPSEEK_BASE_URL",
            "OLLAMA_HOST",
            "OLLAMA_MODEL",
        ] {
            std::env::remove_var(name);
        }
        std::env::set_var("OPENAI_API_KEY", "sk-env-tier-test");
        std::env::set_var("OPENAI_MODEL", "env-specific-model");
        std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    }

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(pool)
        .await
        .unwrap();

    let app = server::build_router("personal");

    // (a) Env-only, no server row: env tier is populated from the
    // provider-specific variables; the server tier is genuinely absent.
    let (status, effective) = request(&app, "GET", "/api/ai/config/effective", None).await;
    assert_eq!(status, StatusCode::OK, "{effective}");
    assert_eq!(effective["tiers"]["env_provider"], "openai", "{effective}");
    assert_eq!(
        effective["tiers"]["env_model"], "env-specific-model",
        "{effective}"
    );
    assert_eq!(
        effective["tiers"]["env_url"], "https://env.example/v1",
        "{effective}"
    );
    assert!(
        effective["tiers"]["server_provider"].is_null(),
        "no server row must report a null server tier: {effective}"
    );
    assert!(effective["tiers"]["server_model"].is_null(), "{effective}");
    assert!(effective["tiers"]["user_provider"].is_null(), "{effective}");

    // (b) Server row + a DIVERGENT user row. The server tier must come from
    // the server row, never from the runtime (which the user row now owns).
    let (status, saved) = request(
        &app,
        "PUT",
        "/api/ai/config/server",
        Some(json!({
            "provider": "deepseek",
            "api_key": "sk-server-tier",
            "api_url": "https://server.example/v1",
            "model": "server-model",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, saved) = request(
        &app,
        "PUT",
        "/api/ai/config/user",
        Some(json!({
            "provider": "openai",
            "api_key": "sk-user-tier",
            "api_url": "https://user.example/v1",
            "model": "user-model",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (status, effective) = request(&app, "GET", "/api/ai/config/effective", None).await;
    assert_eq!(status, StatusCode::OK, "{effective}");
    assert_eq!(
        effective["tiers"]["server_provider"], "deepseek",
        "the server tier must reflect the server row: {effective}"
    );
    assert_eq!(
        effective["tiers"]["server_model"], "server-model",
        "{effective}"
    );
    assert_eq!(
        effective["tiers"]["server_api_url"], "https://server.example/v1",
        "{effective}"
    );
    assert_eq!(effective["tiers"]["user_provider"], "openai", "{effective}");
    assert_eq!(
        effective["tiers"]["user_model"], "user-model",
        "{effective}"
    );
    assert_eq!(
        effective["tiers"]["user_api_url"], "https://user.example/v1",
        "{effective}"
    );
    assert_eq!(
        effective["effective"]["model"], "user-model",
        "the effective tier is the active runtime (the admin user row applied): {effective}"
    );

    // (c) Generic OXO_FLOW_AI_MODEL outranks the provider-specific model in
    // the runtime factory, so the env tier must report the same value.
    // SAFETY: same single-threaded test process.
    unsafe { std::env::set_var("OXO_FLOW_AI_MODEL", "env-generic-model") };
    let (status, effective) = request(&app, "GET", "/api/ai/config/effective", None).await;
    assert_eq!(status, StatusCode::OK, "{effective}");
    assert_eq!(
        effective["tiers"]["env_model"], "env-generic-model",
        "{effective}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
