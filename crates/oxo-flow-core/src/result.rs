//! File-system guards shared by result/output scanning code.
//!
//! The structured-output layer (`OutputRecord`, `ResultExtractor`) was
//! removed as dead code; what survives is the FIFO/special-file guard
//! (issue #713, pattern from #695/#706) used by the web crate's
//! checkpoint-status reader.

use std::path::Path;

// ---------------------------------------------------------------------------
// FIFO/special-file guard (issue #713, pattern from #695/#706)
// ---------------------------------------------------------------------------

/// `true` only for regular files: `File::open` on a writer-less FIFO blocks
/// forever, and sockets / devices have no meaningful content. All readers
/// below stat first instead of trusting `exists()`, which never blocks but
/// does not exclude special files.
pub fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_regular_file_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_regular_file(&dir.path().join("missing.txt")));
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(is_regular_file(&file));
        assert!(!is_regular_file(dir.path()));
    }
}
