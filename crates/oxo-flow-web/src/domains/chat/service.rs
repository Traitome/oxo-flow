//! Chat service — conversational pipeline creation with multi-agent orchestration.
//!
//! Coordinates: Orchestrator → (Data Agent, Tool Expert, Validator) → Response.
//! All agents call deterministic core APIs — zero write access to DB/FS/process.

use super::types::*;
use crate::ai_provider::AiProvider;
use crate::domains::workflow::service as workflow_svc;

use super::intent::{ChatMode, classify_message};

/// System prompt for conversational turns.
///
/// Deliberately narrow: the chat route answered a pipeline-every-time bug
/// (issue #1040 P2-4), so the reply prompt keeps the model in plain-text
/// copilot territory and out of workflow generation. A message that
/// actually asks for a pipeline never reaches this prompt — the classifier
/// routes it to the generation agent first.
const CHAT_SYSTEM_PROMPT: &str = "\
You are the oxo-flow AI copilot, embedded in a bioinformatics pipeline engine. \
Answer the user's message conversationally and concisely in plain text. \
Do not generate a workflow TOML unless the user explicitly asks for one; \
if they describe a pipeline they want, say that they can ask you to build \
or design it and you will generate a validated .oxoflow workflow. \
Keep answers short and specific.";

/// Correction-round budget shared by both chat routes — the shipped
/// `[ai] max_retries` default, whose economics (knowledge lookups + a
/// correction pass in one budget) apply identically here.
fn chat_max_rounds() -> u32 {
    oxo_flow_ai::config::default_max_retries()
}

/// Who is asking, and what the request is scoped to. `run_id` admits the
/// read-only run-diagnosis tools, scoped to the acting user.
pub struct ChatInvocation<'a> {
    pub run_id: Option<&'a str>,
    pub user_id: &'a str,
    pub is_admin: bool,
}

/// Per-run observability wiring: where live events go, and the flag that
/// stops the loop once the consumer disappears. Bundled so the generation
/// helpers stay within the argument budget.
pub struct ChatRunWiring<'a> {
    pub sink: Option<&'a mut AgentEventSink>,
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

/// Session labels for a web-chat run. The usage log line prints these
/// verbatim, so they must name what actually executes: the resolved backend
/// and its resolved model — never a hard-coded page name and backend label
/// that contradict `GET /api/ai/config/effective` (issue #1040 B9:
/// `provider=web model=claude` while a GLM-class `ANTHROPIC_MODEL` override
/// was in effect).
fn chat_session(provider: &AiProvider) -> oxo_flow_ai::session::AiSession {
    oxo_flow_ai::session::AiSession::new(
        "web-chat",
        "chat",
        provider.name(),
        &provider.model().unwrap_or_else(|| "default".to_string()),
    )
}

/// Run the shared generation harness for a chat request. This is the ONE
/// place the web chat harness is assembled — context, tool registry,
/// session labels, round budget, orchestrator, usage logging — so the
/// streaming and JSON routes cannot drift.
async fn run_chat_generation(
    agent: super::agent::ChatAgent,
    message: &str,
    intent: String,
    invocation: &ChatInvocation<'_>,
    provider: &AiProvider,
    wiring: ChatRunWiring<'_>,
) -> Result<AgentOutcome, String> {
    let ChatRunWiring { sink, cancel } = wiring;
    let ctx = oxo_flow_ai::agent::AgentContext {
        intent,
        command: message.to_string(),
        workflow_path: None,
        workflow_content: None,
        external_sources: vec![],
        max_rounds: chat_max_rounds(),
        tool_registry: super::tools::build_chat_tool_registry(
            invocation.run_id,
            invocation.user_id,
            invocation.is_admin,
        ),
        tool_approver: None,
        session: chat_session(provider),
    };

    let orchestrator = Orchestrator::new(provider.clone(), chat_max_rounds());
    let outcome = orchestrator
        .execute_with_sink(&agent, &ctx, sink, cancel)
        .await;
    // Token spend is logged by the orchestrator itself (all surfaces).
    outcome.map_err(|e| e.to_string())
}

/// Answer a conversational turn with one plain-text completion.
///
/// This is the non-pipeline half of the chat surface (issue #1040 P2-4):
/// one provider round-trip, no tools, no validation loop, no TOML. Kept in
/// the service layer so the JSON and SSE routes share the exact same
/// prompt and error mapping.
pub async fn run_chat_reply(provider: &AiProvider, message: &str) -> Result<String, String> {
    provider
        .chat(CHAT_SYSTEM_PROMPT, message)
        .await
        .map(|text| text.trim().to_string())
        .map_err(|error| {
            tracing::warn!("AI chat reply for {} failed: {error}", provider.name());
            format!(
                "AI provider request failed for {}: {error}",
                provider.name()
            )
        })
}

