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
    use oxo_flow_core::{
        executor::CheckpointState,
        report::{DashboardMetrics, ReportBuilder, ResourceUsage},
    };
    use std::collections::HashMap;

    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    let checkpoint_path = workflow_dir.join(".oxo-flow").join("checkpoint.json");

    // Load checkpoint for execution data (if available)
    let checkpoint = CheckpointState::load_from_file(&checkpoint_path).ok();

    // Build dashboard metrics from checkpoint
    let completed = checkpoint
        .as_ref()
        .map(|c| c.completed_rules.len())
        .unwrap_or(0);
    let failed = checkpoint
        .as_ref()
        .map(|c| c.failed_rules.len())
        .unwrap_or(0);
    let total_rules = config.rules.len();

    let dashboard = DashboardMetrics {
        pipeline_name: config.workflow.name.clone(),
        total_samples: config
            .sample_groups
            .iter()
            .map(|g| g.samples.len())
            .sum::<usize>()
            + config.pairs.len(),
        total_rules,
        succeeded: completed,
        failed,
        total_reads_processed: None,
        mean_mapping_rate: None,
        variants_detected: None,
        actionable_variants: None,
        differentially_expressed_genes: None,
        total_runtime_secs: checkpoint.as_ref().and_then(|c| {
            c.benchmarks
                .values()
                .map(|b| Some(b.wall_time_secs))
                .sum::<Option<f64>>()
        }),
    };

    let mut report = ReportBuilder::new(
        &format!("{} Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );

    // Dashboard (always)
    report = report.dashboard(&dashboard);

    // Workflow info section
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
    report = report.section(oxo_flow_core::report::ReportSection {
        title: "Workflow Information".to_string(),
        id: "workflow-info".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue { pairs: info_pairs },
        subsections: vec![],
    });

    // Rule summary table
    let headers = vec!["Rule", "Description", "Environment", "Threads", "Memory"]
        .into_iter()
        .map(String::from)
        .collect();
    let rows: Vec<Vec<String>> = config
        .rules
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.description.clone().unwrap_or_default(),
                r.environment.kind().to_string(),
                r.resources.threads.to_string(),
                r.resources.memory.clone().unwrap_or("-".into()),
            ]
        })
        .collect();
    report = report.section(oxo_flow_core::report::ReportSection {
        title: "Pipeline Steps".to_string(),
        id: "pipeline-steps".to_string(),
        content: oxo_flow_core::report::ReportContent::Table { headers, rows },
        subsections: vec![],
    });

    // Execution summary from checkpoint
    if let Some(ref cp) = checkpoint {
        // Completed/failed rules
        let mut status_rows = Vec::new();
        for rule in &cp.completed_rules {
            let bench = cp.benchmarks.get(rule);
            status_rows.push(vec![
                rule.clone(),
                "✓ Completed".into(),
                bench.map_or("-".into(), |b| format!("{:.1}s", b.wall_time_secs)),
            ]);
        }
        for rule in &cp.failed_rules {
            status_rows.push(vec![rule.clone(), "✗ Failed".into(), "-".into()]);
        }
        if !status_rows.is_empty() {
            report = report.section(oxo_flow_core::report::ReportSection {
                title: "Execution Results".to_string(),
                id: "execution-results".to_string(),
                content: oxo_flow_core::report::ReportContent::Table {
                    headers: vec!["Rule".into(), "Status".into(), "Duration".into()],
                    rows: status_rows,
                },
                subsections: vec![],
            });
        }

        // Execution time chart
        let mut records: HashMap<String, oxo_flow_core::executor::JobRecord> = HashMap::new();
        for name in cp.benchmarks.keys() {
            records.insert(
                name.clone(),
                oxo_flow_core::executor::JobRecord {
                    rule: name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Success,
                    started_at: None,
                    finished_at: None,
                    exit_code: Some(0),
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    timeout: None,
                    skip_reason: None,
                },
            );
        }
        if !records.is_empty() {
            report = report.execution_chart(&records);
        }

        // Resource usage
        let usage: Vec<ResourceUsage> = cp
            .benchmarks
            .iter()
            .map(|(name, b)| ResourceUsage {
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

    // Environment information
    let env_pairs = vec![
        ("System".to_string(), "conda".to_string()),
        (
            "Available Backends".to_string(),
            "conda, pixi, docker, singularity, venv, system".to_string(),
        ),
    ];
    report = report.section(oxo_flow_core::report::ReportSection {
        title: "Environment".to_string(),
        id: "environment".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue { pairs: env_pairs },
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
            eprintln!("  {} [{}] {}", "•".cyan(), diff.category, diff.description);
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
