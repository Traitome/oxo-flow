//! Per-run lifecycle status file (issue #1078 H2).
//!
//! The checkpoint records *rule* state, not whether the run process reached
//! a terminal outcome. A preflight failure (protected-tree drift, a bad
//! environment spec, a parse-adjacent error) can therefore leave a clean
//! checkpoint from a previous successful run in place — and a supervisor
//! would read it as success. `oxo-flow wait` needs the run's own outcome.
//!
//! `<checkpoint>.status` is written when the run arms (after the workdir
//! lock, before any preflight that can fail) with `state: "running"`, and
//! rewritten on every normal return with `"succeeded"` or `"failed"` plus
//! the error text. It lives beside the checkpoint it describes, so variant
//! runs sharing a workdir cannot alias each other's outcome. A killed
//! process leaves `"running"` — the honest "ended without a terminal
//! status" signal. Writes are best-effort: a status-file failure never
//! changes the run's result.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStatus {
    pub state: RunState,
    pub pid: u32,
    pub started_at_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `<checkpoint>.status` — the status belongs to ONE checkpoint, so
/// variant runs that share a workdir (`--checkpoint smoke.json` next to the
/// default) cannot overwrite each other's outcome (issue #1078 R2).
#[must_use]
pub fn status_path(checkpoint: &Path) -> PathBuf {
    let mut name = checkpoint
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".status");
    checkpoint.with_file_name(name)
}

/// Read the checkpoint's status file, tolerating a missing or malformed
/// file.
#[must_use]
pub fn read_status(checkpoint: &Path) -> Option<RunStatus> {
    let text = std::fs::read_to_string(status_path(checkpoint)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Handle armed by `run_command` after the workdir lock is acquired.
pub struct RunStatusHandle {
    path: Mutex<Option<PathBuf>>,
    started_at_unix: u64,
}

impl Default for RunStatusHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl RunStatusHandle {
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: Mutex::new(None),
            started_at_unix: now_unix(),
        }
    }

    /// Mark the run in flight. Called before any preflight that can fail;
    /// the status is written beside the run's checkpoint.
    pub fn arm(&self, checkpoint: &Path) {
        let path = status_path(checkpoint);
        let status = RunStatus {
            state: RunState::Running,
            pid: std::process::id(),
            started_at_unix: self.started_at_unix,
            finished_at_unix: None,
            message: None,
        };
        write_status(&path, &status);
        *self.path.lock().unwrap_or_else(|error| error.into_inner()) = Some(path);
    }

    /// Record the terminal outcome. No-op when [`Self::arm`] never ran.
    pub fn finish(&self, result: &anyhow::Result<()>) {
        let guard = self.path.lock().unwrap_or_else(|error| error.into_inner());
        let Some(path) = guard.as_ref() else {
            return;
        };
        let status = RunStatus {
            state: if result.is_ok() {
                RunState::Succeeded
            } else {
                RunState::Failed
            },
            pid: std::process::id(),
            started_at_unix: self.started_at_unix,
            finished_at_unix: Some(now_unix()),
            message: result.as_ref().err().map(|error| format!("{error:#}")),
        };
        write_status(path, &status);
    }
}

/// Atomic best-effort write: temp file + rename, never a torn status.
fn write_status(path: &Path, status: &RunStatus) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        tracing::warn!(path = %path.display(), error = %error, "failed to create the run-status directory");
        return;
    }
    let Ok(json) = serde_json::to_vec_pretty(status) else {
        return;
    };
    let tmp = path.with_extension("status.tmp");
    if let Err(error) = std::fs::write(&tmp, &json) {
        tracing::warn!(path = %tmp.display(), error = %error, "failed to write the run-status file");
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        tracing::warn!(path = %path.display(), error = %error, "failed to commit the run-status file");
        let _ = std::fs::remove_file(&tmp);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(dir: &Path) -> PathBuf {
        dir.join("checkpoint.json")
    }

    #[test]
    fn arm_and_finish_write_a_terminal_status() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint(dir.path());
        let handle = RunStatusHandle::new();
        handle.arm(&ck);
        let running = read_status(&ck).expect("running status");
        assert_eq!(running.state, RunState::Running);
        assert_eq!(running.pid, std::process::id());

        handle.finish(&Ok(()));
        let done = read_status(&ck).expect("terminal status");
        assert_eq!(done.state, RunState::Succeeded);
        assert!(done.finished_at_unix.is_some());
        assert!(done.message.is_none());
    }

    #[test]
    fn finish_records_the_failure_message() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint(dir.path());
        let handle = RunStatusHandle::new();
        handle.arm(&ck);
        let error = anyhow::anyhow!("protected data tree drift detected");
        handle.finish(&Err(error));
        let status = read_status(&ck).expect("status");
        assert_eq!(status.state, RunState::Failed);
        assert!(
            status
                .message
                .as_deref()
                .is_some_and(|message| message.contains("drift detected")),
            "{status:?}"
        );
    }

    #[test]
    fn finish_without_arm_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint(dir.path());
        let handle = RunStatusHandle::new();
        handle.finish(&Ok(()));
        assert!(read_status(&ck).is_none());
        assert!(!status_path(&ck).exists());
    }

    #[test]
    fn malformed_or_missing_status_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint(dir.path());
        assert!(read_status(&ck).is_none());
        std::fs::write(status_path(&ck), b"not json").unwrap();
        assert!(read_status(&ck).is_none());
    }

    #[test]
    fn variant_checkpoints_get_distinct_status_files() {
        // Issue #1078 R2: smoke/main variants share a workdir but must not
        // overwrite each other's outcome.
        let dir = tempfile::tempdir().unwrap();
        let smoke = dir.path().join("smoke.checkpoint.json");
        let main = dir.path().join("checkpoint.json");
        assert_ne!(status_path(&smoke), status_path(&main));
        assert_eq!(
            status_path(&smoke).file_name().unwrap().to_string_lossy(),
            "smoke.checkpoint.json.status"
        );
    }
}
