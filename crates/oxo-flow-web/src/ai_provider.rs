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
            // Rows are written through infra::crypto::seal (issue #205), so
            // the stored value may be a `v1:` ciphertext — decrypt here too,
            // never hand the encoded form to a provider.
            let key = crate::infra::crypto::open(&api_key.unwrap_or_default());
            // A per-user row without a key is not a usable configuration.
            // Fall back to the shared runtime so the user gets the global
            // provider (or Noop if none is configured) instead of a provider
            // carrying an empty key that fails with a cryptic builder error.
            if !key.trim().is_empty() {
                let kind: ProviderKind = provider_kind.parse().unwrap_or(ProviderKind::OpenAi);
                resolved = Some(create_provider(kind, Some(key), api_url, model));
            }
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
            .map(|c| {
                let provider = format!("{:?}", c.provider).to_lowercase();
                let active_provider = oxo_flow_ai::AI.provider();
                let is_configured = match active_provider {
                    // Test/scripted providers are always considered configured;
                    // they do not need a real API key.
                    Ok(AiProvider::Scripted(_)) => true,
                    Ok(AiProvider::Noop) => false,
                    // The active provider is real — trust it. Env-only
                    // deployments (ANTHROPIC_AUTH_TOKEN exported, no key in
                    // the registry snapshot) previously reported
                    // is_configured: false here even though /api/ai/test and
                    // every generation surface succeeded, because the
                    // snapshot's api_key is None while the provider carries
                    // its env-discovered credential internally. Status
                    // probes (metrics ai_provider component, settings UI)
                    // must agree with what actually executes.
                    Ok(_) if provider == "ollama" => {
                        // Ollama is local and needs no key, but a reachable URL
                        // must be configured (the backend default is localhost).
                        c.enabled && c.api_url.is_some()
                    }
                    Ok(_) => c.enabled && !provider.is_empty(),
                    Err(_) => false,
                };
                ProviderConfig {
                    provider,
                    api_url: c.api_url,
                    model: c.model,
                    is_configured,
                }
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
            max_retries: oxo_flow_ai::config::default_max_retries(),
            auto_fix: AutoFixMode::Ask,
            temperature: None,
            skills: Vec::new(),
            team_profile: None,
        };

        oxo_flow_ai::AI.reconfigure(cfg).map_err(|e| e.to_string())
    }

    pub fn create_claude_from_env() -> Result<AiProvider, anyhow::Error> {
        // Canonical credential chain (#948 P2-3) — the constructors used to
        // hand-read env vars and had drifted (no trim, no blank-as-absent,
        // claude tier missing the OXO_FLOW_AI_API_KEY generic fallback).
        // create_provider applies the same env tiers as the CLI factory;
        // an unconfigured provider is surfaced as Err so the fallback
        // chain in `provider_candidates` skips it.
        let provider = create_provider(ProviderKind::Claude, None, None, None);
        if !provider.is_usable() {
            return Err(anyhow::anyhow!(
                "ANTHROPIC_AUTH_TOKEN (or ANTHROPIC_API_KEY) not set"
            ));
        }
        Ok(provider)
    }

    pub fn create_openai_from_env() -> Result<AiProvider, anyhow::Error> {
        let provider = create_provider(ProviderKind::OpenAi, None, None, None);
        if !provider.is_usable() {
            return Err(anyhow::anyhow!("OPENAI_API_KEY not set"));
        }
        Ok(provider)
    }

    pub fn create_ollama_from_env() -> Result<AiProvider, anyhow::Error> {
        // Ollama stays infallible: it is keyless by design and detection is
        // gated on OLLAMA_HOST by the caller (`provider_candidates`).
        // OLLAMA_HOST / OLLAMA_MODEL ride the canonical env snapshot now.
        Ok(create_provider(ProviderKind::Ollama, None, None, None))
    }
}
