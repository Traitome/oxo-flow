//! AI provider abstraction layer (compatibility shim over oxo-flow-ai).
//!
//! This module delegates to `oxo_flow_ai` while preserving the original
//! `AiProviderRegistry` API for backward compatibility.

use oxo_flow_ai::config::AutoFixMode;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

// Re-export types unchanged
pub use oxo_flow_ai::provider::{
    AiProvider, ProviderConfig, ProviderKind, ai_config_path, create_provider,
    create_provider_from_env, save_ai_config,
};

/// A saved per-user AI row: (provider kind, api_url, model, api_key).
type UserAiRow = (String, Option<String>, Option<String>, Option<String>);

/// Per-user provider cache (issue #82 follow-up): non-admin users' saved
/// AI keys resolve to THEIR provider, never reconfiguring the shared
/// runtime. Entries are invalidated whenever a config write lands.
static PER_USER_PROVIDERS: LazyLock<Mutex<HashMap<String, AiProvider>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drop every cached per-user provider (server/env config changed).
pub fn invalidate_provider_cache() {
    if let Ok(mut map) = PER_USER_PROVIDERS.lock() {
        map.clear();
    }
}

/// Drop one user's cached provider (their row changed).
pub fn invalidate_provider_for(user_id: &str) {
    if let Ok(mut map) = PER_USER_PROVIDERS.lock() {
        map.remove(user_id);
    }
}

/// Resolve the provider a user's AI calls must use:
/// 1. their own saved row (provider != "disabled")
/// 2. the shared runtime (server row / env / default)
///
/// A saved row WITHOUT an api key yields a provider carrying an empty key
/// — the call fails loudly instead of silently borrowing the server's
/// key (isolation means no shared-secret leakage, in both directions).
pub async fn provider_for(user_id: &str) -> AiProvider {
    if let Some(cached) = PER_USER_PROVIDERS
        .lock()
        .ok()
        .and_then(|map| map.get(user_id).cloned())
    {
        return cached;
    }

    let mut resolved: Option<AiProvider> = None;
    if let Ok(pool) = crate::infra::db::sqlite::try_pool() {
        let row: Option<UserAiRow> = sqlx::query_as(
            "SELECT provider, api_url, model, api_key FROM ai_provider_config \
                 WHERE user_id = ? ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some((provider_kind, api_url, model, api_key)) = row
            && provider_kind != "disabled"
        {
            let kind: ProviderKind = provider_kind.parse().unwrap_or(ProviderKind::OpenAi);
            resolved = Some(create_provider(
                kind,
                Some(api_key.unwrap_or_default()),
                api_url,
                model,
            ));
        }
    }

    // Only per-user ROW hits are cached: the shared-runtime fallback is
    // read live on every call — the global provider can be swapped at
    // runtime (e.g. the scripted test backend reinstalling itself
    // between scenarios), and a cached fallback would serve the STALE
    // provider (chat_agent_integration caught exactly that: "script
    // exhausted" from a cached first install).
    if let Some(provider) = resolved {
        if let Ok(mut map) = PER_USER_PROVIDERS.lock() {
            map.insert(user_id.to_string(), provider.clone());
        }
        return provider;
    }
    AiProviderRegistry::global().get_provider()
}

/// Compatibility wrapper — delegates to `oxo_flow_ai::AiRegistry`.
///
/// All original methods are preserved with the same signatures.
pub struct AiProviderRegistry;

impl AiProviderRegistry {
    pub fn global() -> Self {
        Self
    }

    pub fn init_from_env(&self) {
        oxo_flow_ai::AI.init(None).ok();
    }

    pub fn get_provider(&self) -> AiProvider {
        oxo_flow_ai::AI.provider().unwrap_or(AiProvider::Noop)
    }

    pub fn get_config(&self) -> ProviderConfig {
        oxo_flow_ai::AI
            .config()
            .map(|c| ProviderConfig {
                provider: format!("{:?}", c.provider).to_lowercase(),
                api_url: c.api_url,
                model: c.model,
                is_configured: c.enabled,
            })
            .unwrap_or(ProviderConfig {
                provider: "disabled".to_string(),
                api_url: None,
                model: None,
                is_configured: false,
            })
    }

    pub fn reconfigure(
        &self,
        kind: &str,
        api_key: Option<String>,
        api_url: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        let kind_parsed: ProviderKind = kind.parse().map_err(|e: anyhow::Error| e.to_string())?;

        let cfg = oxo_flow_ai::config::AiConfig {
            enabled: true,
            provider: kind_parsed,
            model,
            api_key,
            api_url,
            max_retries: 3,
            auto_fix: AutoFixMode::Ask,
            temperature: None,
            skills: Vec::new(),
        };

        oxo_flow_ai::AI.reconfigure(cfg).map_err(|e| e.to_string())
    }

    pub fn create_claude_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("CLAUDE_API_KEY"))
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set"))?;
        let model = std::env::var("ANTHROPIC_MODEL").ok();
        let api_url = std::env::var("ANTHROPIC_BASE_URL").ok();
        Ok(create_provider(
            ProviderKind::Claude,
            Some(api_key),
            api_url,
            model,
        ))
    }

    pub fn create_openai_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY not set"))?;
        let model = std::env::var("OPENAI_MODEL").ok();
        let api_url = std::env::var("OPENAI_BASE_URL").ok();
        Ok(create_provider(
            ProviderKind::OpenAi,
            Some(api_key),
            api_url,
            model,
        ))
    }

    pub fn create_ollama_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_url = std::env::var("OLLAMA_HOST").ok();
        let model = std::env::var("OLLAMA_MODEL").ok();
        Ok(create_provider(ProviderKind::Ollama, None, api_url, model))
    }
}
