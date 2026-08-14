//! Chat service — conversational pipeline creation with multi-agent orchestration.
//!
//! Coordinates: Orchestrator → (Data Agent, Tool Expert, Validator) → Response.
//! All agents call deterministic core APIs — zero write access to DB/FS/process.

use super::types::*;
use crate::ai_provider::AiProviderRegistry;
use crate::domains::workflow::service as workflow_svc;

/// Process a chat message and return SSE events via a channel.
/// This is the main entry point for the conversational AI pipeline.
pub async fn process_chat(
    message: &str,
    _session_id: Option<&str>,
    context: Option<&ChatContext>,
    templates: &[String],
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

    // Phase 3: AI generation via provider
    let provider = AiProviderRegistry::global().get_provider();
    let system_prompt = build_system_prompt(&intent, data_report.as_ref(), templates);
    let user_prompt = format!("Generate a .oxoflow pipeline for: {message}");

    let ai_response = provider
        .chat(&system_prompt, &user_prompt)
        .await
        .map_err(|e| format!("AI generation failed: {e}"))?;

    // Phase 4: Extract TOML and validate
    let toml_content =
        extract_toml_from_response(&ai_response).unwrap_or_else(|| ai_response.clone());

    let validation = workflow_svc::validate_pipeline(&toml_content)?;

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

    Ok((ai_response, response))
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

/// Build the system prompt for the AI with all available context.
fn build_system_prompt(
    intent: &str,
    data_report: Option<&serde_json::Value>,
    templates: &[String],
) -> String {
    let mut prompt = format!(
        "You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.\n\n\
         Intent: {intent}\n\n\
         Rules:\n\
         1. Output TOML in ```toml code fences\n\
         2. Use well-known bioinformatics tools with correct command-line syntax\n\
         3. Include [workflow] section with name, version, description\n\
         4. Define rules with name, shell, inputs, outputs, depends\n\
         5. Use {{sample}} wildcard for sample-varying paths\n\
         6. Specify conda environment for each rule when possible\n\
         7. Include resource hints (threads, memory) in [resources] section\n"
    );

    if let Some(report) = data_report {
        if let Some(summary) = report.get("summary") {
            prompt.push_str(&format!(
                "\nData summary: Formats={}, Paired-end={}\n",
                summary
                    .get("formats_detected")
                    .and_then(|f| f.as_array())
                    .map(|a| a
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default(),
                summary
                    .get("paired_end_detected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            ));
        }
        if let Some(sw) = report.get("suggested_workflow")
            && let Some(template) = sw.get("template").and_then(|v| v.as_str())
        {
            prompt.push_str(&format!("Suggested template: {template}\n"));
        }
    }

    if !templates.is_empty() {
        prompt.push_str(&format!(
            "\nAvailable templates for reference: {}\n",
            templates
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    prompt
}

/// Extract TOML content from an AI response (code fences or raw).
fn extract_toml_from_response(response: &str) -> Option<String> {
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            return Some(response[start..start + end].trim().to_string());
        }
    }
    if response.contains("[workflow]") {
        return Some(response.to_string());
    }
    None
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
            Some(&mut sink),
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
/// the degradation path for a round-cap failure (issue #79 P1-10). Prefers
/// a fenced ```toml block; falls back to everything from the first
/// `[workflow]` line to the end. Both forms must contain at least one rules
/// table so prose is never mistaken for a pipeline.
pub fn extract_generated_toml(text: &str) -> Option<String> {
    fn looks_like_pipeline(candidate: &str) -> bool {
        candidate.contains("[workflow]")
            && candidate.lines().any(|l| {
                l.trim_start().starts_with("[[rules]]") || l.trim_start().starts_with("[rules]")
            })
    }

    for fence in ["```toml", "```TOML", "```"] {
        if let Some(start) = text.find(fence) {
            let after = &text[start + fence.len()..];
            let end = after.find("```");
            let block = match end {
                Some(e) => after[..e].trim().to_string(),
                None => after.trim().to_string(),
            };
            if looks_like_pipeline(&block) {
                return Some(block);
            }
        }
    }
    let idx = text.find("[workflow]")?;
    let candidate = text[idx..].trim().to_string();
    if looks_like_pipeline(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

pub async fn run_chat_agent(
    message: &str,
    context: Option<&ChatContext>,
    run_id: Option<&str>,
    sink: Option<&mut AgentEventSink>,
) -> Result<AgentOutcome, String> {
    let provider = AiProviderRegistry::global().get_provider();
    let agent = super::agent::ChatAgent::new(infer_intent(message), message.to_string());
    let ctx = oxo_flow_ai::agent::AgentContext {
        intent: infer_intent(message),
        command: message.to_string(),
        workflow_path: None,
        workflow_content: None,
        external_sources: vec![],
        max_rounds: 6,
        tool_registry: super::tools::build_chat_tool_registry(run_id),
        tool_approver: None,
        session: oxo_flow_ai::session::AiSession::new("web-chat", "chat", "web", "web-provider"),
    };
    // Context-supplied data paths feed the user prompt (deterministic
    // data perception stays out of the model loop).
    if let Some(ctx) = context
        && let Some(paths) = &ctx.data_paths
        && !paths.is_empty()
    {
        let _ = crate::domains::workflow::data::analyze_files(paths, Some(2));
    }

    let orchestrator = Orchestrator::new(provider, 6);
    orchestrator
        .execute_with_sink(&agent, &ctx, sink, None)
        .await
        .map_err(|e| e.to_string())
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

    #[test]
    fn test_extract_toml_fenced() {
        let response = "Here:\n```toml\n[workflow]\nname = \"test\"\n```\nDone";
        let toml = extract_toml_from_response(response);
        assert_eq!(toml, Some("[workflow]\nname = \"test\"".into()));
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt("RNA-seq", None, &["rnaseq".into()]);
        assert!(prompt.contains("RNA-seq"));
        assert!(prompt.contains("rnaseq"));
        assert!(prompt.contains("[workflow]"));
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
