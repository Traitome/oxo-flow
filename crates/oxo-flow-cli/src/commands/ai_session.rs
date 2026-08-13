//! Shared AI session management — wired into all AI command paths.
//!
//! Ensures every AI interaction is logged with cost tracking and
//! session persistence to `.oxo-flow/ai_sessions/`.

use colored::Colorize;
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::session::AiSession;
use oxo_flow_ai::types::Usage;
use std::path::Path;

/// Run an AI operation with full session tracking.
///
/// Wraps the AI call, accumulates usage, saves the session, and
/// prints a cost summary. This is the SINGLE entry point for all
/// AI command interactions — ensuring Layers 1-2-3 are properly
/// connected for observability.
pub struct AiCommandSession {
    session: AiSession,
    provider_name: String,
    model_name: String,
}

impl AiCommandSession {
    /// Begin a new AI session for a command.
    pub fn begin(command: &str, intent: &str, provider: &AiProvider) -> Self {
        let session = AiSession::new(
            command,
            intent,
            provider.name(),
            &provider.model().unwrap_or_else(|| "default".into()),
        );
        Self {
            provider_name: provider.name().to_string(),
            model_name: provider.model().unwrap_or_else(|| "default".into()),
            session,
        }
    }

    /// Create with workflow path context.
    pub fn with_workflow(self, path: &Path) -> Self {
        let session = self.session.with_workflow(path);
        Self {
            session,
            provider_name: self.provider_name,
            model_name: self.model_name,
        }
    }

    /// Record usage from an AI call.
    pub fn record_usage(&mut self, usage: &Usage) {
        self.session.add_usage(usage);
    }

    /// Record a tool invocation made during the agent loop.
    pub fn record_tool_call(
        &mut self,
        name: &str,
        arguments: &str,
        result_preview: &str,
        success: bool,
        duration_ms: u64,
    ) {
        self.session
            .tool_calls
            .push(oxo_flow_ai::session::ToolCallRecord {
                timestamp: chrono::Utc::now(),
                tool_name: name.to_string(),
                arguments: arguments.to_string(),
                result_preview: result_preview.chars().take(200).collect(),
                success,
                duration_ms,
            });
    }

    /// Complete the session successfully and save to disk.
    pub fn complete(mut self, confidence: f64) {
        self.session = self.session.complete(confidence);
        if let Err(e) = oxo_flow_ai::session::save_session(&self.session) {
            tracing::warn!("Failed to save AI session: {e}");
        }
        let input_tokens = self.session.total_usage.prompt_tokens;
        let output_tokens = self.session.total_usage.completion_tokens;
        if input_tokens > 0 || output_tokens > 0 {
            println!(
                "{} AI session: {} | tokens: {} in + {} out",
                "  ✓".green(),
                self.session.id,
                input_tokens,
                output_tokens
            );
        } else {
            println!("{} AI session: {}", "  ✓".green(), self.session.id);
        }
    }

    /// Mark the session as failed and save to disk.
    pub fn fail(mut self, error: &str) {
        self.session = self.session.fail(error);
        let _ = oxo_flow_ai::session::save_session(&self.session);
        tracing::warn!("AI session failed: {} — {}", self.session.id, error);
    }

    /// Borrow the underlying session.
    pub fn session(&self) -> &AiSession {
        &self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_creation() {
        let provider = AiProvider::Noop;
        let cmd = AiCommandSession::begin("test", "intent", &provider);
        assert_eq!(cmd.session.command, "test");
    }
}
