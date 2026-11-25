//! Keyless-provider restore and the single key-usability predicate
//! (Task 7b / round-3 review M2 + M4).
//!
//! `restore_ai_config_from_db` must restore a saved keyless `ollama` or
//! `disabled` row even when it carries a residual key (a stale other-vendor
//! credential or an unreadable ciphertext), because those providers ignore
//! keys. A key-requiring provider must only restore when the stored key is
//! USABLE under the same predicate the save path uses — a `v1:` ciphertext of
//! an empty key is not usable text.

mod common;

async fn clear_rows() {
    sqlx::query("DELETE FROM ai_provider_config")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
}

async fn seed_server_row(provider: &str, api_key: &str, api_url: &str, model: &str) {
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
         search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, \
         created_at, updated_at) \
         VALUES (?, NULL, ?, ?, ?, ?, 1, 1, 0, 3, '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(api_key)
    .bind(api_url)
    .bind(model)
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn keyless_rows_restore_without_a_key_and_keyed_rows_require_a_usable_one() {
    common::ensure_db().await;
    let home =
        std::env::temp_dir().join(format!("oxo-flow-keyless-restore-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temporary HOME");
    // SAFETY: one test in a dedicated binary; the env provider would outrank
    // the DB tier, and a master key would turn the `v1:` residual rows into
    // readable/non-readable states other than the one under test.
    unsafe {
        std::env::set_var("HOME", &home);
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

    // A saved keyless Ollama shared tier restores as configured.
    seed_server_row("ollama", "", "http://ollama.example:11434", "llama3").await;
    oxo_flow_ai::AI.disable().expect("reset the registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(restored.provider, "ollama", "keyless ollama restores");
    assert_eq!(restored.model.as_deref(), Some("llama3"));
    assert_eq!(
        restored.api_url.as_deref(),
        Some("http://ollama.example:11434")
    );
    assert!(restored.is_configured, "keyless ollama is usable");

    // A residual UNREADABLE key is ignored for a keyless provider instead of
    // skipping the row (Ollama never uses a key).
    clear_rows().await;
    seed_server_row(
        "ollama",
        "v1:bm90LXRoZS1rZXk6bm90LXRoZS1rZXk",
        "http://ollama.example:11434",
        "llama3",
    )
    .await;
    oxo_flow_ai::AI.disable().expect("reset the registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(
        restored.provider, "ollama",
        "a residual unreadable key must not block a keyless provider"
    );
    assert!(restored.is_configured);

    // A saved disabled row restores as disabled (no provider).
    clear_rows().await;
    seed_server_row("disabled", "", "", "").await;
    oxo_flow_ai::AI.disable().expect("reset the registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(restored.provider, "disabled");
    assert!(!restored.is_configured);

    // A key-requiring provider with an unreadable/non-empty ciphertext is
    // skipped (the status must not claim a working provider).
    clear_rows().await;
    seed_server_row(
        "deepseek",
        "v1:bm90LXRoZS1rZXk6bm90LXRoZS1rZXk",
        "https://api.deepseek.example/v1",
        "deepseek-chat",
    )
    .await;
    oxo_flow_ai::AI.disable().expect("reset the registry");
    oxo_flow_web::domains::ai::handlers::restore_ai_config_from_db().await;
    let restored = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(
        restored.provider, "disabled",
        "an unusable key must not register a key-requiring provider"
    );
    assert!(!restored.is_configured);

    let _ = std::fs::remove_dir_all(&home);
}
