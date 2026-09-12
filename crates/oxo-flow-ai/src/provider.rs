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

/// Shared HTTP client for all provider backends. Without explicit timeouts a
/// hung endpoint would block the agent loop indefinitely; the request timeout
/// (default 300s, `OXO_FLOW_AI_TIMEOUT_SECS`) must also cover slow long-form
/// completions — a single thinking round routinely runs past two minutes (a
/// measured GLM round on a thinking backend took 145s), and a mid-body
/// timeout surfaces as a confusing "error decoding response body". The 10s
/// connect timeout still fails fast on dead endpoints, so the longer wall
/// only binds while a server is actively generating.
fn provider_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(request_timeout_secs()))
        .build()
        .unwrap_or_default()
}

fn request_timeout_secs() -> u64 {
    parse_timeout_secs(std::env::var("OXO_FLOW_AI_TIMEOUT_SECS").ok().as_deref())
}

fn parse_timeout_secs(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(300)
}

// ── Transport retry ────────────────────────────────────────────────────────

/// Backoff between transient-transport retries: one after 2s, one after 5s.
/// The benchmark's dominant flake class is a single dropped request out of
/// a healthy session, so two quick retries absorb it without turning a
/// genuinely dead endpoint into a minute of hanging.
const TRANSPORT_BACKOFFS: &[std::time::Duration] = &[
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
];

/// Whether a [`reqwest::Error`] is a transient transport failure worth
/// retrying: connection setup/reset and mid-transfer failures ("error
/// sending request" was the observed benchmark flake), plus response
/// bodies that fail to decode ("error decoding response body" — a
/// truncated or garbled body from a flaky proxy; retrying refetches it).
/// Timeouts are deliberately excluded — the request timeout has its own
/// documented knob (`OXO_FLOW_AI_TIMEOUT_SECS`), and a retry would
/// deterministically re-timeout against the same slow completion.
fn is_transient_transport_error(e: &reqwest::Error) -> bool {
    !e.is_timeout() && (e.is_connect() || e.is_request() || e.is_body() || e.is_decode())
}

/// Send a prepared request and hand the response to `decode` inside the
/// retry scope, so a transient failure in either half — the send, or
/// reading/decoding the body — replays the whole attempt with `backoffs`
/// pauses between tries (see [`is_transient_transport_error`]). HTTP error
/// statuses are *not* retried here — they carry provider semantics (429
/// back-off, auth, context overflow) that the caller classifies from the
/// returned status and body. When the transport stays broken after every
/// retry, [`AiError::RetryExhausted`] reports the total attempt count.
async fn send_with_retry<T, Fut>(
    request: reqwest::RequestBuilder,
    provider: &str,
    secret: Option<&str>,
    backoffs: &[std::time::Duration],
    decode: impl Fn(reqwest::Response) -> Fut,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, T), AiError>
