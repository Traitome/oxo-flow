use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One verified file's outcome, machine-readable for `--json`.
#[derive(Debug, Serialize)]
struct VerifyEntry {
    file: String,
    /// `matched`, `mismatched`, `missing`, or `cleaned`.
    status: &'static str,
    /// The checksum recorded in the checkpoint at run time.
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
    /// The checksum recomputed from disk (absent for missing files).
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<String>,
    /// Free-form detail (e.g. a checksum computation error).
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

impl VerifyEntry {
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
    fn cleaned(file: &str, expected: &str) -> Self {
        Self {
            file: file.to_string(),
            status: "cleaned",
            expected: Some(expected.to_string()),
            actual: None,
            note: None,
        }
    }
}

/// Pure classification of recorded checksums against disk (unit-testable).
///
/// `cleaned` entries are outputs the engine deleted by design at the end
/// of a successful run (transform chunk intermediates, `cleanup = true`,
/// issue #315 F2) — reported as cleaned regardless of on-disk presence.
/// Returns `(matched, mismatched, missing, cleaned, entries)` in a
/// deterministic order: sorted `stored` files, then sorted `cleaned` files.
fn verify_files(
    stored: &HashMap<String, String>,
    cleaned: &HashMap<String, String>,
    workdir: &Path,
) -> (usize, usize, usize, usize, Vec<VerifyEntry>) {
    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut missing = 0usize;
    let mut cleaned_count = 0usize;
    let mut entries: Vec<VerifyEntry> = Vec::new();

    // Deterministic order: HashMap iteration is arbitrary, and both the
    // human report and the JSON entries must be byte-stable (the same
    // convention report sections follow, issue #83 P1-4).
    let mut files_to_check: Vec<&String> = stored.keys().collect();
    files_to_check.sort_unstable();

    for file in files_to_check {
        let expected = &stored[file];
        let full_path = workdir.join(file);

        if !full_path.exists() {
            entries.push(VerifyEntry::missing(file, expected));
            missing += 1;
            continue;
        }

        match oxo_flow_core::executor::checkpoint::compute_file_checksum(&full_path) {
            Ok(actual) if actual == *expected => {
                entries.push(VerifyEntry::matched(file, expected, &actual));
                matched += 1;
            }
            Ok(actual) => {
                entries.push(VerifyEntry::mismatched(file, expected, &actual));
                mismatched += 1;
            }
            Err(e) => {
                entries.push(VerifyEntry::error(file, expected, &e.to_string()));
                mismatched += 1;
            }
        }
    }

    let mut cleaned_files: Vec<&String> = cleaned.keys().collect();
    cleaned_files.sort_unstable();
    for file in cleaned_files {
        entries.push(VerifyEntry::cleaned(file, &cleaned[file]));
        cleaned_count += 1;
    }

    (matched, mismatched, missing, cleaned_count, entries)
}

/// Verify output file checksums stored in a checkpoint file.
///
/// Reads the checkpoint JSON, looks for stored checksums (either embedded
/// in the checkpoint under a `"checksums"` key or in a companion file),
/// re-hashes the referenced output files, and reports match/mismatch per
/// file.
pub fn provenance_verify_command(checkpoint_path: PathBuf, json: bool) -> Result<()> {
    let checkpoint_path =
        std::path::absolute(&checkpoint_path).context("failed to resolve checkpoint path")?;

    eprintln!(
        "{} {}",
        "Provenance Verify".bold().cyan(),
        checkpoint_path.display()
    );
    eprintln!();

    // Load checkpoint as generic JSON so we can flexibly look for checksums
    let checkpoint_content = std::fs::read_to_string(&checkpoint_path)
        .with_context(|| format!("failed to read {}", checkpoint_path.display()))?;

    let checkpoint: serde_json::Value =
        serde_json::from_str(&checkpoint_content).with_context(|| {
            format!(
                "failed to parse checkpoint JSON: {}",
                checkpoint_path.display()
            )
        })?;

    // Workflow provenance: the git HEAD SHA recorded at run start
    // (issue #115 pillar 1) — which workflow version produced these
    // results.
    if let Some(sha) = checkpoint
        .get("workflow_git_sha")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        eprintln!("  {} workflow git HEAD: {}", "•".dimmed(), sha);
    }
    if let Some(path) = checkpoint.get("workflow_path").and_then(|v| v.as_str()) {
        eprintln!("  {} workflow path: {}", "•".dimmed(), path);
    }
    eprintln!();

