use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::commands::hash_file_with;

/// One verified file's outcome, machine-readable for `--json`.
#[derive(Debug, Serialize)]
struct VerifyDataEntry {
    file: String,
    /// `matched`, `mismatched`, `missing`, or `extra`.
    status: &'static str,
    /// The md5 recorded in the vendor sidecar (absent for extra files).
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
    /// The md5 recomputed from disk (absent for missing files).
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<String>,
    /// Free-form detail (e.g. a sidecar parse error or checksum failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

impl VerifyDataEntry {
    fn matched(file: &str, expected: &str, actual: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "matched",
            expected: Some(expected.to_string()),
            actual: Some(actual.to_string()),
            note: None,
        }
    }
    fn mismatched(file: &str, expected: &str, actual: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "mismatched",
            expected: Some(expected.to_string()),
            actual: Some(actual.to_string()),
            note: None,
        }
    }
    fn error(file: &str, expected: &str, note: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "mismatched",
            expected: Some(expected.to_string()),
            actual: None,
            note: Some(note.to_string()),
        }
    }
    fn missing(file: &str, expected: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "missing",
            expected: Some(expected.to_string()),
            actual: None,
            note: None,
        }
    }
    fn extra(file: &str, actual: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "extra",
            expected: None,
            actual: Some(actual.to_string()),
            note: None,
        }
    }
    fn extra_error(file: &str, note: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "extra",
            expected: None,
            actual: None,
            note: Some(note.to_string()),
        }
    }
}

/// Parse a vendor md5 sidecar into (relative path → md5 hex, in sidecar order).
///
/// Accepts the two standard `<md5> <path>` forms shipped by `md5sum` and
/// BSD `md5`:
///
/// ```text
/// d41d8cd98f00b204e9800998ecf8427e  raw/S1.fq        (two spaces)
/// d41d8cd98f00b204e9800998ecf8427e *raw/S1.fq        (space + asterisk: binary mode)
/// ```
///
/// Leading `./` on the path is stripped so trees walked with a trailing
/// slash and sidecars written from inside the tree agree. A malformed
/// line (bad digest length, empty path) is a hard error naming the line
/// number — silently skipping it would report a corrupted delivery as
/// clean (the exact failure verify-data exists to catch).
fn parse_md5_sidecar(content: &str) -> Result<Vec<(String, String)>> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (idx, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim_end_matches(['\r']);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let body = line.strip_prefix("\\").unwrap_or(line); // GNU escaped-name marker
        let digest_end = body.find([' ', '\t']).with_context(|| {
            format!(
                "md5 sidecar line {} is malformed (expected \"<32-hex md5> <path>\"): {line}",
                idx + 1
            )
        })?;
        let digest = body[..digest_end].to_ascii_lowercase();
        let path = body[digest_end..].trim_start();
        let path = path.strip_prefix('*').unwrap_or(path).trim();
        if digest.len() != 32 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "md5 sidecar line {} has a malformed digest (expected 32 hex chars): {line}",
                idx + 1
            );
        }
        if path.is_empty() {
            anyhow::bail!("md5 sidecar line {} has an empty path: {line}", idx + 1);
        }
        let path = path.strip_prefix("./").unwrap_or(path);
        if path.starts_with('/') || path.split('/').any(|seg| seg == "..") {
            anyhow::bail!(
                "md5 sidecar line {} escapes the tree (absolute or '..' path): {line}",
                idx + 1
            );
        }
        let normalized = path.to_string();
        if !seen.insert(normalized.clone()) {
            anyhow::bail!(
                "md5 sidecar line {} repeats path '{path}' (an earlier line already lists it)",
                idx + 1
            );
        }
        entries.push((normalized, digest));
    }
    Ok(entries)
}

/// Stream the md5 of one file via the shared stat-first hash helper.
///
/// The regular-file gate lives in `hash_file_with` (issue #714):
/// `File::open` on a writer-less FIFO blocks forever, so regularity is
/// checked before any descriptor is opened.
fn md5_of_file(path: &Path) -> Result<String> {
    use md5::{Digest, Md5};

    hash_file_with(path, Md5::new())
}

