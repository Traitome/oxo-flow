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
    AiResponse, Message, MessageRole, ToolCall, ToolDef, Usage, tool_calls_to_openai,
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
    /// Offline replay provider for tests and evaluation (issue #73).
    Scripted(crate::scripted::ScriptedBackend),
    Noop,
}

impl AiProvider {
    pub fn api_url(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.api_url.clone()),
            Self::OpenAi(p) | Self::DeepSeek(p) => Some(p.api_url.clone()),
            Self::Ollama(p) => Some(p.api_url.clone()),
            Self::Scripted(_) | Self::Noop => None,
        }
    }

    pub fn model(&self) -> Option<String> {
        match self {
            Self::Claude(p) => Some(p.model.clone()),
            Self::OpenAi(p) | Self::DeepSeek(p) => Some(p.model.clone()),
            Self::Ollama(p) => Some(p.model.clone()),
            Self::Scripted(p) => Some(p.model_name()),
            Self::Noop => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude(_) => "claude",
            Self::OpenAi(_) => "openai",
            Self::DeepSeek(_) => "deepseek",
            Self::Ollama(_) => "ollama",
            Self::Scripted(_) => "scripted",
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
            Self::Scripted(p) => p.chat_with_tools(messages, tools).await,
            Self::Noop => Err(AiError::NotConfigured),
        }
    }

    /// Chat with automatic context-overflow recovery (issue #73): on
    /// [`AiError::ContextOverflow`] the transcript is compressed
    /// ([`compress_transcript`]) and the request retried ONCE. A second
    /// overflow surfaces a readable error instead of wasting quota.
    /// Stream a completion token-by-token (openai-compatible providers;
    /// other backends return the whole response as one `Done` chunk).
    pub async fn chat_stream(&self, system: &str, user: &str) -> Result<ChatStream, AiError> {
        match self {
            AiProvider::OpenAi(b) | AiProvider::DeepSeek(b) => b.chat_stream(system, user).await,
            other => {
                let text = other.chat(system, user).await?;
                Ok(Box::pin(futures::stream::iter(vec![Ok(
                    ChatStreamChunk::Done {
                        content: text,
                        usage: None,
                    },
                )])))
            }
        }
    }

    pub async fn chat_with_tools_overflow_safe(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        match self.chat_with_tools(messages, tools).await {
            Err(AiError::ContextOverflow { provider, message }) => {
                tracing::warn!(
                    provider,
                    message,
                    "context overflow — compressing transcript and retrying once"
                );
                const GUIDANCE: &str = "reduce the grounding data you send: fewer \
                                        rules/skills, smaller tool results, or a \
                                        shorter workflow";
                match compress_transcript(messages) {
                    Some(compressed) => match self.chat_with_tools(&compressed, tools).await {
                        Err(AiError::ContextOverflow {
                            provider,
                            message: retry_message,
                        }) => Err(AiError::ContextOverflow {
                            provider,
                            message: format!(
                                "{retry_message} (still overflowing after transcript \
                                 compression — {GUIDANCE})"
                            ),
                        }),
                        other => other,
                    },
                    None => Err(AiError::ContextOverflow {
                        provider,
                        message: format!(
                            "{message} (the transcript has no removable turns — {GUIDANCE})"
                        ),
                    }),
                }
            }
            other => other,
        }
    }
}

/// How many non-system messages the tail keeps when compressing after a
/// context overflow.
pub const COMPRESS_KEEP_TAIL: usize = 6;

/// Compress a transcript for a context-overflow retry: keep every system
/// message and the last [`COMPRESS_KEEP_TAIL`] non-system messages, replace
/// everything dropped in between with a single marker turn.
///
/// Returns `None` when there is nothing to drop (compression cannot help).
pub fn compress_transcript(messages: &[Message]) -> Option<Vec<Message>> {
    let non_system = messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .collect::<Vec<_>>();
    if non_system.len() <= COMPRESS_KEEP_TAIL {
        return None;
    }
    let dropped = non_system.len() - COMPRESS_KEEP_TAIL;
    let mut out: Vec<Message> = messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .cloned()
        .collect();
    out.push(Message::user(&format!(
        "[... {dropped} earlier turns omitted: the context window was exceeded ...]"
    )));
    out.extend(non_system[dropped..].iter().map(|m| (*m).clone()));
    Some(out)
}

