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

    #[error("Network error: {message}")]
    Network { message: String },

    #[error("Rate limited by {provider}. Retry after {retry_after:?}")]
    RateLimited {
        provider: String,
        retry_after: Option<String>,
    },

    #[error("Authentication failed for {provider}: {message}")]
    Auth { provider: String, message: String },
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
        AiError::Config {
            message: format!("JSON serialization error: {e}"),
        }
    }
}
