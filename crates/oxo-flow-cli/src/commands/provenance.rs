use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// One verified file's outcome, machine-readable for `--json`.
#[derive(Debug, Serialize)]
struct VerifyEntry {
    file: String,
    /// `matched`, `mismatched`, or `missing`.
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

    // Try embedded checksums first, then companion file
    let stored_checksums: HashMap<String, String> = if let Some(checksums) =
        checkpoint.get("checksums").and_then(|v| v.as_object())
    {
        checksums
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect()
    } else {
        // Try companion file: checkpoint.checksums.json
        let companion = checkpoint_path.with_extension("checksums.json");
        if companion.exists() {
            let content = std::fs::read_to_string(&companion)
                .context("failed to read companion checksums file")?;
            serde_json::from_str(&content).context("failed to parse companion checksums file")?
        } else {
            HashMap::new()
        }
    };

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
    if stored_checksums.is_empty() {
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
                    "entries": [],
                    "note": "No completed rules or checksums found in the checkpoint.",
                },
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        return Ok(());
    }

    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut missing = 0usize;
    let mut entries: Vec<VerifyEntry> = Vec::new();

    // Deterministic order: HashMap iteration is arbitrary, and both the
    // human report and the JSON entries must be byte-stable (the same
    // convention report sections follow, issue #83 P1-4).
    let mut files_to_check: Vec<&String> = stored_checksums.keys().collect();
    files_to_check.sort_unstable();

    for file in files_to_check {
        let expected = &stored_checksums[file];
        let full_path = workdir.join(file);

        if !full_path.exists() {
            eprintln!("  {} {} (file missing)", "✗".red().bold(), file);
            entries.push(VerifyEntry::missing(file, expected));
            missing += 1;
            continue;
        }

        match oxo_flow_core::executor::checkpoint::compute_file_checksum(&full_path) {
            Ok(actual) if actual == *expected => {
                eprintln!("  {} {} {}", "✓".green().bold(), file, actual.dimmed());
                entries.push(VerifyEntry::matched(file, expected, &actual));
                matched += 1;
            }
            Ok(actual) => {
                eprintln!(
                    "  {} {} (expected: {}, actual: {})",
                    "✗".red().bold(),
                    file,
                    expected,
                    actual
                );
                entries.push(VerifyEntry::mismatched(file, expected, &actual));
                mismatched += 1;
            }
            Err(e) => {
                eprintln!("  {} {} (checksum error: {})", "✗".red().bold(), file, e);
                entries.push(VerifyEntry::error(file, expected, &e.to_string()));
                mismatched += 1;
            }
        }
    }

    eprintln!();
    eprintln!(
        "{} {} matched, {} mismatched, {} missing",
        "Summary:".bold(),
        matched,
        mismatched,
        missing
    );

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
