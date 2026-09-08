use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::commands::print_banner;

/// Replace oxo-flow wildcards (`{name}`) with glob `*` for filesystem matching.
fn replace_oxoflow_wildcards_with_glob(pattern: &str) -> String {
    let mut result = String::new();
    let mut in_wildcard = false;
    for c in pattern.chars() {
        match c {
            '{' => {
                in_wildcard = true;
                result.push('*');
            }
            '}' => {
                in_wildcard = false;
            }
            _ if !in_wildcard => result.push(c),
            _ => {} // skip chars inside wildcard
        }
    }
    result
}

/// Resolve a declared output path against the workdir.
///
/// Relative declarations are workdir-relative (the convention `run`,
/// `validate`, and `quality` all use); absolute ones are left untouched so the
/// caller's safety check can reject them.
fn resolve_output_path(workdir: &Path, output: &str) -> PathBuf {
    let path = Path::new(output);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    }
}

pub fn clean_command(
    workflow: PathBuf,
    dry_run: bool,
    force: bool,
    orphans: bool,
    workdir: Option<PathBuf>,
) -> Result<()> {
    print_banner();

    // If neither --force nor --dry-run is provided, default to dry-run
    // to prevent accidental data loss.
    let is_dry_run = dry_run || !force;

    // Working directory for .oxo-flow artifacts: explicit --workdir wins,
    // default is the workflow file's directory (issue #68).
    let workdir = workdir.unwrap_or_else(|| oxo_flow_core::parent_dir(&workflow).to_path_buf());

    // Refuse to delete while a run is active in this workdir (issue #70):
    // the run's checkpoint and outputs must not vanish mid-flight.
    // --force overrides with a warning.
    if !is_dry_run && oxo_flow_core::executor::WorkdirLock::is_locked(&workdir) {
        if force {
            eprintln!(
                "{} workdir is locked by a running oxo-flow process — cleaning anyway (--force)",
                "Warning:".yellow()
            );
        } else {
            anyhow::bail!(
                "another oxo-flow run is active in this workdir — wait for it to finish \
                 before cleaning, or pass --force to override"
            );
        }
    }

    // Handle orphan cleanup mode
    if orphans {
        let chunks_dir = workdir.join(".oxo-flow/chunks");

        if !chunks_dir.exists() {
            eprintln!("{} No orphan chunks directory found", "Clean:".bold());
            return Ok(());
        }

        // Collect orphan chunk directories
        let mut orphan_dirs: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&chunks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    orphan_dirs.push(path);
                }
            }
        }

        if orphan_dirs.is_empty() {
            eprintln!("{} No orphan chunks found", "Clean:".bold());
            return Ok(());
        }

        if is_dry_run {
            eprintln!("{}", "Would clean orphan chunks (dry-run):".bold().yellow());
            for dir in &orphan_dirs {
                eprintln!("  {} (directory)", dir.display());
            }
            eprintln!(
                "\n{} {} orphan chunk directories",
                "Total:".bold(),
                orphan_dirs.len()
            );
            if !dry_run && !force {
                eprintln!(
                    "\n{}",
                    "Run with --force to actually delete these directories."
                        .bold()
                        .cyan()
                );
            }
        } else {
            // Force is true at this point, proceed with deletion
            let mut deleted = 0usize;
            let mut failed = 0usize;

            for dir in &orphan_dirs {
                match std::fs::remove_dir_all(dir) {
                    Ok(()) => {
                        deleted += 1;
                        eprintln!("  {} {}", "✓".green(), dir.display());
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("  {} {} — {}", "✗".red(), dir.display(), e);
                    }
                }
            }

            // Also clean the parent chunks_dir if it's now empty
            if deleted == orphan_dirs.len()
                && chunks_dir.exists()
                && let Ok(entries) = std::fs::read_dir(&chunks_dir)
                && entries.count() == 0
            {
                std::fs::remove_dir(&chunks_dir).ok();
                eprintln!("  {} {}", "✓".green(), chunks_dir.display());
            }

            eprintln!(
                "\n{} {} orphan directories deleted, {} failed",
                "Done:".bold(),
                deleted,
                failed
            );
        }
        return Ok(());
    }

    // Normal clean mode - clean workflow outputs
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // Build config variable map so {config.key} paths can be expanded
    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    // Collect unique output paths, expanding config variable placeholders
    let mut outputs: Vec<String> = Vec::new();
    for rule in &config.rules {
        for output in &rule.output {
            let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                output,
                &wildcard_values,
            );
            if !outputs.contains(&expanded) {
                outputs.push(expanded);
            }
        }
    }

    // Resolve wildcard patterns to actual files via glob.
    //
    // Declared outputs are relative to the workdir (the same convention every
    // other workflow command uses) — resolving them against the process CWD
    // deleted unrelated files when `clean` ran from another directory
    // (audit finding C1).
    let mut resolved: Vec<(String, PathBuf)> = Vec::new();
    let mut unresolved_wildcards: Vec<String> = Vec::new();
    for output in &outputs {
        let has_wildcard = output.contains('{') && output.contains('}');
        if has_wildcard {
            // Convert oxo-flow wildcard {name} to glob * for matching
            let glob_pattern = replace_oxoflow_wildcards_with_glob(output);
            let full_glob = if Path::new(&glob_pattern).is_absolute() {
                glob_pattern.clone()
            } else {
                workdir.join(&glob_pattern).to_string_lossy().to_string()
            };
            match glob::glob(&full_glob) {
                Ok(paths) => {
                    let mut found = false;
                    for path in paths.flatten() {
                        if !resolved.iter().any(|(_, r)| *r == path) {
                            resolved.push((output.clone(), path));
                            found = true;
                        }
                    }
                    if !found {
                        unresolved_wildcards.push(output.clone());
                    }
                }
                Err(_) => {
                    unresolved_wildcards.push(output.clone());
                }
            }
        } else {
            let path = resolve_output_path(&workdir, output);
            if !resolved.iter().any(|(_, r)| *r == path) {
                resolved.push((output.clone(), path));
            }
        }
    }

    if is_dry_run {
        eprintln!("{}", "Would clean (dry-run):".bold().yellow());
        for (_, path) in &resolved {
            if path.exists() {
                eprintln!("  {} (exists)", path.display().to_string().dimmed());
            } else {
                eprintln!("  {} (not found)", path.display().to_string().dimmed());
            }
        }
        for pattern in &unresolved_wildcards {
            eprintln!("  {} (no files matched)", pattern.dimmed());
        }
        eprintln!(
            "\n{} {} patterns → {} files{}",
            "Total:".bold(),
            outputs.len(),
            resolved.len(),
            if unresolved_wildcards.is_empty() {
                "".to_string()
            } else {
                format!(" (+ {} unresolved wildcards)", unresolved_wildcards.len())
            }
        );
        if !dry_run && !force {
            eprintln!(
                "\n{}",
                "Run with --force to actually delete these files."
                    .bold()
                    .cyan()
            );
        }
    } else {
        // Determine which files are deletable. The declared string is what
        // the safety check inspects (`..`, absolute, `~`); deletion uses the
        // workdir-resolved path.
        let mut deletable: Vec<PathBuf> = Vec::new();
        let skipped_wildcard = unresolved_wildcards.len();
        let mut not_found = 0usize;
        let mut rejected = 0usize;

        for (declared, path) in &resolved {
            if declared.contains("..") || declared.starts_with('/') || declared.starts_with('~') {
                eprintln!(
                    "  {} {} (rejected: unsafe path)",
                    "✗".red().bold(),
                    declared
                );
                rejected += 1;
            } else if path.exists() {
                deletable.push(path.clone());
            } else {
                not_found += 1;
            }
        }

        if deletable.is_empty() {
            eprintln!(
                "{} Nothing to delete ({} not found, {} wildcard skipped, {} rejected)",
                "Clean:".bold(),
                not_found,
                skipped_wildcard,
                rejected
            );
        } else {
            // Confirmation prompt before destructive action (interactive only)
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                eprintln!(
                    "\n{} {} file(s) will be deleted. Continue? [y/N]",
                    "⚠".yellow(),
                    deletable.len()
                );
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).ok();
                if !input.trim().eq_ignore_ascii_case("y") {
                    eprintln!("  Cancelled.");
                    return Ok(());
                }
            }

            let mut deleted = 0usize;
            let mut failed = 0usize;

            for path in &deletable {
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        deleted += 1;
                        eprintln!("  {} {}", "✓".green(), path.display());
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("  {} {} — {}", "✗".red(), path.display(), e);
                    }
                }
            }

            eprintln!(
                "\n{} {} deleted, {} failed, {} not found, {} wildcard skipped, {} rejected",
                "Done:".bold(),
                deleted,
                failed,
                not_found,
                skipped_wildcard,
                rejected
            );
        }
    }
    Ok(())
}
