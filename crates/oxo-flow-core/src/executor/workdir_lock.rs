//! Exclusive workdir lock guarding `.oxo-flow` state (issue #70).
//!
//! Concurrent `oxo-flow run` invocations on the same workdir would race on
//! `.oxo-flow/checkpoint.json` (last-writer-wins). The lock is an advisory
//! `flock` on `.oxo-flow/lock` via `fs2`: the OS releases it automatically
//! when the holding process exits or crashes, so there are no stale locks
//! to clean up.

use crate::error::{OxoFlowError, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// An exclusive lock on a workdir's `.oxo-flow` state, held for the whole
/// run. Released on [`Drop`] — and by the OS even if the process dies.
#[derive(Debug)]
pub struct WorkdirLock {
    file: File,
    path: PathBuf,
}

impl WorkdirLock {
    /// Acquire the exclusive lock, creating `.oxo-flow` if needed.
    ///
    /// Fails with [`OxoFlowError::WorkdirLocked`] when another live process
    /// already holds it.
    pub fn acquire(workdir: &Path) -> Result<Self> {
        let dir = workdir.join(".oxo-flow");
        std::fs::create_dir_all(&dir).map_err(|e| OxoFlowError::Execution {
            rule: "workdir lock".to_string(),
            message: format!("cannot create {}: {e}", dir.display()),
        })?;
        let path = dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| OxoFlowError::Execution {
                rule: "workdir lock".to_string(),
                message: format!("cannot open {}: {e}", path.display()),
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file, path }),
            Err(_) => Err(OxoFlowError::WorkdirLocked { path }),
        }
    }

    /// Probe whether a live process currently holds the workdir lock.
    ///
    /// Best-effort: a missing or unreadable lock file reports `false`.
    /// Races with [`WorkdirLock::acquire`] are advisory by design.
    #[must_use]
    pub fn is_locked(workdir: &Path) -> bool {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .open(workdir.join(".oxo-flow/lock"))
        else {
            return false;
        };
        // WouldBlock (Err) means someone holds it; Ok acquires it and the
        // temporary handle releases on drop.
        file.try_lock_exclusive().is_err()
    }

    /// Path of the lock file (for error messages).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkdirLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let first = WorkdirLock::acquire(dir.path()).unwrap();
        assert!(first.path().exists());
        assert!(WorkdirLock::is_locked(dir.path()));

        let second = WorkdirLock::acquire(dir.path());
        assert!(
            matches!(second, Err(OxoFlowError::WorkdirLocked { .. })),
            "second acquire must fail while the first holds the lock: {second:?}"
        );
    }

    #[test]
    fn drop_releases_lock_for_reacquire() {
        let dir = tempfile::tempdir().unwrap();
        let first = WorkdirLock::acquire(dir.path()).unwrap();
        drop(first);

        assert!(!WorkdirLock::is_locked(dir.path()));
        let second = WorkdirLock::acquire(dir.path()).unwrap();
        assert!(second.path().exists());
    }

    #[test]
    fn missing_lock_file_reports_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!WorkdirLock::is_locked(dir.path()));
    }

    #[test]
    fn locked_error_suggests_waiting_for_the_other_process() {
        let dir = tempfile::tempdir().unwrap();
        let _held = WorkdirLock::acquire(dir.path()).unwrap();
        let err = WorkdirLock::acquire(dir.path()).unwrap_err();
        let suggestion = err.suggestion().unwrap_or_default();
        assert!(
            suggestion.contains("another oxo-flow"),
            "suggestion must point at the concurrent run: {suggestion}"
        );
        assert!(
            suggestion.contains("wait for it to finish"),
            "suggestion must advise waiting: {suggestion}"
        );
    }
}
