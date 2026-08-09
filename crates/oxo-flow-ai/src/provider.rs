//! AI provider abstraction — multi-backend chat + tool calling.
//!
//! Supports: Claude (Anthropic), OpenAI-compatible (DeepSeek, Groq, Azure, etc.),
//! Ollama (local), and a Noop fallback.
//!
//! # Tool calling
//!
//! All backends expose `chat_with_tools()` which accepts `[Message]` and
//! `[ToolDef]`, returning `AiResponse` with optional `tool_calls`.

use crate::error::AiError;
use crate::types::{
    AiResponse, Message, MessageRole, ToolCall, ToolDef, Usage, tool_calls_from_openai,
    tool_calls_to_openai,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

// ── Provider kind ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProviderKind {
    #[default]
    DeepSeek,
    Claude,
    OpenAi,
    Ollama,
}

impl std::str::FromStr for ProviderKind {
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

// ── Provider defaults ──────────────────────────────────────────────────────

const CLAUDE_DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_DEFAULT_MODEL: &str = "gpt-4o";
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-v4-pro";
const DEEPSEEK_API_URL: &str = "https://api.deepseek.com/v1/chat/completions";
const OLLAMA_DEFAULT_MODEL: &str = "llama3";
const OLLAMA_API_URL: &str = "http://localhost:11434/api/chat";

// ── AiProvider enum ────────────────────────────────────────────────────────

/// A configured AI provider instance. Dispatch via the convenience methods
/// [`AiProvider::chat`] and [`AiProvider::chat_with_tools`].
#[derive(Clone)]
pub enum AiProvider {
    Claude(ClaudeBackend),
    OpenAi(OpenAiBackend),
    DeepSeek(OpenAiBackend), // Reuses OpenAI-compatible backend
    Ollama(OllamaBackend),
    Noop,
}

impl AiProvider {
    pub fn api_url(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.api_url.clone()),
            Self::OpenAi(p) | Self::DeepSeek(p) => Some(p.api_url.clone()),
            Self::Ollama(p) => Some(p.api_url.clone()),
            Self::Noop => None,
        }
    }

    pub fn model(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.model.clone()),
            Self::OpenAi(p) | Self::DeepSeek(p) => Some(p.model.clone()),
            Self::Ollama(p) => Some(p.model.clone()),
            Self::Noop => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude(_) => "claude",
            Self::OpenAi(_) => "openai",
            Self::DeepSeek(_) => "deepseek",
            Self::Ollama(_) => "ollama",
            Self::Noop => "disabled",
        }
    }

    /// Simple chat — convenience wrapper for single-turn messaging.
    pub async fn chat(&self, system: &str, user: &str) -> Result<String, AiError> {
        let messages = vec![Message::system(system), Message::user(user)];
        let response = self.chat_with_tools(&messages, &[]).await?;
        response.content.ok_or(AiError::EmptyResponse)
    }

    /// Multi-turn chat with optional tool definitions.
    ///
    /// When `tools` is non-empty, the model may respond with `tool_calls`
    /// instead of text content. The caller should execute the requested
    /// tools and feed results back via subsequent `Message::tool(...)` messages.
    pub async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        match self {
            Self::Claude(p) => p.chat_with_tools(messages, tools).await,
            Self::OpenAi(p) | Self::DeepSeek(p) => p.chat_with_tools(messages, tools).await,
            Self::Ollama(p) => p.chat_with_tools(messages, tools).await,
            Self::Noop => Err(AiError::NotConfigured),
        }
    }
}

// ── Claude backend ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ClaudeBackend {
    pub client: reqwest::Client,
    pub api_key: String,
    pub model: String,
    pub api_url: String,
}