where
    Fut: std::future::Future<Output = Result<T, reqwest::Error>>,
{
    enum StepFailure {
        NonReplayable,
        Transport(reqwest::Error),
    }

    let mask = |message: String| match secret {
        Some(key) => mask_secret(&message, key),
        None => message,
    };
    // Decode failures keep the callers' familiar "response parse failed"
    // prefix; plain transport errors render as reqwest names them.
    let describe = |e: reqwest::Error| {
        if e.is_decode() {
            format!("response parse failed: {e}")
        } else {
            e.to_string()
        }
    };
    let mut attempt = 0usize;
    loop {
        // The decode closure buffers the body, so the builder is
        // replayable; a non-clonable body would be a construction bug,
        // not a runtime case.
        let result = async {
            let response = request
                .try_clone()
                .ok_or(StepFailure::NonReplayable)?
                .send()
                .await
                .map_err(StepFailure::Transport)?;
            let status = response.status();
            let headers = response.headers().clone();
            let decoded = decode(response).await.map_err(StepFailure::Transport)?;
            Ok((status, headers, decoded))
        }
        .await;
        match result {
            Ok(decoded) => return Ok(decoded),
            Err(StepFailure::NonReplayable) => {
                return Err(AiError::Provider {
                    provider: provider.into(),
                    message: "request body is not replayable for retry".into(),
                });
            }
            Err(StepFailure::Transport(e)) if !is_transient_transport_error(&e) => {
                return Err(AiError::Provider {
                    provider: provider.into(),
                    message: mask(describe(e)),
                });
            }
            Err(StepFailure::Transport(e)) if attempt >= backoffs.len() => {
                return Err(AiError::RetryExhausted {
                    attempts: attempt as u32 + 1,
                    message: mask(describe(e)),
                });
            }
            Err(StepFailure::Transport(e)) => {
                let backoff = backoffs[attempt];
                tracing::warn!(
                    provider,
                    attempt = attempt + 1,
                    backoff_secs = backoff.as_secs(),
                    error = mask(e.to_string()),
                    "transient transport error — retrying"
                );
                attempt += 1;
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

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

    /// Whether calls on this provider can possibly succeed. `Noop` is the
    /// resolved nothing-configured state; every other variant carries the
    /// backend it needs. Surfaces gate their AI_NOT_CONFIGURED responses on
    /// this instead of pattern-matching the variant by hand.
    pub fn is_usable(&self) -> bool {
        !matches!(self, Self::Noop)
    }

    /// Whether `other` is the very same provider configuration — name,
    /// model, endpoint, AND credential. Fallback chains use this to skip a
    /// candidate that duplicates an already-attempted one; two rows that
    /// differ only by API key are NOT the same (a gateway deployment may
    /// serve the same URL/model to a user key and a server key).
    pub fn same_configuration(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Claude(a), Self::Claude(b)) => {
                a.model == b.model && a.api_url == b.api_url && a.api_key == b.api_key
            }
            (Self::OpenAi(a), Self::OpenAi(b))
            | (Self::DeepSeek(a), Self::DeepSeek(b))
            | (Self::OpenAi(a), Self::DeepSeek(b))
            | (Self::DeepSeek(a), Self::OpenAi(b)) => {
                a.model == b.model && a.api_url == b.api_url && a.api_key == b.api_key
            }
            (Self::Ollama(a), Self::Ollama(b)) => a.model == b.model && a.api_url == b.api_url,
            // Scripted backends are per-test instances — never dedup them.
            _ => false,
        }
    }

    /// Apply a configured sampling temperature to the underlying backend
    /// (no-op for backends that do not take one: scripted/noop).
    pub fn with_temperature(self, temperature: Option<f64>) -> Self {
        match self {
            Self::Claude(p) => Self::Claude(p.with_temperature(temperature)),
            Self::OpenAi(p) => Self::OpenAi(p.with_temperature(temperature)),
            Self::DeepSeek(p) => Self::DeepSeek(p.with_temperature(temperature)),
            Self::Ollama(p) => Self::Ollama(p.with_temperature(temperature)),
            other => other,
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
/// The kept tail never begins on a tool result whose declaring assistant
/// turn was dropped: Anthropic rejects `tool_result` blocks whose
/// `tool_use_id` was never declared, so such a transcript would turn a
/// recoverable overflow into a hard 400. Dropping the unpaired results
/// (rather than pulling the cut back) also shrinks the retry request.
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
    let mut dropped = non_system.len() - COMPRESS_KEEP_TAIL;
    while dropped < non_system.len() && non_system[dropped].role == MessageRole::Tool {
        dropped += 1;
    }
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

/// Placeholder written over a leaked API key in provider error text.
const MASKED_KEY: &str = "sk-***";

/// Keys shorter than this are passed through unmasked: masking a 3-character
/// string would corrupt unrelated error text without protecting anything.
const MASK_MIN_KEY_LEN: usize = 8;

/// Replace every occurrence of `secret` in `text` with [`MASKED_KEY`].
///
/// Provider error bodies reflect request material — openai-compatible
/// endpoints have been observed echoing the bearer token back inside a 401
/// body — and those messages reach the terminal and the session log. The
/// masking is applied where the error text is built, so no caller can print
/// an unmasked key by forgetting to scrub it.
fn mask_secret(text: &str, secret: &str) -> String {
    if secret.len() < MASK_MIN_KEY_LEN {
        return text.to_string();
    }
    text.replace(secret, MASKED_KEY)
}

/// The `Retry-After` value of a failed response, if the endpoint sent one.
///
/// Only the delay-seconds form is read; the HTTP-date form is left as
/// `None` rather than guessed (the caller's retry policy is seconds-based).
fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .map(String::from)
}

/// Classify a non-success HTTP status + error body into an [`AiError`].
///
/// Body matching is keyword-based: context-window markers map to
/// [`AiError::ContextOverflow`] (retrying the same transcript is
/// pointless), output-size markers to [`AiError::OutputLimit`].
pub fn classify_http_error(provider: &str, status: u16, body: &str) -> AiError {
    classify_http_error_with_retry_after(provider, status, body, None)
}

/// [`classify_http_error`] carrying the response's `Retry-After` value (see
/// [`retry_after_header`]) so callers can honor the endpoint's back-off
/// instead of guessing.
pub fn classify_http_error_with_retry_after(
    provider: &str,
    status: u16,
    body: &str,
    retry_after: Option<String>,
) -> AiError {
    let body_lower = body.to_lowercase();
    if status == 429 {
        return AiError::RateLimited {
            provider: provider.into(),
            retry_after,
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
    /// Sampling temperature sent as the request's `temperature` field;
    /// `None` omits the field and lets the endpoint apply its default.
    pub temperature: Option<f64>,
}

impl ClaudeBackend {
    pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
        Self {
            client: provider_http_client(),
            api_key,
            model: model.unwrap_or_else(|| CLAUDE_DEFAULT_MODEL.to_string()),
            api_url: {
                let mut url = api_url.unwrap_or_else(|| CLAUDE_API_URL.to_string());
                if !url.contains("/v1/messages") {
                    url = format!("{}/v1/messages", url.trim_end_matches('/'));
                }
                url
            },
            temperature: None,
        }
    }

    /// Set the sampling temperature for every request from this backend.
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Build the Anthropic `/v1/messages` body (extracted so the request
    /// shape — including `temperature` — is unit-testable without network).
    fn build_body(&self, messages: &[Message], tools: &[ToolDef]) -> serde_json::Value {
        let (system, anthropic_msgs) = to_anthropic_messages(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "system": system,
            "messages": anthropic_msgs,
            "max_tokens": max_tokens_from_env(),
        });

        if let Some(temperature) = self.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }

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

        body
    }

    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        let body = self.build_body(messages, tools);

        let (status, headers, json) = send_with_retry(
            self.client
                .post(&self.api_url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body),
            "claude",
            Some(&self.api_key),
            TRANSPORT_BACKOFFS,
            |resp| resp.json::<serde_json::Value>(),
        )
        .await?;
        let retry_after = retry_after_header(&headers);

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(classify_http_error_with_retry_after(
                "claude",
                status.as_u16(),
                &mask_secret(err_msg, &self.api_key),
                retry_after,
            ));
        }

        parse_claude_response(&json)
    }
}

/// Convert internal messages to Anthropic's wire format.
///
/// Returns (system, messages). Assistant turns that carry tool calls emit
/// explicit `tool_use` blocks, and `tool_result` blocks whose `tool_use_id`
/// was never declared are dropped — Anthropic rejects such a request
/// outright (400), so a transcript that lost its declaring turn (compressed
/// or otherwise) must not be sent verbatim.
fn to_anthropic_messages(messages: &[Message]) -> (String, Vec<serde_json::Value>) {
    let system = messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .map(|m| m.content.clone())
        .unwrap_or_default();

    // Convert internal messages to Anthropic's wire format. Consecutive tool
    // results coalesce into ONE user message: Anthropic requires ALL
    // tool_result blocks for an assistant turn's tool_use blocks to appear in
    // the message that immediately follows it.
    let mut anthropic_msgs: Vec<serde_json::Value> = Vec::new();
    let mut declared_tool_use_ids: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    for m in messages.iter().filter(|m| m.role != MessageRole::System) {
        let role = match m.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "user", // Anthropic flattens tool results
            MessageRole::System => unreachable!("filtered above"),
        };
        // Assistant messages with tool calls must emit the tool_use
        // blocks — Anthropic rejects tool_result blocks whose
        // tool_use_id was never declared in a prior assistant turn.
        if m.role == MessageRole::Assistant
            && let Some(tool_calls) = &m.tool_calls
            && !tool_calls.is_empty()
        {
            let mut blocks: Vec<serde_json::Value> = Vec::new();
            if !m.content.is_empty() {
                blocks.push(serde_json::json!({"type": "text", "text": m.content}));
            }
            for tc in tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": input,
                }));
                declared_tool_use_ids.insert(tc.id.as_str());
            }
            anthropic_msgs.push(serde_json::json!({"role": role, "content": blocks}));
            continue;
        }
        // Tool result: append to the previous user message when it is a
        // tool-result message (coalescing), else start a new one.
        if m.role == MessageRole::Tool {
            let block = match &m.tool_call_id {
                Some(tc_id) if declared_tool_use_ids.contains(tc_id.as_str()) => {
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc_id,
                        "content": m.content,
                    })
                }
                Some(tc_id) => {
                    tracing::warn!(
                        tool_use_id = tc_id.as_str(),
                        "dropping tool_result whose tool_use was never declared"
                    );
                    continue;
                }
                None => serde_json::json!({
                    "type": "text",
                    "text": m.content,
                }),
            };
            if let Some(last) = anthropic_msgs.last_mut()
                && last["role"] == "user"
                && last["content"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(|b| b["type"] == "tool_result"))
            {
                last["content"].as_array_mut().unwrap().push(block);
            } else {
                anthropic_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": [block],
                }));
            }
            continue;
        }
        anthropic_msgs.push(serde_json::json!({
            "role": role,
            "content": m.content,
        }));
    }

    (system, anthropic_msgs)
}

