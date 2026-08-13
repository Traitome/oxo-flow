//! AI provider abstraction layer (compatibility shim over oxo-flow-ai).
//!
//! This module delegates to `oxo_flow_ai` while preserving the original
//! `AiProviderRegistry` API for backward compatibility.

use oxo_flow_ai::config::AutoFixMode;

// Re-export types unchanged
pub use oxo_flow_ai::provider::{
    AiProvider, ProviderConfig, ProviderKind, ai_config_path, create_provider,
    create_provider_from_env, save_ai_config,
};

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
