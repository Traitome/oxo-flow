//! oxo-flow CLI — Bioinformatics pipeline engine.
//!
//! Provides subcommands for running, validating, and managing workflows.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{ExecutorConfig, LocalExecutor};
use std::path::{Path, PathBuf};

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
                   built from first principles in Rust. It supports conda, pixi, docker,\n\
                   singularity, and venv environments with DAG-based execution."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose (debug-level) logging.
    #[arg(global = true, short = 'v', long)]
    verbose: bool,
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

        /// Number of times to retry failed jobs.
        #[arg(short = 'r', long, default_value = "0")]
        retry: u32,

        /// Timeout per job in seconds (0 = no timeout).
        #[arg(long, default_value = "0")]
        timeout: u64,
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

        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,
    },

    /// Inspect and manage workflow configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate shell completions for oxo-flow.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Reformat a .oxoflow file into canonical TOML form.
    Format {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Write formatted output to a file instead of stdout.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,

        /// Check if the file is already formatted (exit non-zero if not).
        #[arg(long)]
        check: bool,
    },

    /// Run best-practice linting checks on a .oxoflow file.
    Lint {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Treat warnings as errors (non-zero exit).
        #[arg(long)]
        strict: bool,
    },

    /// Manage execution profiles (local, SLURM, PBS, SGE, LSF).
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
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

#[derive(Subcommand, Debug)]
enum ProfileAction {
    /// List available execution profiles.
    List,

    /// Show details of a specific profile.
    Show {
        /// Profile name (local, slurm, pbs, sge, lsf).
        #[arg(value_name = "NAME")]
        name: String,
    },

    /// Show the current active profile.
    Current,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Show all configuration variables from a workflow.
    Show {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },

