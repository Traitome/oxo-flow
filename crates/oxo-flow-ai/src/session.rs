//! AI session logging and modification archiving.
//!
//! Every AI agent interaction produces an `AiSession` record with full
//! conversation history, tool calls, and modifications. Sessions are
//! persisted as JSON files in `.oxo-flow/ai_sessions/`. Modified workflow
//! files are archived in `.oxo-flow/ai_archive/`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::AiError;
use crate::types::Usage;

// ── AiSession ──────────────────────────────────────────────────────────────

/// A complete record of one AI agent interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSession {
    /// Unique session identifier.
    pub id: String,
    /// When the session started.
    pub started_at: DateTime<Utc>,
    /// When the session ended.
    pub ended_at: Option<DateTime<Utc>>,
    /// Which command triggered this session: "template", "dry-run", "run", etc.
    pub command: String,
    /// Path to the workflow file involved (if any).
    pub workflow: Option<PathBuf>,
    /// The user's original intent / input.
    pub user_intent: String,
    /// Full message history (system, user, assistant, tool).
    pub messages: Vec<SessionMessage>,
    /// Every tool call made during the session.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Every modification proposed or applied.
    pub modifications: Vec<Modification>,
    /// Which AI provider was used.
    pub provider: String,
    /// Which model was used.
    pub model: String,
    /// Cumulative token usage.
    pub total_usage: Usage,
    /// Final outcome.
    pub outcome: SessionOutcome,
    /// Agent's confidence estimate (0.0–1.0).
    pub confidence: f64,
    /// Error message if the session failed.
    pub error: Option<String>,
}

impl Default for AiSession {
    fn default() -> Self {
        Self {
            id: String::new(),
            started_at: chrono::Utc::now(),
            ended_at: None,
            command: String::new(),
            workflow: None,
            user_intent: String::new(),
            messages: Vec::new(),
            tool_calls: Vec::new(),
            modifications: Vec::new(),
            provider: String::new(),
            model: String::new(),
            total_usage: crate::types::Usage::default(),
            outcome: SessionOutcome::Running,
            confidence: 0.0,
            error: None,
        }
    }
}

impl AiSession {
    /// Create a new session with a unique ID.
    pub fn new(command: &str, user_intent: &str, provider: &str, model: &str) -> Self {
        let now = Utc::now();
        let id = format!(
            "{}-{}-{}",
            now.format("%Y%m%d-%H%M%S"),
            command,
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        Self {
            id,
            started_at: now,
            ended_at: None,
            command: command.to_string(),
            workflow: None,
            user_intent: user_intent.to_string(),
            messages: Vec::new(),
            tool_calls: Vec::new(),
            modifications: Vec::new(),
            provider: provider.to_string(),
            model: model.to_string(),
            total_usage: Usage::default(),
            outcome: SessionOutcome::Running,
            confidence: 0.0,
            error: None,
        }
    }

    /// Set the workflow path for this session.
    pub fn with_workflow(mut self, path: &Path) -> Self {
        self.workflow = Some(path.to_path_buf());
        self
    }

    /// Mark the session as completed successfully.
    pub fn complete(mut self, confidence: f64) -> Self {
        self.ended_at = Some(Utc::now());
        self.outcome = SessionOutcome::Success;
        self.confidence = confidence;
        self
    }

    /// Mark the session as failed.
    pub fn fail(mut self, error: &str) -> Self {
        self.ended_at = Some(Utc::now());
        self.outcome = SessionOutcome::Failed;
        self.error = Some(error.to_string());
        self
    }

    /// Add usage from a single AI call to the cumulative total.
    pub fn add_usage(&mut self, usage: &Usage) {
        self.total_usage.prompt_tokens += usage.prompt_tokens;
        self.total_usage.completion_tokens += usage.completion_tokens;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionOutcome {
    Running,
    Success,
    Failed,
    Cancelled,
}

// ── Session message (sanitized for persistence) ────────────────────────────

/// A simplified message record for session persistence.
/// Avoids duplicating the full `Message` struct to keep sessions lightweight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content_preview: String, // First 500 chars
    pub content_hash: String,    // SHA-256 for integrity verification
    pub has_tool_calls: bool,
}

impl SessionMessage {
    pub fn from_message(msg: &crate::types::Message) -> Self {
        use sha2::Digest;
        let role = format!("{:?}", msg.role).to_lowercase();
        // Char-boundary safe: a byte slice at 500 panics for CJK/emoji content
        // (audit finding). `truncate_utf8_from_start` is the crate's existing
        // helper for exactly this.
        let preview = if msg.content.len() > 500 {
            format!(
                "{}...",
                crate::types::truncate_utf8_from_start(&msg.content, 500)
            )
        } else {
            msg.content.clone()
        };
        let mut hasher = sha2::Sha256::new();
        hasher.update(msg.content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        Self {
            role,
            content_preview: preview,
            content_hash: hash,
            has_tool_calls: msg.tool_calls.is_some(),
        }
    }
}

// ── Tool call record ───────────────────────────────────────────────────────

/// A record of a single tool invocation during an agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
    pub arguments: String,
    pub result_preview: String, // First 200 chars
    pub success: bool,
    pub duration_ms: u64,
}

// ── Modification record ────────────────────────────────────────────────────

/// A record of one modification the AI agent made or proposed.
///
/// Each modification captures the before/after state so changes can be
/// audited, reverted, or reviewed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modification {
    /// When the modification occurred.
    pub timestamp: DateTime<Utc>,
    /// The file that was modified.
    pub file: PathBuf,
    /// Full content before the modification.
    pub before: String,
    /// Full content after the modification.
    pub after: String,
    /// AI agent's explanation for the change.
    pub reason: String,
    /// Which correction round produced this modification.
    pub round: u32,
    /// Whether the modification was actually applied to the file.
    pub applied: bool,
}

// ── Session persistence ────────────────────────────────────────────────────

/// Directory for session JSON files (public for status/listing commands).
pub fn sessions_dir() -> PathBuf {
    resolve_oxo_flow_dir().join("ai_sessions")
}

/// Directory for archived workflow snapshots.
fn archive_dir() -> PathBuf {
    resolve_oxo_flow_dir().join("ai_archive")
}

/// Resolve the oxo-flow data directory (project-local or global).
fn resolve_oxo_flow_dir() -> PathBuf {
    // Prefer project-local .oxo-flow if it exists
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let local = cwd.join(".oxo-flow");
    if local.exists() {
        return local;
    }
    // Fall back to global
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".oxo-flow")
}

