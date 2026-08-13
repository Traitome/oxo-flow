//! Agent orchestrator — the execution loop for AI agents.
//!
//! The orchestrator runs the standard agent loop:
//!
//! ```text
//! plan → [gather (tool calls)] → act → validate → archive
//!            ↑                                  |
//!            └── validation failed: feedback ───┘
//! ```

use chrono::Utc;

use super::{Agent, AgentContext, AgentOutcome};
use crate::error::AiError;
use crate::provider::AiProvider;
use crate::session::{Modification, ToolCallRecord, archive_before_modify};
use crate::types::Message;

// ── Orchestrator ───────────────────────────────────────────────────────────

/// Executes the standard agent loop: plan → gather → act → validate → archive.
pub struct Orchestrator {
    provider: AiProvider,
    max_rounds: u32,
}

impl Orchestrator {
    pub fn new(provider: AiProvider, max_rounds: u32) -> Self {
        Self {
            provider,
            max_rounds,
        }
    }

    /// Run an agent to completion.
    ///
    /// Returns `AgentOutcome` on success, or `AiError` if the provider fails
    /// irrecoverably (e.g., auth error, rate limit).
    pub async fn execute(
        &self,
        agent: &dyn Agent,
        ctx: &AgentContext,
    ) -> Result<AgentOutcome, AiError> {
        let mut session = ctx.session.clone();
        let mut messages: Vec<Message> = Vec::new();
        let mut modifications: Vec<Modification> = Vec::new();
        let mut tool_call_records: Vec<ToolCallRecord> = Vec::new();

        // 1. PLAN — build system + user messages
        let system_msg = agent.plan(ctx);
        let user_msg = agent.user_message(ctx);
        messages.push(system_msg);
        messages.push(user_msg);

        let mut rounds: u32 = 0;
        #[allow(unused_assignments)]
        let mut final_content: Option<String> = None;

        loop {
            rounds += 1;
            if rounds > self.max_rounds {
                return Err(AiError::MaxRoundsExceeded {
                    max: self.max_rounds,
                });
            }

            // 2. GATHER/ACT — call AI with current messages + tools
            let tool_defs = ctx.tool_registry.to_defs();
            let response = self.provider.chat_with_tools(&messages, &tool_defs).await?;

            session.add_usage(&response.usage);

            // Handle tool calls from the model
            if let Some(tool_calls) = &response.tool_calls
                && !tool_calls.is_empty()
            {
                // Record assistant's tool call request
                messages.push(Message::assistant_with_tools(tool_calls.clone()));

                for tc in tool_calls {
                    let start = std::time::Instant::now();

                    // Human-approval gate: non-read-only tools (e.g. MCP
                    // tools without a readOnlyHint) execute only when an
                    // approver is provided AND approves this invocation.
                    let approver_ref = ctx.tool_approver.as_ref().map(|a| a.as_ref());
                    let approved = tool_call_approved(
                        &ctx.tool_registry,
                        approver_ref,
                        &tc.name,
                        &tc.arguments,
                    );
                    let result = if !approved {
                        Err(AiError::ToolError {
                            tool: tc.name.clone(),
                            message: "execution requires human approval".to_string(),
                        })
                    } else {
                        ctx.tool_registry.execute(&tc.name, &tc.arguments).await
                    };
                    let duration_ms = start.elapsed().as_millis() as u64;

                    match &result {
                        Ok(content) => {
                            messages.push(Message::tool(&tc.id, &tc.name, content));
                            tool_call_records.push(ToolCallRecord {
                                timestamp: Utc::now(),
                                tool_name: tc.name.clone(),
                                arguments: tc.arguments.clone(),
                                result_preview: if content.len() > 200 {
                                    format!("{}...", &content[..200])
                                } else {
                                    content.clone()
                                },
                                success: true,
                                duration_ms,
                            });
                        }
                        Err(e) => {
                            messages.push(Message::tool(&tc.id, &tc.name, &format!("Error: {e}")));
                            tool_call_records.push(ToolCallRecord {
                                timestamp: Utc::now(),
                                tool_name: tc.name.clone(),
                                arguments: tc.arguments.clone(),
                                result_preview: format!("Error: {e}"),
                                success: false,
                                duration_ms,
                            });
                        }
                    }
                }
                // Continue loop — model will process tool results
                continue;
            }

            // Model returned text content (no tool calls)
            if let Some(content) = &response.content {
                // Extract content (agent-specific, e.g., strip code fences)
                let extracted = agent.extract_content(content);

                if let Some(ref text) = extracted {
                    // 3. VALIDATE
                    let validation = agent.validate(text, ctx);

                    if validation.passed {
                        // 4. ARCHIVE — backup before applying changes
                        if let Some(ref wf_path) = ctx.workflow_path
                            && let Some(ref wf_content) = ctx.workflow_content
                        {
                            // Archive the original workflow
                            if let Ok(archive_path) =
                                archive_before_modify(wf_path, wf_content, &session.id)
                            {
                                // Record the modification
                                modifications.push(Modification {
                                    timestamp: Utc::now(),
                                    file: wf_path.clone(),
                                    before: wf_content.clone(),
                                    after: text.clone(),
                                    reason: validation.summary.clone(),
                                    round: rounds,
                                    applied: true,
                                });
                                tool_call_records.push(ToolCallRecord {
                                    timestamp: Utc::now(),
                                    tool_name: "archive".into(),
                                    arguments: String::new(),
                                    result_preview: format!(
                                        "Original archived to {}",
                                        archive_path.display()
                                    ),
                                    success: true,
                                    duration_ms: 0,
                                });
                            }
                        }

                        final_content = Some(text.clone());
                        break;
                    }

                    // Validation failed — feed errors back
                    let feedback = format!(
                        "Your previous output failed validation:\n{}\n\nPlease fix these issues and provide the corrected output.",
                        validation.errors.join("\n")
                    );
                    messages.push(Message::user(&feedback));
                    messages.push(Message::assistant(&format!(
                        "The previous output failed validation. Here are the issues:\n{}",
                        validation.errors.join("\n")
                    )));
                    // Continue loop — model will fix
                    continue;
                }
            }

            // No content, no tool calls — ask model to try again
            messages.push(Message::user(
                "Your response did not contain any content or tool calls. Please provide the expected output.",
            ));
        }

        // Build outcome
        let success = final_content.is_some();
        let confidence = if rounds == 1 {
            0.90
        } else {
            0.90 - (rounds as f64 * 0.05)
        };

        // Record messages in session (sanitized previews)
        session.messages = messages
            .iter()
            .map(crate::session::SessionMessage::from_message)
            .collect();
        session.tool_calls = tool_call_records;
        session.modifications = modifications;

        let completed = if success {
            session.complete(confidence)
        } else {
            session.fail("No valid content produced")
        };

        Ok(AgentOutcome {
            success,
            content: final_content,
            rounds,
            summary: if success {
                format!(
                    "{} completed in {rounds} round(s) with {:.0}% confidence.",
                    agent.name(),
                    confidence * 100.0
                )
            } else {
                format!("{} failed after {rounds} round(s).", agent.name())
            },
            confidence,
            session: completed,
        })
    }
}

