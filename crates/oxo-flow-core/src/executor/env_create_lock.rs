//! Cross-process serialization of environment creation, per environment.
//!
//! Two `oxo-flow run` processes creating the SAME conda env concurrently
//! corrupt each other's transaction (live evidence: rnaseq's env history
//! showed `+fq` followed by `-fq` 12s later — the loser's transaction
//! removed the winner's packages). DIFFERENT envs are independent by
//! conda/pixi semantics (conda's package cache has its own locking, pixi
//! has pixi.lock), so the lock is keyed by the environment: one lock file
//! per env cache key under `~/.oxo-flow/locks/`. A slow solve for env A
//! therefore never blocks env B's creation (live evidence: a 3.5h
//! bioconductor solve held the OLD global lock and stalled every rule
//! needing ANY env setup for hours).
//!
//! The wait is bounded: after the timeout (default 2h, `OXO_ENV_LOCK_TIMEOUT_SECS`
//! to override) the acquisition fails with a diagnostic instead of hanging
//! forever. The OS releases the flock automatically when the holder exits,
//! so there are no stale locks to clean up.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default bounded wait for the per-env lock: legitimate env solves take
/// tens of minutes; anything beyond this is a stuck holder and should
/// fail fast with a diagnostic rather than hang the queue.
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// How often the waiter logs while blocked.
const WAIT_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// An exclusive machine-wide lock for ONE environment's creation, held
/// across the whole setup+verify sequence.
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

fn lock_timeout() -> Duration {
    std::env::var("OXO_ENV_LOCK_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_LOCK_TIMEOUT)
}

/// Lock file path for an environment cache key — stable per key, distinct
/// across keys. Extracted so tests exercise the production derivation, not
/// a re-typed copy of it.
fn lock_path_for(dir: &Path, key: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
    dir.join(format!("env-{}.lock", &digest[..16]))
}

impl EnvCreateLock {
    /// Acquire the per-environment exclusive lock for `key`, creating
    /// `~/.oxo-flow/locks` if needed.
    ///
    /// Blocking up to the timeout, logging periodically while waiting.
    /// Best-effort on filesystem errors: returns `None` and env setup
    /// proceeds unlocked rather than failing the run. A TIMEOUT is not
    /// best-effort — it returns an error the caller surfaces (a stuck
    /// holder must be visible, not silently swallowed).
    pub fn acquire(key: &str) -> std::io::Result<Option<Self>> {
        let Some(dir) = home_dir().map(|h| h.join(".oxo-flow").join("locks")) else {
            return Ok(None);
        };
        std::fs::create_dir_all(&dir).ok();
        // One lock file per environment cache key — different envs
        // never contend.
        let path = lock_path_for(&dir, key);
        let file = match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
        {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };

        let deadline = Instant::now() + lock_timeout();
        let mut last_log = Instant::now() - WAIT_LOG_INTERVAL;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    let _ = file.set_len(0);
                    let _ = std::io::Write::write_all(
                        &mut &file,
                        format!("{}\n", std::process::id()).as_bytes(),
                    );
                    return Ok(Some(Self { file, path }));
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(Error::new(
                            ErrorKind::TimedOut,
                            format!(
                                "env create lock {} still held after {}s (holder pid file: {}) — a previous env setup appears stuck; \
                                 kill the holder or raise OXO_ENV_LOCK_TIMEOUT_SECS if the solve is legitimately slow",
                                path.display(),
                                lock_timeout().as_secs(),
                                path.display()
                            ),
                        ));
                    }
                    if last_log.elapsed() >= WAIT_LOG_INTERVAL {
                        tracing::warn!(
                            path = %path.display(),
                            "waiting for env create lock held by another process"
                        );
                        last_log = Instant::now();
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
                Err(e) => return Err(e),
            }
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

    #[test]
    fn lock_path_is_stable_per_key_and_distinct_across_keys() {
        // The whole point of the per-env design: a slow solve for one env
        // must not block another env's creation — same key always maps to
        // the same lock file, different keys never do.
        let dir = Path::new("/tmp/oxo-locks-test");
        let a1 = lock_path_for(dir, "key-a");
        let a2 = lock_path_for(dir, "key-a");
        let b = lock_path_for(dir, "key-b");
        assert_eq!(a1, a2, "same key must map to the same lock file");
        assert_ne!(a1, b, "different keys must map to different lock files");
    }
}