impl ClaudeBackend {
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

    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        // Extract system message for Anthropic's top-level "system" field
        let system = messages
            .iter()
            .find(|m| m.role == MessageRole::System)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Convert remaining messages to Anthropic format
        let anthropic_msgs: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .filter_map(|m| {
                let role = match m.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "user", // Anthropic flattens tool results
                    MessageRole::System => return None,
                };
                let mut obj = serde_json::json!({
                    "role": role,
                    "content": m.content,
                });
                // Attach tool results if present
                if let Some(ref tc_id) = m.tool_call_id {
                    obj = serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tc_id,
                            "content": m.content,
                        }]
                    });
                }
                Some(obj)
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "system": system,
            "messages": anthropic_msgs,
            "max_tokens": 4096,
        });

        // Add tools in Anthropic format if provided
        if !tools.is_empty() {
            let anthropic_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(anthropic_tools);
        }

        let resp = self
            .client
            .post(&self.api_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Provider {
                provider: "claude".into(),
                message: e.to_string(),
            })?;

        let status = resp.status();
        let json: serde_json::Value = resp.json().await.map_err(|e| AiError::Provider {
            provider: "claude".into(),
            message: format!("response parse failed: {e}"),
        })?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    provider: "claude".into(),
                    retry_after: None,
                });
            }
            return Err(AiError::Provider {
                provider: "claude".into(),
                message: format!("HTTP {status}: {err_msg}"),
            });
        }

        parse_claude_response(&json)
    }
}

fn parse_claude_response(json: &serde_json::Value) -> Result<AiResponse, AiError> {
    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(arr) = json["content"].as_array() {
        for block in arr {
            match block["type"].as_str() {
                Some("text") => {
                    content = block["text"].as_str().map(String::from);
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        arguments: serde_json::to_string(&block["input"]).unwrap_or_default(),
                    });
                }
                _ => {}
            }
        }
    }

    let usage = Usage {
        prompt_tokens: json["usage"]["input_tokens"].as_u64().unwrap_or(0),
        completion_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0),
    };

    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls"
    } else {
        json["stop_reason"].as_str().unwrap_or("stop")
    };

    Ok(AiResponse {
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        usage,
        finish_reason: finish_reason.to_string(),
    })
}

// ── OpenAI-compatible backend (DeepSeek, OpenAI, Groq, Azure, etc.) ────────

#[derive(Clone)]
pub struct OpenAiBackend {
    pub client: reqwest::Client,
    pub api_key: String,
    pub model: String,
    pub api_url: String,
}

impl OpenAiBackend {
    pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: model.unwrap_or_else(|| OPENAI_DEFAULT_MODEL.to_string()),
            api_url: {
                let url = api_url.unwrap_or_else(|| OPENAI_API_URL.to_string());
                if url.contains("/chat/completions") {
                    url
                } else if url.contains("/v1") {
                    format!("{}/chat/completions", url.trim_end_matches('/'))
                } else {
                    format!("{}/v1/chat/completions", url.trim_end_matches('/'))
                }
            },
        }
    }

    /// Build the base request body shared by chat() and chat_with_tools().
    fn build_body(&self, messages: &[Message], tools: &[ToolDef]) -> serde_json::Value {
        let openai_msgs: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({
                    "role": match m.role {
                        MessageRole::System => "system",
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        MessageRole::Tool => "tool",
                    },
                    "content": m.content,
                });
                if let Some(ref tc) = m.tool_calls {
                    obj["tool_calls"] = tool_calls_to_openai(tc);
                }
                if let Some(ref tcid) = m.tool_call_id {
                    obj["tool_call_id"] = serde_json::Value::String(tcid.clone());
                }
                if let Some(ref name) = m.name {
                    obj["name"] = serde_json::Value::String(name.clone());
                }
                obj
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": openai_msgs,
        });

        if !tools.is_empty() {
            let openai_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(openai_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        body
    }

    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        let body = self.build_body(messages, tools);

        let resp = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Provider {
                provider: "openai".into(),
                message: e.to_string(),
            })?;

        let status = resp.status();
        let json: serde_json::Value = resp.json().await.map_err(|e| AiError::Provider {
            provider: "openai".into(),
            message: format!("response parse failed: {e}"),
        })?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    provider: "openai".into(),
                    retry_after: None,
                });
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(AiError::Auth {
                    provider: "openai".into(),
                    message: err_msg.to_string(),
                });
            }
            return Err(AiError::Provider {
                provider: "openai".into(),
                message: format!("HTTP {status}: {err_msg}"),
            });
        }

        parse_openai_response(&json)
    }
}

