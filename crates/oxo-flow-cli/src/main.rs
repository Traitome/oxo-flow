#![forbid(unsafe_code)]
//! oxo-flow CLI — Bioinformatics pipeline engine.
//!
//! Provides subcommands for running, validating, and managing workflows.

pub mod commands;

use crate::commands::batch::batch_command;
use crate::commands::clean::clean_command;
use crate::commands::cluster::cluster_command;
use crate::commands::completions::handle_completions;
use crate::commands::infra::{env_command, package_command, profile_command};
use crate::commands::output::{handle_diff, handle_export, handle_graph, handle_report};
use crate::commands::project::{init_command, template_command};
use crate::commands::provenance::provenance_verify_command;
use crate::commands::publish::publish_command;
use crate::commands::quality::{
    format_command, lint_command, touch_command, validate_command, watch_command,
};
use crate::commands::run::{
    debug_command, dry_run_command, handle_status, resume_command, run_command,
};
use anyhow::Result;
use clap::{Parser, Subcommand};
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
                   built from first principles in Rust. It supports conda, pixi, docker,\n\
                   singularity, and venv environments with DAG-based execution."
)]
pub struct Cli {
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

    /// Output machine-readable JSON to stdout (suppresses human-readable stderr output).
    #[arg(global = true, long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Execute a workflow.
    Run {
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
        #[arg(short = 'k', long)]
        keep_going: bool,
        #[arg(short = 'd', long)]
        workdir: Option<PathBuf>,
        #[arg(short = 't', long)]
        target: Vec<String>,
        #[arg(short = 'r', long, default_value = "0")]
        retry: u32,
        #[arg(long, default_value = "0")]
        timeout: String,
        #[arg(long)]
        resume_failed: bool,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, default_value = "0")]
        max_threads: u32,
        #[arg(long, default_value = "0")]
        max_memory: u64,
        #[arg(long)]
        skip_env_setup: bool,
        #[arg(long, help = "Skip automatic reference/index building")]
        skip_ref_build: bool,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        provenance: bool,
        #[arg(long, help = "Execute from a published .tar.zst bundle")]
        bundle: Option<PathBuf>,
        #[arg(
            long = "arg",
            value_name = "KEY=VALUE",
            help = "Set a workflow config value (overrides [config] defaults). Repeatable."
        )]
        args: Vec<String>,
        #[arg(
            value_name = "KEY=VALUE",
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Direct config overrides: KEY=VALUE, --KEY=VALUE, or --KEY VALUE"
        )]
        config_overrides: Vec<String>,
        #[arg(
            long = "sample",
            value_name = "SAMPLE",
            help = "Add a sample to the run (repeatable, merges with all sources)"
        )]
        extra_samples: Vec<String>,
        /// Enable AI error recovery on rule failure.
        #[arg(long)]
        ai_recover: bool,
    },
    /// Resume an interrupted workflow from a checkpoint.
    Resume {
        #[arg(value_name = "CHECKPOINT")]
        checkpoint: PathBuf,
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
        /// Enable AI error recovery on rule failure.
        #[arg(long)]
        ai_recover: bool,
    },
    /// Preview execution without running any commands.
    DryRun {
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,
        #[arg(short = 't', long)]
        target: Vec<String>,
        /// Enable AI-powered analysis of the workflow.
        #[arg(long)]
        ai: bool,
    },
    /// Validate a .oxoflow workflow file.
    Validate {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(
            long,
            help = "Validate as a sub-workflow fragment (skip DAG validation)"
        )]
        as_include: bool,
        /// Enable AI-powered semantic validation.
        #[arg(long)]
        ai: bool,
    },
    /// Initialize a new workflow project.
    Init {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(short = 'd', long)]
        dir: Option<PathBuf>,
    },
    /// Generate a workflow from a predefined template or via AI.
    Template {
        #[arg(value_name = "TEMPLATE")]
        template: Option<String>,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Enable AI-powered workflow generation from natural language.
        #[arg(long)]
        ai: bool,
        /// URL(s) to use as reference material for AI generation.
        #[arg(long = "from-url", value_name = "URL")]
        from_url: Vec<String>,
        /// File(s) to use as reference material for AI generation.
        #[arg(long = "from-file", value_name = "PATH")]
        from_file: Vec<PathBuf>,
        /// Maximum AI correction rounds (overrides config).
        #[arg(long = "ai-max-retries", value_name = "N")]
        ai_max_retries: Option<u32>,
    },
    /// Output the workflow DAG for visualization.
    Graph {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'f', long, default_value = "ascii")]
        format: String,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Show execution status from a checkpoint file.
    Status {
        #[arg(value_name = "CHECKPOINT")]
        checkpoint: PathBuf,
    },
    /// Pull a published bundle from a remote source.
    Pull {
        #[arg(value_name = "URL")]
        url: String,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Inspect and manage workflow configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Compare two .oxoflow workflow files and show differences.
    Diff {
        #[arg(value_name = "WORKFLOW_A")]
        workflow_a: PathBuf,
        #[arg(value_name = "WORKFLOW_B")]
        workflow_b: PathBuf,
    },
    /// Debug a workflow: show expanded commands after variable substitution.
    Debug {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'r', long = "rule")]
        rule_name: Option<String>,
        /// Enable AI-powered command explanation.
        #[arg(long)]
        ai: bool,
    },
    /// Clean workflow outputs and temporary files.
    Clean {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        orphans: bool,
    },
    /// Manage software environments.
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },
    /// Reformat a .oxoflow file into canonical TOML form.
    Format {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Run best-practice linting checks on a .oxoflow file.
    Lint {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(long)]
        strict: bool,
        /// Enable AI-powered semantic linting.
        #[arg(long)]
        ai: bool,
    },
    /// Watch workflow file for changes and re-validate/re-run.
    Watch {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(long)]
        run: bool,
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
    },
    /// Mark workflow outputs as up-to-date without re-executing rules.
    Touch {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'r', long = "rule")]
        rules: Vec<String>,
    },
    /// Generate reports from workflow execution.
    Report {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'f', long, default_value = "html")]
        format: String,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        #[arg(
            long = "checkpoint",
            value_name = "PATH",
            help = "Path to checkpoint file (default: .oxo-flow/checkpoint.json)"
        )]
        checkpoint_path: Option<PathBuf>,
    },
    /// Package a workflow into a container image.
    Package {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'f', long, default_value = "docker")]
        format: String,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Start the web interface server.
    Serve {
        /// Server operation mode: personal, team, or hpc.
        #[arg(long, default_value = "personal", env = "OXO_FLOW_MODE")]
        mode: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(short = 'p', long, default_value = "8080")]
        port: u16,
        #[arg(long, default_value = "/")]
        base_path: String,
    },
    /// Generate shell completions for oxo-flow.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Manage execution profiles.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Export a workflow to a container definition or standalone TOML.
    Export {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'f', long, default_value = "docker")]
        format: String,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Manage cluster job submission and monitoring.
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },
    /// Execute a command template in parallel across multiple items.
    Batch {
        #[arg(value_name = "TEMPLATE")]
        template: String,
        #[arg(value_name = "ITEMS")]
        items: Vec<String>,
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
        #[arg(short = 'x', long)]
        stop_on_error: bool,
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,
        #[arg(long = "json-output", help = "Output results as formatted JSON")]
        json_output: bool,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(short = 'd', long)]
        workdir: Option<PathBuf>,
        #[arg(short = 'e', long)]
        environment: Option<String>,
        #[arg(long)]
        checksum: bool,
        #[arg(long)]
        generate_workflow: bool,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    /// Verify output file integrity using stored checksums.
    Provenance {
        #[command(subcommand)]
        action: ProvenanceAction,
    },
    /// Output the JSON Schema for the .oxoflow format.
    Schema,
    /// Show execution history from checkpoints.
    History {
        #[arg(value_name = "DIR")]
        dir: Option<PathBuf>,
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },
    /// Run a workflow in test mode, validating and verifying outputs.
    Test {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        run: bool,
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
    },
    /// Publish a workflow with its environment files into a bundle.
    Publish {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        #[arg(long, help = "Generate conda lockfiles for reproducible environments")]
        with_lockfiles: bool,
    },
    /// Verify or display license status.
    License {
        /// Path to license file to verify (optional; checks current status if omitted)
        #[arg(value_name = "LICENSE_PATH")]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum EnvAction {
    List {
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,
    },
    Check {
        #[arg(value_name = "WORKFLOW")]
        workflow: Option<PathBuf>,
    },
    /// Create a new environment from a spec file.
    Create {
        #[arg(value_name = "SPEC")]
        spec: PathBuf,
        #[arg(short = 'n', long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProfileAction {
    List,
    Show {
        #[arg(value_name = "NAME")]
        name: String,
    },
    Current,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    Show {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },
    #[command(alias = "check")]
    Stats {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
    },
    Get {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(value_name = "KEY")]
        key: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ClusterAction {
    Submit {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(short = 'b', long)]
        backend: String,
        #[arg(short = 'q', long)]
        queue: Option<String>,
        #[arg(short = 'a', long)]
        account: Option<String>,
        #[arg(short = 'o', long, default_value = "cluster_scripts")]
        output: PathBuf,
        #[arg(short = 't', long)]
        target: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        /// Generate job scripts with dependency support and a wrapper script
        #[arg(long)]
        with_dependencies: bool,
    },
    Status {
        #[arg(short = 'b', long)]
        backend: String,
        #[arg(value_name = "JOB_IDS")]
        job_ids: Vec<String>,
    },
    Cancel {
        #[arg(short = 'b', long)]
        backend: String,
        #[arg(value_name = "JOB_IDS")]
        job_ids: Vec<String>,
    },
    Logs {
        #[arg(short = 'b', long)]
        backend: String,
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProvenanceAction {
    /// Verify output file checksums from a checkpoint or provenance file.
    Verify {
        #[arg(value_name = "CHECKPOINT_PATH")]
        checkpoint: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        colored::control::set_override(false);
    }

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

    // Suppress banner in quiet mode
    crate::commands::set_quiet_mode(cli.quiet);

    match cli.command {
        Commands::Run {
            workflow,
            jobs,
            keep_going,
            workdir,
            target,
            retry,
            timeout,
            resume_failed,
            profile,
            max_threads,
            max_memory,
            skip_env_setup,
            skip_ref_build,
            cache_dir,
            provenance,
            bundle,
            args,
            config_overrides,
            extra_samples,
            ai_recover,
        } => {
            let (wf, wd) = if let Some(bundle_path) = bundle {
                let (extracted_wf, extracted_dir) =
                    crate::commands::bundle::extract_and_verify_bundle(&bundle_path)?;
                // Respect explicit -d flag; otherwise use extracted dir
                let effective_wd = workdir.unwrap_or(extracted_dir);
                (Some(extracted_wf), Some(effective_wd))
            } else {
                (workflow, workdir)
            };
            // Merge direct `KEY=VALUE` / `--KEY VALUE` overrides with `--arg KEY=VALUE`
            // (backward compat). Later entries win on duplicate keys.
            let mut merged_args = config_overrides;
            merged_args.extend(args);
            run_command(
                wf,
                jobs,
                keep_going,
                wd,
                target,
                retry,
                timeout,
                resume_failed,
                profile,
                max_threads,
                max_memory,
                skip_env_setup,
                skip_ref_build,
                cache_dir,
                provenance,
                cli.json,
                merged_args,
                extra_samples,
                ai_recover,
            )
            .await?
        }
        Commands::Resume {
            checkpoint,
            jobs,
            ai_recover,
        } => resume_command(checkpoint, jobs, ai_recover).await?,
        Commands::DryRun {
            workflow,
            target,
            ai,
        } => dry_run_command(workflow, target, cli.verbose, cli.json, ai).await?,
        Commands::Validate {
            workflow,
            as_include,
            ai,
        } => {
            validate_command(workflow, as_include, cli.json, ai).await?;
        }
        Commands::Init { name, dir } => init_command(name, dir)?,
        Commands::Template {
            template,
            output,
            ai,
            from_url,
            from_file,
            ai_max_retries,
        } => template_command(template, output, ai, from_url, from_file, ai_max_retries).await?,
        Commands::Graph {
            workflow,
            format,
            output,
        } => handle_graph(workflow, format, output)?,
        Commands::Status { checkpoint } => handle_status(checkpoint, cli.json).await?,
        Commands::Pull { url, output } => crate::commands::pull::pull_command(&url, output).await?,
        Commands::Config { action } => crate::commands::infra::handle_config(action)?,
        Commands::Diff {
            workflow_a,
            workflow_b,
        } => handle_diff(workflow_a, workflow_b)?,
        Commands::Debug {
            workflow,
            rule_name,
            ai,
        } => debug_command(workflow, rule_name, ai).await?,
        Commands::Clean {
            workflow,
            dry_run,
            force,
            orphans,
        } => clean_command(workflow, dry_run, force, orphans)?,
        Commands::Env { action } => env_command(action)?,
        Commands::Format {
            workflow,
            output,
            check,
        } => format_command(workflow, output, check)?,
        Commands::Lint {
            workflow,
            strict,
            ai,
        } => lint_command(workflow, strict, cli.json, ai).await?,
        Commands::Watch {
            workflow,
            run,
            jobs,
        } => watch_command(workflow, run, jobs).await?,
        Commands::Touch { workflow, rules } => touch_command(workflow, rules)?,
        Commands::Report {
            workflow,
            format,
            output,
            checkpoint_path,
        } => handle_report(workflow, format, output, checkpoint_path)?,
        Commands::Package {
            workflow,
            format,
            output,
        } => package_command(workflow, format, output)?,
        Commands::Serve {
            mode,
            host,
            port,
            base_path,
        } => crate::commands::web::handle_serve(mode, host, port, base_path).await?,
        Commands::Completions { shell } => handle_completions(shell)?,
        Commands::Profile { action } => profile_command(action)?,
        Commands::Export {
            workflow,
            format,
            output,
        } => handle_export(workflow, format, output)?,
        Commands::Cluster { action } => cluster_command(action).await?,
        Commands::Batch {
            template,
            items,
            jobs,
            stop_on_error,
            file,
            json_output: json,
            dry_run,
            workdir,
            environment,
            checksum,
            generate_workflow,
            output,
        } => {
            batch_command(
                template,
                items,
                jobs,
                stop_on_error,
                file,
                json,
                dry_run,
                workdir,
                environment,
                checksum,
                generate_workflow,
                output,
            )
            .await?
        }
        Commands::Provenance { action } => match action {
            ProvenanceAction::Verify { checkpoint } => provenance_verify_command(checkpoint)?,
        },
        Commands::Schema => {
            let schema = include_str!("../schema/oxoflow-v1.schema.json");
            println!("{schema}");
        }
        Commands::History { dir, limit } => {
            use colored::Colorize;
            let base = dir.unwrap_or_else(|| PathBuf::from("."));
            let checkpoint_path = base.join(".oxo-flow").join("checkpoint.json");

            if checkpoint_path.exists() {
                if let Ok(state) =
                    oxo_flow_core::executor::CheckpointState::load_from_file(&checkpoint_path)
                {
                    eprintln!("{} {}", "History:".bold().cyan(), checkpoint_path.display());
                    eprintln!(
                        "  Workflow: {}",
                        state.workflow_path.as_deref().unwrap_or("unknown")
                    );
                    eprintln!("  Completed: {}", state.completed_rules.len());
                    eprintln!("  Failed:    {}", state.failed_rules.len());
                    if !state.benchmarks.is_empty() {
                        let total: f64 = state.benchmarks.values().map(|b| b.wall_time_secs).sum();
                        eprintln!("  Total time: {:.1}s", total);
                    }
                    if !state.completed_rules.is_empty() {
                        eprintln!("\n  {} (showing up to {})", "Recent rules:".bold(), limit);
                        for rule in state.completed_rules.iter().take(limit) {
                            let bench = state.benchmarks.get(rule);
                            let time =
                                bench.map_or("-".into(), |b| format!("{:.1}s", b.wall_time_secs));
                            eprintln!("    ✓ {} ({})", rule, time);
                        }
                    }
                } else {
                    eprintln!("  {} failed to parse checkpoint", "✗".red());
                }
            } else {
                eprintln!(
                    "{} No checkpoint found at {}. Run a workflow first.",
                    "Note:".yellow(),
                    checkpoint_path.display()
                );
            }
        }
        Commands::Test {
            workflow,
            output,
            run,
            jobs,
        } => {
            use colored::Colorize;
            eprintln!(
                "{} Running test suite for {}\n",
                "🧪".bold(),
                workflow.display()
            );
            // 1. Validate
            eprintln!("{} Validation...", "1.".bold());
            validate_command(workflow.clone(), false, cli.json, false).await?;
            // 2. Lint
            eprintln!("{} Lint...", "2.".bold());
            lint_command(workflow.clone(), false, cli.json, false).await?;
            // 3. Dry-run
            eprintln!("{} Dry-run...", "3.".bold());
            dry_run_command(Some(workflow.clone()), vec![], cli.verbose, cli.json, false).await?;
            // 4. Optional: run with --run flag
            if run {
                eprintln!("{} Execution...", "4.".bold());
                run_command(
                    Some(workflow),
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
                    false,           // skip_ref_build
                    None,            // cache_dir
                    false,           // provenance
                    cli.json,
                    vec![], // cli_args
                    vec![], // extra_samples
                    false,  // ai_recover
                )
                .await?;
            }
            // 5. Optional: verify output file existence
            if let Some(output_path) = output {
                if output_path.exists() {
                    eprintln!(
                        "{} Output file exists: {}",
                        "✓".green().bold(),
                        output_path.display()
                    );
                } else {
                    eprintln!(
                        "{} Output file not found: {}",
                        "✗".red().bold(),
                        output_path.display()
                    );
                    std::process::exit(1);
                }
            }
            eprintln!("\n{} All checks passed.", "✓".green().bold());
        }
        Commands::Publish {
            workflow,
            output,
            with_lockfiles,
        } => publish_command(workflow, output, with_lockfiles)?,
        Commands::License { path } => {
            use colored::Colorize;
            let status = oxo_flow_web::check_license();
            if let Some(p) = path {
                match oxo_license::load_and_verify(Some(&p), &oxo_flow_web::OXO_FLOW_CONFIG) {
                    Ok(license) => {
                        println!("{} License verified successfully", "✓".green().bold());
                        println!("  Type:    {}", license.payload.license_type);
                        println!("  Issued:  {}", license.payload.issued_to_org);
                        println!("  Schema:  {}", license.payload.schema);
                        println!("  ID:      {}", license.payload.license_id);
                    }
                    Err(e) => {
                        eprintln!("{} License verification failed: {e}", "✗".red().bold());
                        std::process::exit(1);
                    }
                }
            } else {
                println!("License status:");
                if status.valid {
                    println!(
                        "  Status:  {} ({})",
                        "Valid".green().bold(),
                        status.license_type.as_deref().unwrap_or("unknown")
                    );
                    if let Some(org) = &status.issued_to {
                        println!("  Issued:  {org}");
                    }
                } else {
                    println!("  Status:  {}", "Invalid".red().bold());
                }
                println!("  Message: {}", status.message);
            }
        }
    }

    Ok(())
}