    // Try embedded checksums first, then companion file. Non-string values
    // are a hard parse error — silently coercing them to "" would falsify
    // the audit trail without a diagnostic.
    let mut stored_checksums: HashMap<String, String> = HashMap::new();
    if let Some(checksums) = checkpoint.get("checksums").and_then(|v| v.as_object()) {
        for (k, v) in checksums {
            let s = v
                .as_str()
                .with_context(|| format!("checkpoint checksums value for '{k}' is not a string"))?;
            stored_checksums.insert(k.clone(), s.to_string());
        }
    } else {
        // Try companion file: checkpoint.checksums.json
        let companion = checkpoint_path.with_extension("checksums.json");
        if companion.exists() {
            let content = std::fs::read_to_string(&companion)
                .context("failed to read companion checksums file")?;
            stored_checksums = serde_json::from_str(&content)
                .context("failed to parse companion checksums file")?;
        }
    }

    // Chunk outputs deleted by design at the end of a successful run
    // (transform `cleanup = true`, issue #315 F2) — embedded under their
    // own key, never in the companion file (which predates the
    // cleaned/missing distinction).
    let mut cleaned_checksums: HashMap<String, String> = HashMap::new();
    if let Some(checksums) = checkpoint
        .get("cleaned_checksums")
        .and_then(|v| v.as_object())
    {
        for (k, v) in checksums {
            let s = v.as_str().with_context(|| {
                format!("checkpoint cleaned_checksums value for '{k}' is not a string")
            })?;
            cleaned_checksums.insert(k.clone(), s.to_string());
        }
    }