/// The pure tree-walk + classification core (unit-testable).
///
/// Returns `(matched, mismatched, missing, extra, entries)` in a
/// deterministic order: sidecar entries in sorted path order, then
/// on-disk extras in sorted path order (`sort_by_file_name`).
fn verify_tree(
    expected: &[(String, String)],
    tree: &Path,
    ignore: &[&str],
) -> (usize, usize, usize, usize, Vec<VerifyDataEntry>) {
    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut missing = 0usize;
    let mut extra = 0usize;
    let mut entries: Vec<VerifyDataEntry> = Vec::new();

    let mut sorted: Vec<&(String, String)> = expected.iter().collect();
    sorted.sort_unstable();

    let mut listed: BTreeSet<&str> = BTreeSet::new();
    for (file, digest) in sorted {
        listed.insert(file.as_str());
        let full_path = tree.join(file);
        if !full_path.exists() {
            entries.push(VerifyDataEntry::missing(file, digest));
            missing += 1;
            continue;
        }
        match md5_of_file(&full_path) {
            Ok(actual) if actual == *digest => {
                entries.push(VerifyDataEntry::matched(file, digest, &actual));
                matched += 1;
            }
            Ok(actual) => {
                entries.push(VerifyDataEntry::mismatched(file, digest, &actual));
                mismatched += 1;
            }
            Err(e) => {
                entries.push(VerifyDataEntry::error(file, digest, &e.to_string()));
                mismatched += 1;
            }
        }
    }

    // On-disk extras: files present under the tree but absent from the
    // sidecar (or explicitly ignored). Deterministic sorted order
    // (`sort_by_file_name` — walkdir's default is OS readdir order);
    // the walk skips engine internals and FIFOs (stat-first gate parity).
    //
    // Traversal errors (e.g. an unreadable subdirectory) are reported as
    // extra-error entries instead of silently skipped — an audit that
    // couldn't fully walk the tree must not present a clean verdict (#938
    // m5). Symlinks are neither followed nor hashed: `follow_links(false)`
    // yields them with a non-regular file type, so they stay invisible to
    // this audit — documented in docs/guide/src/commands/verify-data.md.
    for entry in walkdir::WalkDir::new(tree)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                // A failed directory read yields no entry; report the
                // traversal failure itself so the summary reflects it.
                // Fail-closed even for paths the Ok arm would skip
                // (`ignore`-listed, `.oxo-flow/`): the walk could not see
                // past this point, so an unreadable engine-internal
                // directory still invalidates a clean verdict (#938 m5).
                let rel = entry_root_relative(tree, e.path().unwrap_or(tree));
                entries.push(VerifyDataEntry::extra_error(
                    &rel,
                    &format!("tree walk failed: {e}"),
                ));
                extra += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(tree)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        if listed.contains(rel.as_str()) || ignore.contains(&rel.as_str()) {
            continue;
        }
        let rel_owned = rel.trim_start_matches("./").to_string();
        if listed.contains(rel_owned.as_str())
            || ignore.contains(&rel_owned.as_str())
            || engine_internal(&rel_owned)
        {
            continue;
        }
        match md5_of_file(entry.path()) {
            Ok(actual) => {
                entries.push(VerifyDataEntry::extra(&rel_owned, &actual));
                extra += 1;
            }
            Err(e) => {
                entries.push(VerifyDataEntry::extra_error(&rel_owned, &e.to_string()));
                extra += 1;
            }
        }
    }

    (matched, mismatched, missing, extra, entries)
}