/// Output-token ceiling for the Anthropic Messages backend.
///
/// Defaults to 16384 but is overridable via `OXO_FLOW_AI_MAX_TOKENS`.
/// Thinking-style backends (e.g. DeepSeek or GLM served behind an
/// Anthropic-compatible endpoint) emit `thinking` blocks whose tokens count
/// against `max_tokens`; the old hard 4096 there was consumed by reasoning
/// before any text was produced (GLM returned empty content on every round),
/// so pipeline generation silently lost its TOML. 16384 clears the thinking
/// budget with room for the TOML (model-axis calibration, 5/5 archetypes).
fn max_tokens_from_env() -> u32 {
    parse_max_tokens(std::env::var("OXO_FLOW_AI_MAX_TOKENS").ok().as_deref())
}

fn parse_max_tokens(value: Option<&str>) -> u32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(16384)
}

fn parse_claude_response(json: &serde_json::Value) -> Result<AiResponse, AiError> {
    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(arr) = json["content"].as_array() {
        for block in arr {
            match block["type"].as_str() {
                Some("text") => {
                    // A Claude response can carry several text blocks (text,
                    // tool_use, text) — appending keeps all assistant prose;
                    // overwriting silently dropped every block but the last.
                    if let Some(text) = block["text"].as_str() {
                        content.get_or_insert_with(String::new).push_str(text);
                    }
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
        reasoning_content: None,
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
    /// Provider name carried in error messages — the openai-compatible
    /// backend also serves DeepSeek, whose errors must not claim "openai".
    pub label: String,
    /// Sampling temperature sent as the request's `temperature` field;
    /// `None` omits the field and lets the endpoint apply its default.
    pub temperature: Option<f64>,
}

impl OpenAiBackend {
    pub fn new(api_key: String, model: Option<String>, api_url: Option<String>) -> Self {
        Self::new_labeled(api_key, model, api_url, "openai")
    }

    /// Same protocol, different identity: DeepSeek (and other
    /// openai-compatible endpoints) reuse this backend with their own
    /// label so error messages name the provider the user configured.
    pub fn new_labeled(
        api_key: String,
        model: Option<String>,
        api_url: Option<String>,
        label: &str,
    ) -> Self {
        Self {
            client: provider_http_client(),
            api_key,
            label: label.to_string(),
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
            temperature: None,
        }
    }

    /// Set the sampling temperature for every request from this backend.
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.temperature = temperature;
        self
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
                // DeepSeek reasoning models require the assistant's reasoning
                // content to be echoed back verbatim on subsequent calls. An
                // EMPTY string is not "reasoning content": transcripts built
                // by the agent loop default it to `Some("")` for every
                // assistant turn, and some openai-compatible endpoints reject
                // an empty `reasoning_content` field outright — omit it.
                if let Some(rc) = &m.reasoning_content
                    && !rc.is_empty()
                {
                    obj["reasoning_content"] = serde_json::Value::String(rc.clone());
                }
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

        if let Some(temperature) = self.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }

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

        let (status, headers, json) = send_with_retry(
            self.client
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&body),
            &self.label,
            Some(&self.api_key),
            TRANSPORT_BACKOFFS,
            |resp| resp.json::<serde_json::Value>(),
        )
        .await?;
        let retry_after = retry_after_header(&headers);

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(classify_http_error_with_retry_after(
                &self.label,
                status.as_u16(),
                &mask_secret(err_msg, &self.api_key),
                retry_after,
            ));
        }

        parse_openai_response(&json, &self.label)
    }
}

fn parse_openai_response(json: &serde_json::Value, label: &str) -> Result<AiResponse, AiError> {
    let choice = json["choices"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or(AiError::Provider {
            provider: label.into(),
            message: "no choices in response".into(),
        })?;

    let message = &choice["message"];
    let content = message["content"].as_str().map(String::from);
    let reasoning_content = message["reasoning_content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
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
        reasoning_content,
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
    /// Sampling temperature sent in the request's `options.temperature`;
    /// `None` omits it and lets the model's own default apply.
    pub temperature: Option<f64>,
}

impl OllamaBackend {
    pub fn new(model: Option<String>, api_url: Option<String>) -> Self {
        Self {
            client: provider_http_client(),
            model: model.unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_string()),
            api_url: {
                let url = api_url.unwrap_or_else(|| OLLAMA_API_URL.to_string());
                if !url.contains("/chat") {
                    format!("{}/chat", url.trim_end_matches('/'))
                } else {
                    url
                }
            },
            temperature: None,
        }
    }

    /// Set the sampling temperature for every request from this backend.
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.temperature = temperature;
        self
    }

    async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse, AiError> {
        let body = build_ollama_body(messages, tools, &self.model, self.temperature);

        let (status, _headers, json) = send_with_retry(
            self.client
                .post(&self.api_url)
                .header("content-type", "application/json")
                .json(&body),
            "ollama",
            None,
            TRANSPORT_BACKOFFS,
            |resp| resp.json::<serde_json::Value>(),
        )
        .await?;

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
            reasoning_content: None,
            tool_calls: None, // Ollama tool support varies by model; start with text-only
            usage: Usage::default(), // Ollama doesn't report token counts
            finish_reason: if done { "stop".into() } else { "length".into() },
        })
    }
}

