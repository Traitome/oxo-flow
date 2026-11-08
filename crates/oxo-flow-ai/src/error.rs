//! AI-specific error types.

use std::path::PathBuf;

/// Errors that can occur during AI operations.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI provider not configured. Set OXO_FLOW_AI_PROVIDER and OXO_FLOW_AI_API_KEY.")]
    NotConfigured,

    /// The process was started with the explicit off switch
    /// (`OXO_FLOW_AI_PROVIDER=disabled`). Runtime reconfiguration is refused
    /// so no persisted setting, API request, or config-file tier can
    /// resurrect AI behind the operator's back.
    #[error("AI is disabled by OXO_FLOW_AI_PROVIDER=disabled; runtime reconfiguration is refused")]
    DisabledByEnv,

    #[error("AI provider '{provider}' failed: {message}")]
    Provider { provider: String, message: String },

    /// The request exceeded the provider client's per-request timeout
    /// (`OXO_FLOW_AI_TIMEOUT_SECS`). Kept distinct from [`AiError::Provider`]
    /// so HTTP surfaces can map this class to the documented
    /// `504 AI_TIMEOUT` even when the client timer fires before the route's
    /// total budget (review wave 2 HIGH-1).
    #[error("AI provider '{provider}' request timed out: {message}")]
    Timeout { provider: String, message: String },

    #[error("AI request failed after {attempts} attempts: {message}")]
    RetryExhausted { attempts: u32, message: String },

    #[error("AI response did not contain expected content")]
    EmptyResponse,

    #[error("Tool '{tool}' execution failed: {message}")]
    ToolError { tool: String, message: String },

    #[error("Tool '{tool}' not found in registry")]
    ToolNotFound { tool: String },

    #[error("Agent exceeded max rounds ({max})")]
    MaxRoundsExceeded { max: u32 },

    #[error("Session persistence failed at {path}: {message}")]
    SessionError { path: PathBuf, message: String },

    #[error("Configuration error: {message}")]
    Config { message: String },

    #[error("MCP transport error: {message}")]
    Transport { message: String },

    #[error("MCP protocol error: {message}")]
    Protocol { message: String },

    /// The server terminated the MCP session (HTTP 404 while a
    /// `Mcp-Session-Id` was attached). The transport spec requires the
    /// client to start a new session with a fresh `initialize`; the client
    /// does this internally, so callers only see this error when the
    /// re-initialized session also fails.
    #[error("MCP session expired (HTTP 404); reinitialize required")]
    SessionExpired,

    #[error("Network error: {message}")]
    Network { message: String },

    /// A provider response could not be parsed as the expected JSON shape.
    /// Distinct from [`AiError::Config`]: the configuration is fine — the
    /// model's/server's payload is what failed to parse.
    #[error("Invalid AI response: {message}")]
    InvalidResponse { message: String },

    /// Rate limited (HTTP 429). `retry_after` is whole seconds, parsed from
    /// the provider's `Retry-After` header (digits form; HTTP-date form is
    /// reported as absent — callers fall back to their own backoff).
    #[error("Rate limited by {provider}.{}", match retry_after {
        Some(secs) => format!(" Retry-After: {secs}s"),
        None => String::new(),
    })]
    RateLimited {
        provider: String,
        retry_after: Option<u64>,
    },

    #[error("Authentication failed for {provider}: {message}")]
    Auth { provider: String, message: String },

    /// The request exceeded the provider's context window. Retrying the
    /// same transcript is pointless — compress it first.
    #[error("Context window exceeded for {provider}: {message}")]
    ContextOverflow { provider: String, message: String },

    /// The provider rejected the requested output size.
    #[error("Output limit exceeded for {provider}: {message}")]
    OutputLimit { provider: String, message: String },
}

impl AiError {
    /// Whether this failure is the provider's per-request timeout.
    ///
    /// The non-streaming AI JSON routes answer `504 AI_TIMEOUT` for this
    /// class, whether the route's total budget or the provider client's
    /// timer expired first.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }
}

impl From<serde_json::Error> for AiError {
    fn from(e: serde_json::Error) -> Self {
        AiError::InvalidResponse {
            message: format!("JSON parse/serialization error: {e}"),
        }
    }
}