fn parse_openai_response(json: &serde_json::Value) -> Result<AiResponse, AiError> {
    let choice = json["choices"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or(AiError::Provider {
            provider: "openai".into(),
            message: "no choices in response".into(),
        })?;

    let message = &choice["message"];
    let content = message["content"].as_str().map(String::from);
    let finish_reason = choice["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();

    let tool_calls = if let Some(tc_json) = message["tool_calls"].as_array() {
        let parsed = tool_calls_from_openai(&serde_json::Value::Array(tc_json.clone()));
        if parsed.as_ref().is_some_and(|tcs| !tcs.is_empty()) {
            parsed
        } else {
            None
        }
    } else {
        None
    };

    let usage = Usage {
        prompt_tokens: json["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        completion_tokens: json["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    };

    Ok(AiResponse {
        content,
        tool_calls,
        usage,
        finish_reason,
    })
}

// ── Ollama backend ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct OllamaBackend {
    pub client: reqwest::Client,
    pub model: String,
    pub api_url: String,
}

impl OllamaBackend {
    pub fn new(model: Option<String>, api_url: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            model: model.unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_string()),
            api_url: {
                let url = api_url.unwrap_or_else(|| OLLAMA_API_URL.to_string());
                if !url.contains("/chat") {
                    format!("{}/chat", url.trim_end_matches('/'))
                } else {
                    url
                }
            },
        }
    }

    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        let body = build_ollama_body(messages, tools, &self.model);

        let resp = self
            .client
            .post(&self.api_url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Provider {
                provider: "ollama".into(),
                message: e.to_string(),
            })?;

        let status = resp.status();
        let json: serde_json::Value = resp.json().await.map_err(|e| AiError::Provider {
            provider: "ollama".into(),
            message: format!("response parse failed: {e}"),
        })?;

        if !status.is_success() {
            return Err(AiError::Provider {
                provider: "ollama".into(),
                message: format!("HTTP {status}: {json}"),
            });
        }

        let content = json["message"]["content"].as_str().map(String::from);
        let done = json["done"].as_bool().unwrap_or(true);

        Ok(AiResponse {
            content,
            tool_calls: None, // Ollama tool support varies by model; start with text-only
            usage: Usage::default(), // Ollama doesn't report token counts
            finish_reason: if done { "stop".into() } else { "length".into() },
        })
    }
}

fn build_ollama_body(messages: &[Message], tools: &[ToolDef], model: &str) -> serde_json::Value {
    let ollama_msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": match m.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    _ => "user",
                },
                "content": m.content,
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "model": model,
        "messages": ollama_msgs,
        "stream": false,
    });

    // Ollama supports tools in newer versions; include if provided
    if !tools.is_empty() {
        let ollama_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(ollama_tools);
    }

    body
}

// ── Factory functions ──────────────────────────────────────────────────────

/// Create an AI provider from kind and optional overrides.
/// Override parameters take precedence over environment variables.
pub fn create_provider(
    kind: ProviderKind,
    api_key: Option<String>,
    api_url: Option<String>,
    model: Option<String>,
) -> AiProvider {
    let key = api_key.or_else(|| std::env::var("OXO_FLOW_AI_API_KEY").ok());
    let url = api_url.or_else(|| std::env::var("OXO_FLOW_AI_API_URL").ok());
    let mdl = model.or_else(|| std::env::var("OXO_FLOW_AI_MODEL").ok());

    match kind {
        ProviderKind::Claude => {
            let api_key = key
                .or_else(|| std::env::var("ANTHROPIC_AUTH_TOKEN").ok())
                .unwrap_or_default();
            let api_url = url
                .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
                .unwrap_or_else(|| CLAUDE_API_URL.to_string());
            let model_name = mdl.or_else(|| std::env::var("ANTHROPIC_MODEL").ok());
            AiProvider::Claude(ClaudeBackend::new(api_key, model_name, Some(api_url)))
        }
        ProviderKind::OpenAi => {
            let api_key = key
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .unwrap_or_default();
            let api_url = url.or_else(|| std::env::var("OPENAI_BASE_URL").ok());
            let model_name = mdl.or_else(|| std::env::var("OPENAI_MODEL").ok());
            AiProvider::OpenAi(OpenAiBackend::new(api_key, model_name, api_url))
        }
        ProviderKind::DeepSeek => {
            let api_key = key
                .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
                .unwrap_or_default();
            let api_url = url
                .or_else(|| std::env::var("DEEPSEEK_BASE_URL").ok())
                .unwrap_or_else(|| DEEPSEEK_API_URL.to_string());
            let model_name = mdl.or_else(|| Some(DEEPSEEK_DEFAULT_MODEL.to_string()));
            AiProvider::DeepSeek(OpenAiBackend::new(api_key, model_name, Some(api_url)))
        }
        ProviderKind::Ollama => AiProvider::Ollama(OllamaBackend::new(mdl, url)),
    }
}

