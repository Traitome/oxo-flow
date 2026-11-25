//! Admin/personal save survives a restart AND wins over a pre-existing
//! private row (Task 7b / I1, review C1).
//!
//! The old SPA wrote only the personal pseudo-user (`"default"`) row, and
//! both `provider_for()` and `restore_ai_config_from_db()` prefer that row
//! over the server tier. The fixed primary Save must therefore keep the
//! effective tier in sync: this binary seeds exactly that stale row first,
//! then verifies the durable server save, the pseudo-user mirror, the
//! per-call provider, a blank-key re-save, and the startup restore.
//!
//! One test in a dedicated binary: env mutation and the AI registry are
//! process-global.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

async fn put_server_config(app: &axum::Router, body: Value) -> (StatusCode, Value) {
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

#[tokio::test]
async fn server_save_updates_a_pre_existing_personal_row_and_survives_restart() {
    common::ensure_db().await;

    let home = std::env::temp_dir().join(format!(
        "oxo-flow-restart-persistence-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&home).expect("temporary HOME");
    // SAFETY: single test in a dedicated test binary; no other thread reads
    // these variables concurrently. An env provider would outrank the DB
    // tier on restart (and `restore_ai_config_from_db` deliberately skips).
    unsafe {
        std::env::set_var("HOME", &home);
        for name in [
            "OXO_FLOW_AI_PROVIDER",
            "OXO_FLOW_AI_MODEL",
            "OXO_FLOW_AI_API_URL",
            "OXO_FLOW_AI_API_KEY",
            "OXO_FLOW_MASTER_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_API_KEY",
            "OPENAI_API_KEY",
            "DEEPSEEK_API_KEY",
            "OLLAMA_HOST",
        ] {
            std::env::remove_var(name);
        }
    }

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(pool)
        .await
        .unwrap();
    // The normal upgrade state: the pre-fix SPA wrote a private row for the
    // personal pseudo-user. Its advanced toggle (search disabled) must
    // survive the primary Save.
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, 'default', 'openai', 'sk-stale-key', 'https://old.example/v1', \
                 'stale-model', 0, 1, 0, 2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(pool)
    .await
    .unwrap();

    let app = server::build_router("personal");
    let (status, saved) = put_server_config(
        &app,
        json!({
            "provider": "deepseek",
            "api_key": "sk-new-key",
            "api_url": "https://api.deepseek.example/v1",
            "model": "deepseek-new-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["provider"], "deepseek", "{saved}");
    assert_eq!(saved["applied_to_runtime"], true, "{saved}");
    assert_eq!(saved["disabled_by_env"], false, "{saved}");

    // Both tiers now carry the saved values. The private row keeps its
    // advanced toggle settings; the fresh server row gets the defaults.
    for user_id in [None::<&str>, Some("default")] {
        let expected_search = if user_id.is_none() { 1 } else { 0 };
        let row: (String, String, String, String, i64) = match user_id {
            None => sqlx::query_as(
                "SELECT provider, api_key, api_url, model, search_enabled \
                 FROM ai_provider_config WHERE user_id IS NULL",
            ),
            Some(user_id) => sqlx::query_as(
                "SELECT provider, api_key, api_url, model, search_enabled \
                 FROM ai_provider_config WHERE user_id = ?",
            )
            .bind(user_id),
        }
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(row.0, "deepseek", "tier {user_id:?}");
        assert_eq!(row.1, "sk-new-key", "tier {user_id:?} keeps the new key");
        assert_eq!(row.2, "https://api.deepseek.example/v1", "tier {user_id:?}");
        assert_eq!(row.3, "deepseek-new-model", "tier {user_id:?}");
        assert_eq!(
            row.4, expected_search,
            "tier {user_id:?} advanced-toggle expectation"
        );
    }

    // Per-call resolution uses the new value, not the stale private row.
    let provider = oxo_flow_web::ai_provider::provider_for("default").await;
    assert_eq!(provider.name(), "deepseek");
    assert_eq!(provider.model().as_deref(), Some("deepseek-new-model"));

    // A blank-key re-save keeps the stored key (documented contract) while
    // applying the new model; the save returns the stored row, so activation
    // uses the persisted key.
    let (status, saved) = put_server_config(
        &app,
        json!({
            "provider": "deepseek",
            "api_url": "https://api.deepseek.example/v1",
            "model": "deepseek-v2-model",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    for user_id in [None::<&str>, Some("default")] {
        let (key, model): (String, String) = match user_id {
            None => sqlx::query_as(
                "SELECT api_key, model FROM ai_provider_config WHERE user_id IS NULL",
            ),
            Some(user_id) => {
                sqlx::query_as("SELECT api_key, model FROM ai_provider_config WHERE user_id = ?")
                    .bind(user_id)
            }
        }
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(
            key, "sk-new-key",
            "blank save must keep the key: {user_id:?}"
        );
        assert_eq!(model, "deepseek-v2-model", "tier {user_id:?}");
    }
    let provider = oxo_flow_web::ai_provider::provider_for("default").await;
    assert_eq!(provider.model().as_deref(), Some("deepseek-v2-model"));

    // Simulate a restart: the registry loses its runtime state, then the
    // startup restore runs. The mirrored private row must restore the new
    // values (the stale row would otherwise win).
    oxo_flow_ai::AI
        .disable()
        .expect("reset the runtime registry");
    assert!(
        !oxo_flow_web::ai_provider::AiProviderRegistry::global()
            .get_config()
            .is_configured,
        "precondition: the simulated restart starts unconfigured"
    );
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;

    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(restored.provider, "deepseek", "restored provider");
    assert_eq!(
        restored.model.as_deref(),
        Some("deepseek-v2-model"),
        "restored model"
    );
    assert_eq!(
        restored.api_url.as_deref(),
        Some("https://api.deepseek.example/v1"),
        "restored endpoint"
    );
    assert!(restored.is_configured, "restored provider is configured");

    let _ = std::fs::remove_dir_all(&home);
}
