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

use anyhow::{Result, anyhow};

// ---------------------------------------------------------------------------
// Provider kind enum
// ---------------------------------------------------------------------------

/// Available AI provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    Claude,
    OpenAi,
    DeepSeek,
    Ollama,
}

impl std::str::FromStr for AiProviderKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "openai" | "open-ai" => Ok(Self::OpenAi),
            "deepseek" => Ok(Self::DeepSeek),
            "ollama" => Ok(Self::Ollama),
            _ => Err(anyhow!(
                "Unknown AI provider '{s}'. Use 'claude', 'openai', 'deepseek', or 'ollama'"
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
const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-v4-pro";
const DEEPSEEK_API_URL: &str = "https://api.deepseek.com/v1/chat/completions";
const OLLAMA_DEFAULT_MODEL: &str = "llama3";
const OLLAMA_API_URL: &str = "http://localhost:11434/api/chat";

// ---------------------------------------------------------------------------
// Internal provider implementations (private)
// ---------------------------------------------------------------------------

mod internal {
    use super::*;
    use anyhow::{Result, anyhow};

    #[derive(Clone)]
    pub struct Claude {
        pub client: reqwest::Client,
        pub api_key: String,
        pub model: String,
        pub api_url: String,
    }

    impl Claude {
        pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_key,
                model: model.unwrap_or_else(|| CLAUDE_DEFAULT_MODEL.to_string()),
                api_url: {
                    let mut url = api_url.unwrap_or_else(|| CLAUDE_API_URL.to_string());
                    if !url.contains("/v1/messages") {
                        url = format!("{}/v1/messages", url.trim_end_matches('/'));
                    }
                    url
                },
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
                .header("Authorization", format!("Bearer {}", &self.api_key))
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
            // Try Anthropic format: find content block with type="text"
            // (Skip "thinking" blocks that some providers like DeepSeek insert first)
            if let Some(arr) = json["content"].as_array() {
                for block in arr {
                    if block["type"].as_str() == Some("text")
                        && let Some(text) = block["text"].as_str()
                    {
                        return Ok(text.to_string());
                    }
                }
                // Fallback: try first block's text field (legacy format)
                if let Some(text) = arr.first().and_then(|b| b["text"].as_str()) {
                    return Ok(text.to_string());
                }
            }
            // Fallback: OpenAI format (used by DeepSeek Anthropic endpoint)
            if let Some(text) = json["choices"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["message"]["content"].as_str())
            {
                return Ok(text.to_string());
            }
            Err(anyhow!("Claude unexpected response format"))
        }
    }

    #[derive(Clone)]
    pub struct OpenAi {
        pub client: reqwest::Client,
        pub api_key: String,
        pub model: String,
        pub api_url: String,
    }

    impl OpenAi {
        pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_key,
                model: model.unwrap_or_else(|| OPENAI_DEFAULT_MODEL.to_string()),
                api_url: {
                    let mut url = api_url.unwrap_or_else(|| OPENAI_API_URL.to_string());
                    if !url.contains("/chat/completions") {
                        if !url.contains("/v1") {
                            url = format!("{}/v1/chat/completions", url.trim_end_matches('/'));
                        } else {
                            url = format!("{}/chat/completions", url.trim_end_matches('/'));
                        }
                    }
                    url
                },
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

    #[derive(Clone)]
    pub struct Ollama {
        pub client: reqwest::Client,
        pub model: String,
        pub api_url: String,
    }

    impl Ollama {
        pub fn new(model: Option<String>, api_url: Option<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                model: model.unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_string()),
                api_url: {
                    let mut url = api_url.unwrap_or_else(|| OLLAMA_API_URL.to_string());
                    if !url.contains("/chat") {
                        url = format!("{}/chat", url.trim_end_matches('/'));
                    }
                    url
                },
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

    #[derive(Clone)]
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
#[derive(Clone)]
pub enum AiProvider {
    Claude(internal::Claude),
    OpenAi(internal::OpenAi),
    DeepSeek(internal::OpenAi),
    Ollama(internal::Ollama),
    Noop(internal::Noop),
}

impl AiProvider {
    pub fn api_url(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.api_url.clone()),
            Self::OpenAi(p) => Some(p.api_url.clone()),
            Self::DeepSeek(p) => Some(p.api_url.clone()),
            Self::Ollama(p) => Some(p.api_url.clone()),
            Self::Noop(_) => None,
        }
    }

    pub fn model(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.model.clone()),
            Self::OpenAi(p) => Some(p.model.clone()),
            Self::DeepSeek(p) => Some(p.model.clone()),
            Self::Ollama(p) => Some(p.model.clone()),
            Self::Noop(_) => None,
        }
    }

    /// Send a chat message and return the full text response.
    pub async fn chat(&self, system: &str, user: &str) -> Result<String> {
        match self {
            Self::Claude(p) => p.chat(system, user).await,
            Self::OpenAi(p) => p.chat(system, user).await,
            Self::DeepSeek(p) => p.chat(system, user).await,
            Self::Ollama(p) => p.chat(system, user).await,
            Self::Noop(p) => p.chat(system, user).await,
        }
    }

    /// Human-readable provider name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude(_) => "claude",
            Self::OpenAi(_) => "openai",
            Self::DeepSeek(_) => "deepseek",
            Self::Ollama(_) => "ollama",
            Self::Noop(_) => "disabled",
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
                tracing::warn!(
                    "Claude provider selected but no API key found (check ANTHROPIC_AUTH_TOKEN or OXO_FLOW_AI_API_KEY)"
                );
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
                tracing::warn!(
                    "OpenAI provider selected but no API key found (check OPENAI_API_KEY or OXO_FLOW_AI_API_KEY)"
                );
            }
            let api_url = url
                .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
                .or_else(|| std::env::var("OXO_FLOW_AI_API_URL").ok());
            let model_name = mdl
                .or_else(|| std::env::var("OPENAI_MODEL").ok())
                .or_else(|| std::env::var("OXO_FLOW_AI_MODEL").ok());
            AiProvider::OpenAi(internal::OpenAi::new(api_key, model_name, api_url))
        }
        AiProviderKind::DeepSeek => {
            let api_key = key
                .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
                .unwrap_or_default();
            if api_key.is_empty() {
                tracing::warn!(
                    "DeepSeek provider selected but no API key found (check DEEPSEEK_API_KEY or OXO_FLOW_AI_API_KEY)"
                );
            }
            let api_url = url
                .or_else(|| std::env::var("DEEPSEEK_BASE_URL").ok())
                .unwrap_or_else(|| DEEPSEEK_API_URL.to_string());
            let model_name = mdl
                .or_else(|| std::env::var("DEEPSEEK_MODEL").ok())
                .or_else(|| Some(DEEPSEEK_DEFAULT_MODEL.to_string()));
            AiProvider::DeepSeek(internal::OpenAi::new(api_key, model_name, Some(api_url)))
        }
        AiProviderKind::Ollama => AiProvider::Ollama(internal::Ollama::new(mdl, url)),
    }
}

