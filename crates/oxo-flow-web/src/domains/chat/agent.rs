//! The web chat agent — a thin `Agent` implementation over oxo-flow-ai's
//! orchestrator, grounded in the embedded knowledge bases (assembled via
//! `knowledge::assembler::for_generate`) and validated by the core engine.

use oxo_flow_ai::agent::{Agent, AgentContext, ValidationResult};
use oxo_flow_ai::types::Message;

use crate::domains::workflow::service as workflow_svc;

pub struct ChatAgent {
    pub intent: String,
    pub user_message: String,
}

impl ChatAgent {
    pub fn new(intent: String, user_message: String) -> Self {
        Self {
            intent,
            user_message,
        }
    }
}

impl Agent for ChatAgent {
    fn name(&self) -> &str {
        "chat-agent"
    }

    fn plan(&self, ctx: &AgentContext) -> Message {
        let assembled = oxo_flow_ai::knowledge::assembler::for_generate(ctx);
        let mut prompt = format!(
            "You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.\n\n\
             Intent: {}\n\n\
             Rules:\n\
             1. Output the TOML in ```toml code fences\n\
             2. Use well-known bioinformatics tools with correct command-line syntax\n\
             3. Include the [workflow] section with name, version, description\n\
             4. Define rules with name, input, output, shell\n\
             5. Use {{sample}} wildcard for sample-varying paths\n\
             6. input and output MUST be TOML arrays, e.g. input = [\"reads/{{sample}}.fastq.gz\"]\n\
             7. When validation errors are reported back, fix the TOML directly\n\
                from the error text — do not call more tools\n",
            self.intent
        );
        for addition in &assembled.system_additions {
            prompt.push_str("\n\n");
            prompt.push_str(addition);
        }
        Message::system(&prompt)
    }

    fn user_message(&self, _ctx: &AgentContext) -> Message {
        Message::user(&format!(
            "Generate a .oxoflow pipeline for: {}",
            self.user_message
        ))
    }

    /// Strip TOML code fences; keep raw TOML as-is.
    fn extract_content(&self, content: &str) -> Option<String> {
        if let Some(start) = content.find("```toml") {
            let start = start + 7;
            if let Some(end) = content[start..].find("```") {
                return Some(content[start..start + end].trim().to_string());
            }
        }
        if content.contains("[workflow]") {
            return Some(content.to_string());
        }
        Some(content.trim().to_string())
    }

    /// Grounded validation: the extracted TOML must pass the core engine's
    /// own validation — errors feed back into the loop for correction.
    fn validate(&self, content: &str, _ctx: &AgentContext) -> ValidationResult {
        match workflow_svc::validate_pipeline(content) {
            Ok(v) if v.valid => ValidationResult::passed(),
            Ok(v) => ValidationResult {
                passed: false,
                errors: v.errors.iter().map(|e| e.message.clone()).collect(),
                warnings: vec![],
                summary: format!("{} validation error(s)", v.errors.len()),
            },
            Err(e) => ValidationResult {
                passed: false,
                errors: vec![e],
                warnings: vec![],
                summary: "validation failed".into(),
            },
        }
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
}
