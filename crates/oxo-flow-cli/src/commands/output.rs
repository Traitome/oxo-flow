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

pub fn handle_report(workflow: PathBuf, format: String, output: Option<PathBuf>) -> Result<()> {
    use oxo_flow_core::{executor::CheckpointState, report::ReportBuilder};

    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    let checkpoint_path = workflow_dir.join(".oxo-flow").join("checkpoint.json");

    let checkpoint = CheckpointState::load_from_file(&checkpoint_path).ok();

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

    // ── Build report (works for ANY workflow type: bioinformatics, shell, ETL, etc.) ──

    let mut report = ReportBuilder::new(
        &format!("{} Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );

    // 1. Generic dashboard (works for any task type)
    report = report.generic_dashboard(total_rules, completed, failed, total_runtime);

    // 2. Workflow metadata
    let mut info_pairs = vec![
        ("Name".to_string(), config.workflow.name.clone()),
        ("Version".to_string(), config.workflow.version.clone()),
        ("Total Rules".to_string(), config.rules.len().to_string()),
    ];
    if let Some(ref desc) = config.workflow.description {
        info_pairs.push(("Description".to_string(), desc.clone()));
    }
    if let Some(ref author) = config.workflow.author {
        info_pairs.push(("Author".to_string(), author.clone()));
    }
    if let Some(ref genome) = config.workflow.genome_build {
        info_pairs.push(("Genome Build".to_string(), genome.clone()));
    }
    if !config.config.is_empty() {
        info_pairs.push((
            "Config Variables".to_string(),
            config.config.keys().cloned().collect::<Vec<_>>().join(", "),
        ));
    }
    // Sample/pair info
    let sample_count: usize = config.sample_groups.iter().map(|g| g.samples.len()).sum();
    let pair_count = config.pairs.len();
    if sample_count > 0 {
        info_pairs.push(("Samples".to_string(), sample_count.to_string()));
    }
    if pair_count > 0 {
        info_pairs.push(("Pairs".to_string(), pair_count.to_string()));
    }
    report = report.section(oxo_flow_core::report::ReportSection {
        title: "Workflow Information".to_string(),
        id: "workflow-info".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue { pairs: info_pairs },
        subsections: vec![],
    });

    // 3. Task summary (works for shell scripts, bioinfo tools, ETL, anything)
    report = report.task_summary(&config.rules);

    // 4. Command manifest (shows exactly what each task executes)
    report = report.command_manifest(&config.rules);

    // 5. I/O file manifest
    report = report.io_manifest(&config.rules);

    // 6. Execution status (if checkpoint exists)
    if let Some(ref cp) = checkpoint {
        report = report.execution_status(&cp.completed_rules, &cp.failed_rules, &cp.benchmarks);

        // Resource usage
        let usage: Vec<oxo_flow_core::report::ResourceUsage> = cp
            .benchmarks
            .iter()
            .map(|(name, b)| oxo_flow_core::report::ResourceUsage {
                rule: name.clone(),
                wall_time_secs: b.wall_time_secs,
                max_memory_mb: b.max_memory_mb,
                cpu_seconds: b.cpu_seconds,
                threads: 0,
                status: if cp.completed_rules.contains(name) {
                    "success".into()
                } else {
                    "failed".into()
                },
            })
            .collect();
        if !usage.is_empty() {
            report = report.resource_usage(&usage);
        }
    }

    // 7. Environment info
    report = report.section(oxo_flow_core::report::ReportSection {
        title: "Environment".to_string(),
        id: "environment".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue {
            pairs: vec![
                (
                    "Available Backends".to_string(),
                    "conda, pixi, docker, singularity, venv, system".to_string(),
                ),
                (
                    "oxo-flow Version".to_string(),
                    env!("CARGO_PKG_VERSION").to_string(),
                ),
            ],
        },
        subsections: vec![],
    });

    let report = report.build();

    let content = match format.as_str() {
        "html" | "htm" => report.to_html(),
        "json" => report.to_json().map_err(|e| anyhow::anyhow!(e))?,
        other => anyhow::bail!(
            "unsupported report format: '{}'. Supported formats: html, json",
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
        "singularity" => {
            let pkg = oxo_flow_core::container::PackageConfig {
                format: oxo_flow_core::container::ContainerFormat::Singularity,
                ..Default::default()
            };
            oxo_flow_core::container::generate_singularity_def(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "toml" => oxo_flow_core::format::format_workflow(&config),
        _ => {
            let pkg = oxo_flow_core::container::PackageConfig::default();
            oxo_flow_core::container::generate_dockerfile(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
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