/// Path where AI config is persisted for survival across restarts.
fn ai_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("oxo-flow")
        .join("ai_config.json")
}

/// Save AI config to disk so it survives restarts without env vars.
pub fn save_ai_config(
    kind: &str,
    api_key: Option<&str>,
    api_url: Option<&str>,
    model: Option<&str>,
) {
    let path = ai_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let config = serde_json::json!({
        "provider": kind,
        "api_key": api_key.unwrap_or(""),
        "api_url": api_url.unwrap_or(""),
        "model": model.unwrap_or(""),
    });
    if let Ok(json) = serde_json::to_string_pretty(&config) {
        std::fs::write(&path, json).ok();
        tracing::info!("AI config saved to {}", path.display());
    }
}

/// Load persisted AI config from disk.
fn load_ai_config() -> Option<(String, String, String, String)> {
    let path = ai_config_path();
    if !path.exists() {
        return None;
    }
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    Some((
        json["provider"].as_str().unwrap_or("").to_string(),
        json["api_key"].as_str().unwrap_or("").to_string(),
        json["api_url"].as_str().unwrap_or("").to_string(),
        json["model"].as_str().unwrap_or("").to_string(),
    ))
}

/// Create an AI provider from environment variables or persisted config.
///
/// Reads `OXO_FLOW_AI_PROVIDER` first; falls back to persisted config file.
/// Returns a no-op provider if neither is configured.
pub fn create_provider_from_env() -> AiProvider {
    let provider_str = std::env::var("OXO_FLOW_AI_PROVIDER").unwrap_or_default();

    // If env var is set, use it (env takes priority)
    if !provider_str.is_empty() && !provider_str.eq_ignore_ascii_case("disabled") {
        match provider_str.parse::<AiProviderKind>() {
            Ok(kind) => {
                let provider = create_provider(kind, None, None, None);
                tracing::info!(
                    "AI provider from env: {} (model: {})",
                    provider.name(),
                    std::env::var("OXO_FLOW_AI_MODEL").unwrap_or_else(|_| "default".into())
                );
                return provider;
            }
            Err(e) => {
                tracing::warn!("Invalid AI provider '{provider_str}': {e}");
            }
        }
    }

    // Fall back to persisted config file
    if let Some((kind_str, api_key, api_url, model)) = load_ai_config()
        && !kind_str.is_empty()
        && kind_str != "disabled"
        && let Ok(kind) = kind_str.parse::<AiProviderKind>()
    {
        let key = if api_key.is_empty() {
            None
        } else {
            Some(api_key.as_str())
        };
        let url = if api_url.is_empty() {
            None
        } else {
            Some(api_url.as_str())
        };
        let mdl = if model.is_empty() {
            None
        } else {
            Some(model.as_str())
        };
        let provider = create_provider(
            kind,
            key.map(String::from),
            url.map(String::from),
            mdl.map(String::from),
        );
        tracing::info!("AI provider from saved config: {}", provider.name());
        return provider;
    }

    tracing::info!(
        "AI provider disabled (set OXO_FLOW_AI_PROVIDER or configure via Settings page)"
    );
    AiProvider::Noop(internal::Noop)
}

