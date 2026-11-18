//! The single staged-write discipline for oxo-flow.
//!
//! Every place that must not leave a half-written file behind — checkpoint
//! saves, AI-recover workflow rewrites, the recovered `.bak` guard — stages
//! the bytes into a same-directory temporary sibling, fsyncs that file,
//! `rename`s it over the target (atomic on POSIX), then fsyncs the parent
//! directory so the rename itself survives a crash. Power loss can then
//! leave either the previous complete version or the new one, never a
//! truncated file.
//!
//! Reuse this module; do not hand-roll another copy of the dance. It is the
//! portable implementation lifted out of the executor (the `sync_path` /
//! `sync_parent_dir` primitives from `process.rs` and the staged-write body
//! from `checkpoint.rs`) so the CLI and core share one discipline instead of
//! each retyping it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter making every staged filename unique within this process
/// (combined with the PID, unique on the machine), so concurrent writers
/// never rename or delete each other's staged file.
static STAGED_WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

/// `{name}.{seq}.{pid}.tmp` beside `path`: a sibling staged name no other
/// concurrent write can collide with, and one that stays on the same
/// filesystem so the final `rename` cannot cross a mount boundary.
#[must_use]
pub fn staged_tmp_path(path: &Path) -> PathBuf {
    let seq = STAGED_WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{seq}.{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// fsync a path's contents. Opening a directory handle is not portable
/// (Windows), so a failed open of a directory degrades to a debug note
/// instead of failing the write.
pub fn sync_path(path: &Path) -> std::io::Result<()> {
    match std::fs::File::open(path) {
        Ok(file) => file.sync_all(),
        Err(e) if path.is_dir() => {
            tracing::debug!(path = %path.display(), error = %e, "skipping fsync of directory (unsupported on this platform)");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// fsync the parent directory of `path` so a rename into it is durable; on
/// platforms that cannot open a directory handle this is best-effort. Falls
/// back to `"."` for a bare filename (see [`crate::parent_dir`]).
pub fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    sync_path(crate::parent_dir(path))
}

/// The shared staged-write body: write `content` to `tmp_path`, fsync it,
/// `rename` it over `path`, then fsync the parent directory. On any failure
/// the staged temp file is removed so the next attempt cannot trip over a
/// partial leftover; the real target (old or new) is left untouched.
///
/// The caller owns the staged path (see [`staged_tmp_path`]) and any parent
/// directory creation, so the same body serves the checkpoint JSON writer,
/// the recovered-workflow writer, and any future caller without each
/// re-deriving the fsync/rename order.
pub fn publish_staged(content: &str, path: &Path, tmp_path: &Path) -> std::io::Result<()> {
    let result = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = std::fs::File::create(tmp_path)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(tmp_path, path)?;
        // fsync the parent directory so the rename is durable (POSIX).
        // Propagate both the open and the sync error: on EIO/quota/
        // remount-ro the caller must NOT believe the write committed.
        sync_parent_dir(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp_path);
    }
    result
}

/// Write `content` to `path` in one call: stage into a fresh sibling via
/// [`staged_tmp_path`], then [`publish_staged`]. Returns the underlying
/// `std::io::Error`; callers wrap it with their own context.
pub fn write_file_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = staged_tmp_path(path);
    publish_staged(content, path, &tmp)
}

/// Byte-exact [`write_file_atomic`] for payloads that must not be
/// reinterpreted as UTF-8: stages a copy of `source` beside `destination`,
/// fsyncs it, renames it over `destination`, then fsyncs the parent so a
/// non-UTF-8 workflow source survives byte-for-byte. Reuses the same staged
/// discipline so a crash leaves the old or the new file, never a truncated
/// one; on failure the staged temp file is removed and the target is kept.
pub fn copy_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    let tmp = staged_tmp_path(destination);
    let result = (|| -> std::io::Result<()> {
        std::fs::copy(source, &tmp)?;
        sync_path(&tmp)?;
        std::fs::rename(&tmp, destination)?;
        sync_parent_dir(destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_file_atomic_is_byte_exact_for_invalid_utf8() {
        let dir = std::env::temp_dir().join(format!("atomic-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        let payload: Vec<u8> = vec![0x41, 0xFF, 0xFE, 0x01, 0x42];
        std::fs::write(&src, &payload).unwrap();

        copy_file_atomic(&src, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), payload);
        assert!(!staged_tmp_path(&dst).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