/// Persist a completed session to disk.
pub fn save_session(session: &AiSession) -> Result<PathBuf, AiError> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir).map_err(|e| AiError::SessionError {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session).map_err(|e| AiError::SessionError {
        path: path.clone(),
        message: e.to_string(),
    })?;

    write_json_atomic(&path, &json)?;

    tracing::info!(session = %session.id, "AI session saved to {}", path.display());
    Ok(path)
}

/// Write `json` to `path` atomically: a temp sibling plus a rename.
///
/// A crash (or a full disk) mid-write used to leave a truncated JSON file
/// that `ai status` then failed to parse. `rename` is atomic within a
/// filesystem, so readers always see either the previous complete version
/// or the new one — never a partial write. The temp file is created in the
/// destination directory so the rename cannot cross filesystems.
fn write_json_atomic(path: &Path, json: &str) -> Result<(), AiError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session.json".to_string());
    let tmp = dir.join(format!(
        ".{stem}.tmp.{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::write(&tmp, json).map_err(|e| AiError::SessionError {
        path: tmp.clone(),
        message: e.to_string(),
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AiError::SessionError {
            path: path.to_path_buf(),
            message: e.to_string(),
        }
    })
}

/// Archive a workflow file before modification.
///
/// Saves the current content as a timestamped snapshot so it can be
/// restored if needed.
pub fn archive_before_modify(
    workflow_path: &Path,
    content: &str,
    session_id: &str,
) -> Result<PathBuf, AiError> {
    let dir = archive_dir().join(
        workflow_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    );
    std::fs::create_dir_all(&dir).map_err(|e| AiError::SessionError {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{timestamp}-{session_id}-before.oxoflow");
    let path = dir.join(&filename);

    std::fs::write(&path, content).map_err(|e| AiError::SessionError {
        path: path.clone(),
        message: e.to_string(),
    })?;

    tracing::info!("Workflow archived before modification: {}", path.display());
    Ok(path)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_json_atomic_publishes_and_leaves_no_temp_file() {
        // A truncated session file made `ai status` fail to parse; the
        // write must go through a temp sibling + rename, leaving only the
        // complete file behind.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.json");

        write_json_atomic(&path, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");

        // Overwrite: the old complete version is replaced atomically.
        write_json_atomic(&path, "{\"b\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"b\":2}");

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must be renamed away: {leftovers:?}"
        );
    }

    #[test]
    fn session_has_unique_id() {
        let s1 = AiSession::new("template", "RNA-seq", "deepseek", "deepseek-v4-pro");
        let s2 = AiSession::new("template", "RNA-seq", "deepseek", "deepseek-v4-pro");
        assert_ne!(s1.id, s2.id);
    }

    #[test]
    fn session_complete_updates_state() {
        let session = AiSession::new("template", "test", "deepseek", "dp-v4");
        let completed = session.complete(0.95);
        assert_eq!(completed.outcome, SessionOutcome::Success);
        assert!(completed.ended_at.is_some());
        assert_eq!(completed.confidence, 0.95);
    }

    #[test]
    fn session_fail_records_error() {
        let session = AiSession::new("template", "test", "deepseek", "dp-v4");
        let failed = session.fail("timeout");
        assert_eq!(failed.outcome, SessionOutcome::Failed);
        assert_eq!(failed.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn modification_records_before_after() {
        let m = Modification {
            timestamp: Utc::now(),
            file: PathBuf::from("test.oxoflow"),
            before: "old".into(),
            after: "new".into(),
            reason: "fixed memory".into(),
            round: 1,
            applied: true,
        };
        assert_eq!(m.before, "old");
        assert_eq!(m.after, "new");
        assert!(m.applied);
    }

    #[test]
    fn sessions_dir_exists() {
        let dir = sessions_dir();
        assert!(dir.to_string_lossy().contains("oxo-flow"));
        assert!(dir.to_string_lossy().contains("ai_sessions"));
    }

    #[test]
    fn archive_dir_exists() {
        let dir = archive_dir();
        assert!(dir.to_string_lossy().contains("oxo-flow"));
        assert!(dir.to_string_lossy().contains("ai_archive"));
    }

    #[test]
    fn session_message_preview_is_char_boundary_safe() {
        // A 500-byte slice of CJK content lands mid-character; the old
        // `&msg.content[..500]` panicked while persisting the session.
        let content = "汉".repeat(400); // 1200 bytes, every char 3 bytes
        assert!(content.len() > 500);
        let msg = crate::types::Message::assistant(&content);
        let preview = SessionMessage::from_message(&msg);
        assert!(preview.content_preview.ends_with("..."));
        assert!(preview.content_preview.len() <= 503);
        assert!(
            preview
                .content_preview
                .is_char_boundary(preview.content_preview.len() - 3)
        );
    }
}
