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

    /// Whether the provider was called and may have billed before failing.
    ///
    /// A max-round exhaustion means every configured round ran; an empty or
    /// invalid response and a tool error prove the provider answered at
    /// least once. Transport/auth/rate-limit/HTTP failures are not counted —
    /// they carry no completion evidence, so the fallback chain may still
    /// try the next credential without doubling a generation that already
    /// produced output. Used by the web translate spend guard.
    pub fn may_have_billed(&self) -> bool {
        matches!(
            self,
            Self::MaxRoundsExceeded { .. }
                | Self::EmptyResponse
                | Self::InvalidResponse { .. }
                | Self::ToolError { .. }
                | Self::ToolNotFound { .. }
        )
    }

    /// A credential- and endpoint-free summary for user-visible attribution.
    ///
    /// [`Display`](std::fmt::Display) can embed the request URL (reqwest's
    /// `error sending request for url (...)`) or upstream response text.
    /// Web surfaces that return failure reasons to callers must use this
    /// summary instead: it keeps the actionable category (auth, timeout,
    /// max rounds, rate limit, overflow) and drops hosts, paths, bodies,
    /// and credentials.
    pub fn attribution_reason(&self) -> String {
        match self {
            Self::NotConfigured => "AI provider not configured".into(),
            Self::DisabledByEnv => "AI disabled by environment".into(),
            Self::Provider { message, .. } => {
                let status = message
                    .strip_prefix("HTTP ")
                    .and_then(|rest| {
                        rest.split(|c: char| c == ':' || c.is_ascii_whitespace())
                            .next()
                    })
                    .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()));
                match status {
                    Some(code) => format!("provider error (HTTP {code})"),
                    None => "provider error".into(),
                }
            }
            Self::Timeout { .. } => "request timed out".into(),
            Self::RetryExhausted { attempts, .. } => {
                format!("request failed after {attempts} attempts")
            }
            Self::EmptyResponse => "empty AI response".into(),
            Self::ToolError { tool, .. } => format!("tool '{tool}' failed"),
            Self::ToolNotFound { tool } => format!("tool '{tool}' not found"),
            Self::MaxRoundsExceeded { max } => format!("Agent exceeded max rounds ({max})"),
            Self::SessionError { .. } => "session archive write failed".into(),
            Self::Config { .. } => "AI configuration error".into(),
            Self::Transport { .. } => "MCP transport error".into(),
            Self::Protocol { .. } => "MCP protocol error".into(),
            Self::SessionExpired => "MCP session expired".into(),
            Self::Network { .. } => "network error (provider endpoint unreachable)".into(),
            Self::InvalidResponse { .. } => "invalid AI response".into(),
            Self::RateLimited { retry_after, .. } => match retry_after {
                Some(secs) => format!("rate limited (retry after {secs}s)"),
                None => "rate limited".into(),
            },
            Self::Auth { .. } => "authentication failed".into(),
            Self::ContextOverflow { .. } => "context window exceeded".into(),
            Self::OutputLimit { .. } => "output limit exceeded".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_reason_drops_endpoint_urls_and_upstream_bodies() {
        // The live matrix saw the provider endpoint echoed by reqwest's
        // Display; the user-visible reason must never carry it.
        let network = AiError::Network {
            message: "error sending request for url \
                      (http://127.0.0.1:9/v1/chat/completions)"
                .into(),
        };
        let reason = network.attribution_reason();
        assert!(!reason.contains("http"), "{reason}");
        assert!(!reason.contains("127.0.0.1"), "{reason}");
        assert!(reason.contains("network"), "{reason}");

        let provider = AiError::Provider {
            provider: "openai".into(),
            message: "HTTP 502: bad gateway at https://internal.example/v1".into(),
        };
        assert_eq!(provider.attribution_reason(), "provider error (HTTP 502)");

        let timeout = AiError::Timeout {
            provider: "openai".into(),
            message: "timed out for http://10.0.0.1:8080/v1".into(),
        };
        let reason = timeout.attribution_reason();
        assert!(!reason.contains("10.0.0.1"), "{reason}");
        assert!(reason.contains("timed out"), "{reason}");
    }

    #[test]
    fn may_have_billed_covers_the_max_round_incident_class() {
        // The live incident burned 10 OpenAI rounds before falling back; the
        // guard must stop the chain for this class even though the error
        // carries no completion tokens (it never reached a final answer).
        assert!(AiError::MaxRoundsExceeded { max: 10 }.may_have_billed());
        assert!(AiError::EmptyResponse.may_have_billed());
        assert!(
            AiError::InvalidResponse {
                message: "bad json".into()
            }
            .may_have_billed()
        );
        assert!(
            AiError::ToolError {
                tool: "lookup_tool".into(),
                message: "bad args".into()
            }
            .may_have_billed()
        );

        // No completion evidence: the chain may still try the next
        // credential.
        assert!(
            !AiError::Network {
                message: "x".into()
            }
            .may_have_billed()
        );
        assert!(
            !AiError::Auth {
                provider: "x".into(),
                message: "401".into()
            }
            .may_have_billed()
        );
        assert!(
            !AiError::Timeout {
                provider: "x".into(),
                message: "x".into()
            }
            .may_have_billed()
        );
        assert!(
            !AiError::RateLimited {
                provider: "x".into(),
                retry_after: None
            }
            .may_have_billed()
        );
    }
}
