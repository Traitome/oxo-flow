//! oxo-flow-ai — AI provider abstraction, agent framework, and knowledge system.
//!
//! This crate is the shared AI foundation for all oxo-flow tools:
//! CLI, web server, IDE plugins, and third-party integrations.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  AiRegistry (global singleton)          │
//! │  AI::init() → AI::is_enabled() gate     │
//! ├─────────────────────────────────────────┤
//! │  provider  │  config  │  session        │
//! │  multi-    │  chain   │  audit +        │
//! │  backend   │  resolve │  archive        │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Quick start
//!
//! ```rust,ignore
//! // Initialize at startup
//! oxo_flow_ai::AI.init(None).ok();
//!
//! // Gate: skip AI if not configured
//! if oxo_flow_ai::AI.is_enabled() {
//!     let provider = oxo_flow_ai::AI.provider().unwrap();
//!     // let response = provider.chat("You are helpful.", "Hello!").await?;
//! }
//! ```

pub mod agent;
pub mod config;
pub mod error;
pub mod knowledge;
pub mod mcp;
pub mod plugin;
pub mod provider;
pub mod session;
pub mod skill;
pub mod tools;
pub mod types;

use std::sync::RwLock;

use config::AiConfig;
use error::AiError;
use provider::{AiProvider, create_provider};
use session::AiSession;

// ── Global registry ────────────────────────────────────────────────────────

/// Global AI registry — initialized once at process startup.
///
/// # Example
///
/// ```rust,ignore
/// oxo_flow_ai::AI.init(None).ok();
/// ```
pub struct AiRegistry {
    provider: RwLock<Option<AiProvider>>,
    config: RwLock<AiConfig>,
}

impl Default for AiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AiRegistry {
    pub fn new() -> Self {
        Self {
            provider: RwLock::new(None),
            config: RwLock::new(AiConfig::default()),
        }
    }

    /// Initialize the registry from environment variables + config file.
    ///
    /// Call once at process startup. If `overrides` is provided, it
    /// takes highest priority.
    pub fn init(&self, overrides: Option<AiConfig>) -> Result<(), AiError> {
        // Start with env-based config
        let mut config = AiConfig::from_env();

        // Apply explicit overrides
        if let Some(overrides) = overrides {
            config.merge(&overrides);
        }

        // Create the provider
        let provider = if config.enabled {
            create_provider(
                config.provider,
                config.api_key.clone(),
                config.api_url.clone(),
                config.model.clone(),
            )
        } else {
            AiProvider::Noop
        };

        let name = provider.name().to_string();
        let model = provider.model();

        // Store in the global
        if let Ok(mut p) = AI.provider.write() {
            *p = Some(provider);
        }
        if let Ok(mut c) = AI.config.write() {
            *c = config;
        }

        tracing::info!(
            "AI registry initialized: provider={name}, model={}, enabled={}",
            model.as_deref().unwrap_or("default"),
            AI.is_enabled()
        );
        Ok(())
    }

    /// Check if AI features are enabled.
    ///
    /// Returns `false` if AI is disabled or not configured. All AI code
    /// paths should check this before making any calls.
    pub fn is_enabled(&self) -> bool {
        self.config.read().map(|c| c.enabled).unwrap_or(false)
    }

    /// Get the current AI provider instance.
    ///
    /// Returns an error if AI is not configured.
    pub fn provider(&self) -> Result<AiProvider, AiError> {
        self.provider
            .read()
            .map_err(|_| AiError::Config {
                message: "AI registry lock poisoned".into(),
            })?
            .clone()
            .ok_or(AiError::NotConfigured)
    }

    /// Get the current configuration snapshot.
    pub fn config(&self) -> Result<AiConfig, AiError> {
        self.config
            .read()
            .map(|c| c.clone())
            .map_err(|_| AiError::Config {
                message: "AI registry lock poisoned".into(),
            })
    }

    /// Reconfigure the provider at runtime.
    ///
    /// Useful for web-based settings pages that persist AI config.
    pub fn reconfigure(&self, config: AiConfig) -> Result<(), AiError> {
        let provider = if config.enabled {
            create_provider(
                config.provider,
                config.api_key.clone(),
                config.api_url.clone(),
                config.model.clone(),
            )
        } else {
            AiProvider::Noop
        };

        if let Ok(mut p) = self.provider.write() {
            *p = Some(provider);
        }
        if let Ok(mut c) = self.config.write() {
            *c = config;
        }

        // Persist to disk for survival across restarts
        if let Ok(cfg) = self.config() {
            provider::save_ai_config(
                &format!("{:?}", cfg.provider).to_lowercase(),
                None, // Don't persist API key when reconfiguring at runtime
                cfg.api_url.as_deref(),
                cfg.model.as_deref(),
            );
        }

        Ok(())
    }

    /// Create a new session for tracking an AI interaction.
    pub fn new_session(&self, command: &str, user_intent: &str) -> AiSession {
        let provider = self.provider().unwrap_or(AiProvider::Noop);
        AiSession::new(
            command,
            user_intent,
            provider.name(),
            &provider.model().unwrap_or_default(),
        )
    }
}

/// Global AI registry singleton.
pub static AI: std::sync::LazyLock<AiRegistry> = std::sync::LazyLock::new(AiRegistry::new);

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_default_is_disabled() {
        let reg = AiRegistry::new();
        assert!(!reg.is_enabled());
    }

    #[test]
    fn registry_default_provider_is_noop() {
        let reg = AiRegistry::new();
        let provider = reg.provider().unwrap_or(AiProvider::Noop);
        assert_eq!(provider.name(), "disabled");
    }

    #[test]
    fn global_is_disabled_by_default() {
        assert!(!AI.is_enabled());
    }

    #[test]
    fn new_session_has_command() {
        let reg = AiRegistry::new();
        let session = reg.new_session("template", "RNA-seq");
        assert_eq!(session.command, "template");
        assert_eq!(session.user_intent, "RNA-seq");
    }
}
