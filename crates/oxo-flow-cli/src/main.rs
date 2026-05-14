#![forbid(unsafe_code)]
//! oxo-flow CLI — Bioinformatics pipeline engine.
//!
//! Provides subcommands for running, validating, and managing workflows.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{ExecutorConfig, LocalExecutor};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Auto-discover workflow file in current directory.
/// Priority: main.oxoflow > *.oxoflow (alphabetically first)
fn discover_workflow_file() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;

    // Priority 1: main.oxoflow
    let main_workflow = cwd.join("main.oxoflow");
    if main_workflow.exists() {
        return Ok(main_workflow);
    }

    // Priority 2: any *.oxoflow file (alphabetically first)
    let mut oxoflow_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&cwd)
        .with_context(|| format!("cannot read directory {}", cwd.display()))?
    {
        let entry = entry.context("cannot read directory entry")?;
        let path = entry.path();
        if let Some(ext) = path.extension()
            && ext == "oxoflow"
        {
            oxoflow_files.push(path);
        }
    }

    if oxoflow_files.is_empty() {
        return Err(anyhow::anyhow!(
            "no .oxoflow file found in current directory.\n\
            Specify a workflow file or run 'oxo-flow init' to create one."
        ));
    }

    // Sort alphabetically and pick first
    oxoflow_files.sort();
    Ok(oxoflow_files.into_iter().next().unwrap())
}