/// Classify a non-success HTTP status + error body into an [`AiError`].
///
/// Body matching is keyword-based: context-window markers map to
/// [`AiError::ContextOverflow`] (retrying the same transcript is
/// pointless), output-size markers to [`AiError::OutputLimit`].
pub fn classify_http_error(provider: &str, status: u16, body: &str) -> AiError {
    let body_lower = body.to_lowercase();
    if status == 429 {
        return AiError::RateLimited {
            provider: provider.into(),
            retry_after: None,
        };
    }
    if status == 401 || status == 403 {
        return AiError::Auth {
            provider: provider.into(),
            message: body.to_string(),
        };
    }
    const OVERFLOW_MARKERS: &[&str] = &[
        "context length",
        "context window",
        "maximum context",
        "context_length_exceeded",
        "input length",
        "input too long",
        "too many tokens",
        "maximum prompt",
        "prompt too long",
    ];
    const OUTPUT_MARKERS: &[&str] = &[
        "max_tokens",
        "maximum output",
        "output too long",
        "completion tokens",
    ];
    if status == 413
        || (status == 400 || status == 422)
            && OVERFLOW_MARKERS.iter().any(|m| body_lower.contains(m))
    {
        return AiError::ContextOverflow {
            provider: provider.into(),
            message: body.to_string(),
        };
    }
    if (status == 400 || status == 422) && OUTPUT_MARKERS.iter().any(|m| body_lower.contains(m)) {
        return AiError::OutputLimit {
            provider: provider.into(),
            message: body.to_string(),
        };
    }
    AiError::Provider {
        provider: provider.into(),
        message: format!("HTTP {status}: {body}"),
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
            return Err(classify_http_error("claude", status.as_u16(), err_msg));
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
            return Err(classify_http_error("openai", status.as_u16(), err_msg));
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
        Some(parse_tool_calls_strict(tc_json)?)
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

/// Parse OpenAI tool calls strictly (issue #73).
///
/// Structurally broken calls (missing `function`, `name`, or `id`) are an
/// error — silently dropping them hides a model failure from the caller.
/// Broken *arguments* JSON is repaired to `{}`: models like DeepSeek have
/// been observed to truncate long argument strings.
fn parse_tool_calls_strict(arr: &[serde_json::Value]) -> Result<Vec<ToolCall>, AiError> {
    arr.iter()
        .map(|tc| {
            let func = tc.get("function").ok_or_else(|| AiError::ToolError {
                tool: "<unknown>".into(),
                message: "tool call missing 'function' block".into(),
            })?;
            let name =
                func.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AiError::ToolError {
                        tool: "<unknown>".into(),
                        message: "tool call missing function name".into(),
                    })?;
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AiError::ToolError {
                    tool: name.into(),
                    message: "tool call missing id".into(),
                })?;
            let raw_arguments = func
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let arguments = if serde_json::from_str::<serde_json::Value>(raw_arguments).is_ok() {
                raw_arguments.to_string()
            } else {
                tracing::warn!(
                    tool = name,
                    "tool call arguments are not valid JSON — repairing to {{}}"
                );
                "{}".to_string()
            };
            Ok(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments,
            })
        })
        .collect()
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

// ── Streaming (openai-compatible SSE) ──────────────────────────────────────

/// One parsed SSE event from an openai-compatible streaming response.
enum SseEvent {
    /// A content delta (text fragment).
    Delta(String),
    /// The stream terminator.
    Done,
    /// Parsed but carries nothing actionable (e.g. a role-only frame).
    Other,
}

