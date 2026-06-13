//! AI provider abstraction layer.
//!
//! Supports multiple backends: Anthropic Claude, OpenAI-compatible (DeepSeek,
//! Groq, Azure, etc.), and Ollama (local). Configuration is read from
//! environment variables at process start.
//!
//! # Environment Variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `OXO_FLOW_AI_PROVIDER` | `"disabled"` | `"claude"`, `"openai"`, `"ollama"`, or `"disabled"` |
//! | `OXO_FLOW_AI_API_KEY` | — | API key for Claude or OpenAI-compatible |
//! | `OXO_FLOW_AI_API_URL` | (provider default) | Custom API endpoint URL |
//! | `OXO_FLOW_AI_MODEL` | (provider default) | Model name override |

use anyhow::{anyhow, Result};

// ---------------------------------------------------------------------------
// Provider kind enum
// ---------------------------------------------------------------------------

/// Available AI provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    Claude,
    OpenAi,
    Ollama,
}

impl std::str::FromStr for AiProviderKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "openai" | "open-ai" => Ok(Self::OpenAi),
            "ollama" => Ok(Self::Ollama),
            _ => Err(anyhow!(
                "Unknown AI provider '{s}'. Use 'claude', 'openai', or 'ollama'"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Provider defaults
// ---------------------------------------------------------------------------

const CLAUDE_DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_DEFAULT_MODEL: &str = "gpt-4o";
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const OLLAMA_DEFAULT_MODEL: &str = "llama3";
const OLLAMA_API_URL: &str = "http://localhost:11434/api/chat";

// ---------------------------------------------------------------------------
// Internal provider implementations (private)
// ---------------------------------------------------------------------------

mod internal {
    use super::*;
    use anyhow::{anyhow, Result};

    pub struct Claude {
        client: reqwest::Client,
        api_key: String,
        model: String,
        api_url: String,
    }

    impl Claude {
        pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_key,
                model: model.unwrap_or_else(|| CLAUDE_DEFAULT_MODEL.to_string()),
                api_url: api_url.unwrap_or_else(|| CLAUDE_API_URL.to_string()),
            }
        }

        pub fn name(&self) -> &str {
            "claude"
        }

        pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
            let body = serde_json::json!({
                "model": self.model,
                "system": system,
                "messages": [{"role": "user", "content": user}],
                "max_tokens": 4096,
            });
            let resp = self
                .client
                .post(&self.api_url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("Claude API request failed: {e}"))?;
            let status = resp.status();
            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow!("Claude API response parse failed: {e}"))?;
            if !status.is_success() {
                let err_msg = json
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown");
                return Err(anyhow!("Claude API error ({status}): {err_msg}"));
            }
            let text = json["content"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|b| b["text"].as_str())
                .ok_or_else(|| anyhow!("Claude unexpected response format"))?;
            Ok(text.to_string())
        }
    }

    pub struct OpenAi {
        client: reqwest::Client,
        api_key: String,
        model: String,
        api_url: String,
    }

    impl OpenAi {
        pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_key,
                model: model.unwrap_or_else(|| OPENAI_DEFAULT_MODEL.to_string()),
                api_url: api_url.unwrap_or_else(|| OPENAI_API_URL.to_string()),
            }
        }

        pub fn name(&self) -> &str {
            "openai"
        }

        pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
            let body = serde_json::json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            });
            let resp = self
                .client
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("OpenAI API request failed: {e}"))?;
            let status = resp.status();
            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow!("OpenAI API response parse failed: {e}"))?;
            if !status.is_success() {
                let err_msg = json
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown");
                return Err(anyhow!("OpenAI API error ({status}): {err_msg}"));
            }
            let text = json["choices"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["message"]["content"].as_str())
                .ok_or_else(|| anyhow!("OpenAI unexpected response format"))?;
            Ok(text.to_string())
        }
    }

    pub struct Ollama {
        client: reqwest::Client,
        model: String,
        api_url: String,
    }

    impl Ollama {
        pub fn new(model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                model: model.unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_string()),
                api_url: api_url.unwrap_or_else(|| OLLAMA_API_URL.to_string()),
            }
        }
        pub fn name(&self) -> &str {
            "ollama"
        }

        pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
            let body = serde_json::json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
                "stream": false,
            });
            let resp = self
                .client
                .post(&self.api_url)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("Ollama API request failed: {e}"))?;
            let status = resp.status();
            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow!("Ollama API response parse failed: {e}"))?;
            if !status.is_success() {
                return Err(anyhow!("Ollama API error ({status}): {json}"));
            }
            let text = json["message"]["content"]
                .as_str()
                .ok_or_else(|| anyhow!("Ollama unexpected response format"))?;
            Ok(text.to_string())
        }
    }

    pub struct Noop;

    impl Noop {
        pub fn name(&self) -> &str {
            "disabled"
        }
        pub async fn chat(&self, _system: &str, _user: &str) -> Result<String> {
            Err(anyhow!(
                "AI provider not configured. Set OXO_FLOW_AI_PROVIDER and OXO_FLOW_AI_API_KEY."
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Public AiProvider enum (enum-based dispatch, no trait objects)
// ---------------------------------------------------------------------------

/// An AI provider instance. Use the enum variant methods or the convenience
/// [`AiProvider::chat`] and [`AiProvider::name`] to interact.
pub enum AiProvider {
    Claude(internal::Claude),
    OpenAi(internal::OpenAi),
    Ollama(internal::Ollama),
    Noop(internal::Noop),
}

impl AiProvider {
    /// Send a chat message and return the full text response.
    pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
        match self {
            Self::Claude(p) => p.chat(system, user).await,
            Self::OpenAi(p) => p.chat(system, user).await,
            Self::Ollama(p) => p.chat(system, user).await,
            Self::Noop(p) => p.chat(system, user).await,
        }
    }

    /// Human-readable provider name.
    pub fn name(&self) -> &str {
        match self {
            Self::Claude(p) => p.name(),
            Self::OpenAi(p) => p.name(),
            Self::Ollama(p) => p.name(),
            Self::Noop(p) => p.name(),
        }
    }
}

// ---------------------------------------------------------------------------
// Factory functions
// ---------------------------------------------------------------------------

/// Create an AI provider from the given kind and optional overrides.
///
/// Overrides take precedence over environment variables.
pub fn create_provider(
    kind: AiProviderKind,
    api_key: Option<String>,
    api_url: Option<String>,
    model: Option<String>,
) -> AiProvider {
    let key = api_key.or_else(|| std::env::var("OXO_FLOW_AI_API_KEY").ok());
    let url = api_url.or_else(|| std::env::var("OXO_FLOW_AI_API_URL").ok());
    let mdl = model.or_else(|| std::env::var("OXO_FLOW_AI_MODEL").ok());

    match kind {
        AiProviderKind::Claude => {
            let api_key = key
                .or_else(|| std::env::var("ANTHROPIC_AUTH_TOKEN").ok())
                .unwrap_or_default();
            if api_key.is_empty() {
                tracing::warn!("Claude provider selected but no API key found (check ANTHROPIC_AUTH_TOKEN or OXO_FLOW_AI_API_KEY)");
            }
            let api_url = url
                .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
                .unwrap_or_else(|| CLAUDE_API_URL.to_string());
            let model_name = mdl
                .or_else(|| std::env::var("ANTHROPIC_MODEL").ok())
                .or_else(|| std::env::var("OXO_FLOW_AI_MODEL").ok());
            AiProvider::Claude(internal::Claude::new(api_key, model_name, Some(api_url)))
        }
        AiProviderKind::OpenAi => {
            let api_key = key
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .unwrap_or_default();
            if api_key.is_empty() {
                tracing::warn!("OpenAI provider selected but no API key found (check OPENAI_API_KEY or OXO_FLOW_AI_API_KEY)");
            }
            let api_url = url
                .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
                .or_else(|| std::env::var("OXO_FLOW_AI_API_URL").ok());
            let model_name = mdl
                .or_else(|| std::env::var("OPENAI_MODEL").ok())
                .or_else(|| std::env::var("OXO_FLOW_AI_MODEL").ok());
            AiProvider::OpenAi(internal::OpenAi::new(api_key, model_name, api_url))
        }
        AiProviderKind::Ollama => AiProvider::Ollama(internal::Ollama::new(mdl, url)),
    }
}

/// Create an AI provider from environment variables alone.
///
/// Reads `OXO_FLOW_AI_PROVIDER` to determine the kind. Returns a no-op
/// provider if the env var is unset or `"disabled"`.
pub fn create_provider_from_env() -> AiProvider {
    let provider_str = std::env::var("OXO_FLOW_AI_PROVIDER").unwrap_or_default();
    if provider_str.is_empty() || provider_str.eq_ignore_ascii_case("disabled") {
        tracing::info!("AI provider disabled (set OXO_FLOW_AI_PROVIDER to enable)");
        return AiProvider::Noop(internal::Noop);
    }
    match provider_str.parse::<AiProviderKind>() {
        Ok(kind) => {
            let provider = create_provider(kind, None, None, None);
            tracing::info!(
                "AI provider: {} (model: {})",
                provider.name(),
                std::env::var("OXO_FLOW_AI_MODEL").unwrap_or_else(|_| "default".into())
            );
            provider
        }
        Err(e) => {
            tracing::warn!("Invalid AI provider '{provider_str}': {e}. Falling back to disabled.");
            AiProvider::Noop(internal::Noop)
        }
    }
}

/// A lazily-initialized, thread-safe AI provider registry.
pub struct AiProviderRegistry {
    provider: std::sync::OnceLock<AiProvider>,
}

impl Default for AiProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AiProviderRegistry {
    pub const fn new() -> Self {
        Self {
            provider: std::sync::OnceLock::new(),
        }
    }

    /// Get the global singleton registry.
    pub fn global() -> &'static Self {
        static REGISTRY: AiProviderRegistry = AiProviderRegistry::new();
        &REGISTRY
    }

    /// Initialize from environment variables (call once at startup).
    pub fn init_from_env(&self) {
        let provider = create_provider_from_env();
        let _ = self.provider.set(provider);
    }

    /// Initialize with explicit parameters (call once at startup).
    pub fn init(
        &self,
        kind: AiProviderKind,
        api_key: Option<String>,
        api_url: Option<String>,
        model: Option<String>,
    ) {
        let provider = create_provider(kind, api_key, api_url, model);
        let _ = self.provider.set(provider);
    }

    /// Get the registered provider, or a no-op fallback.
    pub fn provider(&self) -> &AiProvider {
        self.provider
            .get()
            .unwrap_or(&AiProvider::Noop(internal::Noop))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_parse() {
        assert_eq!(
            "claude".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::Claude
        );
        assert_eq!(
            "openai".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::OpenAi
        );
        assert_eq!(
            "ollama".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::Ollama
        );
        assert!("invalid".parse::<AiProviderKind>().is_err());
        assert!("".parse::<AiProviderKind>().is_err());
    }

    #[test]
    fn provider_kind_case_insensitive() {
        assert_eq!(
            "Claude".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::Claude
        );
        assert_eq!(
            "OPENAI".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::OpenAi
        );
        assert_eq!(
            "Ollama".parse::<AiProviderKind>().unwrap(),
            AiProviderKind::Ollama
        );
    }

    #[tokio::test]
    async fn noop_provider_returns_error() {
        let provider = AiProvider::Noop(internal::Noop);
        assert_eq!(provider.name(), "disabled");
        assert!(provider.chat("system", "user").await.is_err());
    }

    #[test]
    fn registry_default_is_noop() {
        let reg = AiProviderRegistry::new();
        assert_eq!(reg.provider().name(), "disabled");
    }

    #[test]
    fn create_from_env_disabled_by_default() {
        // Env var may or may not be set; test assumes unset
        let provider = create_provider_from_env();
        assert_eq!(provider.name(), "disabled");
    }
}
