//! The web chat agent — a thin facade over the SHARED
//! [`PipelineGenAgent`] persona (issue #342), grounded in the embedded
//! knowledge bases and validated by the web workflow service.
//!
//! Before the unification this type carried its own 7-rule prompt that
//! described a schema the engine does not have (`inputs`/`outputs` maps,
//! `depends`, a `[resources]` section) — every artifact failed the static
//! gates with E017. The prompt, extraction, and validation contract now
//! come from one place; only the validator binding is web-specific.

use oxo_flow_ai::agent::pipeline_gen::PipelineGenAgent;
use oxo_flow_ai::agent::{Agent, AgentContext, ValidationResult};
use oxo_flow_ai::types::Message;

use crate::domains::workflow::service as workflow_svc;

pub struct ChatAgent {
    inner: PipelineGenAgent,
}

impl ChatAgent {
    pub fn new(intent: String, user_message: String) -> Self {
        Self {
            inner: PipelineGenAgent::new(intent)
                .with_validator(workflow_svc::pipeline_output_validator())
                // The raw user message (free-form chat turn) supplements the
                // intent-derived request line the shared persona builds.
                .with_user_addition(format!("## User Message\n{user_message}")),
        }
    }

    /// Additional user-prompt sections (data report, template hints).
    pub fn with_user_addition(mut self, section: impl Into<String>) -> Self {
        self.inner = self.inner.with_user_addition(section);
        self
    }
}

impl Agent for ChatAgent {
    fn name(&self) -> &str {
        "chat-agent"
    }

    fn plan(&self, ctx: &AgentContext) -> Message {
        self.inner.plan(ctx)
    }

    fn user_message(&self, ctx: &AgentContext) -> Message {
        self.inner.user_message(ctx)
    }

    fn extract_content(&self, response_content: &str) -> Option<String> {
        self.inner.extract_content(response_content)
    }

    fn validate(&self, content: &str, ctx: &AgentContext) -> ValidationResult {
        self.inner.validate(content, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_content_strips_fences() {
        let agent = ChatAgent::new("qc".into(), "run qc".into());
        let out = agent.extract_content("Here:\n```toml\n[workflow]\nname = \"x\"\n```\nDone");
        assert_eq!(out.as_deref(), Some("[workflow]\nname = \"x\""));
    }

    #[test]
    fn validate_rejects_invalid_toml() {
        let agent = ChatAgent::new("qc".into(), "run qc".into());
        let result = agent.validate(
            "[workflow]\nname = ",
            &AgentContext {
                intent: "x".into(),
                command: "x".into(),
                workflow_path: None,
                workflow_content: None,
                external_sources: vec![],
                max_rounds: 1,
                tool_registry: oxo_flow_ai::tools::ToolRegistry::new(),
                tool_approver: None,
                session: oxo_flow_ai::session::AiSession::new("t", "t", "noop", "none"),
            },
        );
        assert!(!result.passed);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn plan_teaches_the_engine_schema() {
        // The regression the unification fixes: the persona must describe
        // [[rules]] arrays, not the dialect the engine rejects.
        let agent = ChatAgent::new("qc".into(), "run qc".into());
        let msg = agent.plan(&AgentContext {
            intent: "x".into(),
            command: "x".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            max_rounds: 1,
            tool_registry: oxo_flow_ai::tools::ToolRegistry::new(),
            tool_approver: None,
            session: oxo_flow_ai::session::AiSession::new("t", "t", "noop", "none"),
        });
        assert!(msg.content.contains("[[rules]]"));
        assert!(msg.content.contains("depends_on"));
    }
}
