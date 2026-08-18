//! Cross-process serialization of environment creation.
//!
//! Two `oxo-flow run` processes on the same machine used to create conda
//! envs concurrently: conda's package-cache/post-link contention plus the
//! stacked memory peaks OOM-killed small boxes (live evidence: the
//! tx-ubuntu campaign overload episodes, load 56-79). This advisory flock
//! serializes the whole create+verify sequence across processes; the OS
//! releases it automatically when the holder exits, so there are no stale
//! locks to clean up.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// An exclusive machine-wide lock for environment creation, held across the
/// whole setup+verify sequence. Blocking by design: a concurrent run's env
/// create waits for this one to finish instead of racing it.
#[derive(Debug)]
pub struct EnvCreateLock {
    file: File,
    path: PathBuf,
}

/// Best-effort home directory (no external dependency for one path).
fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

impl EnvCreateLock {
    /// Acquire the blocking exclusive lock, creating `~/.oxo-flow` if needed.
    ///
    /// Best-effort: on filesystem errors returns `None` and env setup
    /// proceeds unlocked rather than failing the run.
    #[must_use]
    pub fn acquire() -> Option<Self> {
        let dir = home_dir()?.join(".oxo-flow");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("env-create.lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        match file.lock_exclusive() {
            Ok(()) => Some(Self { file, path }),
            Err(_) => None,
        }
    }

    /// Path of the lock file (for diagnostics).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EnvCreateLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_across_handles() {
        // A temp dir, not the real ~/.oxo-flow: the test must not depend on
        // (or touch) the developer's home state, and a fresh CI runner has
        // no ~/.oxo-flow yet (live evidence: CI failure, NotFound on open).
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("env-create.lock");
        let open = || {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)
                .unwrap()
        };
        let first = open();
        first.lock_exclusive().unwrap();

        // A second handle cannot take the lock while the first holds it.
        let second = open();
        assert!(second.try_lock_exclusive().is_err());

        drop(first);
        // After release the lock is acquirable again.
        assert!(second.try_lock_exclusive().is_ok());
    }
}
