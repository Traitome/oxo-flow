//! Team-admin primary Save keeps the shared AND private tiers in sync
//! (Task 7b / I1, review C1 + round-2 Important 1).
//!
//! In team mode an admin's own AI calls resolve their private DB row first
//! (`provider_for`), so a server-tier save that leaves a historical private
//! row untouched would keep using the old provider/key for that admin — and
//! a blank-key admin user PUT could reconfigure the whole runtime with the
//! stale private key. This binary pins the blank-key contract against real
//! handlers:
//!
//! - a fresh shared tier cannot be left keyless for a key-requiring provider
//!   (`400 AI_KEY_REQUIRED`; keyless Ollama is allowed);
//! - a blank-key save with an existing server key keeps that key, mirrors it
//!   into the acting admin's row (toggles preserved), so the admin's user PUT
//!   and the global runtime both use the server key;
//! - a simulated restart restores the shared tier.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const ADMIN_ID: &str = "task7b-team-admin";
const ADMIN_TOKEN: &str = "task7b-team-admin-token";

async fn seed_admin() {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, 'task7b-admin', 'admin', 'password', ?)",
    )
    .bind(ADMIN_ID)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
    )
    .bind(ADMIN_TOKEN)
    .bind(ADMIN_ID)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_row(
    user_id: Option<&str>,
    provider: &str,
    api_key: &str,
    api_url: &str,
    model: &str,
    search_enabled: i64,
) {
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 1, 0, 3, '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(provider)
    .bind(api_key)
    .bind(api_url)
    .bind(model)
    .bind(search_enabled)
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
}