    // Output paths resolve against the workdir the run recorded in the
    // checkpoint (issue #68), NOT against the checkpoint's own directory:
    // `.oxo-flow/` is a sibling of the data files, and resolving against
    // it reported every intact output as "file missing" while the actual
    // file sat untouched next to it (issue #142 H7). Legacy checkpoints
    // without a recorded workdir fall back to the checkpoint's parent.
    let workdir = checkpoint
        .get("workdir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            checkpoint_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        });
    if json {
        eprintln!("  {} workdir: {}", "•".dimmed(), workdir.display());
    }

    // Determine which files to verify
    if stored_checksums.is_empty() && cleaned_checksums.is_empty() {
        // Fallback: try to discover output files from completed rules
        // Look for files matching rule output patterns in the workdir
        if let Some(completed) = checkpoint.get("completed_rules").and_then(|v| v.as_array()) {
            eprintln!(
                "  {} No stored checksums found. Run workflow with --provenance to enable tracking.",
                "Note:".bold().yellow()
            );
            eprintln!(
                "  {} completed rules: {}\n",
                "Found".bold(),
                completed.len()
            );
            // Just show completed rules as a summary
            for rule_val in completed {
                if let Some(rule) = rule_val.as_str() {
                    eprintln!("  {} {}", "✓".green(), rule);
                }
            }
            eprintln!(
                "\n{} To verify integrity, provide a checksums file.",
                "Hint:".bold().cyan()
            );
            if json {
                let output = serde_json::json!({
                    "command": "provenance",
                    "verify": {
                        "checkpoint": checkpoint_path.display().to_string(),
                        "matched": 0,
                        "mismatched": 0,
                        "missing": 0,
                        "cleaned": 0,
                        "entries": [],
                        "note": "No stored checksums found. Run workflow with --provenance to enable tracking.",
                    },
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            return Ok(());
        }
        eprintln!(
            "  {} No completed rules or checksums found.",
            "✗".red().bold()
        );
        if json {
            let output = serde_json::json!({
                "command": "provenance",
                "verify": {
                    "checkpoint": checkpoint_path.display().to_string(),
                    "matched": 0,
                    "mismatched": 0,
                    "missing": 0,
                    "cleaned": 0,
                    "entries": [],
                    "note": "No completed rules or checksums found in the checkpoint.",
                },
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        return Ok(());
    }

    let (matched, mismatched, missing, cleaned, entries) =
        verify_files(&stored_checksums, &cleaned_checksums, &workdir);

    for entry in &entries {
        match entry.status {
            "matched" => eprintln!(
                "  {} {} {}",
                "✓".green().bold(),
                entry.file,
                entry.actual.as_deref().unwrap_or_default().dimmed()
            ),
            "missing" => eprintln!("  {} {} (file missing)", "✗".red().bold(), entry.file),
            "cleaned" => eprintln!(
                "  {} {} {} (cleaned by design)",
                "✓".cyan().bold(),
                entry.file,
                entry.expected.as_deref().unwrap_or_default().dimmed()
            ),
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
    if cleaned > 0 {
        eprintln!(
            "{} {} matched, {} mismatched, {} missing, {} cleaned (by design)",
            "Summary:".bold(),
            matched,
            mismatched,
            missing,
            cleaned
        );
    } else {
        eprintln!(
            "{} {} matched, {} mismatched, {} missing",
            "Summary:".bold(),
            matched,
            mismatched,
            missing
        );
    }

    // Machine-readable verify result — stdout carries ONLY this document
    // in --json mode (the human report above stays on stderr).
    if json {
        let output = serde_json::json!({
            "command": "provenance",
            "verify": {
                "checkpoint": checkpoint_path.display().to_string(),
                "matched": matched,
                "mismatched": mismatched,
                "missing": missing,
                "cleaned": cleaned,
                "entries": entries,
            },
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    // Missing files are a verification failure too: a vanished output is
    // exactly what integrity verification must catch. Exiting 0 there made
    // the path-resolution false-negative fully silent (issue #142 H7).
    if mismatched > 0 || missing > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        oxo_flow_core::executor::checkpoint::compute_file_checksum(&path).unwrap()
    }

    #[test]
    fn verify_files_classifies_matched_missing_and_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let good_sha = write_file(dir.path(), "good.txt", "hello\n");

        let mut stored = HashMap::new();
        stored.insert("good.txt".to_string(), good_sha);
        stored.insert("gone.txt".to_string(), "sha256:dead".to_string());
        let mut cleaned = HashMap::new();
        cleaned.insert(
            ".oxo-flow/chunks/chr/chr1.out".to_string(),
            "sha256:beef".to_string(),
        );

        let (matched, mismatched, missing, cleaned_n, entries) =
            verify_files(&stored, &cleaned, dir.path());
        assert_eq!((matched, mismatched, missing, cleaned_n), (1, 0, 1, 1));
        assert_eq!(entries.len(), 3);
        let statuses: Vec<&str> = entries.iter().map(|e| e.status).collect();
        assert!(statuses.contains(&"cleaned"));
        assert!(statuses.contains(&"matched"));
        assert!(statuses.contains(&"missing"));
    }

    #[test]
    fn cleaned_entries_report_cleaned_even_when_file_still_exists() {
        // A chunk whose file survived (e.g. the run failed before cleanup)
        // is still "cleaned", never "matched" — its lifecycle is
        // engine-governed either way (issue #315 F2).
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join(".oxo-flow/chunks");
        fs::create_dir_all(&sub).unwrap();
        let sha = write_file(&sub, "chr1.out", "data\n");

        let mut cleaned = HashMap::new();
        cleaned.insert(".oxo-flow/chunks/chr1.out".to_string(), sha);
        let (matched, mismatched, missing, cleaned_n, entries) =
            verify_files(&HashMap::new(), &cleaned, dir.path());
        assert_eq!((matched, mismatched, missing, cleaned_n), (0, 0, 0, 1));
        assert_eq!(entries[0].status, "cleaned");
    }
}
