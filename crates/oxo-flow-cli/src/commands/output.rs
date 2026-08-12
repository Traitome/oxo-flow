//! Logic for output-related subcommands: graph, report, diff, export.

use crate::commands::{print_banner, resolve_workflow};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::path::PathBuf;

pub fn handle_graph(workflow: PathBuf, format: String, output: Option<PathBuf>) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let dag = WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

    let result = match format.as_str() {
        "ascii" => dag.to_ascii().map_err(|e| anyhow::anyhow!(e)),
        "dot" => Ok(dag.to_dot()),
        "dot-clustered" => dag.to_dot_clustered().map_err(|e| anyhow::anyhow!(e)),
        "tree" => dag.to_ascii_tree().map_err(|e| anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("unsupported graph format: {}", format)),
    }?;

    if let Some(path) = output {
        std::fs::write(&path, result)?;
        eprintln!("{} Graph saved to {}", "✓".green(), path.display());
    } else {
        println!("{}", result);
    }

    Ok(())
}

pub fn handle_report(
    workflow: PathBuf,
    format: String,
    output: Option<PathBuf>,
    checkpoint_path: Option<PathBuf>,
) -> Result<()> {
    use oxo_flow_core::{executor::CheckpointState, report::ReportBuilder};

    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // Determine checkpoint path: explicit > workflow-relative > warn
    let checkpoint_path = checkpoint_path.unwrap_or_else(|| {
        let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
        workflow_dir.join(".oxo-flow").join("checkpoint.json")
    });

    let checkpoint = match CheckpointState::load_from_file(&checkpoint_path) {
        Ok(cp) => Some(cp),
        Err(_) => {
            eprintln!(
                "  {} No checkpoint found at {}",
                "Note:".yellow(),
                checkpoint_path.display()
            );
            eprintln!(
                "  {} Report will show template-level data only. Run the workflow first for execution metrics.",
                "Info:".dimmed()
            );
            None
        }
    };

    let completed = checkpoint
        .as_ref()
        .map(|c| c.completed_rules.len())
        .unwrap_or(0);
    let failed = checkpoint
        .as_ref()
        .map(|c| c.failed_rules.len())
        .unwrap_or(0);
    let total_rules = config.rules.len();
    let total_runtime = checkpoint.as_ref().and_then(|c| {
        c.benchmarks
            .values()
            .map(|b| Some(b.wall_time_secs))
            .sum::<Option<f64>>()
    });

    // ── Build report using the pluggable section system ──
    // Domain auto-detection tailors sections to the workflow type.
    // Users can override via [report].sections in their .oxoflow file.

    let domain = oxo_flow_core::report::WorkflowDomain::detect(&config.rules);
    let ctx = oxo_flow_core::report::ReportContext {
        config: &config,
        checkpoint: checkpoint.as_ref(),
        domain,
    };

    // Resolve section filter from [report].sections config (if present).
    let section_filter: Option<std::collections::HashSet<String>> =
        config.report.as_ref().and_then(|r| {
            let sections = &r.sections;
            if sections.is_empty() {
                None
            } else {
                Some(sections.iter().cloned().collect())
            }
        });

    let registry = oxo_flow_core::report::SectionRegistry::with_defaults();
    let sections = registry.generate(&ctx, section_filter.as_ref());

    let mut report = ReportBuilder::new(
        &format!("{} Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );
    for section in sections {
        report = report.section(section);
    }

    // Always add a Task Summary table for quick reference
    report = report.task_summary(&config.rules);

    let report = report.build();

    let content = match format.as_str() {
        "html" | "htm" => report.to_html(),
        "json" => report.to_json().map_err(|e| anyhow::anyhow!(e))?,
        "pdf" => {
            let pdf_output = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}_report.pdf", config.workflow.name)));
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async { report.to_pdf(&pdf_output).await })?;
            eprintln!(
                "{} PDF report written to {}",
                "✓".green(),
                pdf_output.display()
            );
            return Ok(());
        }
        "pdf-command" => {
            let pdf_output = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}_report.pdf", config.workflow.name)));
            println!(
                "{}",
                report.to_pdf_command(&pdf_output.to_string_lossy(), vec![])
            );
            return Ok(());
        }
        other => anyhow::bail!(
            "unsupported report format: '{}'. Supported formats: html, json, pdf, pdf-command",
            other
        ),
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &content)?;
            eprintln!("Report written to {}", path.display());
        }
        None => {
            println!("{content}");
        }
    }

    Ok(())
}

pub fn handle_diff(workflow_a: PathBuf, workflow_b: PathBuf) -> Result<()> {
    print_banner();
    let config_a = WorkflowConfig::from_file(&workflow_a)
        .with_context(|| format!("failed to parse {}", workflow_a.display()))?;
    let config_b = WorkflowConfig::from_file(&workflow_b)
        .with_context(|| format!("failed to parse {}", workflow_b.display()))?;

    let diffs = oxo_flow_core::format::diff_workflows(&config_a, &config_b);

    if diffs.is_empty() {
        eprintln!("{} Workflows are identical", "✓".green().bold());
    } else {
        eprintln!(
            "{} {} difference(s) between {} and {}:",
            "Diff:".bold().yellow(),
            diffs.len(),
            workflow_a.display(),
            workflow_b.display()
        );
        for diff in &diffs {
            let cat_color = match diff.category.as_str() {
                "added" | "rule added" => "✓".green(),
                "removed" | "rule removed" => "✗".red(),
                "changed" => "~".yellow(),
                _ => "•".cyan(),
            };
            eprintln!(
                "  {} [{}] {}",
                cat_color,
                diff.category.cyan(),
                diff.description
            );
        }
    }
    Ok(())
}

pub fn handle_export(workflow: PathBuf, format: String, output: Option<PathBuf>) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let content = match format.as_str() {
        "docker" => {
            let pkg = oxo_flow_core::container::PackageConfig::default();
            oxo_flow_core::container::generate_dockerfile(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "singularity" => {
            let pkg = oxo_flow_core::container::PackageConfig {
                format: oxo_flow_core::container::ContainerFormat::Singularity,
                ..Default::default()
            };
            oxo_flow_core::container::generate_singularity_def(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "toml" => oxo_flow_core::format::format_workflow(&config),
        other => anyhow::bail!(
            "unsupported export format '{}'. Supported formats: docker, singularity, toml",
            other
        ),
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &content)?;
            eprintln!(
                "{} Exported {} to {}",
                "✓".green().bold(),
                format,
                path.display()
            );
        }
        None => {
            println!("{content}");
        }
    }

    Ok(())
}