/// Create a provider from environment variables or persisted config.
pub fn create_provider_from_env() -> AiProvider {
    let provider_str = std::env::var("OXO_FLOW_AI_PROVIDER").unwrap_or_default();

    if !provider_str.is_empty()
        && !provider_str.eq_ignore_ascii_case("disabled")
        && let Ok(kind) = provider_str.parse::<ProviderKind>()
    {
        let provider = create_provider(kind, None, None, None);
        tracing::info!(
            "AI provider from env: {} (model: {})",
            provider.name(),
            provider.model().unwrap_or_else(|| "default".into())
        );
        return provider;
    }

    // Fall back to persisted config
    if let Some((kind_str, api_key, api_url, model)) = load_ai_config()
        && !kind_str.is_empty()
        && kind_str != "disabled"
        && let Ok(kind) = kind_str.parse::<ProviderKind>()
    {
        let key = if api_key.is_empty() {
            None
        } else {
            Some(api_key)
        };
        let url = if api_url.is_empty() {
            None
        } else {
            Some(api_url)
        };
        let mdl = if model.is_empty() { None } else { Some(model) };
        let provider = create_provider(kind, key, url, mdl);
        tracing::info!("AI provider from saved config: {}", provider.name());
        return provider;
    }

    tracing::info!("AI provider disabled (set OXO_FLOW_AI_PROVIDER or configure via Settings)");
    AiProvider::Noop
}

// ── Config persistence ─────────────────────────────────────────────────────

pub fn ai_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("oxo-flow")
        .join("ai_config.json")
}

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

// ── ProviderConfig ─────────────────────────────────────────────────────────

/// Runtime configuration snapshot (no secrets).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub is_configured: bool,
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_parse() {
        assert_eq!(
            "claude".parse::<ProviderKind>().unwrap(),
            ProviderKind::Claude
        );
        assert_eq!(
            "openai".parse::<ProviderKind>().unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            "deepseek".parse::<ProviderKind>().unwrap(),
            ProviderKind::DeepSeek
        );
        assert_eq!(
            "ollama".parse::<ProviderKind>().unwrap(),
            ProviderKind::Ollama
        );
        assert!("invalid".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn provider_kind_case_insensitive() {
        assert_eq!(
            "DeepSeek".parse::<ProviderKind>().unwrap(),
            ProviderKind::DeepSeek
        );
        assert_eq!(
            "CLAUDE".parse::<ProviderKind>().unwrap(),
            ProviderKind::Claude
        );
    }

    #[test]
    fn noop_provider_returns_error() {
        let provider = AiProvider::Noop;
        assert_eq!(provider.name(), "disabled");
    }

    #[tokio::test]
    async fn noop_chat_returns_not_configured() {
        let result = AiProvider::Noop.chat("system", "user").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn noop_chat_with_tools_returns_not_configured() {
        let result = AiProvider::Noop.chat_with_tools(&[], &[]).await;
        assert!(result.is_err());
    }

    #[test]
    fn openai_backend_builds_body_with_tools() {
        let backend = OpenAiBackend::new("sk-test".into(), Some("gpt-4o".into()), None);
        let messages = vec![Message::system("You are helpful."), Message::user("Hello")];
        let tools = vec![ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        }];
        let body = backend.build_body(&messages, &tools);
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert!(body["tools"].as_array().is_some());
    }

    #[test]
    fn config_path_is_in_home() {
        let path = ai_config_path();
        assert!(path.to_string_lossy().contains(".config"));
        assert!(path.to_string_lossy().contains("oxo-flow"));
    }

    #[test]
    fn parse_openai_response_text() {
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });
        let response = parse_openai_response(&json).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello!"));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.prompt_tokens, 10);
    }

    #[test]
    fn parse_openai_response_tool_calls() {
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup_tool",
                            "arguments": "{\"tool\": \"STAR\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20
            }
        });
        let response = parse_openai_response(&json).unwrap();
        assert!(response.content.is_none());
        let tc = response.tool_calls.unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].name, "lookup_tool");
        assert_eq!(tc[0].arguments, r#"{"tool": "STAR"}"#);
    }

    #[test]
    fn parse_claude_response_text() {
        let json = serde_json::json!({
            "content": [{
                "type": "text",
                "text": "Here is your workflow"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        });
        let response = parse_claude_response(&json).unwrap();
        assert_eq!(response.content.as_deref(), Some("Here is your workflow"));
    }
}