fn build_ollama_body(
    messages: &[Message],
    tools: &[ToolDef],
    model: &str,
    temperature: Option<f64>,
) -> serde_json::Value {
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

    if let Some(temperature) = temperature {
        body["options"] = serde_json::json!({"temperature": temperature});
    }

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

/// Append a raw SSE chunk to `buffer` and return every frame it completes.
///
/// Carriage returns are discarded on the way in so CRLF-terminated streams
/// (proxies that re-frame SSE with `\r\n`) split on the same blank-line
/// boundary as LF-terminated ones — including when a `\r\n` pair straddles
/// two chunks, because the `\r` is dropped wherever it lands. A raw CR can
/// only be a line terminator here: any CR inside event data is escaped
/// inside the JSON payload.
fn push_sse_chunk(buffer: &mut String, chunk: &str) -> Vec<String> {
    buffer.extend(chunk.chars().filter(|c| *c != '\r'));
    let mut frames = Vec::new();
    while let Some(pos) = buffer.find("\n\n") {
        frames.push(buffer[..pos].to_string());
        buffer.drain(..pos + 2);
    }
    frames
}

/// Token usage carried by a streamed frame, when the endpoint sends one
/// (requested via `stream_options.include_usage`).
fn usage_from_frame(frame: &str) -> Option<Usage> {
    let line = frame.lines().find(|l| l.contains("\"usage\""))?;
    let json: serde_json::Value =
        serde_json::from_str(line.trim().strip_prefix("data:").unwrap_or("").trim()).ok()?;
    Some(Usage {
        prompt_tokens: json["usage"]["prompt_tokens"].as_u64()?,
        completion_tokens: json["usage"]["completion_tokens"].as_u64()?,
    })
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
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            Message {
                role: MessageRole::User,
                content: user.to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        let mut body = self.build_body(&messages, &[]);
        body["stream"] = serde_json::json!(true);
        // Ask the endpoint to close the stream with a usage frame: without
        // it a streamed call reports no token counts at all, so the caller's
        // cost and session accounting sees zeros. The openai-compatible
        // endpoints this backend serves (OpenAI, DeepSeek) support it.
        body["stream_options"] = serde_json::json!({"include_usage": true});

        // Streaming keeps the raw response: the body is consumed
        // incrementally below, so only the send half is replayable.
        let (status, headers, resp) = send_with_retry(
            self.client
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&body),
            &self.label,
            Some(&self.api_key),
            TRANSPORT_BACKOFFS,
            |resp| async move { Ok::<_, reqwest::Error>(resp) },
        )
        .await?;
        let retry_after = retry_after_header(&headers);
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let err_msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|j| j["error"]["message"].as_str().map(String::from))
                .unwrap_or(text);
            return Err(classify_http_error_with_retry_after(
                &self.label,
                status.as_u16(),
                &mask_secret(&err_msg, &self.api_key),
                retry_after,
            ));
        }

        let stream = resp.bytes_stream();
        let label = self.label.clone();
        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut content = String::new();
            let mut usage: Option<Usage> = None;
            for await chunk in stream {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(AiError::Provider {
                            provider: label.clone(),
                            message: format!("stream read failed: {e}"),
                        });
                        return;
                    }
                };
                // SSE frames end with a blank line; `push_sse_chunk` keeps any
                // trailing partial frame in the buffer for the next read.
                for frame in push_sse_chunk(&mut buffer, &String::from_utf8_lossy(&bytes)) {
                    if let Some(u) = usage_from_frame(&frame) {
                        usage = Some(u);
                    }
                    for event in parse_openai_sse(&frame) {
                        if let SseEvent::Delta(d) = event {
                            content.push_str(&d);
                            yield Ok(ChatStreamChunk::Text(d));
                        }
                    }
                }
            }
            // Flush any remaining partial frame.
            if !buffer.trim().is_empty() {
                if let Some(u) = usage_from_frame(&buffer) {
                    usage = Some(u);
                }
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
            AiProvider::DeepSeek(OpenAiBackend::new_labeled(
                api_key,
                model_name,
                Some(api_url),
                "deepseek",
            ))
        }
        ProviderKind::Ollama => AiProvider::Ollama(OllamaBackend::new(mdl, url)),
    }
}

/// Create a provider from environment variables or persisted config.
pub fn create_provider_from_env() -> AiProvider {
    let provider_str = std::env::var("OXO_FLOW_AI_PROVIDER").unwrap_or_default();
    resolve_provider(
        &provider_str,
        load_ai_config()
            .as_ref()
            .map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str())),
    )
}

/// Pure provider-resolution core: the env provider string plus an optional
/// persisted config.
///
/// Split from the env/file I/O so the precedence contract is unit-testable
/// without process-global env mutation (this crate forbids unsafe, and
/// edition-2024 env mutation requires it). Precedence: env > persisted —
/// and `OXO_FLOW_AI_PROVIDER=disabled` is an explicit opt-out that must
/// win over a saved config (issue #142 M10): previously the "disabled"
/// spelling skipped the env branch and fell through to the persisted
/// provider, so the override silently did nothing.
fn resolve_provider(provider_env: &str, saved: Option<(&str, &str, &str, &str)>) -> AiProvider {
    if provider_env.eq_ignore_ascii_case("disabled") {
        tracing::info!("AI provider disabled via OXO_FLOW_AI_PROVIDER=disabled");
        return AiProvider::Noop;
    }

    if !provider_env.is_empty()
        && let Ok(kind) = provider_env.parse::<ProviderKind>()
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
    if let Some((kind_str, api_key, api_url, model)) = saved
        && !kind_str.is_empty()
        && kind_str != "disabled"
        && let Ok(kind) = kind_str.parse::<ProviderKind>()
    {
        // A saved config with no key is an unfinished setup, not a working
        // provider: treat it as unconfigured so status probes do not fire
        // real (and guaranteed-failing) network calls on an empty key.
        if api_key.is_empty() {
            tracing::info!(
                "AI provider {} saved without an API key — treated as unconfigured",
                kind_str
            );
            return AiProvider::Noop;
        }
        let url = if api_url.is_empty() {
            None
        } else {
            Some(api_url.to_string())
        };
        let mdl = if model.is_empty() {
            None
        } else {
            Some(model.to_string())
        };
        let provider = create_provider(kind, Some(api_key.to_string()), url, mdl);
        tracing::info!("AI provider from saved config: {}", provider.name());
        return provider;
    }

    tracing::info!("AI provider disabled (set OXO_FLOW_AI_PROVIDER or configure via Settings)");
    AiProvider::Noop
}

// ── Config persistence ─────────────────────────────────────────────────────

/// Canonical AI config path — `~/.oxo-flow/ai_config.json` (matches the
/// `AiConfig` loader docs in config.rs and the skills dir convention).
/// The legacy `~/.config/oxo-flow/ai_config.json` location is still READ
/// as a migration fallback (see `legacy_ai_config_path`) until the
/// user's next `ai setup` rewrites it.
pub fn ai_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join(".oxo-flow")
        .join("ai_config.json")
}

/// Pre-unification location (v0.12 and earlier): the runtime read
/// `~/.config/oxo-flow/ai_config.json` while the documented path was
/// `~/.oxo-flow/ai_config.json` — providers silently ignored files
/// written to the documented location.
fn legacy_ai_config_path() -> std::path::PathBuf {
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
    save_ai_config_to(&ai_config_path(), kind, api_key, api_url, model);
}