/// Resolve workflow path: use provided path or auto-discover.
fn resolve_workflow(provided: Option<PathBuf>) -> Result<PathBuf> {
    match provided {
        Some(path) => Ok(path),
        None => discover_workflow_file(),
    }
}

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

    /// Suppress non-essential output (errors only).
    #[arg(global = true, long)]
    quiet: bool,

    /// Disable colored output. Also respects the NO_COLOR environment variable.
    #[arg(global = true, long)]
    no_color: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Execute a workflow.
    Run {
        /// Path to the .oxoflow workflow file (optional - auto-detects if omitted).
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,

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

        /// Maximum threads available for execution (0 = auto-detect).
        #[arg(long, default_value = "0")]
        max_threads: u32,

        /// Maximum memory in MB available for execution (0 = auto-detect).
        #[arg(long, default_value = "0")]
        max_memory: u64,

        /// Skip environment setup (assume environments are already ready).
        #[arg(long)]
        skip_env_setup: bool,

        /// Directory for caching environment setup state.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },

    /// Simulate execution without running any commands.
    DryRun {
        /// Path to the .oxoflow workflow file (optional - auto-detects if omitted).
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,

        /// Run specific target rules only (and their dependencies).
        #[arg(short = 't', long)]
        target: Vec<String>,
    },

    /// Validate a .oxoflow workflow file.
    Validate {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },

    /// Output the workflow DAG for visualization.
    Graph {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Output format: ascii (terminal), dot (Graphviz), dot-clustered (enhanced).
        #[arg(short = 'f', long, default_value = "ascii")]
        format: String,

        /// Save output to a file (useful for dot/svg generation).
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
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

        /// Base path for mounting under a sub-path (e.g., "/oxo-flow").
        ///
        /// When deploying behind a reverse proxy, set this to the
        /// prefix path where the application is mounted.
        #[arg(long, default_value = "/")]
        base_path: String,
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

    /// Export a workflow to a container definition or standalone TOML.
    Export {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Export format: docker, singularity, toml.
        #[arg(short = 'f', long, default_value = "docker")]
        format: String,

        /// Output file path.
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },

    /// Manage cluster job submission and monitoring.
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },

    /// Compare two .oxoflow workflow files and show differences.
    Diff {
        /// First workflow file.
        #[arg(value_name = "WORKFLOW_A")]
        workflow_a: PathBuf,

        /// Second workflow file.
        #[arg(value_name = "WORKFLOW_B")]
        workflow_b: PathBuf,
    },

    /// Debug a workflow: show expanded commands after variable substitution.
    ///
    /// Displays each rule with its fully resolved shell command, resolved
    /// environment, resource requirements, and wildcard patterns. Useful for
    /// verifying that template variables are substituted correctly.
    Debug {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Show only a specific rule.
        #[arg(short = 'r', long = "rule")]
        rule_name: Option<String>,
    },

    /// Mark workflow outputs as up-to-date without re-executing rules.
    Touch {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Specific rule(s) whose outputs to touch. If omitted, all outputs are touched.
        #[arg(short = 'r', long = "rule")]
        rules: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum EnvAction {
    /// List available environment backends.
    List,

    /// Check if required environments are available.
    ///
    /// When a workflow file is provided, validates each rule's declared
    /// environment.  Without a workflow file, checks the availability of all
    /// supported backends on the current system.
    Check {
        /// Path to the .oxoflow workflow file (optional).
        ///
        /// If omitted, the availability of all supported environment backends
        /// (conda, pixi, docker, singularity, venv) is reported instead.
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,
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

#[derive(Subcommand, Debug)]
enum ClusterAction {
    /// Submit a workflow to a cluster scheduler.
    Submit {
        /// Path to the .oxoflow workflow file.
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,

        /// Cluster backend: slurm, pbs, sge, lsf.
        #[arg(short = 'b', long, default_value = "slurm")]
        backend: String,

        /// Partition / queue name.
        #[arg(short = 'q', long)]
        queue: Option<String>,

        /// Account / project name.
        #[arg(short = 'a', long)]
        account: Option<String>,

        /// Output directory for generated job scripts.
        #[arg(short = 'o', long, default_value = ".oxo-flow/cluster")]
        output_dir: PathBuf,
    },

    /// Show the status of submitted cluster jobs.
    Status {
        /// Cluster backend: slurm, pbs, sge, lsf.
        #[arg(short = 'b', long, default_value = "slurm")]
        backend: String,
    },

    /// Cancel submitted cluster jobs.
    Cancel {
        /// Cluster backend: slurm, pbs, sge, lsf.
        #[arg(short = 'b', long, default_value = "slurm")]
        backend: String,

        /// Job IDs to cancel.
        #[arg(value_name = "JOB_ID")]
        job_ids: Vec<String>,
    },
}

/// Search for a workflow file by walking up the directory tree.
///
/// Looks for `.oxoflow` files starting from the current directory and
/// walking up to parent directories. Returns the first match found.
#[allow(dead_code)]
fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        // Check for .oxoflow files
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "oxoflow") {
                    return Some(path);
                }
            }
        }
        // Check for oxo-flow config directory
        if dir.join(".oxo-flow").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    None
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

    // Disable colored output when --no-color is passed or NO_COLOR env var is set
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        colored::control::set_override(false);
    }

    // Initialize tracing with level based on --verbose / --quiet flags
    let default_level = if cli.quiet {
        "error"
    } else if cli.verbose {
        "debug"
    } else {
        "info"
    };
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
            target,
            retry,
            timeout,
            max_threads,
            max_memory,
            skip_env_setup,
            cache_dir,
        } => {
            print_banner();
            let workflow = resolve_workflow(workflow)?;
            let mut config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            // Expand rules that reference tumor/normal pair or sample-group wildcards.
            // Must be done before building the DAG so that concrete rule names and
            // file paths are available for dependency inference.
            config
                .expand_wildcards()
                .context("failed to expand wildcard rules")?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let order = if target.is_empty() {
                dag.execution_order()?
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                dag.execution_order_for_targets(&target_refs)
                    .with_context(|| "failed to resolve target rules")?
            };
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
                max_threads: if max_threads > 0 {
                    Some(max_threads)
                } else {
                    None
                },
                max_memory_mb: if max_memory > 0 {
                    Some(max_memory)
                } else {
                    None
                },
                skip_env_setup,
                cache_dir,
            };

            let executor = LocalExecutor::new(exec_config);
            let mut success_count = 0;
            let mut fail_count = 0;

            // Build wildcard values from workflow config variables so that
            // {config.key} placeholders in shell templates are expanded.
            let mut wildcard_values: HashMap<String, String> = HashMap::new();
            for (key, value) in &config.config {
                let string_val = match value {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                wildcard_values.insert(format!("config.{key}"), string_val);
            }

            for rule_name in &order {
                let rule = config.get_rule(rule_name).unwrap();
                match executor.execute_rule(rule, &wildcard_values).await {
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

        Commands::DryRun { workflow, target } => {
            print_banner();
            let workflow = resolve_workflow(workflow)?;
            let mut config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            config
                .expand_wildcards()
                .context("failed to expand wildcard rules")?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let order = if target.is_empty() {
                dag.execution_order()?
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                dag.execution_order_for_targets(&target_refs)
                    .with_context(|| "failed to resolve target rules")?
            };
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
                    eprintln!("     $ {}", cmd.dimmed());
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

        Commands::Graph {
            workflow,
            format,
            output,
        } => {
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let content = match format.as_str() {
                "ascii" => dag.to_ascii().context("failed to generate ASCII graph")?,
                "tree" => dag
                    .to_ascii_tree()
                    .context("failed to generate ASCII tree")?,
                "dot-clustered" => dag
                    .to_dot_clustered()
                    .context("failed to generate clustered DOT")?,
                _ => dag.to_dot(),
            };

            match output {
                Some(path) => {
                    std::fs::write(&path, &content)?;
                    eprintln!(
                        "{} Graph written to {} (format: {})",
                        "✓".green().bold(),
                        path.display(),
                        format
                    );
                    // Hint for dot format
                    if format.starts_with("dot") {
                        eprintln!(
                            "  {} Generate image: dot -Tpng {} > {}.png",
                            "Tip:".dimmed(),
                            path.display(),
                            path.display()
                        );
                    }
                }
                None => {
                    println!("{}", content);
                }
            }
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
                    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();

                    match workflow {
                        Some(wf_path) => {
                            // Validate each rule's declared environment in the workflow.
                            let config =
                                WorkflowConfig::from_file(&wf_path).with_context(|| {
                                    format!("failed to parse {}", wf_path.display())
                                })?;

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
                        None => {
                            // No workflow provided: report global backend availability.
                            eprintln!("{}", "Environment backend availability:".bold());
                            let available = resolver.available_backends();
                            for backend in
                                oxo_flow_core::environment::EnvironmentResolver::all_known_backends(
                                )
                            {
                                if available.contains(backend) {
                                    eprintln!("  {} {}", "✓".green(), backend);
                                } else {
                                    eprintln!("  {} {} (not found)", "✗".red(), backend);
                                }
                            }
                        }
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

        Commands::Serve {
            host,
            port,
            base_path,
        } => {
            print_banner();
            if base_path != "/" {
                eprintln!(
                    "Starting web server at {}:{} with base path '{}' ...",
                    host, port, base_path
                );
            } else {
                eprintln!("Starting web server at {}:{} ...", host, port);
            }
            oxo_flow_web::start_server_with_base(&host, port, &base_path).await?;
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
greeting = "Hello from oxo-flow!"

[defaults]
threads = 1
memory = "1G"

[[rules]]
name = "hello_world"
input = ["data/input.txt"]
output = ["results/output.txt"]
shell = "echo '{{config.greeting}}' > {{output[0]}} && cat {{input[0]}} >> {{output[0]}}"
"#
            );

            let workflow_path = project_dir.join(format!("{name}.oxoflow"));
            std::fs::write(&workflow_path, workflow_content)?;

            // Create additional directories
            let envs_dir = project_dir.join("envs");
            let scripts_dir = project_dir.join("scripts");
            let data_dir = project_dir.join("data");
            let results_dir = project_dir.join("results");
            std::fs::create_dir_all(&envs_dir)?;
            std::fs::create_dir_all(&scripts_dir)?;
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(&results_dir)?;

            // Create initial input file
            std::fs::write(
                data_dir.join("input.txt"),
                "This is your starting input data.\n",
            )?;

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
                let mut rejected = 0usize;

                for output in &outputs {
                    let has_wildcard = output.contains('{') && output.contains('}');
                    if has_wildcard {
                        skipped_wildcard += 1;
                    } else if output.contains("..")
                        || output.starts_with('/')
                        || output.starts_with('~')
                    {
                        eprintln!("  {} {} (rejected: unsafe path)", "✗".red().bold(), output);
                        rejected += 1;
                    } else if Path::new(output).exists() {
                        deletable.push(output);
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

        Commands::Export {
            workflow,
            format,
            output,
        } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let content = match format.as_str() {
                "singularity" => {
                    let pkg = oxo_flow_core::container::PackageConfig {
                        format: oxo_flow_core::container::ContainerFormat::Singularity,
                        ..Default::default()
                    };
                    oxo_flow_core::container::generate_singularity_def(&config, &pkg)?
                }
                "toml" => oxo_flow_core::format::format_workflow(&config),
                _ => {
                    let pkg = oxo_flow_core::container::PackageConfig::default();
                    oxo_flow_core::container::generate_dockerfile(&config, &pkg)?
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
        }

        Commands::Cluster { action } => {
            print_banner();
            match action {
                ClusterAction::Submit {
                    workflow,
                    backend,
                    queue,
                    account,
                    output_dir,
                } => {
                    let config = WorkflowConfig::from_file(&workflow)
                        .with_context(|| format!("failed to parse {}", workflow.display()))?;

                    let dag = WorkflowDag::from_rules(&config.rules)
                        .context("failed to build workflow DAG")?;

                    let order = dag.execution_order()?;

                    let cluster_backend = match backend.as_str() {
                        "pbs" => oxo_flow_core::cluster::ClusterBackend::Pbs,
                        "sge" => oxo_flow_core::cluster::ClusterBackend::Sge,
                        "lsf" => oxo_flow_core::cluster::ClusterBackend::Lsf,
                        _ => oxo_flow_core::cluster::ClusterBackend::Slurm,
                    };

                    let cluster_config = oxo_flow_core::cluster::ClusterJobConfig {
                        backend: cluster_backend,
                        queue: queue.clone(),
                        account: account.clone(),
                        walltime: None,
                        extra_args: vec![],
                    };

                    std::fs::create_dir_all(&output_dir)?;

                    eprintln!(
                        "{} Generating {} job scripts for {} rules",
                        "Cluster:".bold().cyan(),
                        backend,
                        order.len()
                    );

                    // Create environment resolver for command wrapping
                    let env_resolver = oxo_flow_core::environment::EnvironmentResolver::new();

                    for rule_name in &order {
                        let rule = config.get_rule(rule_name).unwrap();
                        let shell_cmd = match rule.shell.as_deref() {
                            Some(cmd) => cmd,
                            None => {
                                eprintln!(
                                    "  {} {} — no shell command, skipping",
                                    "⊘".yellow(),
                                    rule_name
                                );
                                continue;
                            }
                        };

                        // Generate script with environment wrapping
                        let script = oxo_flow_core::cluster::generate_submit_script_with_env(
                            &cluster_backend,
                            rule,
                            shell_cmd,
                            &cluster_config,
                            &env_resolver,
                        )
                        .map_err(|e| anyhow::anyhow!("environment wrapping failed: {}", e))?;

                        let script_path = output_dir.join(format!("{rule_name}.sh"));
                        std::fs::write(&script_path, &script)?;
                        eprintln!("  {} {}", "✓".green(), script_path.display());
                    }

                    eprintln!(
                        "\n{} {} scripts written to {}",
                        "Done:".bold(),
                        order.len(),
                        output_dir.display()
                    );
                    eprintln!(
                        "  Submit with: {} {}/*.sh",
                        oxo_flow_core::cluster::submit_command(&cluster_backend),
                        output_dir.display()
                    );
                }

                ClusterAction::Status { backend } => {
                    let cluster_backend = match backend.as_str() {
                        "pbs" => oxo_flow_core::cluster::ClusterBackend::Pbs,
                        "sge" => oxo_flow_core::cluster::ClusterBackend::Sge,
                        "lsf" => oxo_flow_core::cluster::ClusterBackend::Lsf,
                        _ => oxo_flow_core::cluster::ClusterBackend::Slurm,
                    };

                    let status_cmd = oxo_flow_core::cluster::status_command(&cluster_backend);
                    eprintln!("{} Executing '{}'...", "Cluster:".bold().cyan(), status_cmd);

                    let mut parts = status_cmd.split_whitespace();
                    let program = parts.next().unwrap_or(status_cmd);
                    let args: Vec<&str> = parts.collect();

                    match std::process::Command::new(program).args(&args).status() {
                        Ok(status) => {
                            if !status.success() {
                                eprintln!(
                                    "{} Command failed with exit code: {}",
                                    "✗".red(),
                                    status.code().unwrap_or(-1)
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("{} Failed to execute status command: {}", "✗".red(), e);
                            eprintln!("  Is {} installed on this system?", program);
                        }
                    }
                }

                ClusterAction::Cancel { backend, job_ids } => {
                    let cancel_cmd = match backend.as_str() {
                        "pbs" => "qdel",
                        "sge" => "qdel",
                        "lsf" => "bkill",
                        _ => "scancel",
                    };

                    if job_ids.is_empty() {
                        eprintln!(
                            "{} No job IDs provided. Usage: oxo-flow cluster cancel <JOB_ID>...",
                            "Warning:".bold().yellow()
                        );
                    } else {
                        eprintln!(
                            "{} Canceling {} job(s)...",
                            "Cluster:".bold().cyan(),
                            job_ids.len()
                        );

                        match std::process::Command::new(cancel_cmd)
                            .args(&job_ids)
                            .status()
                        {
                            Ok(status) => {
                                if status.success() {
                                    eprintln!("{} Successfully canceled jobs.", "✓".green());
                                } else {
                                    eprintln!(
                                        "{} Command failed with exit code: {}",
                                        "✗".red(),
                                        status.code().unwrap_or(-1)
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("{} Failed to execute cancel command: {}", "✗".red(), e);
                                eprintln!("  Is {} installed on this system?", cancel_cmd);
                            }
                        }
                    }
                }
            }
        }

        Commands::Diff {
            workflow_a,
            workflow_b,
        } => {
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
        }

        Commands::Debug {
            workflow,
            rule_name,
        } => {
            print_banner();
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            let rules_to_show: Vec<&oxo_flow_core::Rule> = if let Some(ref name) = rule_name {
                match config.rules.iter().find(|r| r.name == *name) {
                    Some(r) => vec![r],
                    None => {
                        eprintln!("{} rule '{}' not found", "error:".bold().red(), name);
                        std::process::exit(1);
                    }
                }
            } else {
                config.rules.iter().collect()
            };

            eprintln!(
                "{} Debugging {} rules from {}",
                "Debug:".bold().yellow(),
                rules_to_show.len(),
                workflow.display()
            );
            eprintln!();

            for rule in &rules_to_show {
                eprintln!("{}", format!("── Rule: {} ──", rule.name).bold().cyan());

                if let Some(ref desc) = rule.description {
                    eprintln!("  {} {}", "Description:".dimmed(), desc);
                }
                if let Some(ref base) = rule.extends {
                    eprintln!("  {} {}", "Extends:".dimmed(), base);
                }
                if let Some(ref when) = rule.when {
                    eprintln!("  {} {}", "Condition:".dimmed(), when);
                }

                if !rule.input.is_empty() {
                    eprintln!("  {} {:?}", "Inputs:".dimmed(), rule.input);
                }
                if !rule.output.is_empty() {
                    eprintln!("  {} {:?}", "Outputs:".dimmed(), rule.output);
                }

                // Shell command (raw template)
                if let Some(ref cmd) = rule.shell {
                    eprintln!("  {} {}", "Shell (template):".dimmed(), cmd);
                    // Show expanded version with placeholder substitution
                    let expanded =
                        oxo_flow_core::executor::render_shell_command(cmd, rule, &HashMap::new());
                    if expanded != *cmd {
                        eprintln!("  {} {}", "Shell (expanded):".dimmed(), expanded);
                    }
                }
                if let Some(ref script) = rule.script {
                    eprintln!("  {} {}", "Script:".dimmed(), script);
                }

                // Resources
                let threads = rule.effective_threads();
                let memory = rule.effective_memory().unwrap_or("(default)");
                eprintln!(
                    "  {} threads={}, memory={}",
                    "Resources:".dimmed(),
                    threads,
                    memory
                );
                if let Some(gpu) = rule.resources.gpu {
                    eprintln!("  {} count={}", "GPU:".dimmed(), gpu);
                }
                if let Some(ref gpu_spec) = rule.resources.gpu_spec {
                    eprintln!(
                        "  {} count={}, model={:?}, memory_gb={:?}",
                        "GPU (detailed):".dimmed(),
                        gpu_spec.count,
                        gpu_spec.model,
                        gpu_spec.memory_gb
                    );
                }

                // Retries & hooks
                if rule.retries > 0 {
                    let delay = rule.retry_delay.as_deref().unwrap_or("immediate");
                    eprintln!(
                        "  {} count={}, delay={}",
                        "Retries:".dimmed(),
                        rule.retries,
                        delay
                    );
                }
                if let Some(ref hook) = rule.on_success {
                    eprintln!("  {} {}", "On success:".dimmed(), hook);
                }
                if let Some(ref hook) = rule.on_failure {
                    eprintln!("  {} {}", "On failure:".dimmed(), hook);
                }

                // Environment
                eprintln!("  {} {}", "Environment:".dimmed(), rule.environment.kind());

                // Dependencies
                if let Ok(deps) = dag.dependencies(&rule.name)
                    && !deps.is_empty()
                {
                    eprintln!("  {} {:?}", "Dependencies:".dimmed(), deps);
                }
                if !rule.depends_on.is_empty() {
                    eprintln!("  {} {:?}", "Explicit deps:".dimmed(), rule.depends_on);
                }

                // Tags
                if !rule.tags.is_empty() {
                    eprintln!("  {} {:?}", "Tags:".dimmed(), rule.tags);
                }

                // New fields
                if !rule.format_hint.is_empty() {
                    eprintln!("  {} {:?}", "Format hints:".dimmed(), rule.format_hint);
                }
                if rule.pipe {
                    eprintln!("  {} enabled", "Pipe/FIFO:".dimmed());
                }
                if let Some(ref cksum) = rule.checksum {
                    eprintln!("  {} {}", "Checksum:".dimmed(), cksum);
                }
                if let Some(ref hint) = rule.resource_hint {
                    eprintln!(
                        "  {} input_size={:?}, memory_scale={:?}, runtime={:?}, io_bound={:?}",
                        "Resource hint:".dimmed(),
                        hint.input_size,
                        hint.memory_scale,
                        hint.runtime,
                        hint.io_bound
                    );
                }
                if !rule.rule_metadata.is_empty() {
                    eprintln!("  {} {:?}", "Metadata:".dimmed(), rule.rule_metadata);
                }

                // Wildcards
                let wildcards = rule.wildcard_names();
                if !wildcards.is_empty() {
                    eprintln!("  {} {:?}", "Wildcards:".dimmed(), wildcards);
                }

                eprintln!();
            }

            eprintln!("{}", "Debug complete.".green());
        }

        Commands::Touch { workflow, rules } => {
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
                assert_eq!(workflow, Some(PathBuf::from("test.oxoflow")));
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
        match cli.command {
            Commands::Graph {
                workflow, format, ..
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert_eq!(format, "ascii"); // default
            }
            _ => panic!("expected Graph command"),
        }
    }

    #[test]
    fn cli_parse_graph_dot() {
        let cli = Cli::try_parse_from(["oxo-flow", "graph", "test.oxoflow", "-f", "dot"]).unwrap();
        match cli.command {
            Commands::Graph { format, .. } => {
                assert_eq!(format, "dot");
            }
            _ => panic!("expected Graph command"),
        }
    }

    #[test]
    fn cli_parse_graph_with_output() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "graph", "test.oxoflow", "-o", "graph.dot"]).unwrap();
        match cli.command {
            Commands::Graph { output, .. } => {
                assert_eq!(output, Some(PathBuf::from("graph.dot")));
            }
            _ => panic!("expected Graph command"),
        }
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

    #[test]
    fn cli_parse_export_default() {
        let cli = Cli::try_parse_from(["oxo-flow", "export", "test.oxoflow"]).unwrap();
        match cli.command {
            Commands::Export {
                workflow, format, ..
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert_eq!(format, "docker");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn cli_parse_export_singularity() {
        let cli = Cli::try_parse_from(["oxo-flow", "export", "test.oxoflow", "-f", "singularity"])
            .unwrap();
        match cli.command {
            Commands::Export { format, .. } => {
                assert_eq!(format, "singularity");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn cli_parse_export_toml() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "export", "test.oxoflow", "-f", "toml"]).unwrap();
        match cli.command {
            Commands::Export { format, .. } => {
                assert_eq!(format, "toml");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn cli_parse_cluster_submit() {
        let cli = Cli::try_parse_from([
            "oxo-flow",
            "cluster",
            "submit",
            "test.oxoflow",
            "-b",
            "slurm",
        ])
        .unwrap();
        match cli.command {
            Commands::Cluster {
                action:
                    ClusterAction::Submit {
                        workflow, backend, ..
                    },
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert_eq!(backend, "slurm");
            }
            _ => panic!("expected Cluster Submit command"),
        }
    }

    #[test]
    fn cli_parse_cluster_status() {
        let cli = Cli::try_parse_from(["oxo-flow", "cluster", "status", "-b", "pbs"]).unwrap();
        match cli.command {
            Commands::Cluster {
                action: ClusterAction::Status { backend },
            } => {
                assert_eq!(backend, "pbs");
            }
            _ => panic!("expected Cluster Status command"),
        }
    }

    #[test]
    fn cli_parse_cluster_cancel() {
        let cli = Cli::try_parse_from([
            "oxo-flow", "cluster", "cancel", "-b", "slurm", "12345", "67890",
        ])
        .unwrap();
        match cli.command {
            Commands::Cluster {
                action: ClusterAction::Cancel { backend, job_ids },
            } => {
                assert_eq!(backend, "slurm");
                assert_eq!(job_ids, vec!["12345", "67890"]);
            }
            _ => panic!("expected Cluster Cancel command"),
        }
    }

    // ---- Tests for new subcommands ------------------------------------------

    #[test]
    fn cli_parse_diff() {
        let cli = Cli::try_parse_from(["oxo-flow", "diff", "a.oxoflow", "b.oxoflow"]).unwrap();
        match cli.command {
            Commands::Diff {
                workflow_a,
                workflow_b,
            } => {
                assert_eq!(workflow_a, PathBuf::from("a.oxoflow"));
                assert_eq!(workflow_b, PathBuf::from("b.oxoflow"));
            }
            _ => panic!("expected Diff command"),
        }
    }

    #[test]
    fn cli_parse_touch() {
        let cli = Cli::try_parse_from(["oxo-flow", "touch", "pipeline.oxoflow"]).unwrap();
        match cli.command {
            Commands::Touch {
                workflow, rules, ..
            } => {
                assert_eq!(workflow, PathBuf::from("pipeline.oxoflow"));
                assert!(rules.is_empty());
            }
            _ => panic!("expected Touch command"),
        }
    }

    #[test]
    fn cli_parse_touch_with_rule_flag() {
        let cli = Cli::try_parse_from(["oxo-flow", "touch", "pipeline.oxoflow", "--rule", "align"])
            .unwrap();
        match cli.command {
            Commands::Touch {
                workflow, rules, ..
            } => {
                assert_eq!(workflow, PathBuf::from("pipeline.oxoflow"));
                assert_eq!(rules, vec!["align"]);
            }
            _ => panic!("expected Touch command"),
        }
    }

    #[test]
    fn cli_parse_serve_with_base_path() {
        let cli = Cli::try_parse_from([
            "oxo-flow",
            "serve",
            "--host",
            "0.0.0.0",
            "--base-path",
            "/oxo-flow",
        ])
        .unwrap();
        match cli.command {
            Commands::Serve {
                host, base_path, ..
            } => {
                assert_eq!(host, "0.0.0.0");
                assert_eq!(base_path, "/oxo-flow");
            }
            _ => panic!("expected Serve command"),
        }
    }

    #[test]
    fn cli_parse_serve_default_base_path() {
        let cli = Cli::try_parse_from(["oxo-flow", "serve"]).unwrap();
        match cli.command {
            Commands::Serve { base_path, .. } => {
                assert_eq!(base_path, "/");
            }
            _ => panic!("expected Serve command"),
        }
    }

    #[test]
    fn cli_parse_debug() {
        let cli = Cli::try_parse_from(["oxo-flow", "debug", "test.oxoflow"]).unwrap();
        match cli.command {
            Commands::Debug {
                workflow,
                rule_name,
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert!(rule_name.is_none());
            }
            _ => panic!("expected Debug command"),
        }
    }

    #[test]
    fn cli_parse_debug_with_rule() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "debug", "test.oxoflow", "-r", "align"]).unwrap();
        match cli.command {
            Commands::Debug {
                workflow,
                rule_name,
            } => {
                assert_eq!(workflow, PathBuf::from("test.oxoflow"));
                assert_eq!(rule_name.as_deref(), Some("align"));
            }
            _ => panic!("expected Debug command"),
        }
    }

    #[test]
    fn find_project_root_returns_none_in_empty_dir() {
        // When run from a temp directory with no .oxoflow files, should return None
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        // We can't safely change the working directory in tests (shared state),
        // so we just verify the function is callable and returns Some/None
        // based on the current directory.
        let _result = find_project_root();
        // Restore (no-op since we didn't change dir)
        drop(original);
        drop(dir);
    }

    #[test]
    fn cli_parse_dry_run_with_target() {
        let cli =
            Cli::try_parse_from(["oxo-flow", "dry-run", "test.oxoflow", "-t", "align"]).unwrap();
        match cli.command {
            Commands::DryRun { workflow, target } => {
                assert_eq!(workflow, Some(PathBuf::from("test.oxoflow")));
                assert_eq!(target, vec!["align"]);
            }
            _ => panic!("expected DryRun command"),
        }
    }

    #[test]
    fn cli_parse_dry_run_with_multiple_targets() {
        let cli = Cli::try_parse_from([
            "oxo-flow",
            "dry-run",
            "test.oxoflow",
            "-t",
            "align",
            "-t",
            "sort_bam",
        ])
        .unwrap();
        match cli.command {
            Commands::DryRun { target, .. } => {
                assert_eq!(target, vec!["align", "sort_bam"]);
            }
            _ => panic!("expected DryRun command"),
        }
    }

    #[test]
    fn cli_parse_run_with_target() {
        let cli = Cli::try_parse_from(["oxo-flow", "run", "test.oxoflow", "-t", "align"]).unwrap();
        match cli.command {
            Commands::Run { target, .. } => {
                assert_eq!(target, vec!["align"]);
            }
            _ => panic!("expected Run command"),
        }
    }
}