/// Parse a raw SSE body (server-sent `data:` lines separated by blank lines)
/// into deltas. `[DONE]` and unparseable lines are skipped — the stream's
/// terminal state is signaled by the caller when the HTTP body ends.
fn parse_openai_sse(body: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for block in body.split(
        "

",
    ) {
        for line in block.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                if data == "[DONE]" {
                    events.push(SseEvent::Done);
                }
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let Some(delta) = json["choices"][0]["delta"]["content"].as_str() else {
                continue;
            };
            if delta.is_empty() {
                events.push(SseEvent::Other);
            } else {
                events.push(SseEvent::Delta(delta.to_string()));
            }
        }
    }
    events
}

/// A chunk of a streamed chat completion.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatStreamChunk {
    /// A token delta — append to the running transcript.
    Text(String),
    /// The stream finished; `content` is the complete accumulated text.
    Done {
        content: String,
        usage: Option<Usage>,
    },
}

/// Boxed stream of completion chunks.
pub type ChatStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatStreamChunk, AiError>> + Send>>;

impl OpenAiBackend {
    /// Stream a completion via the openai-compatible SSE protocol
    /// (`stream: true`). Emits `Text` deltas then a final `Done` chunk.
    pub async fn chat_stream(&self, system: &str, user: &str) -> Result<ChatStream, AiError> {
        let messages = vec![
            Message {
                role: MessageRole::System,
                content: system.to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            Message {
                role: MessageRole::User,
                content: user.to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        let mut body = self.build_body(&messages, &[]);
        body["stream"] = serde_json::json!(true);

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
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let err_msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|j| j["error"]["message"].as_str().map(String::from))
                .unwrap_or(text);
            return Err(classify_http_error("openai", status.as_u16(), &err_msg));
        }

        let stream = resp.bytes_stream();
        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut content = String::new();
            let mut usage: Option<Usage> = None;
            for await chunk in stream {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(AiError::Provider {
                            provider: "openai".into(),
                            message: format!("stream read failed: {e}"),
                        });
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                // SSE frames end with a blank line; keep any trailing partial
                // frame in the buffer for the next read.
                while let Some(pos) = buffer.find("\n\n") {
                    let frame = buffer[..pos].to_string();
                    buffer.drain(..pos + 2);
                    let events = parse_openai_sse(&frame);
                    // capture usage from the final frames (best-effort)
                    if let Some(line) = frame.lines().find(|l| l.contains("\"usage\""))
                        && let Ok(v) = serde_json::from_str::<serde_json::Value>(
                            line.trim().strip_prefix("data:").unwrap_or("").trim(),
                        )
                        && let (Some(p), Some(c)) = (
                            v["usage"]["prompt_tokens"].as_u64(),
                            v["usage"]["completion_tokens"].as_u64(),
                        )
                    {
                        usage = Some(Usage { prompt_tokens: p, completion_tokens: c });
                    }
                    for event in events {
                        match event {
                            SseEvent::Delta(d) => {
                                content.push_str(&d);
                                yield Ok(ChatStreamChunk::Text(d));
                            }
                            SseEvent::Done => {}
                            SseEvent::Other => {}
                        }
                    }
                }
            }
            // Flush any remaining partial frame.
            if !buffer.trim().is_empty() {
                for event in parse_openai_sse(&buffer) {
                    if let SseEvent::Delta(d) = event {
                        content.push_str(&d);
                        yield Ok(ChatStreamChunk::Text(d));
                    }
                }
            }
            yield Ok(ChatStreamChunk::Done { content, usage });
        };
        Ok(Box::pin(stream))
    }
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
    fn parse_openai_response_repairs_truncated_tool_arguments() {
        // DeepSeek has been observed to truncate long tool-call arguments;
        // the broken JSON must be repaired to "{}" rather than silently
        // dropped (issue #73).
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
                            "arguments": "{\"tool\": \"STA"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let response = parse_openai_response(&json).unwrap();
        let calls = response
            .tool_calls
            .expect("repaired call must not be dropped");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup_tool");
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn parse_openai_response_keeps_valid_calls_beside_repaired_ones() {
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "lookup_tool", "arguments": "{\"tool\": \"STAR\"}"}
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": {"name": "lookup_skill", "arguments": "{\"query\": \"truncated"}
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let response = parse_openai_response(&json).unwrap();
        let calls = response.tool_calls.unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, r#"{"tool": "STAR"}"#);
        assert_eq!(calls[1].arguments, "{}");
    }