/// [`save_ai_config`] against an explicit path (testable without touching
/// `$HOME`).
///
/// `api_key: None` means "keep the stored key": a runtime reconfiguration
/// (`POST /api/ai/config`, `AI::reconfigure`) that carries no key must not
/// destroy a credential the user saved earlier. Pass `Some("")` to clear it
/// deliberately.
fn save_ai_config_to(
    path: &std::path::Path,
    kind: &str,
    api_key: Option<&str>,
    api_url: Option<&str>,
    model: Option<&str>,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let api_key = match api_key {
        Some(key) => key.to_string(),
        None => read_stored_api_key(path),
    };
    let config = serde_json::json!({
        "provider": kind,
        "api_key": api_key,
        "api_url": api_url.unwrap_or(""),
        "model": model.unwrap_or(""),
    });
    if let Ok(json) = serde_json::to_string_pretty(&config) {
        match create_private_file(path) {
            Ok(file) => {
                use std::io::Write;
                let mut file = file;
                // The file holds a live API key in plaintext — restrict it
                // to the owner so shared HPC systems and group-readable
                // homes don't leak it. The mode is applied at creation
                // (below), so there is no world-readable window; this
                // second step also narrows a pre-existing wider file, and a
                // failure is reported rather than swallowed.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Err(e) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
                        tracing::warn!(
                            "Failed to restrict AI config {} to owner-only: {e}",
                            path.display()
                        );
                    }
                }
                if let Err(e) = file.write_all(json.as_bytes()) {
                    tracing::warn!("Failed to write AI config to {}: {e}", path.display());
                } else {
                    tracing::info!("AI config saved to {}", path.display());
                }
            }
            Err(e) => tracing::warn!("Failed to create AI config at {}: {e}", path.display()),
        }
    }
}

/// Create/truncate a config file owner-only on unix (`0600` at creation —
/// a `File::create` + later `chmod` leaves the key world-readable until the
/// chmod lands, and the chmod's error was previously ignored).
fn create_private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The api_key currently stored at `path` (empty when absent/unreadable).
fn read_stored_api_key(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json["api_key"].as_str().map(String::from))
        .unwrap_or_default()
}

