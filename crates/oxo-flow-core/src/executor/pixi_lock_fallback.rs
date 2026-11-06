//! Graceful recovery from a stale `pixi.lock` (issue #1011).
//!
//! A committed lock can go stale out from under a workflow when an
//! upstream channel removes a package build mid-cohort (live evidence:
//! a venus deployment lost a pinned conda-forge python build and the run
//! record kept no drift trace). The fallback re-solves from the manifest,
//! parks the fresh solution in `pixi.lock.new` beside the committed lock,
//! restores the committed bytes verbatim, and hands the caller a
//! package-level diff for a loud warning.
//!
//! Pure building blocks live here; the orchestration call site is the
//! setup-failure arm of `LocalExecutor::ensure_environment_ready`.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use thiserror::Error;

/// Side-car file receiving the fresh re-solved lock next to the committed
/// `pixi.lock` (issue #1011). The committed lock is never silently
/// rewritten; the side-car is left for human review and explicit adoption.
pub(crate) const NEW_LOCK_NAME: &str = "pixi.lock.new";

/// Upper bound on diff entries rendered per side before collapsing into a
/// "(+N more)" line, so a pathological re-solve cannot flood the run log.
const MAX_DIFF_ENTRIES: usize = 10;

/// Matches a lock package entry (`- conda: <url>` / `- pypi: <url>`) in a
/// pixi.lock. Entries appear in BOTH `environments.*.packages` (indented)
/// and the top-level `packages:` list; every non-package `- ` line
/// (virtual packages, depends/constrains/run_exports constraints, channel
/// `- url:`) fails the `conda|pypi` discriminator.
static LOCK_PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*-[ \t]+(?:conda|pypi):[ \t]+(\S+)")
        .expect("lock package regex must compile")
});

/// Error surfaced by [`re_solve_in_place`].
#[derive(Debug, Error)]
pub(crate) enum ReSolveError {
    /// The re-solve itself failed; carries the child stderr so the caller
    /// can surface the original root cause instead of masking it.
    #[error("pixi re-solve failed: {0}")]
    Solve(String),
    /// Lock-file bookkeeping (stash / move / restore) failed.
    #[error("pixi.lock fallback bookkeeping failed: {0}")]
    Io(String),
}

/// Classifies pixi stderr as the stale-lock fetch signature (issue #1011).
///
/// Matches the live failure shape (pixi 0.84.1, lock pinning a build that
/// vanished from the channel): `failed to fetch <pkg>.conda` plus an HTTP
/// 404/410 status error. A manifest-level solver conflict (`failed to
/// solve requirements` / `No candidates were found`) is deterministic and
/// must NOT match — re-solving cannot fix it, so firing the fallback would
/// only produce the same failure twice.
pub(crate) fn is_stale_lock_fetch_failure(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("failed to fetch")
        && (lower.contains("http status client error")
            || lower.contains("404")
            || lower.contains("410"))
}

/// Extracts the set of package specs from a pixi.lock. Entries appear in
/// BOTH the indented `environments.*.packages` section and the top-level
/// `packages:` list; set semantics dedupe them.
pub(crate) fn lock_package_set(lock_content: &str) -> BTreeSet<String> {
    LOCK_PACKAGE_RE
        .captures_iter(lock_content)
        .map(|caps| caps[1].to_string())
        .collect()
}

/// Package-level diff between the committed lock and the fresh re-solve.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageDiff {
    pub removed: BTreeSet<String>,
    pub added: BTreeSet<String>,
}

/// Diffs two lock contents at the package level (set semantics, so
/// section duplication inside one lock does not produce phantom diffs).
pub(crate) fn package_diff(old_lock: &str, new_lock: &str) -> PackageDiff {
    let old = lock_package_set(old_lock);
    let new = lock_package_set(new_lock);
    PackageDiff {
        removed: old.difference(&new).cloned().collect(),
        added: new.difference(&old).cloned().collect(),
    }
}

