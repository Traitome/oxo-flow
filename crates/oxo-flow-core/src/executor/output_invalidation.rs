//! Output invalidation for failed rules (issue #118).
//!
//! A rule that fails mid-write leaves its declared outputs on disk. Without
//! cleanup, the freshness gate (`should_skip_rule` — outputs exist and are
//! newer than inputs) treats the failed rule as up-to-date on the next run
//! and skips it, so downstream rules consume partial or garbage files (live:
//! a failed postprocess left a 0-byte BAM; the re-run skipped the rule and
//! the downstream sort died reading the header).
//!
//! Fix: before executing, snapshot each declared output's existence and
//! mtime; on failure, invalidate only what THIS attempt produced:
//!
//! - created during the attempt → deleted;
//! - pre-existing but modified during the attempt → moved aside as
//!   `<name>.oxo-failed` (recoverable, never silently destroyed);
//! - pre-existing and untouched → left alone (user data survives a
//!   failure that never reached the file).

use crate::rule::Rule;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Pre-execution snapshot of one declared output path.
#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    path: PathBuf,
    existed: bool,
    mtime: Option<std::time::SystemTime>,
    size: Option<u64>,
}

/// Snapshot a rule's declared outputs (config placeholders expanded using
/// `wildcard_values`, mirroring `should_skip_rule`'s expansion rules).
/// Paths that still contain wildcard patterns after expansion are skipped —
/// the same conservative behavior as the freshness gate.
pub fn snapshot_outputs(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<OutputSnapshot> {
    rule.output
        .iter()
        .filter_map(|output| {
            let expanded = super::checkpoint::expand_config_in_path(output, wildcard_values);
            if expanded.contains('{') {
                return None;
            }
            let path = workdir.join(expanded);
            let meta = std::fs::metadata(&path).ok();
            let existed = meta.is_some();
            let mtime = meta.as_ref().and_then(|m| m.modified().ok());
            let size = meta.map(|m| m.len());
            Some(OutputSnapshot {
                path,
                existed,
                mtime,
                size,
            })
        })
        .collect()
}

/// Invalidate the outputs a failed rule attempt produced or modified.
///
/// Deletes files created during the attempt; moves pre-existing files the
/// attempt modified aside to `<name>.oxo-failed` so the failure is
/// recoverable and the freshness gate no longer sees a "fresh" output.
/// A pre-existing file is considered modified when its mtime or its size
/// differs from the snapshot — the size check catches rewrites that
/// restore the old timestamp. Files matching both are left alone. Every
/// step is best-effort with a warning — cleanup must never mask the rule's
/// own failure.
pub async fn invalidate_failed_outputs(snapshots: &[OutputSnapshot]) {
    for snapshot in snapshots {
        let current_meta = match tokio::fs::metadata(&snapshot.path).await {
            Ok(meta) => meta,
            Err(_) => continue, // gone already — nothing to invalidate
        };
        if !snapshot.existed {
            // Created during the failed attempt — remove outright.
            // Directories need `remove_dir_all`: `remove_file` EISDIRs on
            // them (warned and swallowed), leaving a stale dir that the
            // freshness gate then treats as a fresh output — the re-run is
            // skipped and downstream rules consume the partial contents
            // (#118's residual form).
            let removal = if current_meta.is_dir() {
                tokio::fs::remove_dir_all(&snapshot.path).await
            } else {
                tokio::fs::remove_file(&snapshot.path).await
            };
            if let Err(e) = removal {
                tracing::warn!(
                    file = %snapshot.path.display(),
                    is_dir = current_meta.is_dir(),
                    error = %e,
                    "failed to remove output created by a failed rule"
                );
            } else {
                tracing::debug!(
                    file = %snapshot.path.display(),
                    "removed output created by a failed rule"
                );
            }
            continue;
        }
        // Pre-existing: invalidate only if the attempt modified it. The
        // mtime comparison alone misses rewrites that restore the old
        // timestamp (tools doing that are rare but real, and coarse
        // filesystems can share mtimes) — a size change catches those
        // (issue #136).
        let mtime_changed = match (&snapshot.mtime, current_meta.modified().ok()) {
            (Some(before), Some(after)) => after != *before,
            // Unreadable mtime: be conservative and leave the file alone.
            _ => false,
        };
        let size_changed = snapshot.size != Some(current_meta.len());
        if mtime_changed || size_changed {
            let aside = aside_path(&snapshot.path);
            if tokio::fs::metadata(&aside).await.is_ok() {
                // `rename` would silently overwrite a previous failure's
                // evidence — skip it so recovery keeps every snapshot of
                // what failed, and leave the current file in place (issue
                // #136).
                tracing::warn!(
                    file = %snapshot.path.display(),
                    aside = %aside.display(),
                    "not moving failed output aside: the aside name is already \
                     held by a previous failure — keeping the current file in place"
                );
            } else if let Err(e) = tokio::fs::rename(&snapshot.path, &aside).await {
                tracing::warn!(
                    file = %snapshot.path.display(),
                    error = %e,
                    "failed to move aside output modified by a failed rule"
                );
            } else {
                tracing::warn!(
                    file = %snapshot.path.display(),
                    aside = %aside.display(),
                    "moved aside pre-existing output modified by a failed rule"
                );
            }
        }
    }
}

/// `<path>.oxo-failed` sibling for a moved-aside output. A previous failure
/// already holding that name is never overwritten — the caller skips the
/// rename with a warning so recovery evidence is never destroyed.
fn aside_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".oxo-failed");
    path.with_file_name(name)
}

