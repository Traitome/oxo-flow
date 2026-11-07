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
        self.total_usage.cache_read_tokens += usage.cache_read_tokens;
        self.total_usage.cache_creation_tokens += usage.cache_creation_tokens;
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
        let hash = hex::encode(sha2::Sha256::digest(msg.content.as_bytes()));

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

/// Explicit archive-directory override.
///
/// Set `OXO_FLOW_AI_SESSION_DIR` to keep session JSON somewhere other than
/// `<cwd|$HOME>/.oxo-flow/ai_sessions` — required for test processes (which
/// used to fall back to the operator's real `$HOME` archive) and useful on
/// shared HPC homes or read-only project trees.
pub const SESSION_DIR_ENV: &str = "OXO_FLOW_AI_SESSION_DIR";

/// Directory for session JSON files (public for status/listing commands).
pub fn sessions_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(SESSION_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
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
///
/// Unit tests inside this crate must never write into the operator's real
/// archive: `cargo test` used to emit `command=test` records (with the
/// `scripted` stub's rate-limit errors) into `$HOME/.oxo-flow/ai_sessions`,
/// which — at the 200-file cap — pruned the very session an operator was
/// investigating (issue #1040 B1/B3). Tests that exercise the write path call
/// `save_session_to` on a temp dir; integration tests set
/// [`SESSION_DIR_ENV`].
pub fn save_session(session: &AiSession) -> Result<PathBuf, AiError> {
    let dir = sessions_dir();
    if cfg!(test) && std::env::var_os(SESSION_DIR_ENV).is_none() {
        return Ok(dir.join(format!("{}.json", session.id)));
    }
    save_session_to(&dir, session)
}

/// Persist `session` as JSON inside `dir` (temp sibling + atomic rename).
pub(crate) fn save_session_to(dir: &Path, session: &AiSession) -> Result<PathBuf, AiError> {
    std::fs::create_dir_all(dir).map_err(|e| AiError::SessionError {
        path: dir.to_path_buf(),
        message: e.to_string(),
    })?;

    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session).map_err(|e| AiError::SessionError {
        path: path.clone(),
        message: e.to_string(),
    })?;

    write_json_atomic(&path, &json)?;
    // #544: sessions hold user intent, full tool-call arguments and
    // before/after workflow content — world-readable archives leaked all
    // of it on shared HPC homes (ai_config.json already gets 0600).
    restrict_to_owner(&path);

    prune_oldest(dir, "json", MAX_SESSION_ARCHIVES);

    tracing::info!(session = %session.id, "AI session saved to {}", path.display());
    Ok(path)
}

/// Archives kept per directory: the oldest sessions are pruned after each
/// save so the archive cannot grow unbounded (#544).
const MAX_SESSION_ARCHIVES: usize = 200;

/// Archive snapshots kept per workflow: same unbounded-growth rationale as
/// the session cap (#544, P3-10) — snapshots carry the full pre-modification
/// workflow content, so the oldest are pruned after each write.
const MAX_WORKFLOW_ARCHIVES: usize = 200;

/// chmod 0600 — best-effort (non-unix targets skip).
fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Fresh archives are exempt from pruning. A real session written seconds
/// ago is still being inspected (and a stopped run's failure record is the
/// whole point of archiving it); rotation must only ever reclaim old
/// history, never the file under investigation (issue #1040 B1).
const PRUNE_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(600);

/// Delete the oldest files with `extension` in `dir` beyond `keep`
/// (best-effort), leaving files younger than [`PRUNE_MIN_AGE`] alone.
/// Shared by session saves (`.json` at one level) and workflow archives
/// (`.oxoflow` inside per-workflow subdirectories, P3-10).
fn prune_oldest(dir: &Path, extension: &str, keep: usize) {
    prune_oldest_older_than(dir, extension, keep, PRUNE_MIN_AGE);
}

fn prune_oldest_older_than(dir: &Path, extension: &str, keep: usize, min_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut sessions: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == extension))
        .filter_map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, e.path()))
        })
        .collect();
    if sessions.len() <= keep {
        return;
    }
    sessions.sort();
    let excess = sessions.len() - keep;
    let now = std::time::SystemTime::now();
    for (modified, path) in sessions.into_iter().take(excess) {
        let age = now.duration_since(modified).unwrap_or_default();
        if age < min_age {
            // Sorted oldest-first, so every remaining candidate is at least
            // as fresh — the grace window applies to the whole tail.
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(error = %e, path = %path.display(), "prune failed");
        }
    }
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
    // Same exposure surface as sessions (#544): snapshots carry the full
    // pre-modification workflow content, so archives stay owner-only too.
    restrict_to_owner(&path);

    // P3-10: one snapshot per applied modification previously grew without
    // bound — prune the per-workflow archive dir like the session dir.
    prune_oldest(&dir, "oxoflow", MAX_WORKFLOW_ARCHIVES);

    tracing::info!("Workflow archived before modification: {}", path.display());
    Ok(path)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[test]