/// Renders one diff side, collapsing overflow into a `(+N more)` line.
fn render_diff_side(label: &str, entries: &BTreeSet<String>) -> Vec<String> {
    let mut lines: Vec<String> = entries
        .iter()
        .take(MAX_DIFF_ENTRIES)
        .map(|pkg| format!("{label}: {pkg}"))
        .collect();
    let overflow = entries.len().saturating_sub(MAX_DIFF_ENTRIES);
    if overflow > 0 {
        lines.push(format!("{label}: (+{overflow} more)"));
    }
    lines
}

/// Renders the diff for the loud warning. BTreeSet iteration keeps the
/// output deterministic across runs (persisted-artifact invariant).
pub(crate) fn format_package_diff(diff: &PackageDiff) -> String {
    if diff.removed.is_empty() && diff.added.is_empty() {
        return "package sets identical between committed lock and re-solve".to_string();
    }
    let mut lines = render_diff_side("removed", &diff.removed);
    lines.extend(render_diff_side("added", &diff.added));
    lines.join("\n")
}

/// Captures the committed lock bytes before anything touches the workflow
/// tree, and restores them verbatim afterwards.
#[derive(Debug)]
pub(crate) struct LockStash {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl LockStash {
    /// Reads the committed lock bytes before anything can overwrite them.
    pub(crate) fn capture(lock_path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            path: lock_path.to_path_buf(),
            bytes: std::fs::read(lock_path)?,
        })
    }

    /// Writes the captured bytes back atomically (temp file + rename in
    /// the same directory) so a crash never leaves a torn lock file.
    pub(crate) fn restore(&self) -> std::io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("lock path has no parent directory"))?;
        let tmp = dir.join(format!(
            ".{}.restore.tmp",
            self.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("pixi.lock")
        ));
        std::fs::write(&tmp, &self.bytes)?;
        match std::fs::rename(&tmp, &self.path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

/// Outcome of a successful in-place re-solve (issue #1011).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReSolvedLock {
    /// Fresh lock content pixi wrote during the re-solve (also parked in
    /// `pixi.lock.new` when [`Self::side_car_parked`] is true).
    pub(crate) content: String,
    /// Whether the side-car parking succeeded. A park failure degrades to a
    /// warning — the committed tree comes back byte-identical either way.
    pub(crate) side_car_parked: bool,
}

