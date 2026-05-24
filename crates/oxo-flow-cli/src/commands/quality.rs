use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::path::{Path, PathBuf};

use crate::commands::print_banner;

pub fn validate_command(workflow: PathBuf, as_include: bool) -> Result<()> {
    let config_res = WorkflowConfig::from_file(&workflow);
    match config_res {
        Ok(cfg) => {
            if cfg.rules.is_empty() {
                eprintln!("{} {} — 0 rules", "✓".green().bold(), workflow.display());
                eprintln!(
                    "  {} Workflow has no rules. Add [[rules]] sections to define pipeline steps.",
                    "⚠ Warning:".yellow().bold()
                );
                return Ok(());
            }

            // Run semantic validation (E001-E008)
            let validation = oxo_flow_core::format::validate_format(&cfg);
            let mut error_count = 0usize;

            for d in &validation.diagnostics {
                if d.severity == oxo_flow_core::format::Severity::Error {
                    error_count += 1;
                    eprintln!("  {} [{}]: {}", "error".red().bold(), d.code, d.message);
                    if let Some(ref rule) = d.rule {
                        eprintln!("    rule: {}", rule);
                    }
                    if let Some(ref suggestion) = d.suggestion {
                        eprintln!("    hint: {}", suggestion);
                    }
                }
            }

            // Check for missing input files (skip for --as-include)
            let mut missing_inputs = Vec::new();
            if !as_include {
                for rule in &cfg.rules {
                    for input in &rule.input {
                        // Only check if it's not a wildcard path and doesn't exist
                        if !input.contains('{')
                            && !input.contains('}')
                            && !Path::new(input).exists()
                        {
                            // Also check if it's an output of another rule
                            let is_generated =
                                cfg.rules.iter().any(|r| r.output.to_vec().contains(input));

                            if !is_generated {
                                missing_inputs.push(input);
                            }
                        }
                    }
                }
            }

            // Validate DAG construction (skip for --as-include)
            if as_include {
                // For sub-workflow fragments, skip DAG validation
                if error_count == 0 {
                    eprintln!(
                        "{} {} — {} rules (fragment validation)",
                        "✓".green().bold(),
                        workflow.display(),
                        cfg.rules.len()
                    );
                } else {
                    eprintln!(
                        "{} {} — {} validation error(s)",
                        "✗".red().bold(),
                        workflow.display(),
                        error_count
                    );
                }
            } else {
                match WorkflowDag::from_rules(&cfg.rules) {
                    Ok(dag) => {
                        if error_count == 0 {
                            eprintln!(
                                "{} {} — {} rules, {} dependencies",
                                "✓".green().bold(),
                                workflow.display(),
                                dag.node_count(),
                                dag.edge_count()
                            );
                        } else {
                            eprintln!(
                                "{} {} — {} validation error(s)",
                                "✗".red().bold(),
                                workflow.display(),
                                error_count
                            );
                        }

                        if !missing_inputs.is_empty() {
                            eprintln!(
                                "\n  {} The following input files do not exist:",
                                "⚠ Warning:".yellow().bold()
                            );
                            for input in missing_inputs {
                                eprintln!("    - {}", input);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "{} {} — DAG error: {}",
                            "✗".red().bold(),
                            workflow.display(),
                            e
                        );
                        std::process::exit(1);
                    }
                }
            }

            // Exit with error if validation failed
            if error_count > 0 {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{} {} — {}", "✗".red().bold(), workflow.display(), e);
            std::process::exit(1);
        }
    }
    Ok(())
}

pub fn lint_command(workflow: PathBuf, strict: bool) -> Result<()> {
    print_banner();
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint_diags = oxo_flow_core::format::lint_format(&config);

    // Read the raw file content for secret scanning
    let raw_content = std::fs::read_to_string(&workflow).ok();
    let secret_diags = if let Some(content) = raw_content {
        oxo_flow_core::format::scan_for_secrets(&content)
    } else {
        Vec::new()
    };

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    let mut info_count = 0usize;

    for d in validation
        .diagnostics
        .iter()
        .chain(lint_diags.iter())
        .chain(secret_diags.iter())
    {
        let prefix = match d.severity {
            oxo_flow_core::format::Severity::Error => {
                error_count += 1;
                "error".red().bold().to_string()
            }
            oxo_flow_core::format::Severity::Warning => {
                warning_count += 1;
                "warning".yellow().bold().to_string()
            }
            oxo_flow_core::format::Severity::Info => {
                info_count += 1;
                "info".blue().to_string()
            }
        };
        eprint!("  {} [{}]: {}", prefix, d.code, d.message);
        if let Some(ref rule) = d.rule {
            eprint!(" (rule: {})", rule);
        }
        eprintln!();
    }

    eprintln!(
        "\n{} {} error(s), {} warning(s), {} info",
        "Summary:".bold(),
        error_count,
        warning_count,
        info_count
    );

    if error_count > 0 || (strict && warning_count > 0) {
        std::process::exit(1);
    }
    Ok(())
}

pub fn format_command(workflow: PathBuf, output: Option<PathBuf>, check: bool) -> Result<()> {
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let formatted = oxo_flow_core::format::format_workflow(&config);

    if check {
        let original = std::fs::read_to_string(&workflow)?;
        if original.trim() == formatted.trim() {
            eprintln!(
                "{} {} is already formatted",
                "✓".green().bold(),
                workflow.display()
            );
        } else {
            eprintln!(
                "{} {} needs formatting",
                "✗".red().bold(),
                workflow.display()
            );
            std::process::exit(1);
        }
    } else {
        match output {
            Some(path) => {
                std::fs::write(&path, &formatted)?;
                eprintln!("Formatted workflow written to {}", path.display());
            }
            None => {
                print!("{formatted}");
            }
        }
    }
    Ok(())
}

pub fn touch_command(workflow: PathBuf, rules: Vec<String>) -> Result<()> {
    print_banner();
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let rules_to_touch: Vec<&oxo_flow_core::rule::Rule> = if rules.is_empty() {
        config.rules.iter().collect()
    } else {
        config
            .rules
            .iter()
            .filter(|r| rules.contains(&r.name))
            .collect()
    };

    let mut touched = 0usize;
    let mut skipped = 0usize;

    let base_dir = std::env::current_dir().unwrap_or_default();

    for rule in &rules_to_touch {
        for output in &rule.output {
            let has_wildcard = output.contains('{') && output.contains('}');
            if has_wildcard {
                skipped += 1;
                continue;
            }

            // Path safety: reject path traversal and absolute paths
            if output.contains("..") || output.starts_with('/') || output.starts_with('~') {
                eprintln!("  {} {} (rejected: unsafe path)", "✗".red().bold(), output);
                continue;
            }

            let path = base_dir.join(output);
            if path.exists() {
                // Update modification time
                match filetime::set_file_mtime(&path, filetime::FileTime::now()) {
                    Ok(()) => {
                        touched += 1;
                        eprintln!("  {} {}", "✓".green(), output);
                    }
                    Err(e) => {
                        eprintln!("  {} {} ({})", "✗".red(), output, e);
                    }
                }
            } else {
                // Create empty file to mark as "done"
                if let Some(parent) = path.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    eprintln!(
                        "  {} {} (cannot create directory: {})",
                        "✗".red(),
                        output,
                        e
                    );
                    continue;
                }
                match std::fs::write(&path, "") {
                    Ok(()) => {
                        touched += 1;
                        eprintln!("  {} {} (created)", "✓".green(), output);
                    }
                    Err(e) => {
                        eprintln!("  {} {} (failed: {})", "✗".red(), output, e);
                    }
                }
            }
        }
    }

    eprintln!(
        "\n{} {} file(s) touched, {} wildcard patterns skipped",
        "Done:".bold(),
        touched,
        skipped
    );
    Ok(())
}

pub async fn watch_command(workflow: PathBuf, auto_run: bool, jobs: usize) -> Result<()> {
    print_banner();

    let workflow_path =
        std::path::absolute(&workflow).context("failed to resolve workflow path")?;

    if !workflow_path.exists() {
        eprintln!(
            "{} Workflow file not found: {}",
            "error:".bold().red(),
            workflow_path.display()
        );
        std::process::exit(1);
    }

    eprintln!(
        "{} {} for changes...",
        "Watching".bold().cyan(),
        workflow_path.display()
    );
    eprintln!("  Press Ctrl+C to stop.");

    let mut last_mtime = std::fs::metadata(&workflow_path)
        .and_then(|m| m.modified())
        .ok();

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let current_mtime = std::fs::metadata(&workflow_path)
            .and_then(|m| m.modified())
            .ok();

        let changed = match (last_mtime, current_mtime) {
            (Some(last), Some(current)) => current != last,
            _ => false,
        };

        if changed {
            eprintln!(
                "\n{} Change detected, re-validating...",
                "Change detected:".bold().green()
            );

            // Run validate + optional dry-run/run for quick feedback
            match validate_command(workflow_path.clone(), false) {
                Ok(()) => {
                    if auto_run {
                        eprintln!();
                        let _ = crate::commands::run::run_command(
                            Some(workflow_path.clone()),
                            jobs,
                            false,           // keep_going
                            None,            // workdir
                            vec![],          // target
                            0,               // retry
                            "0".to_string(), // timeout
                            false,           // resume_failed
                            None,            // profile
                            0,               // max_threads
                            0,               // max_memory
                            false,           // skip_env_setup
                            None,            // cache_dir
                            false,           // provenance
                        )
                        .await;
                    } else {
                        // Dry-run to show execution plan
                        eprintln!();
                        let _ = crate::commands::run::dry_run_command(
                            Some(workflow_path.clone()),
                            vec![],
                            false,
                        )
                        .await;
                    }
                    eprintln!();
                }
                Err(e) => {
                    eprintln!("  validation error: {}\n", e);
                }
            }

            // Run lint
            match lint_command(workflow_path.clone(), false) {
                Ok(()) => {
                    eprintln!();
                }
                Err(e) => {
                    eprintln!("  lint error: {}\n", e);
                }
            }

            eprintln!(
                "{} {} for changes...",
                "Watching".bold().cyan(),
                workflow_path.display()
            );
            last_mtime = current_mtime;
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_as_include_skips_dag_validation() {
        // Create a fragment with rules that reference undefined inputs
        let fragment = r#"
[workflow]
name = "qc-fragment"

[[rules]]
name = "fastqc"
input = ["{sample}.fastq"]
output = ["{sample}_fastqc.html"]
shell = "fastqc {input}"
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should pass with --as-include (skips DAG validation)
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("validate")
            .arg("--as-include")
            .arg(file.path())
            .assert()
            .success();
    }

    #[test]
    fn test_as_include_validates_syntax() {
        // Create an invalid fragment (missing required 'name' field)
        let fragment = r#"
[workflow]
name = "bad-fragment"

[[rules]]
# Missing required 'name' field
input = ["test.txt"]
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should fail even with --as-include (syntax errors)
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("validate")
            .arg("--as-include")
            .arg(file.path())
            .assert()
            .failure();
    }
}
