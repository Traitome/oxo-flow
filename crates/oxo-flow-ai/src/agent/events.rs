//! Orchestrator event stream — real-time observability for agent loops.
//!
//! The web chat forwards these over SSE; the CLI passes `None` and sees no
//! behavior change. Text events carry complete round responses (the agent
//! loop is full-response; token-level streaming is not implemented).

use serde::Serialize;

/// Structured progress for one agent-loop round.
///
/// Emitted before each provider call and at every phase boundary so long
/// multi-round generations (8–14 minutes of tool + validation rounds) have
/// observable motion instead of a static "planning" label. The web chat
/// forwards this verbatim over SSE; the CLI ignores it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressEvent {
    /// 1-based loop round (0 for the initial plan phase).
    pub round: u32,
    /// Round budget for this generation.
    pub max_rounds: u32,
    /// Phase within the round: `plan`, `generate`, `tool`, `validate`.
    pub step: String,
    /// Tool name when `step == "tool"`.
    pub tool: Option<String>,
    /// Milliseconds since the loop started.
    pub elapsed_ms: u64,
    /// Cumulative prompt tokens billed so far.
    pub prompt_tokens: u64,
    /// Cumulative completion tokens billed so far.
    pub completion_tokens: u64,
}

/// One observable step in the agent loop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AgentEvent {
    /// The loop entered a phase (planning, executing tools, validating).
    Status(String),
    /// Structured round progress (round index, phase, elapsed, usage).
    Progress(ProgressEvent),
    /// The model requested a tool call.
    ToolCall { name: String, args: String },
    /// A tool call finished; `summary` is a truncated preview.
    ToolResult { name: String, summary: String },
    /// The model returned text this round (complete response).
    Text(String),
    /// A structured action the caller may act on (reserved).
    Action(String, serde_json::Value),
    /// The loop finished.
    Done,
}

/// Sink receiving orchestrator events (mutable, single-threaded by the loop).
pub type AgentEventSink = dyn FnMut(AgentEvent) + Send;