    #[test]
    fn parse_openai_response_errors_on_unrepairable_tool_call() {
        // A tool call without an id/name cannot be repaired — the caller
        // must see the error instead of a silently empty response.
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {"name": "lookup_tool", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let err = parse_openai_response(&json).unwrap_err();
        assert!(
            matches!(err, AiError::ToolError { .. }),
            "expected ToolError, got {err:?}"
        );
    }

    #[test]
    fn classify_http_errors_by_status_and_body() {
        // Context overflows come back as 400/413 with context markers.
        assert!(matches!(
            classify_http_error(
                "openai",
                400,
                "This model's maximum context length is 65536 tokens."
            ),
            AiError::ContextOverflow { .. }
        ));
        assert!(matches!(
            classify_http_error("openai", 413, "payload too large"),
            AiError::ContextOverflow { .. }
        ));
        assert!(matches!(
            classify_http_error(
                "openai",
                400,
                "context_length_exceeded: input exceeds limit"
            ),
            AiError::ContextOverflow { .. }
        ));
        // Output-limit errors mention max_tokens.
        assert!(matches!(
            classify_http_error("openai", 400, "max_tokens is too large: 100000 > 8192"),
            AiError::OutputLimit { .. }
        ));
        // Existing classifications are preserved.
        assert!(matches!(
            classify_http_error("openai", 429, "rate limit"),
            AiError::RateLimited { .. }
        ));
        assert!(matches!(
            classify_http_error("openai", 401, "bad key"),
            AiError::Auth { .. }
        ));
        assert!(matches!(
            classify_http_error("openai", 500, "server exploded"),
            AiError::Provider { .. }
        ));
    }

    #[test]
    fn compress_transcript_keeps_system_and_tail() {
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::user("u3"),
            Message::assistant("a3"),
            Message::user("u4"),
            Message::assistant("a4"),
            Message::user("u5"),
            Message::assistant("a5"),
            Message::user("final"),
        ];
        let compressed = compress_transcript(&messages).expect("droppable turns exist");
        // System + marker + last COMPRESS_KEEP_TAIL non-system messages.
        assert!(compressed[0].role == MessageRole::System);
        assert!(compressed[1].content.contains("omitted"));
        assert_eq!(compressed.len(), 2 + COMPRESS_KEEP_TAIL);
        assert_eq!(compressed.last().unwrap().content, "final");
        assert!(
            compressed.iter().any(|m| m.content == "a5"),
            "recent turns must survive"
        );
    }

    #[test]
    fn compress_transcript_returns_none_when_nothing_to_drop() {
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("final"),
        ];
        assert!(compress_transcript(&messages).is_none());
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

    #[test]
    fn parse_sse_chunks_extracts_deltas_and_done() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"fast\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"p\"}}]}\n\ndata: [DONE]\n\n";
        let chunks = parse_openai_sse(body);
        assert_eq!(chunks.len(), 3);
        assert!(matches!(&chunks[0], SseEvent::Delta(s) if s == "fast"));
        assert!(matches!(&chunks[1], SseEvent::Delta(s) if s == "p"));
        assert!(matches!(&chunks[2], SseEvent::Done));
    }

    #[test]
    fn parse_sse_chunks_skips_usage_and_garbage_lines() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],\"usage\":{\"total_tokens\":7}}\n\ndata: {\"garbage\": true}\n\nnot-a-data-line\n\ndata: [DONE]\n\n";
        let chunks = parse_openai_sse(body);
        assert!(
            chunks
                .iter()
                .any(|c| matches!(c, SseEvent::Delta(s) if s == "x"))
        );
        assert!(chunks.iter().any(|c| matches!(c, SseEvent::Done)));
        assert_eq!(chunks.len(), 2);
    }
}
