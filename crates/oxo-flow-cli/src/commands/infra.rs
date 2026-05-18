use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use std::path::PathBuf;

use crate::commands::print_banner;

use crate::{ConfigAction, EnvAction, ProfileAction};

pub fn env_command(action: EnvAction) -> Result<()> {
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
                    let config = WorkflowConfig::from_file(&wf_path)
                        .with_context(|| format!("failed to parse {}", wf_path.display()))?;

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
                        oxo_flow_core::environment::EnvironmentResolver::all_known_backends()
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
        EnvAction::Create { spec, name } => {
            let name_str = name.unwrap_or_else(|| "default".to_string());
            eprintln!(
                "{} Creating environment '{}' from '{}' is not yet implemented.",
                "Note:".bold().cyan(),
                name_str,
                spec.display()
            );
            eprintln!("  Environments are currently created automatically during 'oxo-flow run'.");
        }
    }
    Ok(())
}

pub fn handle_config(action: ConfigAction) -> Result<()> {
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

            eprintln!("\n{}", "Config Variables:".bold());
            if config.config.is_empty() {
                eprintln!("  (none)");
            } else {
                for (k, v) in &config.config {
                    eprintln!("  {} = {}", k, v);
                }
            }
        }
        ConfigAction::Stats { workflow } => {
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let stats = oxo_flow_core::format::workflow_stats(&config);
            eprintln!("{}", "Workflow Statistics:".bold());
            eprintln!("  Workflow:           {}", config.workflow.name);
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
        ConfigAction::Get { workflow, key } => {
            let config = WorkflowConfig::from_file(&workflow)?;
            if let Some(val) = config.config.get(&key) {
                println!("{}", val);
            } else {
                return Err(anyhow::anyhow!("config key '{}' not found", key));
            }
        }
    }
    Ok(())
}

pub fn package_command(workflow: PathBuf, format: String, output: Option<PathBuf>) -> Result<()> {
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
    Ok(())
}

pub fn profile_command(action: ProfileAction) -> Result<()> {
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
            "local" | "default" => {
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
    Ok(())
}