/// Read the persisted AI config: `(provider, api_key, api_url, model)`.
/// Empty strings mean the field was absent. Canonical path first, with the
/// pre-unification location still honored as a migration fallback (see
/// [`ai_config_path`]).
pub fn load_ai_config() -> Option<(String, String, String, String)> {
    // Canonical path first; a legacy-location config is honored only when
    // no canonical file exists (migration — see ai_config_path docs).
    let path = ai_config_path();
    let path = if path.exists() {
        path
    } else {
        let legacy = legacy_ai_config_path();
        if legacy.exists() {
            tracing::info!(
                "reading legacy AI config at {} (run 'oxo-flow ai setup' to migrate)",
                legacy.display()
            );
            legacy
        } else {
            return None;
        }
    };
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
    fn max_tokens_env_override_parses_strictly() {
        // Thinking backends burn the default budget on reasoning blocks and
        // truncate before the answer (GLM produced empty text at 4096); the
        // default is calibrated above that ceiling (16384) and the env
        // override must accept only clean positive integers, otherwise
        // falling back to that default.
        assert_eq!(parse_max_tokens(Some("16384")), 16384);
        assert_eq!(parse_max_tokens(Some(" 8192 ")), 8192);
        assert_eq!(parse_max_tokens(Some("0")), 16384);
        assert_eq!(parse_max_tokens(Some("-1")), 16384);
        assert_eq!(parse_max_tokens(Some("abc")), 16384);
        assert_eq!(parse_max_tokens(Some("")), 16384);
        assert_eq!(parse_max_tokens(None), 16384);
    }

    #[test]
    fn timeout_env_override_parses_strictly() {
        // Same contract as the max_tokens override: clean positive integers
        // only, 300s fallback (a measured thinking round ran 145s).
        assert_eq!(parse_timeout_secs(Some("600")), 600);
        assert_eq!(parse_timeout_secs(Some(" 90 ")), 90);
        assert_eq!(parse_timeout_secs(Some("0")), 300);
        assert_eq!(parse_timeout_secs(Some("x")), 300);
        assert_eq!(parse_timeout_secs(None), 300);
    }

    #[tokio::test]
    async fn transport_retry_exhausts_connection_failures_into_retry_exhausted() {
        // Connection-refused is the benchmark's observed transport flake
        // class: the send path must retry through the backoff budget and
        // then report the total attempt count, not a bare provider error.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // nothing listens now — connects are refused

        let client = reqwest::Client::new();
        let request = client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("content-type", "application/json")
            .body(r#"{"x":1}"#);

        let backoffs = [std::time::Duration::from_millis(1)];
        // Pass-through decode, as the streaming site uses: only the send
        // half is under test here.
        let err = send_with_retry(request, "test", None, &backoffs, |resp| async move {
            Ok::<_, reqwest::Error>(resp)
        })
        .await
        .unwrap_err();

        match err {
            AiError::RetryExhausted { attempts, message } => {
                assert_eq!(attempts, backoffs.len() as u32 + 1);
                assert!(
                    message.contains("error sending request"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected RetryExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_retry_skips_timeouts() {
        // A timed-out request would deterministically re-timeout on retry —
        // the remedy is the documented OXO_FLOW_AI_TIMEOUT_SECS knob — so a
        // timeout must surface as-is instead of burning the retry budget.
        // (reqwest classifies timeouts under is_request(), the same kind as
        // the transient "error sending request" — the exclusion guard is
        // what keeps them apart.)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Accept and hold the connection open without ever responding.
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(5));
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();
        let request = client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("content-type", "application/json")
            .body(r#"{"x":1}"#);

        let backoffs = [std::time::Duration::from_millis(1)];
        let err = send_with_retry(request, "test", None, &backoffs, |resp| async move {
            Ok::<_, reqwest::Error>(resp)
        })
        .await
        .unwrap_err();

        assert!(
            matches!(err, AiError::Provider { .. }),
            "timeouts must not be retried, got {err:?}"
        );
    }

    #[tokio::test]
    async fn transport_retry_replays_garbled_response_bodies() {
        // The wave-F2 benchmark failure: a 200 whose body fails to decode
        // ("error decoding response body" — a truncated/garbled response
        // from a flaky proxy). The retry scope covers the decode, so the
        // request is refetched instead of surfacing the parse failure.
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let responses = [
                // Headers claim JSON; the body is not decodable as JSON.
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 8\r\nconnection: close\r\n\r\nnot-json",
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            ];
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf); // drain the request head
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let client = reqwest::Client::new();
        let request = client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("content-type", "application/json")
            .body(r#"{"x":1}"#);

        let backoffs = [std::time::Duration::from_millis(1)];
        let (status, _, json) = send_with_retry(request, "test", None, &backoffs, |resp| {
            resp.json::<serde_json::Value>()
        })
        .await
        .unwrap();

        assert!(status.is_success());
        assert_eq!(json, serde_json::json!({}));
    }

    #[tokio::test]
    async fn transport_retry_exhausts_garbled_bodies_with_parse_prefix() {
        // Deterministic garbage must not loop forever: after the backoff
        // budget the decode failure surfaces as RetryExhausted, keeping the
        // "response parse failed" prefix the session archival path reports.
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let response = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 8\r\nconnection: close\r\n\r\nnot-json";
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let client = reqwest::Client::new();
        let request = client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("content-type", "application/json")
            .body(r#"{"x":1}"#);

        let backoffs = [std::time::Duration::from_millis(1)];
        let err = send_with_retry(request, "test", None, &backoffs, |resp| {
            resp.json::<serde_json::Value>()
        })
        .await
        .unwrap_err();

        match err {
            AiError::RetryExhausted { attempts, message } => {
                assert_eq!(attempts, backoffs.len() as u32 + 1);
                assert!(
                    message.contains("response parse failed"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected RetryExhausted, got {other:?}"),
        }
    }

    #[test]
    fn claude_response_concatenates_multiple_text_blocks() {
        // Claude can interleave blocks (text, tool_use, text). Overwriting
        // dropped every text block but the last — the model's reasoning
        // around a tool call disappeared from the response.
        let json = serde_json::json!({
            "content": [
                {"type": "text", "text": "Let me look at the workflow. "},
                {"type": "tool_use", "id": "tu_1", "name": "read_file",
                 "input": {"path": "main.oxoflow"}},
                {"type": "text", "text": "Then I will patch rule B."}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 20}
        });
        let parsed = parse_claude_response(&json).unwrap();
        assert_eq!(
            parsed.content.as_deref(),
            Some("Let me look at the workflow. Then I will patch rule B.")
        );
        assert_eq!(parsed.tool_calls.as_ref().map(|c| c.len()), Some(1));
    }

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
    fn mask_secret_redacts_key_echoed_in_error_body() {
        // Regression: openai-compatible endpoints reflect the bearer token
        // back inside the error body, and that text reached the terminal and
        // the session log.
        let key = "sk-fake-51b482c0abcd1234";
        let body = format!(
            r#"{{"error":{{"message":"Incorrect API key provided: {key}. You can find your key at https://api.openai.com."}}}}"#
        );
        let masked = mask_secret(&body, key);
        assert!(!masked.contains(key), "key must not survive: {masked}");
        assert!(masked.contains(MASKED_KEY));
        // Everything around the key stays readable.
        assert!(masked.contains("Incorrect API key provided:"));
        assert!(masked.contains("https://api.openai.com"));
    }

    #[test]
    fn mask_secret_masks_every_occurrence_and_keeps_other_text() {
        let key = "sk-fake-51b482c0abcd1234";
        let text = format!("auth failed for {key} (Bearer {key}) retrying");
        assert_eq!(
            mask_secret(&text, key),
            "auth failed for sk-*** (Bearer sk-***) retrying"
        );
        assert_eq!(
            mask_secret("HTTP 500: upstream unavailable", key),
            "HTTP 500: upstream unavailable"
        );
    }

    #[test]
    fn mask_secret_leaves_short_values_alone() {
        // Below MASK_MIN_KEY_LEN a mask would corrupt unrelated error text
        // while protecting nothing.
        assert_eq!(mask_secret("token abc present", "abc"), "token abc present");
        assert_eq!(mask_secret("any text", ""), "any text");
    }

    #[test]
    fn classified_error_carries_masked_body() {
        // The masking happens before classification, so every variant built
        // from the body (including Auth) carries masked text.
        let key = "sk-fake-51b482c0abcd1234";
        let body = format!("invalid key {key}");
        let err = classify_http_error("openai", 401, &mask_secret(&body, key));
        let rendered = err.to_string();
        assert!(!rendered.contains(key));
        assert!(rendered.contains(MASKED_KEY));
    }

    #[test]
    fn noop_provider_returns_error() {
        let provider = AiProvider::Noop;
        assert_eq!(provider.name(), "disabled");
    }

    #[test]
    fn env_disabled_wins_over_persisted_config() {
        // Regression (issue #142 M10): `OXO_FLOW_AI_PROVIDER=disabled`
        // used to skip the env branch and fall through to the persisted
        // provider — the explicit opt-out silently did nothing. The env
        // tier is the highest-precedence tier, so it must be able to turn
        // AI off even when a saved config exists.
        let saved = ("deepseek", "sk-test", "", "");
        assert!(
            matches!(resolve_provider("disabled", Some(saved)), AiProvider::Noop),
            "OXO_FLOW_AI_PROVIDER=disabled must win over a saved config"
        );
        // Case-insensitive, like the ProviderKind parse.
        assert!(
            matches!(resolve_provider("DISABLED", Some(saved)), AiProvider::Noop),
            "the disabled spelling must be case-insensitive"
        );
    }

    #[test]
    fn env_absent_falls_back_to_saved_config() {
        let saved = ("deepseek", "sk-test", "", "");
        let provider = resolve_provider("", Some(saved));
        assert_eq!(provider.name(), "deepseek");
        // No env, no config → Noop.
        assert!(matches!(resolve_provider("", None), AiProvider::Noop));
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
        // Canonical location: ~/.oxo-flow/ai_config.json (matches the
        // AiConfig docs); the legacy ~/.config location is a read-only
        // migration fallback.
        let path = ai_config_path();
        assert!(path.to_string_lossy().contains(".oxo-flow"));
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
        let response = parse_openai_response(&json, "openai").unwrap();
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
        let response = parse_openai_response(&json, "openai").unwrap();
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
        let response = parse_openai_response(&json, "openai").unwrap();
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
        let response = parse_openai_response(&json, "openai").unwrap();
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
        let err = parse_openai_response(&json, "openai").unwrap_err();
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
    fn to_anthropic_messages_emits_tool_use_blocks() {
        let messages = vec![
            Message::system("sys"),
            Message::user("do it"),
            Message::assistant_with_tools(vec![ToolCall {
                id: "tc-1".into(),
                name: "lookup_tool".into(),
                arguments: "{\"query\":\"fastqc\"}".into(),
            }]),
            Message::tool("tc-1", "lookup_tool", "found 8"),
        ];
        let (system, msgs) = to_anthropic_messages(&messages);
        assert_eq!(system, "sys");
        // msgs: [user, assistant-with-tool_use, user-with-tool_result]
        let assistant = &msgs[1];
        assert_eq!(assistant["role"], "assistant");
        let blocks = assistant["content"].as_array().unwrap();
        let tool_use = blocks
            .iter()
            .find(|b| b["type"] == "tool_use")
            .expect("assistant turn must carry the tool_use block");
        assert_eq!(tool_use["id"], "tc-1");
        assert_eq!(tool_use["name"], "lookup_tool");
        assert_eq!(tool_use["input"]["query"], "fastqc");
        let result = &msgs[2];
        assert_eq!(result["role"], "user");
        assert_eq!(result["content"][0]["tool_use_id"], "tc-1");
    }

    #[test]
    fn two_round_tool_transcript_pairs_every_tool_use() {
        // Mirror the orchestrator's two-round transcript exactly.
        let messages = vec![
            Message::system("sys"),
            Message::user("make a pipeline"),
            Message::assistant_with_tools(vec![ToolCall {
                id: "call_00".into(),
                name: "lookup_tool".into(),
                arguments: "{\"query\":\"fastqc\"}".into(),
            }]),
            Message::tool("call_00", "lookup_tool", "found 8"),
            Message::assistant_with_tools(vec![ToolCall {
                id: "call_01".into(),
                name: "lookup_skill".into(),
                arguments: "{\"query\":\"fastqc qc\"}".into(),
            }]),
            Message::tool("call_01", "lookup_skill", "no skills"),
        ];
        let (_system, msgs) = to_anthropic_messages(&messages);
        // Every tool_use block must be immediately followed by a matching
        // tool_result block in the NEXT message (Anthropic's pairing rule).
        for (i, msg) in msgs.iter().enumerate() {
            if msg["role"] != "assistant" {
                continue;
            }
            let uses: Vec<&serde_json::Value> = msg["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|b| b["type"] == "tool_use")
                .collect();
            if uses.is_empty() {
                continue;
            }
            let next = &msgs[i + 1];
            assert_eq!(next["role"], "user", "tool_result must follow tool_use");
            let results: Vec<&serde_json::Value> = next["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|b| b["type"] == "tool_result")
                .collect();
            for u in &uses {
                assert!(
                    results.iter().any(|r| r["tool_use_id"] == u["id"]),
                    "tool_use {} has no immediate tool_result.\nassistant={}\nnext={}",
                    u["id"],
                    msg,
                    next
                );
            }
        }
    }

    #[test]
    fn multi_tool_round_coalesces_results_into_one_message() {
        // A single assistant round may carry MULTIPLE tool_use blocks;
        // Anthropic requires ALL their tool_result blocks in the ONE message
        // that immediately follows.
        let messages = vec![
            Message::system("sys"),
            Message::user("make a pipeline"),
            Message::assistant_with_tools(vec![
                ToolCall {
                    id: "call_00".into(),
                    name: "lookup_tool".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "call_01".into(),
                    name: "lookup_skill".into(),
                    arguments: "{}".into(),
                },
            ]),
            Message::tool("call_00", "lookup_tool", "found 8"),
            Message::tool("call_01", "lookup_skill", "none"),
        ];
        let (_system, msgs) = to_anthropic_messages(&messages);
        // [user, assistant(2 tool_use), user(2 tool_result)]
        assert_eq!(msgs.len(), 3, "results must coalesce: {msgs:#?}");
        let results = &msgs[2]["content"];
        let results = results.as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r["tool_use_id"] == "call_00"));
        assert!(results.iter().any(|r| r["tool_use_id"] == "call_01"));
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

    #[test]
    fn compress_transcript_drops_unpaired_tool_results() {
        // A cut that lands mid tool group would keep tool_result blocks
        // whose declaring assistant turn was dropped — Anthropic rejects
        // those outright, turning a recoverable overflow into a hard 400.
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant_with_tools(vec![
                ToolCall {
                    id: "call_00".into(),
                    name: "lookup_tool".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "call_01".into(),
                    name: "lookup_skill".into(),
                    arguments: "{}".into(),
                },
            ]),
            Message::tool("call_00", "lookup_tool", "found 8"),
            Message::tool("call_01", "lookup_skill", "none"),
            Message::user("u3"),
            Message::assistant("a3"),
            Message::user("u4"),
            Message::assistant("a4"),
        ];
        // 10 non-system turns, keep 6 → the cut lands on call_00's result;
        // both of its unpaired results are dropped with the declaring turn.
        let compressed = compress_transcript(&messages).expect("droppable turns exist");
        assert_eq!(compressed.len(), 2 + 4, "got {compressed:#?}");
        assert!(
            compressed.iter().all(|m| m.role != MessageRole::Tool),
            "unpaired tool results must not survive compression: {compressed:#?}"
        );
        assert!(compressed[1].content.contains("6 earlier turns omitted"));
        assert_eq!(compressed.last().unwrap().content, "a4");

        // Whatever survives must still convert to a paired Anthropic
        // transcript (no tool_result without a preceding tool_use).
        let (_system, msgs) = to_anthropic_messages(&compressed);
        let mut declared: Vec<String> = Vec::new();
        for m in &msgs {
            for block in m["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("tool_use") => {
                        declared.push(block["id"].as_str().unwrap_or("").to_string());
                    }
                    Some("tool_result") => assert!(
                        declared.contains(&block["tool_use_id"].as_str().unwrap_or("").to_string()),
                        "tool_result {} without a declared tool_use",
                        block["tool_use_id"]
                    ),
                    _ => {}
                }
            }
        }
        assert!(
            declared.is_empty(),
            "the compressed tail declares no tool_use, so none may remain"
        );

        // A cut that does NOT break a tool group keeps the pairing intact.
        let paired = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::user("u3"),
            Message::assistant("a3"),
            Message::user("u4"),
            Message::assistant_with_tools(vec![ToolCall {
                id: "call_09".into(),
                name: "lookup_tool".into(),
                arguments: "{}".into(),
            }]),
            Message::tool("call_09", "lookup_tool", "found 8"),
        ];
        let compressed = compress_transcript(&paired).expect("droppable turns exist");
        assert!(
            compressed.iter().any(|m| m.role == MessageRole::Tool),
            "a tool group inside the kept tail must survive: {compressed:#?}"
        );
        let (_system, msgs) = to_anthropic_messages(&compressed);
        let last = msgs.last().unwrap();
        assert_eq!(last["content"][0]["tool_use_id"], "call_09");
    }

    #[test]
    fn to_anthropic_messages_drops_undeclared_tool_result() {
        // A transcript that lost its declaring assistant turn (compressed,
        // reloaded from a session, or hand-built) must not be sent verbatim:
        // Anthropic rejects a tool_result whose tool_use_id was never
        // declared and the whole request fails with a 400.
        let messages = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::tool("call_orphan", "lookup_tool", "found 8"),
            Message::user("u2"),
        ];
        let (_system, msgs) = to_anthropic_messages(&messages);
        assert_eq!(
            msgs.len(),
            2,
            "the orphan tool_result turn is dropped: {msgs:#?}"
        );
        assert!(
            msgs.iter().all(|m| m["content"].as_array().is_none()),
            "no block array may remain: {msgs:#?}"
        );

        // A declared result still passes through unchanged.
        let paired = vec![
            Message::system("sys"),
            Message::user("u1"),
            Message::assistant_with_tools(vec![ToolCall {
                id: "call_ok".into(),
                name: "lookup_tool".into(),
                arguments: "{}".into(),
            }]),
            Message::tool("call_ok", "lookup_tool", "found 8"),
        ];
        let (_system, msgs) = to_anthropic_messages(&paired);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "call_ok");
    }

    #[test]
    fn request_bodies_include_temperature_when_set() {
        let messages = vec![Message::user("hi")];
        // 0.0 is a meaningful value ("deterministic") and must be sent,
        // not treated as unset.
        let claude = ClaudeBackend::new("k".into(), None, None).with_temperature(Some(0.0));
        assert_eq!(claude.build_body(&messages, &[])["temperature"], 0.0);
        let openai = OpenAiBackend::new("k".into(), None, None).with_temperature(Some(0.3));
        assert_eq!(openai.build_body(&messages, &[])["temperature"], 0.3);
        let ollama = build_ollama_body(&messages, &[], "llama3", Some(0.7));
        assert_eq!(ollama["options"]["temperature"], 0.7);

        // Unset → the field is omitted and the endpoint default applies.
        let claude = ClaudeBackend::new("k".into(), None, None);
        assert!(
            claude
                .build_body(&messages, &[])
                .get("temperature")
                .is_none()
        );
        let openai = OpenAiBackend::new("k".into(), None, None);
        assert!(
            openai
                .build_body(&messages, &[])
                .get("temperature")
                .is_none()
        );
        assert!(
            build_ollama_body(&messages, &[], "llama3", None)
                .get("options")
                .is_none()
        );
    }

    #[test]
    fn rate_limited_carries_retry_after_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        assert_eq!(retry_after_header(&headers).as_deref(), Some("30"));
        match classify_http_error_with_retry_after(
            "openai",
            429,
            "slow down",
            retry_after_header(&headers),
        ) {
            AiError::RateLimited { retry_after, .. } => {
                assert_eq!(retry_after.as_deref(), Some("30"))
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }

        // The HTTP-date form is not guessed, and an absent header stays None.
        let mut http_date = reqwest::header::HeaderMap::new();
        http_date.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert!(retry_after_header(&http_date).is_none());
        assert!(retry_after_header(&reqwest::header::HeaderMap::new()).is_none());
        // Callers that classify without a header keep the old behavior.
        assert!(matches!(
            classify_http_error("openai", 429, "rate limit"),
            AiError::RateLimited {
                retry_after: None,
                ..
            }
        ));
    }

    #[test]
    fn build_body_omits_empty_reasoning_content() {
        // The agent loop builds every assistant turn with
        // `reasoning_content: Some("")` when the model sent none; some
        // openai-compatible endpoints reject an empty reasoning field.
        let messages = vec![
            Message::system("sys"),
            Message::assistant_with_reasoning("answer", ""),
            Message::assistant_with_tools_and_reasoning(
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "lookup_tool".into(),
                    arguments: "{}".into(),
                }],
                "",
            ),
            Message::assistant_with_reasoning("answer", "step by step"),
        ];
        let backend = OpenAiBackend::new("k".into(), None, None);
        let body = backend.build_body(&messages, &[]);
        let msgs = body["messages"].as_array().unwrap();
        assert!(
            msgs[1].get("reasoning_content").is_none(),
            "empty reasoning must be omitted: {}",
            msgs[1]
        );
        assert!(
            msgs[2].get("reasoning_content").is_none(),
            "empty reasoning must be omitted: {}",
            msgs[2]
        );
        assert_eq!(msgs[3]["reasoning_content"], "step by step");
    }

    #[test]
    fn sse_frames_split_on_crlf_and_split_crlf_pairs() {
        // A proxy that re-frames SSE with CRLF must still stream
        // incrementally: the frame boundary is a blank line in either
        // convention.
        let mut buffer = String::new();
        let frames = push_sse_chunk(
            &mut buffer,
            "data: {\"choices\":[{\"delta\":{\"content\":\"fast\"}}]}\r\n\r\n",
        );
        assert_eq!(frames.len(), 1);
        assert!(frames[0].contains("fast"));
        assert!(buffer.is_empty(), "no partial frame may remain");

        // The \r\n pair straddles two reads — the \r must not block the split.
        let mut buffer = String::new();
        assert!(push_sse_chunk(&mut buffer, "data: {\"a\":1}\r").is_empty());
        let frames = push_sse_chunk(&mut buffer, "\n\r\ndata: {\"b\":2}\n\n");
        assert_eq!(frames.len(), 2, "got {frames:#?}");
        assert!(frames[0].contains("\"a\""));
        assert!(frames[1].contains("\"b\""));
    }

    #[test]
    fn usage_from_frame_reads_streamed_usage() {
        // `stream_options.include_usage` makes the endpoint close the stream
        // with a usage-only frame; without reading it a streamed call
        // reports zero tokens.
        let frame =
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7}}";
        assert_eq!(
            usage_from_frame(frame),
            Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 7,
            })
        );
        assert!(
            usage_from_frame("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}").is_none()
        );
        assert!(usage_from_frame("data: {\"usage\":null}").is_none());
    }

    #[test]
    fn save_ai_config_none_key_preserves_stored_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_config.json");
        save_ai_config_to(
            &path,
            "claude",
            Some("sk-original"),
            Some("https://x"),
            Some("m1"),
        );
        // Runtime reconfiguration without a key must not wipe the credential.
        save_ai_config_to(&path, "openai", None, Some("https://y"), Some("m2"));
        let read = |p: &std::path::Path| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
        };
        let after = read(&path);
        assert_eq!(after["provider"], "openai");
        assert_eq!(
            after["api_key"], "sk-original",
            "stored key must survive a keyless save"
        );
        assert_eq!(after["api_url"], "https://y");
        assert_eq!(after["model"], "m2");
        // An explicit empty key clears it.
        save_ai_config_to(&path, "openai", Some(""), None, None);
        assert_eq!(read(&path)["api_key"], "");
    }

    #[cfg(unix)]
    #[test]
    fn save_ai_config_creates_owner_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_config.json");
        save_ai_config_to(&path, "claude", Some("sk-secret"), None, None);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the config holds an API key and must never be created group/world readable"
        );

        // A pre-existing wider file is narrowed by the next save.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_ai_config_to(&path, "claude", Some("sk-secret"), None, None);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "an existing world-readable config must be narrowed"
        );
    }
}