/// Retention policy for `.oxo-failed` aside files (issue #194 C2): a run
/// start removes aside files older than this many days so failure evidence
/// ages out instead of accumulating indefinitely.
pub const OXOX_FAILED_RETENTION_DAYS: u64 = 7;

/// Remove `.oxo-failed` aside files older than `max_age_days` anywhere
/// under `workdir` (production passes [`OXOX_FAILED_RETENTION_DAYS`]).
/// Called once at run start; best-effort with a count — cleanup must never
/// block a run.
pub fn cleanup_stale_failed_asides(workdir: &Path, max_age_days: u64) -> usize {
    use std::collections::VecDeque;
    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);
    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    let mut queue: VecDeque<PathBuf> = VecDeque::from([workdir.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Never descend into the engine's own state dir — asides
                // live next to workflow outputs, not inside .oxo-flow.
                if path.file_name().and_then(|n| n.to_str()) == Some(".oxo-flow") {
                    continue;
                }
                queue.push_back(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".oxo-failed"))
                && std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age > max_age)
                && std::fs::remove_file(&path).is_ok()
            {
                tracing::debug!(file = %path.display(), "removed stale .oxo-failed aside (retention)");
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Rule;
    use std::collections::HashMap;

    fn rule_with_outputs(outputs: &[&str]) -> Rule {
        Rule {
            name: "r".to_string(),
            output: outputs.iter().map(|o| o.to_string()).collect(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn created_output_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_with_outputs(&["out.txt"]);
        let values = HashMap::new();
        // Snapshot before anything exists.
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].existed);
        // The failed attempt "writes" the output, then we invalidate.
        std::fs::write(dir.path().join("out.txt"), b"partial").unwrap();
        invalidate_failed_outputs(&snapshots).await;
        assert!(!dir.path().join("out.txt").exists());
    }

    #[tokio::test]
    async fn created_directory_output_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_with_outputs(&["out/"]);
        let values = HashMap::new();
        // Snapshot before anything exists.
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].existed);
        // The failed attempt "writes" a directory output with content.
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/partial.txt"), b"partial").unwrap();
        invalidate_failed_outputs(&snapshots).await;
        assert!(
            !dir.path().join("out").exists(),
            "a directory created by a failed attempt must be removed — remove_file \
             EISDIRs on directories, so the stale dir would look fresh to the \
             freshness gate and the re-run would be skipped"
        );
    }

    #[tokio::test]
    async fn untouched_preexisting_output_survives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), b"user-data").unwrap();
        let rule = rule_with_outputs(&["out.txt"]);
        let values = HashMap::new();
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        assert!(snapshots[0].existed);
        // No modification — the file must survive byte-identical.
        invalidate_failed_outputs(&snapshots).await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "user-data"
        );
    }

    #[tokio::test]
    async fn existing_failed_aside_is_never_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, b"user-data").unwrap();
        // A previous failure's evidence already holds the aside name.
        let aside = dir.path().join("out.txt.oxo-failed");
        std::fs::write(&aside, b"previous-failure-evidence").unwrap();
        let rule = rule_with_outputs(&["out.txt"]);
        let values = HashMap::new();
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        // The attempt overwrites the pre-existing output.
        std::fs::write(&path, b"corrupt").unwrap();
        invalidate_failed_outputs(&snapshots).await;
        assert_eq!(
            std::fs::read_to_string(&aside).unwrap(),
            "previous-failure-evidence",
            "a second failure must never destroy the first failure's evidence"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "corrupt",
            "the current failure's output stays put when the aside name is taken"
        );
    }

    #[tokio::test]
    async fn same_mtime_but_resized_output_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, b"user-data").unwrap();
        // Pin the mtime so the failed attempt's rewrite can restore it —
        // the old mtime-only check then sees no change at all.
        let mtime =
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(&path).unwrap());
        let rule = rule_with_outputs(&["out.txt"]);
        let values = HashMap::new();
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        // The attempt overwrites the file with different content, then a
        // coarse-grained filesystem (or a tool preserving timestamps)
        // restores the original mtime.
        std::fs::write(&path, b"partial-corrupt-output").unwrap();
        filetime::set_file_mtime(&path, mtime).unwrap();
        invalidate_failed_outputs(&snapshots).await;
        assert!(
            !path.exists(),
            "a resized rewrite must invalidate even when the mtime matches"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt.oxo-failed")).unwrap(),
            "partial-corrupt-output",
            "the failed content must be preserved for recovery"
        );
    }

    #[tokio::test]
    async fn modified_preexisting_output_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), b"user-data").unwrap();
        let rule = rule_with_outputs(&["out.txt"]);
        let values = HashMap::new();
        let snapshots = snapshot_outputs(&rule, dir.path(), &values);
        // The attempt overwrites the pre-existing file.
        std::fs::write(dir.path().join("out.txt"), b"corrupt").unwrap();
        invalidate_failed_outputs(&snapshots).await;
        assert!(!dir.path().join("out.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt.oxo-failed")).unwrap(),
            "corrupt",
            "the failed content must be preserved for recovery"
        );
    }

    #[tokio::test]
    async fn wildcard_outputs_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_with_outputs(&["results/{sample}.txt"]);
        let values = HashMap::new();
        assert!(snapshot_outputs(&rule, dir.path(), &values).is_empty());
    }

    #[test]
    fn cleanup_stale_failed_asides_removes_aged_evidence_and_spares_fresh() {
        // issue #194 C2: retention with max_age_days = 0 removes every
        // `.oxo-failed` aside (all files are older than an instant-old
        // threshold), spares non-aside files, and does not descend into
        // `.oxo-flow`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("results")).unwrap();
        std::fs::create_dir_all(dir.path().join(".oxo-flow")).unwrap();
        std::fs::write(dir.path().join("results/a.txt.oxo-failed"), "x").unwrap();
        std::fs::write(dir.path().join("results/b.txt"), "keep").unwrap();
        std::fs::write(dir.path().join(".oxo-flow/secret.oxo-failed"), "keep").unwrap();
        let removed = cleanup_stale_failed_asides(dir.path(), 0);
        assert_eq!(removed, 1, "only the results aside is aged out");
        assert!(!dir.path().join("results/a.txt.oxo-failed").exists());
        assert!(dir.path().join("results/b.txt").exists());
        assert!(dir.path().join(".oxo-flow/secret.oxo-failed").exists());
    }
}
