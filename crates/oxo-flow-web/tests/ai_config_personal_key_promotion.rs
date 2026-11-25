//! Personal-mode blank-key save promotes the operator's stored key
//! (Task 7b / I1, review round-2 Important 1).
//!
//! Personal mode has one pseudo-user (`"default"`). A fresh shared-tier save
//! with a blank key must not persist a keyless row (startup restore would
//! skip it): it promotes that user's stored key when the provider matches,
//! and otherwise rejects with `AI_KEY_REQUIRED`. This binary pins both.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

async fn put_server(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/ai/config/server")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
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

async fn seed_default_row(provider: &str, api_key: &str, model: &str) {
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, 'default', ?, ?, 'https://personal.example/v1', ?, 0, 1, 0, 2, \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(api_key)
    .bind(model)
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
}

async fn seed_server_row(provider: &str, api_key: &str, model: &str) {
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, NULL, ?, ?, 'https://shared.example/v1', ?, 1, 1, 0, 3, \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(api_key)
    .bind(model)
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
}

async fn clear_rows() {
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn personal_blank_key_promotes_a_matching_stored_key_and_never_leaves_a_keyless_row() {
    common::ensure_db().await;
    // SAFETY: one test in a dedicated binary; env provider/key/master-key
    // settings would outrank or alter the DB rows under test.
    unsafe {
        for name in [
            "OXO_FLOW_AI_PROVIDER",
            "OXO_FLOW_AI_API_KEY",
            "OXO_FLOW_MASTER_KEY",
            "OPENAI_API_KEY",
            "DEEPSEEK_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OLLAMA_HOST",
        ] {
            std::env::remove_var(name);
        }
    }
    clear_rows().await;
    let app = server::build_router("personal");

    // A fresh personal shared tier with no stored key and nothing to
    // promote is rejected, not persisted keyless.
    let (status, saved) = put_server(
        &app,
        json!({
            "provider": "deepseek",
            "api_url": "https://shared.example/v1",
            "model": "shared-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{saved}");
    assert_eq!(saved["code"], "AI_KEY_REQUIRED", "{saved}");
    let server_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_provider_config WHERE user_id IS NULL")
            .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
            .await
            .unwrap();
    assert_eq!(server_rows, 0, "no keyless shared row may be persisted");

    // A matching stored personal key is promoted into the shared tier; the
    // private row's advanced toggle survives the mirror.
    seed_default_row("deepseek", "sk-personal-key", "personal-old-model").await;
    let (status, saved) = put_server(
        &app,
        json!({
            "provider": "deepseek",
            "api_url": "https://shared.example/v1",
            "model": "shared-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["applied_to_runtime"], true, "{saved}");

    let server_row: (String, String, String, String) = sqlx::query_as(
        "SELECT provider, api_key, api_url, model FROM ai_provider_config \
         WHERE user_id IS NULL",
    )
    .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
    assert_eq!(
        server_row,
        (
            "deepseek".to_string(),
            "sk-personal-key".to_string(),
            "https://shared.example/v1".to_string(),
            "shared-model".to_string()
        ),
        "the personal key must be promoted into the shared tier"
    );
    let mirror: (String, String, String, i64) = sqlx::query_as(
        "SELECT provider, api_key, model, search_enabled FROM ai_provider_config \
         WHERE user_id = 'default'",
    )
    .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
    assert_eq!(mirror.0, "deepseek");
    assert_eq!(mirror.1, "sk-personal-key");
    assert_eq!(mirror.2, "shared-model");
    assert_eq!(mirror.3, 0, "the private advanced toggle is preserved");

    let provider = oxo_flow_web::ai_provider::provider_for("default").await;
    assert_eq!(provider.name(), "deepseek");
    assert_eq!(provider.model().as_deref(), Some("shared-model"));

    // Restart restores the promoted shared key.
    oxo_flow_ai::AI
        .disable()
        .expect("reset the runtime registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(restored.provider, "deepseek", "restored provider");
    assert_eq!(restored.model.as_deref(), Some("shared-model"));
    assert!(
        restored.is_configured,
        "promoted key restores as configured"
    );

    // A personal provider switch with an existing server key must not reuse
    // it (M1); switching to a keyless provider clears the old server key.
    clear_rows().await;
    seed_server_row("openai", "sk-openai-server", "gpt-4o").await;
    seed_default_row("openai", "sk-openai-personal", "gpt-4o").await;
    let (status, saved) = put_server(
        &app,
        json!({
            "provider": "claude",
            "api_url": "https://anthropic.example/v1",
            "model": "claude-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{saved}");
    assert_eq!(saved["code"], "AI_KEY_REQUIRED", "{saved}");
    let server_row: (String, String) =
        sqlx::query_as("SELECT provider, api_key FROM ai_provider_config WHERE user_id IS NULL")
            .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
            .await
            .unwrap();
    assert_eq!(
        server_row,
        ("openai".to_string(), "sk-openai-server".to_string()),
        "a blank-key provider switch must not reuse/rewrite the old server key"
    );
    let (status, saved) = put_server(
        &app,
        json!({
            "provider": "ollama",
            "api_url": "http://ollama.example:11434",
            "model": "llama3",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let server_row: (String, String) =
        sqlx::query_as("SELECT provider, api_key FROM ai_provider_config WHERE user_id IS NULL")
            .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
            .await
            .unwrap();
    assert_eq!(
        server_row,
        ("ollama".to_string(), String::new()),
        "switching to a keyless provider with a blank key clears the old key"
    );

    // A DIFFERENT provider's stored key is not promoted: the operator must
    // type the new provider's credential rather than send a foreign key.
    clear_rows().await;
    seed_default_row("openai", "sk-openai-key", "gpt-4o").await;
    let (status, saved) = put_server(
        &app,
        json!({
            "provider": "deepseek",
            "api_url": "https://shared.example/v1",
            "model": "shared-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{saved}");
    assert_eq!(saved["code"], "AI_KEY_REQUIRED", "{saved}");
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM ai_provider_config WHERE user_id IS NULL"
        )
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap()
            == 0,
        "a foreign-provider key must not create a keyless shared row"
    );
}
