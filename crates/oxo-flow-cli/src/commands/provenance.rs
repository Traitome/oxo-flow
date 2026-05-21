use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Verify output file checksums stored in a checkpoint file.
///
/// Reads the checkpoint JSON, looks for stored checksums (either embedded
/// in the checkpoint under a `"checksums"` key or in a companion file),
/// re-hashes the referenced output files, and reports match/mismatch per file.
pub fn provenance_verify_command(checkpoint_path: PathBuf) -> Result<()> {
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

    let workdir = checkpoint_path
        .parent()
        .unwrap_or(std::path::Path::new("."));

    // Determine which files to verify
    let files_to_check: Vec<String> = if stored_checksums.is_empty() {
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
            return Ok(());
        }
        eprintln!(
            "  {} No completed rules or checksums found.",
            "✗".red().bold()
        );
        return Ok(());
    } else {
        stored_checksums.keys().cloned().collect()
    };

    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut missing = 0usize;

    for file in &files_to_check {
        let expected = &stored_checksums[file];
        let full_path = workdir.join(file);

        if !full_path.exists() {
            eprintln!("  {} {} (file missing)", "✗".red().bold(), file);
            missing += 1;
            continue;
        }

        match oxo_flow_core::executor::checkpoint::compute_file_checksum(&full_path) {
            Ok(actual) if actual == *expected => {
                eprintln!("  {} {} {}", "✓".green().bold(), file, actual.dimmed());
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
                mismatched += 1;
            }
            Err(e) => {
                eprintln!("  {} {} (checksum error: {})", "✗".red().bold(), file, e);
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

    if mismatched > 0 || missing > 0 {
        std::process::exit(1);
    }

    Ok(())
}
