//! Chat service — conversational pipeline creation with multi-agent orchestration.
//!
//! Coordinates: Orchestrator → (Data Agent, Tool Expert, Validator) → Response.
//! All agents call deterministic core APIs — zero write access to DB/FS/process.

use super::types::*;
use crate::ai_provider::AiProvider;
use crate::domains::workflow::service as workflow_svc;

/// Process a chat message and return the validated pipeline. This is the
/// JSON variant of the conversational AI pipeline.
///
/// `provider` is the ACTING USER's provider (issue #82 follow-up: chat
/// runs on the caller's own AI credentials, never the shared runtime).
///
/// This endpoint previously ran its own one-shot prompt with no tools and
/// no correction loop — the weakest of the four generation paths. It now
/// runs the same shared persona + orchestrator harness as the SSE chat
/// route and the CLI, differing only in presentation; `run_id` scopes the
/// read-only run-diagnosis tools exactly as the SSE route does.
#[allow(clippy::too_many_arguments)]
pub async fn process_chat(
    message: &str,
    _session_id: Option<&str>,
    context: Option<&ChatContext>,
    templates: &[String],
    provider: &AiProvider,
    run_id: Option<&str>,
    user_id: &str,
    is_admin: bool,
) -> Result<(String, serde_json::Value), String> {
    // Phase 1: Orchestrator — understand intent
    let intent = if let Some(ctx) = context {
        if let Some(ref i) = ctx.intent {
            i.clone()
        } else {
            infer_intent(message)
        }
    } else {
        infer_intent(message)
    };

    // Phase 2: Data Agent — analyze data if paths provided
    let data_report = if let Some(ctx) = context {
        if let Some(ref paths) = ctx.data_paths {
            if !paths.is_empty() {
                analyze_data_paths(paths)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Phase 3: AI generation via the shared agent + orchestrator
    let mut agent = super::agent::ChatAgent::new(intent.clone(), message.to_string());
    if let Some(report) = &data_report
        && let Some(summary) = report.get("summary")
    {
        agent = agent.with_user_addition(format!("## Data Report\n{summary}"));
    }
    if !templates.is_empty() {
        agent = agent.with_user_addition(format!(
            "## Available Templates\n{}",
            templates
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n- ")
        ));
    }

    let ctx = oxo_flow_ai::agent::AgentContext {
        intent: intent.clone(),
        command: message.to_string(),
        workflow_path: None,
        workflow_content: None,
        external_sources: vec![],
        max_rounds: 6,
        tool_registry: super::tools::build_chat_tool_registry(run_id, user_id, is_admin),
        tool_approver: None,
        session: oxo_flow_ai::session::AiSession::new("web-chat", "chat", "web", provider.name()),
    };

    let orchestrator = Orchestrator::new(provider.clone(), 6);
    let outcome = orchestrator
        .execute(&agent, &ctx)
        .await
        .map_err(|e| {
            tracing::warn!("AI provider {} request failed: {e}", provider.name());
            format!(
                "AI provider request failed for {}: please check the provider configuration and network connectivity",
                provider.name()
            )
        })?;
    crate::ai_provider::log_generation_usage("chat", &outcome.session);
    let toml_content = outcome
        .content
        .ok_or_else(|| "AI generation did not produce a valid pipeline".to_string())?;

    // Phase 4: validation is guaranteed by the agent's validator; it is
    // re-run here only to populate the response payload.
    let validation = workflow_svc::validate_pipeline(&toml_content, None)?;

    // Phase 5: Build response
    let pipeline_id = uuid::Uuid::new_v4().to_string();
    let parsed = workflow_svc::parse_pipeline(&toml_content, None).ok();

    let response = serde_json::json!({
        "pipeline_id": pipeline_id,
        "toml_content": toml_content,
        "intent": intent,
        "data_report": data_report,
        "validation": {
            "valid": validation.valid,
            "errors": validation.errors.iter().map(|e| serde_json::json!({
                "code": e.code, "message": e.message, "suggestion": e.suggestion
            })).collect::<Vec<_>>()
        },
        "rules": parsed.as_ref().map(|p| p.rules.iter().map(|r| serde_json::json!({
            "name": r.name,
            "inputs": r.inputs,
            "outputs": r.outputs,
            "environment": r.environment,
            "threads": r.threads
        })).collect::<Vec<_>>()),
        "dag": parsed.as_ref().map(|p| serde_json::json!({
            "nodes": p.dag.nodes.iter().map(|n| serde_json::json!({
                "id": n.id, "label": n.label, "color": n.color
            })).collect::<Vec<_>>(),
            "edges": p.dag.edges.iter().map(|e| serde_json::json!({
                "from": e.from, "to": e.to
            })).collect::<Vec<_>>(),
            "parallel_groups": p.dag.parallel_groups
        }))
    });

    Ok((toml_content.clone(), response))
}

/// Infer the user's intent from their message.
pub fn infer_intent(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("rna-seq") || lower.contains("rnaseq") || lower.contains("transcriptome") {
        "RNA-seq analysis".into()
    } else if lower.contains("variant") || lower.contains("wgs") || lower.contains("germline") {
        "Variant calling".into()
    } else if lower.contains("chip-seq") || lower.contains("chipseq") {
        "ChIP-seq analysis".into()
    } else if lower.contains("single-cell") || lower.contains("scrna") || lower.contains("10x") {
        "Single-cell RNA-seq".into()
    } else if lower.contains("qc") || lower.contains("quality") || lower.contains("fastqc") {
        "Quality control".into()
    } else if lower.contains("alignment") || lower.contains("align") || lower.contains("star") {
        "Read alignment".into()
    } else {
        "Bioinformatics analysis".into()
    }
}

/// Analyze data paths using the deterministic data discovery module.
pub fn analyze_data_paths(paths: &[String]) -> Option<serde_json::Value> {
    let max_depth = Some(2usize);
    match crate::domains::workflow::data::analyze_files(paths, max_depth) {
        Ok(report) => Some(serde_json::json!({
            "files": report.files.iter().map(|f| serde_json::json!({
                "path": f.path, "size": f.size, "format": f.format,
                "format_confidence": f.format_confidence, "sample_name": f.sample_name
            })).collect::<Vec<_>>(),
            "summary": {
                "total_size": report.summary.total_size,
                "formats_detected": report.summary.formats_detected,
                "paired_end_detected": report.summary.paired_end_detected,
            },
            "suggested_workflow": report.suggested_workflow.as_ref().map(|sw| serde_json::json!({
                "template": sw.template, "confidence": sw.confidence, "reason": sw.reason
            }))
        })),
        Err(_) => None,
    }
}

// ── Grounded agent loop (real Orchestrator + knowledge tools) ──────────────

use oxo_flow_ai::agent::AgentOutcome;
use oxo_flow_ai::agent::events::{AgentEvent, AgentEventSink};
use oxo_flow_ai::agent::orchestrator::Orchestrator;

/// A live chat-agent run: the SSE handler drains `events` while the loop
/// executes, and receives the final outcome on `outcome`.
/// One item on the live chat-agent channel: an observable agent event, or
/// the terminal outcome (sent last, before the channel closes).
pub enum ChatStreamEvent {
    Agent(AgentEvent),
    /// Boxed: `AgentOutcome` carries the session record, which is much larger
    /// than the event variants — boxing keeps the channel items small.
    Outcome(Box<Result<AgentOutcome, String>>),
}

/// A live chat-agent run: the SSE handler drains `events` until the channel
/// closes — a single ordered source, no separate oneshot.
pub struct ChatAgentRun {
    pub events: tokio::sync::mpsc::Receiver<ChatStreamEvent>,
}

/// Spawn the agent loop on the tokio runtime. Events are buffered (64) —
/// the handler must keep draining to avoid backpressure.
pub fn spawn_chat_agent(
    message: String,
    _session_id: String,
    context: Option<ChatContext>,
    run_id: Option<String>,
    user_id: String,
    is_admin: bool,
    provider: AiProvider,
) -> ChatAgentRun {
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(64);
    let outcome_tx = event_tx.clone();
    tokio::spawn(async move {
        // Non-blocking: the sink is sync and runs inside the tokio runtime.
        // A full or closed channel drops the event (observability loss only;
        // on client disconnect the closing channel ends the SSE stream,
        // which is the cancellation path).
        // Accumulate text so a round-cap failure can still deliver an
        // already-generated pipeline (issue #79 P1-10). Shared through an
        // Arc so the sink closure can be `move` (it must stay Send) while
        // the post-run read keeps its own handle.
        let text_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink_buffer = text_buffer.clone();
        let mut sink = move |e: AgentEvent| {
            if let AgentEvent::Text(ref chunk) = e {
                // Lock is held for a push_str only; contention is a single
                // writer and the post-run read.
                if let Ok(mut buf) = sink_buffer.lock() {
                    buf.push_str(chunk);
                }
            }
            let _ = event_tx.try_send(ChatStreamEvent::Agent(e));
        };
        let mut result = run_chat_agent(
            &message,
            context.as_ref(),
            run_id.as_deref(),
            &user_id,
            is_admin,
            Some(&mut sink),
            &provider,
        )
        .await;
        // Degradation path: the agent burned its round budget after
        // generating a workflow — deliver the generated TOML instead of
        // discarding it. Validation in the SSE handler still gates it, so
        // garbage never reaches the editor.
        if let Err(e) = &result
            && e.contains("exceeded max rounds")
            && let Some(toml) =
                extract_generated_toml(&text_buffer.lock().map(|b| b.clone()).unwrap_or_default())
        {
            tracing::info!(
                "agent hit its round cap — delivering generated pipeline as degraded outcome"
            );
            result = Ok(AgentOutcome {
                success: true,
                content: Some(toml),
                rounds: u32::MAX / 2, // unknown; keep it out of the way
                summary: "Delivered with the generated pipeline after the agent exceeded its correction-round budget (degraded mode — review before running).".into(),
                confidence: 0.5,
                session: oxo_flow_ai::session::AiSession::default(),
            });
        }
        let _ = outcome_tx.try_send(ChatStreamEvent::Outcome(Box::new(result)));
    });
    ChatAgentRun { events: event_rx }
}

/// Best-effort extraction of a pipeline TOML from accumulated model text —
/// the degradation path for a round-cap failure (issue #79 P1-10).
/// Delegates to the canonical extractor (issue #342: one implementation,
/// with the same fenced-then-raw order and the prose guard).
pub fn extract_generated_toml(text: &str) -> Option<String> {
    oxo_flow_ai::agent::pipeline_gen::extract_toml(text)
}

pub async fn run_chat_agent(
    message: &str,
    context: Option<&ChatContext>,
    run_id: Option<&str>,
    user_id: &str,
    is_admin: bool,
    sink: Option<&mut AgentEventSink>,
    provider: &AiProvider,
) -> Result<AgentOutcome, String> {
    let agent = super::agent::ChatAgent::new(infer_intent(message), message.to_string());
    let ctx = oxo_flow_ai::agent::AgentContext {
        intent: infer_intent(message),
        command: message.to_string(),
        workflow_path: None,
        workflow_content: None,
        external_sources: vec![],
        max_rounds: 6,
        tool_registry: super::tools::build_chat_tool_registry(run_id, user_id, is_admin),
        tool_approver: None,
        session: oxo_flow_ai::session::AiSession::new("web-chat", "chat", "web", provider.name()),
    };
    // Context-supplied data paths feed the user prompt (deterministic
    // data perception stays out of the model loop).
    if let Some(ctx) = context
        && let Some(paths) = &ctx.data_paths
        && !paths.is_empty()
    {
        let _ = crate::domains::workflow::data::analyze_files(paths, Some(2));
    }

    let orchestrator = Orchestrator::new(provider.clone(), 6);
    let outcome = orchestrator
        .execute_with_sink(&agent, &ctx, sink, None)
        .await;
    if let Ok(outcome) = &outcome {
        crate::ai_provider::log_generation_usage("chat", &outcome.session);
    }
    outcome.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_intent_rnaseq() {
        let intent = infer_intent("RNA-seq differential expression");
        assert_eq!(intent, "RNA-seq analysis");
    }

    #[test]
    fn test_infer_intent_variant() {
        let intent = infer_intent("WGS germline variant calling");
        assert_eq!(intent, "Variant calling");
    }

    #[test]
    fn test_infer_intent_qc() {
        let intent = infer_intent("run fastqc quality check");
        assert_eq!(intent, "Quality control");
    }
}

#[cfg(test)]
mod extraction_tests {
    use super::*;

    #[test]
    fn extracts_fenced_toml_block() {
        let text = "Here is your workflow:\n\n```toml\n[workflow]\nname = \"scrna\"\n\n[[rules]]\nname = \"qc\"\nshell = \"fastqc\"\n```\n\nGood luck!";
        let toml = extract_generated_toml(text).expect("fenced block must extract");
        assert!(toml.contains("[workflow]"));
        assert!(toml.contains("[[rules]]"));
        assert!(!toml.contains("Here is your workflow"));
        assert!(!toml.contains("Good luck"));
    }

    #[test]
    fn falls_back_to_trailing_workflow_text() {
        let text = "Sorry for the wait. [workflow]\nname = \"x\"\n\n[[rules]]\nname = \"a\"\nshell = \"echo hi\"\n\nThat is all.";
        let toml = extract_generated_toml(text).expect("trailing text must extract");
        assert!(toml.starts_with("[workflow]"));
        assert!(toml.contains("[[rules]]"));
    }

    #[test]
    fn rejects_prose_without_rules_table() {
        assert_eq!(
            extract_generated_toml("No workflow here, just prose."),
            None
        );
        assert_eq!(
            extract_generated_toml("[workflow]\nname = \"x\"\nbut no rules follow"),
            None
        );
    }
}
