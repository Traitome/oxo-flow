//! AI-specific error types.

use std::path::PathBuf;

/// Errors that can occur during AI operations.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI provider not configured. Set OXO_FLOW_AI_PROVIDER and OXO_FLOW_AI_API_KEY.")]
    NotConfigured,

    #[error("AI provider '{provider}' failed: {message}")]
    Provider { provider: String, message: String },

    #[error("AI request failed after {attempts} attempts: {message}")]
    RetryExhausted { attempts: u32, message: String },

    #[error("AI response did not contain expected content")]
    EmptyResponse,

    #[error("AI response did not contain valid TOML: {details}")]
    InvalidToml { details: String },

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

impl From<reqwest::Error> for AiError {
    fn from(e: reqwest::Error) -> Self {
        AiError::Network {
            message: e.to_string(),
        }
    }
}

impl From<serde_json::Error> for AiError {
    fn from(e: serde_json::Error) -> Self {
        AiError::InvalidResponse {
            message: format!("JSON parse/serialization error: {e}"),
        }
    }
}
