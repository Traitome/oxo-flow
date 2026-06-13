//! Server-Sent Events (SSE) infrastructure for oxo-flow-web.
//!
//! Re-exports from the canonical SSE implementation in `crate::sse`.
//! Provides real-time event broadcasting for workflow execution updates,
//! heartbeat keep-alive, and streaming event delivery to connected clients.

// Re-export the canonical implementations
pub use crate::sse::broadcast_event;
pub use crate::sse::event_tx;
pub use crate::sse::sse_events;
