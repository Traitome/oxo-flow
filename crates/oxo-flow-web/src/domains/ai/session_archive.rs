//! Durable archive for completed web agent-loop runs.
//!
//! The orchestrator archives FAILED generations itself (one place, every
//! surface), but a successful run is returned to the caller and used to be
//! dropped. Web callers persist it here so the documented "every agent-loop
//! run is archived" contract holds for the runs that actually produced
//! output (review wave 2 MEDIUM-2). The writer is the shared crash-safe
//! temp-sibling + atomic-rename helper in
//! `oxo_flow_ai::session::save_session`, which also honors
//! `OXO_FLOW_AI_SESSION_DIR`.

use oxo_flow_ai::session::AiSession;

/// Persist a completed agent-loop session.
///
/// Best-effort: the HTTP request already has its answer, and an archive
/// failure must not turn a successful generation into an error. Failures are
/// logged with the session id so a lost audit record is diagnosable.
pub fn archive_agent_session(session: &AiSession) {
    if let Err(error) = oxo_flow_ai::session::save_session(session) {
        tracing::warn!(
            session = %session.id,
            error = %error,
            "AI session archive could not be written"
        );
    }
}