fn prune_keeps_the_newest_sessions() {
    let dir = tempfile::tempdir().unwrap();
    // Create sequentially; mtime order = creation order (resolution
    // caveats tolerated by asserting only membership counts and the
    // newest survivor).
    for i in 0..6 {
        let p = dir.path().join(format!("s{i}.json"));
        std::fs::write(&p, "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    prune_oldest_older_than(dir.path(), "json", 3, std::time::Duration::ZERO);
    let remaining: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(remaining.len(), 3, "keep exactly 3: {remaining:?}");
    assert!(
        remaining.contains(&"s5.json".to_string()),
        "the newest session must survive: {remaining:?}"
    );
}
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
    fn fresh_archives_are_exempt_from_pruning() {
        // B1: a session archived seconds ago (e.g. an aborted web-chat run)
        // must survive a retention sweep triggered by concurrent writers —
        // the 200-file cap rotated real sessions out when test archives
        // flooded the directory.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..6 {
            std::fs::write(dir.path().join(format!("fresh{i}.json")), "{}").unwrap();
        }
        prune_oldest_older_than(dir.path(), "json", 3, std::time::Duration::from_secs(3600));
        let remaining = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 6, "fresh archives must never be pruned");
    }

    #[test]
    fn save_session_to_persists_an_aborted_run_as_a_failure_record() {
        // B1: when the orchestration loop aborts (provider timeout, client
        // disconnect), the archive must survive with outcome=error and the
        // reason — the post-mortem artifact the browser test could not find.
        let dir = tempfile::tempdir().unwrap();
        let session = AiSession::new("web-chat", "chat", "web", "claude")
            .fail("provider error: request timed out");
        let path = save_session_to(dir.path(), &session).unwrap();

        let saved: AiSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.outcome, SessionOutcome::Failed);
        assert_eq!(
            saved.error.as_deref(),
            Some("provider error: request timed out")
        );
    }

    #[test]
    fn concurrent_reads_never_observe_a_truncated_archive() {
        // The temp-sibling + rename contract: a reader racing the writer sees
        // the previous complete document or the next one, never a partial.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.json");
        let big =
            |tag: usize| serde_json::json!({"n": tag, "pad": "x".repeat(64 * 1024)}).to_string();
        write_json_atomic(&path, &big(0)).unwrap();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let path = path.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut reads = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let Ok(raw) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let parsed: serde_json::Value = serde_json::from_str(&raw)
                        .expect("a reader must never observe a truncated archive");
                    assert!(parsed["pad"].is_string());
                    reads += 1;
                }
                reads
            })
        };
        for i in 1..200 {
            write_json_atomic(&path, &big(i)).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let reads = reader.join().unwrap();
        assert!(reads > 0, "the racing reader must have observed archives");
    }

    #[test]
    fn unit_tests_do_not_write_the_real_session_archive() {
        // The `command=test` storm came from orchestrator unit tests saving
        // into $HOME/.oxo-flow/ai_sessions; under cfg(test) saves are a
        // no-op unless OXO_FLOW_AI_SESSION_DIR redirects them.
        if std::env::var_os(SESSION_DIR_ENV).is_some() {
            return;
        }
        let session = AiSession::new("test", "test", "noop", "none");
        let path = save_session(&session).unwrap();
        assert!(
            !path.exists(),
            "unit tests must not write into the operator's archive: {}",
            path.display()
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
    fn add_usage_accumulates_cache_tokens() {
        // Cache counters must ride the same rollup as the base tokens — a
        // caching-enabled Anthropic endpoint reports them per call, and the
        // archived session's totals must reflect the full bill shape.
        let mut session = AiSession::new("template", "test", "claude", "claude-sonnet-4");
        session.add_usage(&Usage {
            prompt_tokens: 100,
            completion_tokens: 10,
            cache_read_tokens: 40,
            cache_creation_tokens: 20,
        });
        session.add_usage(&Usage {
            prompt_tokens: 200,
            completion_tokens: 5,
            cache_read_tokens: 80,
            cache_creation_tokens: 0,
        });
        assert_eq!(session.total_usage.prompt_tokens, 300);
        assert_eq!(session.total_usage.completion_tokens, 15);
        assert_eq!(session.total_usage.cache_read_tokens, 120);
        assert_eq!(session.total_usage.cache_creation_tokens, 20);
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
    fn archive_prune_keeps_the_newest_snapshots_per_workflow() {
        // P3-10: archives live in per-<stem> subdirectories with an
        // .oxoflow extension; the prune must cap them there without
        // touching other subdirectories or the session dir layout.
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("wf-a");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..6 {
            let p = dir.join(format!("20260101-0{i}0000-sess{i}-before.oxoflow"));
            std::fs::write(&p, "workflow").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // A sibling workflow's archive dir and an unrelated file: neither
        // may be counted or removed.
        let sibling = root.path().join("wf-b");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("keep.oxoflow"), "other").unwrap();
        std::fs::write(root.path().join("stray.json"), "{}").unwrap();

        prune_oldest_older_than(&dir, "oxoflow", 3, std::time::Duration::ZERO);

        let remaining: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), 3, "keep exactly 3: {remaining:?}");
        assert!(
            remaining.contains(&"20260101-050000-sess5-before.oxoflow".to_string()),
            "the newest snapshot must survive: {remaining:?}"
        );
        assert!(
            sibling.join("keep.oxoflow").exists(),
            "sibling workflow archives are separate"
        );
        assert!(root.path().join("stray.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn archived_snapshot_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        // P3-10: archives carry the full pre-modification workflow content,
        // so they get the same 0600 treatment as session JSON (#544).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("20260101-000000-s1-before.oxoflow");
        std::fs::write(&path, "workflow").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        restrict_to_owner(&path);

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "archive must be owner-only"
        );
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