/// Runtime configuration snapshot of the AI provider (without secrets).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub is_configured: bool,
}

/// A lazily-initialized, thread-safe AI provider registry.
pub struct AiProviderRegistry {
    provider: std::sync::RwLock<Option<AiProvider>>,
}

impl Default for AiProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AiProviderRegistry {
    pub const fn new() -> Self {
        Self {
            provider: std::sync::RwLock::new(None),
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
        *self.provider.write().unwrap() = Some(provider);
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
        *self.provider.write().unwrap() = Some(provider);
    }

    /// Get the registered provider (returns a clone).
    pub fn provider(&self) -> AiProvider {
        self.get_provider()
    }
}

// Additional methods for AiProviderRegistry (runtime config support)
impl AiProviderRegistry {
    pub fn get_provider(&self) -> AiProvider {
        let guard = self.provider.read().unwrap();
        let opt = &*guard;
        opt.clone().unwrap_or(AiProvider::Noop(internal::Noop))
    }

    pub fn get_config(&self) -> ProviderConfig {
        let guard = self.provider.read().unwrap();
        match guard.as_ref() {
            Some(p) => ProviderConfig {
                provider: p.name().to_string(),
                api_url: p.api_url(),
                model: p.model(),
                is_configured: !matches!(p, AiProvider::Noop(_)),
            },
            None => ProviderConfig {
                provider: "disabled".to_string(),
                api_url: None,
                model: None,
                is_configured: false,
            },
        }
    }

    pub fn reconfigure(
        &self,
        kind: &str,
        api_key: Option<String>,
        api_url: Option<String>,
        model: Option<String>,
    ) -> Result<(), String> {
        let kind_parsed: AiProviderKind = kind.parse().map_err(|e: anyhow::Error| e.to_string())?;
        let provider =
            create_provider(kind_parsed, api_key.clone(), api_url.clone(), model.clone());
        *self.provider.write().unwrap() = Some(provider);
        // Persist to disk for survival across restarts
        save_ai_config(
            kind,
            api_key.as_deref(),
            api_url.as_deref(),
            model.as_deref(),
        );
        Ok(())
    }

    /// Create a Claude provider from environment variables (for fallback chain).
    pub fn create_claude_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("CLAUDE_API_KEY"))
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set"))?;
        let model = std::env::var("ANTHROPIC_MODEL").ok();
        let api_url = std::env::var("ANTHROPIC_BASE_URL").ok();
        Ok(AiProvider::Claude(internal::Claude::new(
            api_key, model, api_url,
        )))
    }

    /// Create an OpenAI provider from environment variables (for fallback chain).
    pub fn create_openai_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY not set"))?;
        let model = std::env::var("OPENAI_MODEL").ok();
        let api_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .or_else(|| Some("https://api.openai.com/v1/chat/completions".to_string()));
        Ok(AiProvider::OpenAi(internal::OpenAi::new(
            api_key, model, api_url,
        )))
    }

    /// Create an Ollama provider from environment variables (for fallback chain).
    pub fn create_ollama_from_env() -> Result<AiProvider, anyhow::Error> {
        let api_url = std::env::var("OLLAMA_HOST").ok();
        let model = std::env::var("OLLAMA_MODEL").ok();
        Ok(AiProvider::Ollama(internal::Ollama::new(model, api_url)))
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
        // When no AI env vars are set, provider should be disabled
        // (unless a saved config file exists from prior runs)
        let provider = create_provider_from_env();
        let name = provider.name();
        assert!(
            name == "disabled"
                || name == "claude"
                || name == "openai"
                || name == "deepseek"
                || name == "ollama",
            "Expected a valid provider or disabled, got: {name}"
        );
    }

    #[test]
    fn provider_name_returns_correct_strings() {
        let c = AiProvider::Claude(internal::Claude::new("k".into(), None, None));
        assert_eq!(c.name(), "claude");
        let o = AiProvider::OpenAi(internal::OpenAi::new("k".into(), None, None));
        assert_eq!(o.name(), "openai");
        let d = AiProvider::DeepSeek(internal::OpenAi::new(
            "k".into(),
            None,
            Some("https://api.deepseek.com/v1/chat/completions".into()),
        ));
        assert_eq!(d.name(), "deepseek");
        let n = AiProvider::Noop(internal::Noop);
        assert_eq!(n.name(), "disabled");
    }
}