async fn put(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header("authorization", format!("Bearer {ADMIN_TOKEN}"))
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

async fn row(user_id: Option<&str>) -> Option<(String, String, String, String, i64)> {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let query = match user_id {
        Some(_) => {
            "SELECT provider, api_key, api_url, model, search_enabled \
             FROM ai_provider_config WHERE user_id = ?"
        }
        None => {
            "SELECT provider, api_key, api_url, model, search_enabled \
             FROM ai_provider_config WHERE user_id IS NULL"
        }
    };
    let mut q = sqlx::query_as(query);
    if let Some(user_id) = user_id {
        q = q.bind(user_id);
    }
    q.fetch_optional(pool).await.unwrap()
}

#[tokio::test]
async fn team_admin_blank_key_save_never_leaves_a_keyless_shared_tier() {
    common::ensure_db().await;
    // SAFETY: one test in a dedicated binary; the pin/key env would outrank
    // the DB tiers. A master key would change the stored representation the
    // assertions compare, so it is cleared too.
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
    seed_admin().await;
    let app = server::build_router("team");

    // ── Fresh shared tier + seeded keyed private row, blank request key ──
    seed_row(
        Some(ADMIN_ID),
        "deepseek",
        "sk-private-stale",
        "https://private.example/v1",
        "private-model",
        0,
    )
    .await;
    let (status, saved) = put(
        &app,
        "/api/ai/config/server",
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
        saved["message"]
            .as_str()
            .is_some_and(|message| message.contains("API key")),
        "{saved}"
    );
    assert!(
        saved["detail"].is_string() && saved["suggestion"].is_string(),
        "structured error carries detail and a remedy: {saved}"
    );
    assert!(
        row(None).await.is_none(),
        "a rejected save must not persist a keyless shared row"
    );
    assert_eq!(
        row(Some(ADMIN_ID)).await.expect("private row").0,
        "deepseek",
        "the private row is untouched by the rejected save"
    );

    // Keyless Ollama is exempt from the key requirement.
    let (status, saved) = put(
        &app,
        "/api/ai/config/server",
        json!({
            "provider": "ollama",
            "api_url": "http://ollama.example:11434",
            "model": "llama3",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let server_row = row(None).await.expect("keyless ollama row");
    assert_eq!(server_row.0, "ollama");
    assert_eq!(server_row.1, "", "Ollama stays keyless");
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();

    // ── Provider switch + blank key: never reuse the old vendor's key ────
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    seed_row(
        None,
        "openai",
        "sk-openai-server-key",
        "https://openai.example/v1",
        "gpt-4o",
        1,
    )
    .await;
    seed_row(
        Some(ADMIN_ID),
        "openai",
        "sk-openai-private-key",
        "https://openai.example/v1",
        "gpt-4o",
        0,
    )
    .await;
    let (status, saved) = put(
        &app,
        "/api/ai/config/server",
        json!({
            "provider": "claude",
            "api_url": "https://anthropic.example/v1",
            "model": "claude-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{saved}");
    assert_eq!(saved["code"], "AI_KEY_REQUIRED", "{saved}");
    let server_row = row(None).await.expect("unchanged server row");
    assert_eq!(
        server_row,
        (
            "openai".to_string(),
            "sk-openai-server-key".to_string(),
            "https://openai.example/v1".to_string(),
            "gpt-4o".to_string(),
            1
        ),
        "a blank-key provider switch must not rewrite/reuse the old key"
    );
    // Supplying a key for the new provider replaces the old credential.
    let (status, saved) = put(
        &app,
        "/api/ai/config/server",
        json!({
            "provider": "claude",
            "api_key": "sk-claude-key",
            "api_url": "https://anthropic.example/v1",
            "model": "claude-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        row(None).await.expect("claude server row"),
        (
            "claude".to_string(),
            "sk-claude-key".to_string(),
            "https://anthropic.example/v1".to_string(),
            "claude-model".to_string(),
            1
        )
    );

    // ── Existing keyed server row + divergent private key, blank save ────
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    seed_row(
        None,
        "deepseek",
        "sk-server-key",
        "https://old-shared.example/v1",
        "old-shared-model",
        1,
    )
    .await;
    seed_row(
        Some(ADMIN_ID),
        "deepseek",
        "sk-private-stale",
        "https://private.example/v1",
        "private-model",
        0,
    )
    .await;
    let (status, saved) = put(
        &app,
        "/api/ai/config/server",
        json!({
            "provider": "deepseek",
            "api_url": "https://shared.example/v1",
            "model": "shared-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["applied_to_runtime"], true, "{saved}");

    // The blank key kept the server key, and the mirror wrote the server
    // key/provider/model into the admin's private row while preserving its
    // advanced toggle.
    let server_row = row(None).await.expect("server row");
    assert_eq!(
        server_row,
        (
            "deepseek".to_string(),
            "sk-server-key".to_string(),
            "https://shared.example/v1".to_string(),
            "shared-model".to_string(),
            1
        )
    );
    let private_row = row(Some(ADMIN_ID)).await.expect("admin row");
    assert_eq!(
        private_row,
        (
            "deepseek".to_string(),
            "sk-server-key".to_string(),
            "https://shared.example/v1".to_string(),
            "shared-model".to_string(),
            0
        ),
        "the admin private row must carry the server key, not the stale key"
    );

    // The SPA's second PUT sends the same blank key: it keeps the mirrored
    // server key (not the old private one) and the runtime/admin resolution
    // stay on the shared values.
    let (status, saved) = put(
        &app,
        "/api/ai/config/user",
        json!({
            "provider": "deepseek",
            "api_url": "https://shared.example/v1",
            "model": "shared-model",
            "search_enabled": false,
            "monitor_enabled": true,
            "auto_retry_enabled": false,
            "max_correction_rounds": 3,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["applied_to_runtime"], true, "{saved}");
    assert_eq!(
        row(Some(ADMIN_ID)).await.expect("admin row").1,
        "sk-server-key",
        "the blank user PUT keeps the mirrored server key"
    );
    let provider = oxo_flow_web::ai_provider::provider_for(ADMIN_ID).await;
    assert_eq!(provider.name(), "deepseek");
    assert_eq!(provider.model().as_deref(), Some("shared-model"));

    // Simulated restart: the shared tier is keyed, so restore uses it.
    oxo_flow_ai::AI
        .disable()
        .expect("reset the runtime registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(restored.provider, "deepseek", "restored provider");
    assert_eq!(restored.model.as_deref(), Some("shared-model"));
    assert_eq!(
        restored.api_url.as_deref(),
        Some("https://shared.example/v1")
    );
    assert!(restored.is_configured, "restored provider is configured");
}