/// Re-solves the environment in place after the committed lock proved
/// stale (issue #1011).
///
/// Sequence (the committed lock is NEVER silently rewritten):
///
/// 1. stash the committed lock bytes
/// 2. delete the committed lock so pixi re-solves from the manifest
/// 3. run `reinstall` (the setup command the executor already built)
/// 4. on success, move the freshly written lock to `pixi.lock.new` and
///    restore the committed bytes verbatim — a failed park only degrades
///    to a warning and never skips the restore
/// 5. on failure, restore the committed bytes and surface the re-solve
///    error — the committed tree is left byte-identical
///
/// Returns the fresh lock content so the caller can emit the package-level
/// diff in its loud warning.
pub(crate) async fn re_solve_in_place<F, Fut>(
    lock_path: &Path,
    reinstall: F,
) -> Result<ReSolvedLock, ReSolveError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let stash = LockStash::capture(lock_path)
        .map_err(|e| ReSolveError::Io(format!("failed to stash committed lock: {e}")))?;
    std::fs::remove_file(lock_path)
        .map_err(|e| ReSolveError::Io(format!("failed to remove stale lock: {e}")))?;

    let reinstall_result = reinstall().await;
    let fresh_lock = if reinstall_result.is_ok() {
        std::fs::read_to_string(lock_path).ok()
    } else {
        None
    };

    let mut side_car_parked = false;
    if let Some(content) = &fresh_lock {
        let side_car = lock_path.with_file_name(NEW_LOCK_NAME);
        match std::fs::write(&side_car, content) {
            Ok(()) => side_car_parked = true,
            Err(e) => {
                // Parking is a review convenience: failing to park must
                // never fail an otherwise successful re-solve, and must
                // never skip the committed-lock restore below.
                tracing::warn!(
                    error = %e,
                    side_car = %side_car.display(),
                    "could not park the re-solved lock for review; proceeding with the re-solved environment"
                );
            }
        }
    }

    if let Err(e) = stash.restore() {
        // The committed tree must never be left without its lock; a
        // restore failure is worse than any solve outcome below.
        return Err(ReSolveError::Io(format!(
            "failed to restore committed lock: {e}"
        )));
    }

    match fresh_lock {
        Some(content) => Ok(ReSolvedLock {
            content,
            side_car_parked,
        }),
        None => match reinstall_result {
            Err(msg) => Err(ReSolveError::Solve(msg)),
            Ok(()) => Err(ReSolveError::Io(
                "re-solve reported success but pixi wrote no lock file".to_string(),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live-captured stderr from pixi 0.84.1 against a lock whose python
    /// build vanished from conda-forge (issue #1011 reproduction probe).
    /// The `[project]`->`[workspace]` deprecation box precedes the error
    /// and must not confuse the classifier.
    const STALE_LOCK_404_STDERR: &str = r#"
 WARN Encountered 1 warning while parsing the manifest:
  ⚠ The `project` field is deprecated. Use `workspace` instead.
   ╭─[/tmp/oxo-resolve2/pixi.toml:1:1]
 1 │ ╭─▶ [project]
 2 │ │   name = "probe"
 3 │ │   channels = ["conda-forge"]
 4 │ ├─▶ platforms = ["osx-arm64"]
   · ╰──── replace this with 'workspace'
 5 │
   ╰────

Error:   × failed to fetch python-3.12.16-hDEADBEEF_0_cpython.conda
  ├─▶ failed to interact with the package cache layer.
  ╰─▶ HTTP status client error (404 Not Found) for url (https://
      conda.anaconda.org/conda-forge/osx-arm64/python-3.12.16-
      hDEADBEEF_0_cpython.conda)
"#;

    #[test]
    fn classifier_matches_live_stale_lock_404() {
        assert!(is_stale_lock_fetch_failure(STALE_LOCK_404_STDERR));
    }

    #[test]
    fn classifier_rejects_manifest_solver_conflict() {
        // Deterministic manifest conflict: re-solving cannot fix it, so
        // firing the fallback would only fail twice.
        let stderr = "Error:   × failed to solve requirements\n  \
                      ├─▶ No candidates were found for python-3.13\n";
        assert!(!is_stale_lock_fetch_failure(stderr));
    }

    #[test]
    fn classifier_ignores_deprecation_warning_alone() {
        let stderr = " WARN Encountered 1 warning while parsing the manifest:\n  \
                      ⚠ The `project` field is deprecated. Use `workspace` instead.\n";
        assert!(!is_stale_lock_fetch_failure(stderr));
    }

    #[test]
    fn classifier_ignores_unrelated_failures() {
        assert!(!is_stale_lock_fetch_failure("sh: pixi: command not found"));
        assert!(!is_stale_lock_fetch_failure(""));
    }

    /// Excerpt of a real pixi.lock (v7): package entries in BOTH the
    /// indented `environments.default.packages` section and the top-level
    /// `packages:` list; virtual/depends/constrains/run_exports lines and
    /// the channel `- url:` entry must not leak into the package set.
    const LOCK_EXCERPT: &str = "\
version: 7
platforms:
- name: osx-arm64
  virtual-packages:
  - __unix=0
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      osx-arm64:
      - conda: https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2026.10.23-hbd8a1cb_0.conda
      - conda: https://conda.anaconda.org/conda-forge/osx-arm64/python-3.12.16-hd05a0c4_0_cpython.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2026.10.23-hbd8a1cb_0.conda
  sha256: 95619b9c1184be7175392eeb0ee7eeb5ccf25d280951f4225f2d62e725790be3
  depends:
  - __unix
  - bzip2 >=1.0.8,<2.0a0
  constrains:
  - expat 2.9.0.*
  run_exports:
    weak:
    - bzip2 >=1.0.8,<2.0a0
- pypi: https://files.pythonhosted.org/packages/numpy-1.26.0.tar.gz
  sha256: abc
";

    #[test]
    fn lock_set_extracts_both_sections_and_dedupes() {
        let set = lock_package_set(LOCK_EXCERPT);
        assert_eq!(set.len(), 3, "set: {set:?}");
        assert!(
            set.contains(
                "https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2026.10.23-hbd8a1cb_0.conda"
            ),
            "set: {set:?}"
        );
        assert!(
            set.contains(
                "https://conda.anaconda.org/conda-forge/osx-arm64/python-3.12.16-hd05a0c4_0_cpython.conda"
            ),
            "set: {set:?}"
        );
        assert!(
            set.contains("https://files.pythonhosted.org/packages/numpy-1.26.0.tar.gz"),
            "set: {set:?}"
        );
    }

    #[test]
    fn package_diff_reports_removed_and_added() {
        let old = "- conda: https://example.org/a-1.0.conda\n\
                   - conda: https://example.org/b-2.0.conda\n";
        let new = "- conda: https://example.org/b-2.0.conda\n\
                   - pypi: https://example.org/c-3.0.tar.gz\n";
        let diff = package_diff(old, new);
        assert_eq!(
            diff.removed,
            BTreeSet::from(["https://example.org/a-1.0.conda".to_string()])
        );
        assert_eq!(
            diff.added,
            BTreeSet::from(["https://example.org/c-3.0.tar.gz".to_string()])
        );
    }

    #[test]
    fn format_package_diff_caps_each_side() {
        let mut old = String::new();
        let mut new = String::new();
        for i in 0..12 {
            old.push_str(&format!("- conda: https://example.org/old{i}.conda\n"));
            new.push_str(&format!("- pypi: https://example.org/new{i}.conda\n"));
        }
        let diff = package_diff(&old, &new);
        let text = format_package_diff(&diff);
        let removed_lines = text.lines().filter(|l| l.starts_with("removed:")).count();
        let added_lines = text.lines().filter(|l| l.starts_with("added:")).count();
        assert_eq!(removed_lines, MAX_DIFF_ENTRIES + 1, "text: {text}");
        assert_eq!(added_lines, MAX_DIFF_ENTRIES + 1, "text: {text}");
        assert!(text.contains("(+2 more)"), "text: {text}");
    }

    #[test]
    fn format_package_diff_empty_reports_no_package_change() {
        let diff = package_diff("", "");
        assert!(diff.removed.is_empty() && diff.added.is_empty());
        assert!(
            format_package_diff(&diff).contains("identical"),
            "empty diff must say so"
        );
    }

    #[test]
    fn stash_round_trip_restores_bytes_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");
        let stash = LockStash::capture(&lock).expect("capture");
        std::fs::write(&lock, b"overwritten").expect("overwrite");
        stash.restore().expect("restore");
        assert_eq!(
            std::fs::read(&lock).expect("read back"),
            b"committed-lock-bytes"
        );
    }

    #[tokio::test]
    async fn re_solve_success_parks_new_lock_and_restores_committed_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");

        let new_content = "version: 7\npackages: []\n".to_string();
        let lock_for_task = lock.clone();
        let content_for_assert = new_content.clone();
        let result = re_solve_in_place(&lock, move || {
            let new_content = new_content.clone();
            let lock = lock_for_task.clone();
            async move {
                std::fs::write(&lock, new_content.as_bytes()).expect("fake solve writes lock");
                Ok(())
            }
        })
        .await;

        let resolved = result.expect("re-solve must succeed");
        assert_eq!(
            resolved.content, content_for_assert,
            "fresh lock content is returned for the diff"
        );
        assert!(
            resolved.side_car_parked,
            "fresh solution must be parked as pixi.lock.new"
        );
        assert_eq!(
            std::fs::read(&lock).expect("read committed lock"),
            b"committed-lock-bytes",
            "committed lock must be restored verbatim"
        );
        let parked = dir.path().join(NEW_LOCK_NAME);
        assert_eq!(
            std::fs::read(&parked).expect("read parked lock"),
            content_for_assert.as_bytes(),
            "fresh solution must be parked as pixi.lock.new"
        );
    }

    #[tokio::test]
    async fn re_solve_failure_restores_committed_lock_and_surfaces_original_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");

        let err = re_solve_in_place(&lock, || async {
            Err("HTTP status client error (404 Not Found)".to_string())
        })
        .await
        .expect_err("re-solve failure must propagate");

        match err {
            ReSolveError::Solve(msg) => assert!(msg.contains("404"), "{msg}"),
            other => panic!("expected Solve error, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&lock).expect("read committed lock"),
            b"committed-lock-bytes",
            "committed lock must be restored even when the re-solve fails"
        );
        assert!(
            !dir.path().join(NEW_LOCK_NAME).exists(),
            "a failed re-solve must not leave a side-car lock"
        );
    }

    #[tokio::test]
    async fn re_solve_without_fresh_lock_restores_committed_lock() {
        // Re-solve reports success but pixi wrote no lock (unexpected
        // backend drift): the committed lock must still come back.
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");

        let err = re_solve_in_place(&lock, || async { Ok(()) })
            .await
            .expect_err("missing fresh lock must surface as an error");

        assert!(matches!(err, ReSolveError::Io(_)), "{err:?}");
        assert_eq!(
            std::fs::read(&lock).expect("read committed lock"),
            b"committed-lock-bytes"
        );
    }

    #[tokio::test]
    async fn re_solve_replaces_stale_side_car() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        let side_car = dir.path().join(NEW_LOCK_NAME);
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");
        std::fs::write(&side_car, b"previous-fallback").expect("write stale side-car");

        let lock_for_task = lock.clone();
        let result = re_solve_in_place(&lock, move || {
            let lock = lock_for_task.clone();
            async move {
                std::fs::write(&lock, b"fresh-solution").expect("fake solve writes lock");
                Ok(())
            }
        })
        .await;
        assert!(result.is_ok(), "{result:?}");

        assert_eq!(
            std::fs::read(&side_car).expect("read side-car"),
            b"fresh-solution",
            "a previous side-car must be replaced by the fresh solution"
        );
        assert_eq!(
            std::fs::read(&lock).expect("read committed lock"),
            b"committed-lock-bytes"
        );
    }

    #[tokio::test]
    async fn re_solve_park_failure_still_restores_committed_lock() {
        // Side-car parking must never skip the committed-lock restore: a
        // blocked pixi.lock.new (here: the name is occupied by a directory)
        // degrades the fallback to a warning, but the committed tree comes
        // back byte-identical.
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("pixi.lock");
        std::fs::write(&lock, b"committed-lock-bytes").expect("write committed lock");
        std::fs::create_dir(dir.path().join(NEW_LOCK_NAME)).expect("occupy side-car name");

        let lock_for_task = lock.clone();
        let fresh = re_solve_in_place(&lock, move || {
            let lock = lock_for_task.clone();
            async move {
                std::fs::write(&lock, b"fresh-solution").expect("fake solve writes lock");
                Ok(())
            }
        })
        .await
        .expect("park failure must not fail an otherwise successful re-solve");

        assert!(!fresh.side_car_parked, "side-car could not be parked");
        assert_eq!(
            std::fs::read(&lock).expect("read committed lock"),
            b"committed-lock-bytes",
            "committed lock must be restored even when parking fails"
        );
    }
}