    /// Show workflow statistics.
    Stats {
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
    let cli = Cli::parse();

    // Initialize tracing with level based on --verbose flag
    let default_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .with_target(false)
        .init();

    match cli.command {
        Commands::Run {
            workflow,
            jobs,
            keep_going,
            workdir,
            target: _,
            retry,
            timeout,
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
                retry_count: retry,
                timeout: if timeout > 0 {
                    Some(std::time::Duration::from_secs(timeout))
                } else {
                    None
                },
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

            // Create additional directories
            let envs_dir = project_dir.join("envs");
            let scripts_dir = project_dir.join("scripts");
            std::fs::create_dir_all(&envs_dir)?;
            std::fs::create_dir_all(&scripts_dir)?;

            // Create a .gitignore with common bioinformatics patterns
            let gitignore_content = "\
# Alignment files
*.bam
*.bam.bai
*.cram
*.cram.crai
*.sam

# Variant files
*.vcf.gz
*.vcf.gz.tbi
*.bcf

# Index files
*.fai
*.dict

# Workflow outputs
logs/
results/
benchmarks/

# oxo-flow internals
.oxo-flow/
.oxo-flow-cache/

# OS files
.DS_Store
Thumbs.db
";
            let gitignore_path = project_dir.join(".gitignore");
            std::fs::write(&gitignore_path, gitignore_content)?;

            eprintln!(
                "{} Created new project at {}",
                "✓".green().bold(),
                project_dir.display()
            );
            eprintln!("  {}", workflow_path.display());
            eprintln!("  {}/", envs_dir.display());
            eprintln!("  {}/", scripts_dir.display());
            eprintln!("  {}", gitignore_path.display());
            eprintln!(
                "\n  Edit {} to define your pipeline.",
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

        Commands::Clean {
            workflow,
            dry_run,
            force,
        } => {
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
                for output in &outputs {
                    let has_wildcard = output.contains('{') && output.contains('}');
                    if has_wildcard {
                        eprintln!("  {} (wildcard, skipped)", output.dimmed());
                    } else if Path::new(output).exists() {
                        eprintln!("  {} (exists)", output);
                    } else {
                        eprintln!("  {} (not found)", output.dimmed());
                    }
                }
                eprintln!("\n{} {} output patterns", "Total:".bold(), outputs.len());
            } else {
                // Determine which files are deletable
                let mut deletable: Vec<&String> = Vec::new();
                let mut skipped_wildcard = 0usize;
                let mut not_found = 0usize;

                for output in &outputs {
                    let has_wildcard = output.contains('{') && output.contains('}');
                    if has_wildcard {
                        skipped_wildcard += 1;
                    } else if Path::new(output).exists() {
                        deletable.push(output);
                    } else {
                        not_found += 1;
                    }
                }

                if deletable.is_empty() {
                    eprintln!(
                        "{} Nothing to delete ({} not found, {} wildcard patterns skipped)",
                        "Clean:".bold(),
                        not_found,
                        skipped_wildcard
                    );
                } else {
                    // Prompt for confirmation unless --force is given
                    if !force {
                        eprintln!(
                            "{} {} file(s) will be deleted. Continue? [y/N]",
                            "Clean:".bold().yellow(),
                            deletable.len()
                        );
                        let mut answer = String::new();
                        std::io::stdin().read_line(&mut answer)?;
                        if answer.trim().to_lowercase() != "y" {
                            eprintln!("Aborted.");
                            return Ok(());
                        }
                    }

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
                        "\n{} {} deleted, {} failed, {} not found, {} wildcard skipped",
                        "Done:".bold(),
                        deleted,
                        failed,
                        not_found,
                        skipped_wildcard
                    );
                }
            }
        }

        Commands::Config { action } => {
            print_banner();
            match action {
                ConfigAction::Show { workflow } => {
                    let config = WorkflowConfig::from_file(&workflow)
                        .with_context(|| format!("failed to parse {}", workflow.display()))?;

                    eprintln!("{}", "Workflow Configuration:".bold());
                    eprintln!("  Name:    {}", config.workflow.name);
                    eprintln!("  Version: {}", config.workflow.version);
                    if let Some(ref desc) = config.workflow.description {
                        eprintln!("  Desc:    {}", desc);
                    }
                    if let Some(ref author) = config.workflow.author {
                        eprintln!("  Author:  {}", author);
                    }

                    if !config.config.is_empty() {
                        eprintln!("\n{}", "  Config Variables:".bold());
                        let mut keys: Vec<&String> = config.config.keys().collect();
                        keys.sort();
                        for key in keys {
                            eprintln!("    {} = {}", key, config.config[key]);
                        }
                    }

                    if !config.includes.is_empty() {
                        eprintln!("\n{}", "  Includes:".bold());
                        for inc in &config.includes {
                            if let Some(ref ns) = inc.namespace {
                                eprintln!("    {} (namespace: {})", inc.path, ns);
                            } else {
                                eprintln!("    {}", inc.path);
                            }
                        }
                    }

                    if !config.execution_groups.is_empty() {
                        eprintln!("\n{}", "  Execution Groups:".bold());
                        for group in &config.execution_groups {
                            eprintln!("    {} ({:?}): {:?}", group.name, group.mode, group.rules);
                        }
                    }
                }
                ConfigAction::Stats { workflow } => {
                    let config = WorkflowConfig::from_file(&workflow)
                        .with_context(|| format!("failed to parse {}", workflow.display()))?;

                    let stats = oxo_flow_core::format::workflow_stats(&config);
                    eprintln!("{}", "Workflow Statistics:".bold());
                    eprintln!("  Rules:              {}", stats.rule_count);
                    eprintln!("  Shell rules:        {}", stats.shell_rules);
                    eprintln!("  Script rules:       {}", stats.script_rules);
                    eprintln!("  Dependencies:       {}", stats.dependency_count);
                    eprintln!("  Parallel groups:    {}", stats.parallel_groups);
                    eprintln!("  Max depth:          {}", stats.max_depth);
                    eprintln!("  Total threads:      {}", stats.total_threads);
                    eprintln!(
                        "  Wildcards:          {} ({:?})",
                        stats.wildcard_count, stats.wildcard_names
                    );
                    if !stats.environments.is_empty() {
                        eprintln!("  Environments:       {:?}", stats.environments);
                    }
                }
            }
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "oxo-flow", &mut std::io::stdout());
        }

        Commands::Format {
            workflow,
            output,
            check,
        } => {
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
        }

        Commands::Lint { workflow, strict } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let validation = oxo_flow_core::format::validate_format(&config);
            let lint_diags = oxo_flow_core::format::lint_format(&config);

            let mut error_count = 0usize;
            let mut warning_count = 0usize;
            let mut info_count = 0usize;

            for d in validation.diagnostics.iter().chain(lint_diags.iter()) {
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
        }

        Commands::Profile { action } => {
            print_banner();
            match action {
                ProfileAction::List => {
                    eprintln!("{}", "Available execution profiles:".bold());
                    let profiles = ["local", "slurm", "pbs", "sge", "lsf"];
                    for p in &profiles {
                        let desc = match *p {
                            "local" => "Local execution (default)",
                            "slurm" => "SLURM cluster scheduler",
                            "pbs" => "PBS/Torque cluster scheduler",
                            "sge" => "Sun Grid Engine (SGE) scheduler",
                            "lsf" => "IBM LSF scheduler",
                            _ => "Unknown",
                        };
                        eprintln!("  {} {} — {}", "•".cyan(), p.bold(), desc);
                    }
                }
                ProfileAction::Show { name } => match name.as_str() {
                    "local" => {
                        eprintln!("{}", "Profile: local".bold());
                        eprintln!("  Executor:    local process");
                        eprintln!("  Max jobs:    auto (CPU count)");
                        eprintln!("  Retries:     0");
                        eprintln!("  Timeout:     none");
                    }
                    "slurm" | "pbs" | "sge" | "lsf" => {
                        let backend = match name.as_str() {
                            "slurm" => oxo_flow_core::cluster::ClusterBackend::Slurm,
                            "pbs" => oxo_flow_core::cluster::ClusterBackend::Pbs,
                            "sge" => oxo_flow_core::cluster::ClusterBackend::Sge,
                            _ => oxo_flow_core::cluster::ClusterBackend::Lsf,
                        };
                        eprintln!("{}", format!("Profile: {}", name).bold());
                        eprintln!(
                            "  Submit cmd:  {}",
                            oxo_flow_core::cluster::submit_command(&backend)
                        );
                        eprintln!(
                            "  Status cmd:  {}",
                            oxo_flow_core::cluster::status_command(&backend)
                        );
                        eprintln!("  Executor:    cluster job submission");
                    }
                    other => {
                        eprintln!("{} Unknown profile: {}", "✗".red().bold(), other);
                        std::process::exit(1);
                    }
                },
                ProfileAction::Current => {
                    eprintln!("{} {}", "Active profile:".bold(), "local".green());
                }
            }
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
            Commands::Clean {
                workflow,
                dry_run,
                force,
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert!(!dry_run);
                assert!(!force);
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

    #[test]
    fn cli_parse_clean_force() {
        let cli = Cli::try_parse_from(["oxo-flow", "clean", "test.oxoflow", "--force"]).unwrap();
        match cli.command {
            Commands::Clean { force, .. } => {
                assert!(force);
            }
            _ => panic!("expected Clean command"),
        }
    }

    #[test]
    fn cli_parse_completions() {
        let cli = Cli::try_parse_from(["oxo-flow", "completions", "bash"]).unwrap();
        match cli.command {
            Commands::Completions { shell } => {
                assert_eq!(shell, clap_complete::Shell::Bash);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn cli_parse_verbose_flag() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "--verbose", "validate", "test.oxoflow"]).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn cli_parse_verbose_short_flag() {
        let cli = Cli::try_parse_from(["oxo-flow", "-v", "validate", "test.oxoflow"]).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn cli_parse_run_with_retry() {
        let cli = Cli::try_parse_from(["oxo-flow", "run", "test.oxoflow", "-r", "3"]).unwrap();
        match cli.command {
            Commands::Run { retry, .. } => {
                assert_eq!(retry, 3);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn cli_parse_run_with_timeout() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "run", "test.oxoflow", "--timeout", "300"]).unwrap();
        match cli.command {
            Commands::Run { timeout, .. } => {
                assert_eq!(timeout, 300);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn cli_parse_format() {
        let cli = Cli::try_parse_from(["oxo-flow", "format", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::Format { .. });
    }

    #[test]
    fn cli_parse_format_check() {
        let cli = Cli::try_parse_from(["oxo-flow", "format", "test.oxoflow", "--check"]).unwrap();
        match cli.command {
            Commands::Format { check, .. } => assert!(check),
            _ => panic!("expected Format command"),
        }
    }

    #[test]
    fn cli_parse_lint() {
        let cli = Cli::try_parse_from(["oxo-flow", "lint", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::Lint { .. });
    }

    #[test]
    fn cli_parse_lint_strict() {
        let cli = Cli::try_parse_from(["oxo-flow", "lint", "test.oxoflow", "--strict"]).unwrap();
        match cli.command {
            Commands::Lint { strict, .. } => assert!(strict),
            _ => panic!("expected Lint command"),
        }
    }

    #[test]
    fn cli_parse_profile_list() {
        let cli = Cli::try_parse_from(["oxo-flow", "profile", "list"]).unwrap();
        matches!(cli.command, Commands::Profile { .. });
    }

    #[test]
    fn cli_parse_profile_show() {
        let cli = Cli::try_parse_from(["oxo-flow", "profile", "show", "slurm"]).unwrap();
        match cli.command {
            Commands::Profile {
                action: ProfileAction::Show { name },
            } => {
                assert_eq!(name, "slurm");
            }
            _ => panic!("expected Profile Show command"),
        }
    }

    #[test]
    fn cli_parse_profile_current() {
        let cli = Cli::try_parse_from(["oxo-flow", "profile", "current"]).unwrap();
        matches!(
            cli.command,
            Commands::Profile {
                action: ProfileAction::Current
            }
        );
    }

    #[test]
    fn cli_parse_config_show() {
        let cli = Cli::try_parse_from(["oxo-flow", "config", "show", "test.oxoflow"]).unwrap();
        matches!(cli.command, Commands::Config { .. });
    }

    #[test]
    fn cli_parse_config_stats() {
        let cli = Cli::try_parse_from(["oxo-flow", "config", "stats", "test.oxoflow"]).unwrap();
        matches!(
            cli.command,
            Commands::Config {
                action: ConfigAction::Stats { .. }
            }
        );
    }
}
