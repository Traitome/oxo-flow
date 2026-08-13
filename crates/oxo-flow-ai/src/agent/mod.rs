//! Agent framework — multi-step AI execution with tool calling.
//!
//! # Architecture
//!
//! ```text
//! AgentContext → Agent.plan() → Orchestrator.execute() loop:
//!   ├── provider.chat_with_tools(messages, tools)
//!   ├── if tool_calls: execute tools, append results, continue
//!   └── if content: validate, if fail → feedback, continue
//!                                if pass → archive, return
//! ```

pub mod events;
pub mod orchestrator;

use async_trait::async_trait;
use std::path::PathBuf;

use crate::session::AiSession;
use crate::tools::ToolRegistry;
use crate::types::Message;

// ── AgentContext ───────────────────────────────────────────────────────────

/// Context passed to every agent run.
pub struct AgentContext {
    /// The user's original intent or query.
    pub intent: String,
    /// Which command invoked the agent: "template", "dry-run", "run", etc.
    pub command: String,
    /// Path to the workflow file (if applicable).
    pub workflow_path: Option<PathBuf>,
    /// Current workflow content (for modify-in-place commands).
    pub workflow_content: Option<String>,
    /// External knowledge sources (URLs, files) provided by the user.
    pub external_sources: Vec<ExternalSource>,
    /// Maximum correction rounds for the validation loop.
    pub max_rounds: u32,
    /// Tools available to the agent.
    pub tool_registry: ToolRegistry,
    /// Human approval callback for non-read-only tools. When `None`,
    /// non-read-only tools are always refused. The callback receives the
    /// tool definition and the JSON-encoded arguments, and returns whether
    /// execution is allowed.
    pub tool_approver: Option<std::sync::Arc<ToolApprover>>,
    /// Session for recording this agent run.
    pub session: crate::session::AiSession,
}

/// Human approval callback for non-read-only tool invocations.
/// Receives the tool definition and JSON-encoded arguments; returns
/// whether the invocation is allowed.
pub type ToolApprover = dyn Fn(&crate::types::ToolDef, &str) -> bool + Send + Sync;

/// An external knowledge source provided by the user.
#[derive(Debug, Clone)]
pub enum ExternalSource {
    /// URL content (already fetched by the CLI layer).
    Url { url: String, content: String },
    /// File content (already read by the CLI layer).
    File { path: PathBuf, content: String },
}

impl ExternalSource {
    /// Format the source for inclusion in an AI prompt.
    pub fn to_prompt_section(&self) -> String {
        match self {
            Self::Url { url, content } => {
                format!("## External Reference: {url}\n\n```\n{content}\n```\n")
            }
            Self::File { path, content } => {
                format!(
                    "## Reference File: {}\n\n```\n{content}\n```\n",
                    path.display()
                )
            }
        }
    }
}

// ── AgentOutcome ───────────────────────────────────────────────────────────

/// Result of an agent run.
pub struct AgentOutcome {
    /// Whether the agent completed successfully.
    pub success: bool,
    /// Generated or modified workflow content.
    pub content: Option<String>,
    /// Number of correction rounds taken.
    pub rounds: u32,
    /// Human-readable summary of what happened.
    pub summary: String,
    /// Agent's confidence (0.0–1.0).
    pub confidence: f64,
    /// Session record for persistence.
    pub session: AiSession,
}

// ── ValidationResult ───────────────────────────────────────────────────────

/// Result of validating agent output.
pub struct ValidationResult {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub summary: String,
}

impl ValidationResult {
    pub fn passed() -> Self {
        Self {
            passed: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            summary: "Validation passed".into(),
        }
    }

    pub fn failed(errors: Vec<String>) -> Self {
        Self {
            passed: false,
            errors,
            warnings: Vec::new(),
            summary: String::new(),
        }
    }
}

// ── Agent trait ────────────────────────────────────────────────────────────

/// An AI agent for a specific command.
///
/// Implementations define the planning prompt and validation logic.
/// The [`orchestrator::Orchestrator`] handles the tool-calling loop.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Build the initial system prompt from the agent context.
    fn plan(&self, ctx: &AgentContext) -> Message;

    /// Build the initial user message from the agent context.
    fn user_message(&self, ctx: &AgentContext) -> Message;

    /// Validate agent output.
    /// Returns ValidationResult::passed() if the output is acceptable.
    /// If validation fails, errors are fed back to the agent for correction.
    fn validate(&self, content: &str, ctx: &AgentContext) -> ValidationResult;

    /// Extract the final content from the AI response.
    /// Default: returns the content as-is, trimming whitespace.
    fn extract_content(&self, response_content: &str) -> Option<String> {
        let trimmed = response_content.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_result_passed() {
        let r = ValidationResult::passed();
        assert!(r.passed);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn validation_result_failed() {
        let r = ValidationResult::failed(vec!["error1".into()]);
        assert!(!r.passed);
        assert_eq!(r.errors.len(), 1);
    }

    #[test]
    fn external_source_url_format() {
        let src = ExternalSource::Url {
            url: "https://example.com/protocol".into(),
            content: "FASTQ QC with fastp".into(),
        };
        let section = src.to_prompt_section();
        assert!(section.contains("example.com"));
        assert!(section.contains("fastp"));
    }

    #[test]
    fn external_source_file_format() {
        let src = ExternalSource::File {
            path: PathBuf::from("refs/template.oxoflow"),
            content: "[workflow]".into(),
        };
        let section = src.to_prompt_section();
        assert!(section.contains("template.oxoflow"));
        assert!(section.contains("[workflow]"));
    }
}