/// Best-effort tree-relative rendering of a walkdir error path. Falls
/// back to `"."` when the error carries no path (or the tree root itself
/// failed) so the JSON never carries an empty-string `file` key.
fn entry_root_relative(tree: &Path, path: &Path) -> String {
    match path.strip_prefix(tree) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().to_string(),
        Ok(_) => ".".to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// Default per-tree exclusion applied to the extras walk: the engine's
/// own artifact directory (`.oxo-flow/`) inside the verified tree is
/// engine bookkeeping, never delivery content.
fn engine_internal(relative: &str) -> bool {
    relative == ".oxo-flow" || relative.starts_with(".oxo-flow/")
}

/// Verify a data tree against a vendor md5 sidecar (issue #935 P2).
///
/// `<tree>` is the directory to audit; `<md5-file>` is a sidecar in the
/// standard `md5sum` text format (`<32-hex md5> <path>`). Paths in the
/// sidecar resolve relative to `<tree>`.
pub fn verify_data_command(tree: PathBuf, from: PathBuf, json: bool) -> Result<()> {
    let tree_abs = std::path::absolute(&tree).context("failed to resolve tree path")?;
    let from_abs = std::path::absolute(&from).context("failed to resolve md5 file path")?;

    if !tree_abs.is_dir() {
        anyhow::bail!("{} is not a directory", tree_abs.display());
    }

    eprintln!(
        "{} {} {} {}",
        "Verify Data".bold().cyan(),
        tree_abs.display(),
        "against".dimmed(),
        from_abs.display()
    );
    eprintln!();

    let sidecar = std::fs::read_to_string(&from_abs)
        .with_context(|| format!("failed to read md5 sidecar {}", from_abs.display()))?;
    let listed = parse_md5_sidecar(&sidecar)
        .with_context(|| format!("failed to parse md5 sidecar {}", from_abs.display()))?;
    if listed.is_empty() {
        eprintln!(
            "  {} md5 sidecar lists no files — nothing to verify (extras scan skipped).",
            "Note:".bold().yellow()
        );
        if json {
            let output = serde_json::json!({
                "command": "verify-data",
                "tree": tree_abs.display().to_string(),
                "from": from_abs.display().to_string(),
                "matched": 0,
                "mismatched": 0,
                "missing": 0,
                "extra": 0,
                "entries": [],
                "note": "The md5 sidecar lists no files.",
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        return Ok(());
    }

    // The sidecar's own file is a common member of delivery trees
    // (`md5.txt` next to the data it describes); hashing it against
    // itself would always mismatch. If the tree contains it, exclude it
    // from the extras walk.
    let mut ignore: Vec<String> = Vec::new();
    if let Ok(rel) = from_abs.strip_prefix(&tree_abs) {
        ignore.push(rel.to_string_lossy().to_string());
    }
    let ignore_refs: Vec<&str> = ignore.iter().map(|s| s.as_str()).collect();

    let (matched, mismatched, missing, extra, entries) =
        verify_tree(&listed, &tree_abs, &ignore_refs);

    for entry in &entries {
        match entry.status {
            "matched" => eprintln!(
                "  {} {} {}",
                "✓".green().bold(),
                entry.file,
                entry.actual.as_deref().unwrap_or_default().dimmed()
            ),
            "missing" => eprintln!(
                "  {} {} (listed in sidecar, file missing)",
                "✗".red().bold(),
                entry.file
            ),
            "extra" => match (&entry.actual, &entry.note) {
                (Some(actual), None) => eprintln!(
                    "  {} {} (not listed in sidecar, md5 {})",
                    "!".yellow().bold(),
                    entry.file,
                    actual
                ),
                (_, Some(note)) => eprintln!(
                    "  {} {} (not listed in sidecar, {})",
                    "!".yellow().bold(),
                    entry.file,
                    note
                ),
                _ => eprintln!(
                    "  {} {} (not listed in sidecar)",
                    "!".yellow().bold(),
                    entry.file
                ),
            },
            _ => match (&entry.expected, &entry.actual, &entry.note) {
                (Some(expected), Some(actual), None) => eprintln!(
                    "  {} {} (expected: {}, actual: {})",
                    "✗".red().bold(),
                    entry.file,
                    expected,
                    actual
                ),
                (_, _, Some(note)) => {
                    eprintln!(
                        "  {} {} (checksum error: {})",
                        "✗".red().bold(),
                        entry.file,
                        note
                    )
                }
                _ => eprintln!("  {} {} (mismatched)", "✗".red().bold(), entry.file),
            },
        }
    }

    eprintln!();
    eprintln!(
        "{} {} matched, {} mismatched, {} missing, {} extra (not listed)",
        "Summary:".bold(),
        matched,
        mismatched,
        missing,
        extra
    );

    // Machine-readable result — stdout carries ONLY this document in
    // --json mode (the human report above stays on stderr).
    if json {
        let output = serde_json::json!({
            "command": "verify-data",
            "tree": tree_abs.display().to_string(),
            "from": from_abs.display().to_string(),
            "matched": matched,
            "mismatched": mismatched,
            "missing": missing,
            "extra": extra,
            "entries": entries,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    // Any anomaly fails the audit: a delivery that lost a file or gained
    // an unlisted one is exactly what post-incident verification must
    // catch, same policy as `provenance verify`.
    if mismatched > 0 || missing > 0 || extra > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn md5_of(content: &str) -> String {
        use md5::Digest;
        let mut h = md5::Md5::new();
        h.update(content.as_bytes());
        hex::encode(h.finalize())
    }

    #[test]
    fn sidecar_parser_accepts_both_md5sum_forms() {
        let content = "\
d41d8cd98f00b204e9800998ecf8427e  raw/S1.fq
0123456789abcdef0123456789abcdef *raw/S2.fq
0123456789abcdef0123456789ABCDEF ./raw/S3.fq
";
        let entries = parse_md5_sidecar(content).unwrap();
        assert_eq!(
            entries,
            vec![
                (
                    "raw/S1.fq".to_string(),
                    "d41d8cd98f00b204e9800998ecf8427e".to_string()
                ),
                (
                    "raw/S2.fq".to_string(),
                    "0123456789abcdef0123456789abcdef".to_string()
                ),
                (
                    "raw/S3.fq".to_string(),
                    "0123456789abcdef0123456789abcdef".to_string()
                ),
            ]
        );
    }

    #[test]
    fn sidecar_parser_rejects_malformed_lines_with_line_numbers() {
        let err = parse_md5_sidecar("d41d8cd  raw/S1.fq\n").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
        let err = parse_md5_sidecar("d41d8cd98f00b204e9800998ecf8427e\n").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
        let err = parse_md5_sidecar("d41d8cd98f00b204e9800998ecf8427e  \n").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
    }

    #[test]
    fn sidecar_parser_skips_blank_and_comment_lines() {
        let content = "\
# vendor manifest
d41d8cd98f00b204e9800998ecf8427e  a.txt

0123456789abcdef0123456789abcdef  b.txt
";
        let entries = parse_md5_sidecar(content).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn sidecar_parser_rejects_duplicate_paths() {
        let err = parse_md5_sidecar(
            "d41d8cd98f00b204e9800998ecf8427e  a.txt\n0123456789abcdef0123456789abcdef  a.txt\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("line 2"), "{err}");
    }

    #[test]
    fn tree_walk_classifies_matched_mismatched_missing_extra() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "good.txt", "hello\n");
        write_file(dir.path(), "bad.txt", "CORRUPTED\n");
        write_file(dir.path(), "unlisted.txt", "extra\n");

        let good_md5 = md5_of("hello\n");
        let bad_md5 = md5_of("CORRUPTED\n");
        let expected = vec![
            ("bad.txt".to_string(), md5_of("original\n")),
            ("good.txt".to_string(), good_md5.clone()),
            ("gone.txt".to_string(), bad_md5.clone()),
        ];

        let (matched, mismatched, missing, extra, entries) =
            verify_tree(&expected, dir.path(), &[]);
        assert_eq!((matched, mismatched, missing, extra), (1, 1, 1, 1));
        // Sidecar entries come sorted (bad → gone → good), extras follow.
        let statuses: Vec<&str> = entries.iter().map(|e| e.status).collect();
        assert_eq!(statuses, vec!["mismatched", "missing", "matched", "extra"]);
    }

    #[test]
    fn extras_walk_is_sorted_and_skips_listed_and_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "listed.txt", "x\n");
        let zeta = write_file(dir.path(), "zeta.txt", "z\n");
        let alpha = write_file(dir.path(), "alpha.txt", "a\n");
        let sub = dir.path().join(".oxo-flow");
        std::fs::create_dir_all(&sub).unwrap();
        let internal = write_file(&sub, "checkpoint.json", "{}\n");

        let listed = vec![("listed.txt".to_string(), md5_of("x\n"))];
        let (_, _, _, extra, entries) = verify_tree(&listed, dir.path(), &[]);
        assert_eq!(extra, 2);
        // Deterministic order: alpha before zeta; engine internals skipped.
        let files: Vec<&str> = entries
            .iter()
            .filter(|e| e.status == "extra")
            .map(|e| e.file.as_str())
            .collect();
        assert_eq!(files, vec!["alpha.txt", "zeta.txt"]);
        assert!(
            entries
                .iter()
                .all(|e| e.file != ".oxo-flow/checkpoint.json")
        );
        let _ = (zeta, alpha, internal);
    }

    #[test]
    fn engine_internal_matches_directory_and_prefix() {
        assert!(engine_internal(".oxo-flow"));
        assert!(engine_internal(".oxo-flow/checkpoint.json"));
        assert!(!engine_internal(".oxo-flowx"));
        assert!(!engine_internal("data/.oxo-flow"));
    }

    #[test]
    fn unreadable_subdirectory_is_reported_not_silently_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "listed.txt", "x\n");
        let locked = dir.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();

        // Strip all permission bits so readdir on `locked/` fails; walkdir
        // then surfaces a traversal error instead of an entry (Unix only —
        // permission bits are advisory on some platforms, so this test
        // only asserts when the lock actually takes effect).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&locked).unwrap();
            let original = meta.permissions().mode() & 0o777;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

            let listed = vec![("listed.txt".to_string(), md5_of("x\n"))];
            let (_, _, _, extra, entries) = verify_tree(&listed, dir.path(), &[]);

            // Restore first so tempdir cleanup can succeed.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(original)).unwrap();

            if extra == 0 {
                // The walk did not fail (e.g. running as root); nothing to
                // assert beyond the clean classification.
                return;
            }
            assert_eq!(extra, 1, "expected exactly the traversal-error entry");
            let entry = entries.last().unwrap();
            assert_eq!(entry.status, "extra");
            assert!(
                entry
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("tree walk failed"),
                "note should name the traversal failure: {:?}",
                entry.note
            );
        }
    }
}
