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

pub fn clean_command(workflow: PathBuf, dry_run: bool, force: bool, orphans: bool) -> Result<()> {
    print_banner();

    // If neither --force nor --dry-run is provided, default to dry-run
    // to prevent accidental data loss.
    let is_dry_run = dry_run || !force;

    // Handle orphan cleanup mode
    if orphans {
        let workdir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
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

    // Resolve wildcard patterns to actual files via glob
    let mut resolved_paths: Vec<String> = Vec::new();
    let mut unresolved_wildcards: Vec<String> = Vec::new();
    for output in &outputs {
        let has_wildcard = output.contains('{') && output.contains('}');
        if has_wildcard {
            // Convert oxo-flow wildcard {name} to glob * for matching
            let glob_pattern = replace_oxoflow_wildcards_with_glob(output);
            match glob::glob(&glob_pattern) {
                Ok(paths) => {
                    let mut found = false;
                    for path in paths.flatten() {
                        let s = path.to_string_lossy().to_string();
                        if !resolved_paths.contains(&s) {
                            resolved_paths.push(s);
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
            resolved_paths.push(output.clone());
        }
    }

    if is_dry_run {
        eprintln!("{}", "Would clean (dry-run):".bold().yellow());
        for path in &resolved_paths {
            if Path::new(path).exists() {
                eprintln!("  {} (exists)", path.dimmed());
            } else {
                eprintln!("  {} (not found)", path.dimmed());
            }
        }
        for pattern in &unresolved_wildcards {
            eprintln!("  {} (no files matched)", pattern.dimmed());
        }
        eprintln!(
            "\n{} {} patterns → {} files{}",
            "Total:".bold(),
            outputs.len(),
            resolved_paths.len(),
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
        // Determine which files are deletable
        let mut deletable: Vec<String> = Vec::new();
        let skipped_wildcard = unresolved_wildcards.len();
        let mut not_found = 0usize;
        let mut rejected = 0usize;

        for output in &resolved_paths {
            if output.contains("..") || output.starts_with('/') || output.starts_with('~') {
                eprintln!("  {} {} (rejected: unsafe path)", "✗".red().bold(), output);
                rejected += 1;
            } else if Path::new(output).exists() {
                deletable.push(output.clone());
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
            let mut deleted = 0usize;
            let mut failed = 0usize;

            for path_str in &deletable {
                match std::fs::remove_file(path_str) {
                    Ok(()) => {
                        deleted += 1;
                        eprintln!("  {} {}", "✓".green(), path_str);
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("  {} {} — {}", "✗".red(), path_str, e);
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