/// Process a chat message and return either a conversational reply or the
/// validated pipeline. This is the JSON variant of the conversational AI
/// pipeline.
///
/// `provider` is the ACTING USER's provider (issue #82 follow-up: chat
/// runs on the caller's own AI credentials, never the shared runtime).
///
/// Pipeline-intent messages run the shared persona + orchestrator harness
/// (same as the SSE chat route and the CLI, differing only in
/// presentation). Chat-intent messages (issue #1040 P2-4) take the
/// single-completion [`run_chat_reply`] path instead — a chat page must
/// answer "Reply with the single word OK." with text, not a 163-second
/// workflow generation.
pub async fn process_chat(
    message: &str,
    context: Option<&ChatContext>,
    templates: &[String],
    provider: &AiProvider,
    invocation: &ChatInvocation<'_>,
) -> Result<(String, serde_json::Value), String> {
    let mode = classify_message(message, context, invocation.run_id.is_some());

    // Conversational intent: answer in text and stop before any generation
    // machinery runs. The response shape carries `mode` so clients render
    // text instead of pipeline cards.
    if mode == ChatMode::Chat {
        let reply = run_chat_reply(provider, message).await?;
        let response = serde_json::json!({
            "mode": mode.as_str(),
            "message": reply,
        });
        return Ok((reply, response));
    }

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
                analyze_data_paths_scoped(paths, invocation.user_id)
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

    let outcome = run_chat_generation(
        agent,
        message,
        intent.clone(),
        invocation,
        provider,
        ChatRunWiring {
            sink: None,
            cancel: None,
        },
    )
    .await
    .map_err(|e| {
        tracing::warn!("AI provider {} request failed: {e}", provider.name());
        format!(
            "AI provider request failed for {}: please check the provider configuration and network connectivity",
            provider.name()
        )
    })?;
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
        "mode": ChatMode::Pipeline.as_str(),
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
///
/// One owner for the keyword → intent mapping: the AI domain's orchestrator
/// holds the canonical (superset) classifier, and chat re-exports it so the
/// two surfaces cannot drift (reuse, never retype). Dependency direction is
/// chat → ai, matching the orchestration flow.
pub use crate::domains::ai::agents::orchestrator::infer_intent;

/// Resolve the caller's workspace scope and analyze their data paths;
/// a workspace error degrades to "no data report" (best-effort surface).
fn analyze_data_paths_scoped(paths: &[String], user_id: &str) -> Option<serde_json::Value> {
    let scope = crate::workspace::user_root(user_id).ok()?;
    analyze_data_paths(paths, &scope)
}

/// Analyze data paths using the deterministic data discovery module.
///
/// `scope` (#521): probes resolve inside the acting user's workspace; the
/// caller derives it from the authenticated identity, never from the
/// client-supplied paths.
pub fn analyze_data_paths(paths: &[String], scope: &std::path::Path) -> Option<serde_json::Value> {
    let max_depth = Some(2usize);
    match crate::domains::workflow::data::analyze_files(paths, max_depth, Some(scope)) {
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
        // A FULL channel drops the event (observability loss only). A
        // CLOSED channel means the SSE client disconnected — the sink flips
        // `cancel`, and the orchestrator stops between provider calls and
        // before each tool instead of burning paid rounds nobody reads.
        // Accumulate text so a round-cap failure can still deliver an
        // already-generated pipeline (issue #79 P1-10). Shared through an
        // Arc so the sink closure can be `move` (it must stay Send) while
        // the post-run read keeps its own handle.
        let text_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink_buffer = text_buffer.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink_cancel = cancel.clone();
        let mut sink = move |e: AgentEvent| {
            if let AgentEvent::Text(ref chunk) = e {
                // Lock is held for a push_str only; contention is a single
                // writer and the post-run read.
                if let Ok(mut buf) = sink_buffer.lock() {
                    buf.push_str(chunk);
                }
            }
            if matches!(
                event_tx.try_send(ChatStreamEvent::Agent(e)),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
            ) {
                sink_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        };
        let mut result = run_chat_agent(
            &message,
            context.as_ref(),
            run_id.as_deref(),
            &user_id,
            is_admin,
            &provider,
            ChatRunWiring {
                sink: Some(&mut sink),
                cancel: Some(&cancel),
            },
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
    provider: &AiProvider,
    wiring: ChatRunWiring<'_>,
) -> Result<AgentOutcome, String> {
    // The streaming route builds the SAME agent as the JSON route: an
    // explicit context intent wins over keyword inference, and analyzed data
    // paths feed the prompt as a data report.
    let intent = context
        .and_then(|c| c.intent.clone())
        .unwrap_or_else(|| infer_intent(message));
    let data_report = context
        .and_then(|c| c.data_paths.as_ref())
        .filter(|p| !p.is_empty())
        .and_then(|paths| analyze_data_paths_scoped(paths, user_id));
    let mut agent = super::agent::ChatAgent::new(intent.clone(), message.to_string());
    if let Some(report) = &data_report
        && let Some(summary) = report.get("summary")
    {
        agent = agent.with_user_addition(format!("## Data Report\n{summary}"));
    }

    let invocation = ChatInvocation {
        run_id,
        user_id,
        is_admin,
    };
    run_chat_generation(agent, message, intent, &invocation, provider, wiring).await
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
    fn chat_session_labels_name_the_resolved_backend_and_model() {
        // Issue #1040 B9: the usage log line printed `provider=web
        // model=claude` while the real backend was claude + a GLM-class
        // ANTHROPIC_MODEL override. The session must carry the resolved
        // backend name and the resolved model.
        let provider = AiProvider::Claude(oxo_flow_ai::provider::ClaudeBackend::new(
            "test-key".to_string(),
            Some("glm-4.7".to_string()),
            Some("https://gateway.example.com".to_string()),
        ));

        let session = chat_session(&provider);

        assert_eq!(session.command, "web-chat");
        assert_eq!(session.provider, "claude");
        assert_eq!(session.model, "glm-4.7");
    }

    #[test]
    fn chat_session_labels_default_when_the_backend_has_no_model() {
        let session = chat_session(&AiProvider::Noop);
        assert_eq!(session.provider, "disabled");
        assert_eq!(session.model, "default");
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