/// Approval policy for a single tool call: read-only tools always run;
/// non-read-only tools run only when an approver is present and approves.
///
/// Shared by the orchestrator's agent loop and the CLI template loop —
/// keep the policy in this one place so the two call paths cannot drift.
pub fn tool_call_approved(
    registry: &crate::tools::ToolRegistry,
    approver: Option<&crate::agent::ToolApprover>,
    name: &str,
    arguments: &str,
) -> bool {
    if registry.is_read_only(name) {
        return true;
    }
    match (registry.get(name), approver) {
        (Some(tool), Some(approve)) => approve(&tool.def(), arguments),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentContext, ValidationResult};
    use crate::provider::AiProvider;
    use crate::session::AiSession;
    use crate::tools::ToolRegistry;
    use async_trait::async_trait;

    /// A simple agent for testing that always returns valid output.
    struct TestAgent;
    #[async_trait]
    impl Agent for TestAgent {
        fn name(&self) -> &str {
            "test-agent"
        }
        fn plan(&self, _ctx: &AgentContext) -> Message {
            Message::system("You are a helpful test agent.")
        }
        fn user_message(&self, _ctx: &AgentContext) -> Message {
            Message::user("Generate test output.")
        }
        fn validate(&self, _content: &str, _ctx: &AgentContext) -> ValidationResult {
            ValidationResult::passed()
        }
    }

    fn test_context() -> AgentContext {
        AgentContext {
            intent: "test".into(),
            command: "test".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            max_rounds: 3,
            tool_registry: ToolRegistry::new(),
            tool_approver: None,
            session: AiSession::new("test", "test", "noop", "none"),
        }
    }

    #[test]
    fn orchestrator_new_has_correct_max_rounds() {
        let orch = Orchestrator::new(AiProvider::Noop, 5);
        assert_eq!(orch.max_rounds, 5);
    }

    /// A tool whose read-only flag can be scripted for approval tests.
    struct ScriptedTool {
        name: &'static str,
        read_only: bool,
    }
    #[async_trait]
    impl crate::tools::Tool for ScriptedTool {
        fn def(&self) -> crate::types::ToolDef {
            crate::types::ToolDef {
                name: self.name.to_string(),
                description: "scripted".into(),
                parameters: serde_json::json!({}),
            }
        }
        async fn execute(&self, _arguments: &str) -> Result<String, crate::error::AiError> {
            Ok("done".into())
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
        fn name(&self) -> &str {
            self.name
        }
    }

    #[test]
    fn approval_gate_policy() {
        let mut registry = crate::tools::ToolRegistry::new();
        registry.register(Box::new(ScriptedTool {
            name: "read_only_tool",
            read_only: true,
        }));
        registry.register(Box::new(ScriptedTool {
            name: "write_tool",
            read_only: false,
        }));

        // Read-only tools always run.
        assert!(tool_call_approved(&registry, None, "read_only_tool", "{}"));
        // Non-read-only without an approver: refused.
        assert!(!tool_call_approved(&registry, None, "write_tool", "{}"));
        // Unknown tools: refused.
        assert!(!tool_call_approved(&registry, None, "ghost", "{}"));
        // Rejecting approver: refused.
        let deny = |_: &crate::types::ToolDef, _: &str| false;
        assert!(!tool_call_approved(
            &registry,
            Some(&deny),
            "write_tool",
            "{}"
        ));
        // Approving approver: allowed.
        let allow = |_: &crate::types::ToolDef, _: &str| true;
        assert!(tool_call_approved(
            &registry,
            Some(&allow),
            "write_tool",
            "{}"
        ));
    }

    #[tokio::test]
    async fn orchestrator_noop_provider_fails() {
        let orch = Orchestrator::new(AiProvider::Noop, 3);
        let agent = TestAgent;
        let ctx = test_context();
        let result = orch.execute(&agent, &ctx).await;
        assert!(result.is_err());
    }
}
