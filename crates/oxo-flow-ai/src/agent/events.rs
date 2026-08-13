//! Orchestrator event stream — real-time observability for agent loops.
//!
//! The web chat forwards these over SSE; the CLI passes `None` and sees no
//! behavior change. Text events carry complete round responses (the agent
//! loop is full-response; token-level streaming is available separately via
//! `AiProvider::chat_stream` for single-shot calls).

use serde::Serialize;

/// One observable step in the agent loop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AgentEvent {
    /// The loop entered a phase (planning, executing tools, validating).
    Status(String),
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
