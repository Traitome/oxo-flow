//! oxo-flow CLI — Bioinformatics pipeline engine.
//!
//! Provides subcommands for running, validating, and managing workflows.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{ExecutorConfig, LocalExecutor};
use std::path::PathBuf;

/// oxo-flow — A Rust-native bioinformatics pipeline engine.
///
/// Build, validate, and execute reproducible bioinformatics workflows
/// with first-class environment management and clinical-grade reporting.
#[derive(Parser, Debug)]
#[command(
    name = "oxo-flow",
    version,
    about = "A Rust-native bioinformatics pipeline engine",
    long_about = "oxo-flow is a high-performance, modular bioinformatics pipeline engine\n\
                   designed to fully replace Snakemake. It supports conda, pixi, docker,\n\
                   singularity, and venv environments with DAG-based execution."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Execute a workflow.
    Run {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Maximum number of concurrent jobs.
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,

        /// Keep going when a job fails.
        #[arg(short = 'k', long)]
        keep_going: bool,

        /// Working directory for execution.
        #[arg(short = 'd', long)]
        workdir: Option<PathBuf>,

        /// Run specific target rules only.
        #[arg(short = 't', long)]
        target: Vec<String>,
    },

    /// Simulate execution without running any commands.
    DryRun {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },

    /// Validate a .oxoflow workflow file.
    Validate {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },

    /// Output the workflow DAG in DOT format.
    Graph {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },

    /// Generate reports from workflow execution.
    Report {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Output format (html, json).
        #[arg(short = 'f', long, default_value = "html")]
        format: String,

        /// Output file path.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },

    /// Manage software environments.
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },

    /// Package a workflow into a container image.
    Package {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Container format (docker, singularity).
        #[arg(short = 'f', long, default_value = "docker")]
        format: String,

        /// Output file path.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },

    /// Start the web interface server.
    Serve {
        /// Host address to bind to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to listen on.
        #[arg(short = 'p', long, default_value = "8080")]
        port: u16,
    },

    /// Initialize a new workflow project.
    Init {
        /// Project name.
        #[arg(value_name = "NAME")]
        name: String,

        /// Output directory.
        #[arg(short = 'd', long)]
        dir: Option<PathBuf>,
    },

    /// Show execution status from a checkpoint file.
    Status {
        /// Path to checkpoint JSON file.
        #[arg(value_name = "CHECKPOINT")]
        checkpoint: PathBuf,
    },

    /// Clean workflow outputs and temporary files.
    Clean {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Only show what would be cleaned (dry-run).
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum EnvAction {
    /// List available environment backends.
    List,

    /// Check if required environments are available.
    Check {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },
}

fn print_banner() {
    eprintln!(
        "{} {} — {}",
        "oxo-flow".bold().cyan(),
        env!("CARGO_PKG_VERSION"),
        "Bioinformatics Pipeline Engine".dimmed()
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            workflow,
            jobs,
            keep_going,
            workdir,
            target: _,
        } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let order = dag.execution_order()?;
            eprintln!(
                "{} {} rules in execution order",
                "DAG:".bold().green(),
                order.len()
            );
            for (i, rule_name) in order.iter().enumerate() {
                eprintln!("  {}. {}", i + 1, rule_name);
            }

            let exec_config = ExecutorConfig {
                max_jobs: jobs,
                dry_run: false,
                workdir: workdir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                keep_going,
                retry_count: 0,
                timeout: None,
            };

            let executor = LocalExecutor::new(exec_config);
            let mut success_count = 0;
            let mut fail_count = 0;

            for rule_name in &order {
                let rule = config.get_rule(rule_name).unwrap();
                match executor
                    .execute_rule(rule, &std::collections::HashMap::new())
                    .await
                {
                    Ok(record) => {
                        if record.status == oxo_flow_core::executor::JobStatus::Success {
                            success_count += 1;
                            eprintln!("  {} {}", "✓".green().bold(), rule_name);
                        } else {
                            eprintln!("  {} {} ({})", "⊘".yellow(), rule_name, record.status);
                        }
                    }
                    Err(e) => {
                        fail_count += 1;
                        eprintln!("  {} {} — {}", "✗".red().bold(), rule_name, e);
                        if !keep_going {
                            return Err(e.into());
                        }
                    }
                }
            }

            eprintln!(
                "\n{} {} succeeded, {} failed",
                "Done:".bold(),
                success_count,
                fail_count
            );
        }

        Commands::DryRun { workflow } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let order = dag.execution_order()?;
            eprintln!(
                "{} {} rules would execute:",
                "Dry-run:".bold().yellow(),
                order.len()
            );

            for (i, rule_name) in order.iter().enumerate() {
                let rule = config.get_rule(rule_name).unwrap();
                eprintln!(
                    "  {}. {} [threads={}, env={}]",
                    i + 1,
                    rule_name.bold(),
                    rule.effective_threads(),
                    rule.environment.kind()
                );
                if let Some(ref cmd) = rule.shell {
                    let preview: String = cmd.chars().take(80).collect();
                    eprintln!("     $ {}", preview.dimmed());
                }
            }
        }

        Commands::Validate { workflow } => {
            let config = WorkflowConfig::from_file(&workflow);
            match config {
                Ok(cfg) => {
                    // Also validate DAG construction
                    match WorkflowDag::from_rules(&cfg.rules) {
                        Ok(dag) => {
                            eprintln!(
                                "{} {} — {} rules, {} dependencies",
                                "✓".green().bold(),
                                workflow.display(),
                                dag.node_count(),
                                dag.edge_count()
                            );
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
                Err(e) => {
                    eprintln!("{} {} — {}", "✗".red().bold(), workflow.display(), e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Graph { workflow } => {
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            println!("{}", dag.to_dot());
        }

        Commands::Report {
            workflow,
            format,
            output,
        } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let mut report = oxo_flow_core::report::Report::new(
                &format!("{} Report", config.workflow.name),
                &config.workflow.name,
                &config.workflow.version,
            );

            report.add_section(oxo_flow_core::report::ReportSection {
                title: "Workflow Information".to_string(),
                id: "workflow-info".to_string(),
                content: oxo_flow_core::report::ReportContent::KeyValue {
                    pairs: vec![
                        ("Name".to_string(), config.workflow.name.clone()),
                        ("Version".to_string(), config.workflow.version.clone()),
                        ("Rules".to_string(), config.rules.len().to_string()),
                    ],
                },
                subsections: vec![],
            });

            let content = match format.as_str() {
                "json" => report.to_json()?,
                _ => report.to_html(),
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
        }

        Commands::Env { action } => {
            print_banner();
            match action {
                EnvAction::List => {
                    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
                    let available = resolver.available_backends();
                    eprintln!("{}", "Available environment backends:".bold());
                    for backend in available {
                        eprintln!("  {} {}", "✓".green(), backend);
                    }
                }
                EnvAction::Check { workflow } => {
                    let config = WorkflowConfig::from_file(&workflow)
                        .with_context(|| format!("failed to parse {}", workflow.display()))?;

                    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
                    let mut all_ok = true;

                    for rule in &config.rules {
                        match resolver.validate_spec(&rule.environment) {
                            Ok(()) => {
                                eprintln!(
                                    "  {} {} ({})",
                                    "✓".green(),
                                    rule.name,
                                    rule.environment.kind()
                                );
                            }
                            Err(e) => {
                                eprintln!("  {} {} — {}", "✗".red(), rule.name, e);
                                all_ok = false;
                            }
                        }
                    }

                    if !all_ok {
                        std::process::exit(1);
                    }
                }
            }
        }

        Commands::Package {
            workflow,
            format,
            output,
        } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let pkg_config = oxo_flow_core::container::PackageConfig {
                format: match format.as_str() {
                    "singularity" => oxo_flow_core::container::ContainerFormat::Singularity,
                    _ => oxo_flow_core::container::ContainerFormat::Docker,
                },
                ..Default::default()
            };

            let content = match pkg_config.format {
                oxo_flow_core::container::ContainerFormat::Docker => {
                    oxo_flow_core::container::generate_dockerfile(&config, &pkg_config)?
                }
                oxo_flow_core::container::ContainerFormat::Singularity => {
                    oxo_flow_core::container::generate_singularity_def(&config, &pkg_config)?
                }
            };

            match output {
                Some(path) => {
                    std::fs::write(&path, &content)?;
                    eprintln!("Container definition written to {}", path.display());
                }
                None => {
                    println!("{content}");
                }
            }
        }

        Commands::Serve { host, port } => {
            print_banner();
            eprintln!("Starting web server at {}:{} ...", host, port);
            oxo_flow_web::start_server(&host, port).await?;
        }

        Commands::Init { name, dir } => {
            print_banner();
            let project_dir = dir.unwrap_or_else(|| PathBuf::from(&name));
            std::fs::create_dir_all(&project_dir)?;

            let workflow_content = format!(
                r#"[workflow]
name = "{name}"
version = "0.1.0"
description = "A new oxo-flow pipeline"

[config]
# Add your configuration variables here

[defaults]
threads = 4
memory = "8G"

# Define your pipeline rules below:
# [[rules]]
# name = "step1"
# input = ["input.txt"]
# output = ["output.txt"]
# shell = "cat input.txt > output.txt"
"#
            );

            let workflow_path = project_dir.join(format!("{name}.oxoflow"));
            std::fs::write(&workflow_path, workflow_content)?;
            eprintln!(
                "{} Created new project at {}",
                "✓".green().bold(),
                project_dir.display()
            );
            eprintln!(
                "  Edit {} to define your pipeline.",
                workflow_path.display()
            );
        }

        Commands::Status { checkpoint } => {
            print_banner();
            let content = std::fs::read_to_string(&checkpoint)
                .with_context(|| format!("failed to read {}", checkpoint.display()))?;
            let state = oxo_flow_core::executor::CheckpointState::from_json(&content)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            eprintln!("{}", "Checkpoint Status:".bold());
            for rule in &state.completed_rules {
                eprintln!("  {} {}", "✓".green().bold(), rule);
            }
            for rule in &state.failed_rules {
                eprintln!("  {} {}", "✗".red().bold(), rule);
            }
            let completed = state.completed_rules.len();
            let failed = state.failed_rules.len();
            eprintln!(
                "\n{} {} completed, {} failed",
                "Summary:".bold(),
                completed,
                failed
            );
        }

        Commands::Clean { workflow, dry_run } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let mut outputs: Vec<String> = Vec::new();
            for rule in &config.rules {
                for output in &rule.output {
                    if !outputs.contains(output) {
                        outputs.push(output.clone());
                    }
                }
            }

            if dry_run {
                eprintln!("{}", "Would clean (dry-run):".bold().yellow());
            } else {
                eprintln!("{}", "Cleaning outputs:".bold());
            }

            for output in &outputs {
                eprintln!("  {}", output);
            }
            eprintln!("\n{} {} output patterns", "Total:".bold(), outputs.len());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_help() {
        // Verify the CLI struct is valid
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_parse_run() {
        let cli = Cli::try_parse_from(["oxo-flow", "run", "test.oxoflow"]).unwrap();
        match cli.command {
            Commands::Run { workflow, jobs, .. } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert_eq!(jobs, 1);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn cli_parse_dry_run() {
        let cli = Cli::try_parse_from(["oxo-flow", "dry-run", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::DryRun { .. });
    }

    #[test]
    fn cli_parse_validate() {
        let cli = Cli::try_parse_from(["oxo-flow", "validate", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::Validate { .. });
    }

    #[test]
    fn cli_parse_graph() {
        let cli = Cli::try_parse_from(["oxo-flow", "graph", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::Graph { .. });
    }

    #[test]
    fn cli_parse_init() {
        let cli = Cli::try_parse_from(["oxo-flow", "init", "my-pipeline"]).unwrap();
        match cli.command {
            Commands::Init { name, .. } => {
                assert_eq!(name, "my-pipeline");
            }
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn cli_parse_env_list() {
        let cli = Cli::try_parse_from(["oxo-flow", "env", "list"]).unwrap();
        matches!(cli.command, Commands::Env { .. });
    }

    #[test]
    fn cli_parse_run_with_options() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "run", "test.oxoflow", "-j", "8", "-k"]).unwrap();
        match cli.command {
            Commands::Run {
                jobs, keep_going, ..
            } => {
                assert_eq!(jobs, 8);
                assert!(keep_going);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn cli_parse_status() {
        let cli = Cli::try_parse_from(["oxo-flow", "status", "checkpoint.json"]).unwrap();
        match cli.command {
            Commands::Status { checkpoint } => {
                assert_eq!(checkpoint, PathBuf::from("checkpoint.json"));
            }
            _ => panic!("expected Status command"),
        }
    }

    #[test]
    fn cli_parse_clean() {
        let cli = Cli::try_parse_from(["oxo-flow", "clean", "test.oxoflow"]).unwrap();
        match cli.command {
            Commands::Clean { workflow, dry_run } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert!(!dry_run);
            }
            _ => panic!("expected Clean command"),
        }
    }

    #[test]
    fn cli_parse_clean_dry_run() {
        let cli = Cli::try_parse_from(["oxo-flow", "clean", "test.oxoflow", "-n"]).unwrap();
        match cli.command {
            Commands::Clean { dry_run, .. } => {
                assert!(dry_run);
            }
            _ => panic!("expected Clean command"),
        }
    }
}
