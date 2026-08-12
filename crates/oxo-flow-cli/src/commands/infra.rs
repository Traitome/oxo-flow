use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use std::path::PathBuf;

use crate::commands::print_banner;

use crate::{ConfigAction, EnvAction, ProfileAction};

pub async fn env_command(action: EnvAction) -> Result<()> {
    print_banner();
    match action {
        EnvAction::List { workflow } => match workflow {
            Some(wf_path) => {
                let config = WorkflowConfig::from_file(&wf_path)
                    .with_context(|| format!("failed to parse {}", wf_path.display()))?;
                eprintln!(
                    "{} {}",
                    "Environments in".bold(),
                    wf_path.display().to_string().dimmed()
                );
                let mut seen = std::collections::HashSet::new();
                for rule in &config.rules {
                    let kind = rule.environment.kind();
                    let spec = kind.to_string();
                    if seen.insert(format!("{}:{spec}", rule.name)) {
                        eprintln!("  {} {} [{}]", "✓".green(), rule.name, spec);
                    }
                }
                if seen.is_empty() {
                    eprintln!(
                        "  (no environment specifications found — rules will use system environment)"
                    );
                }
            }
            None => {
                let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
                let available = resolver.available_backends();
                eprintln!("{}", "Available environment backends:".bold());
                for backend in available {
                    eprintln!("  {} {}", "✓".green(), backend);
                }
            }
        },
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
        EnvAction::Create { spec, name, ai } => {
            // AI mode: SPEC is a natural-language description, not a file path.
            if ai {
                return create_env_from_ai(&spec, name).await;
            }
            let name_str = name.clone().unwrap_or_else(|| {
                spec.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });

            // Determine environment type from file extension or content
            let ext = spec.extension().and_then(|e| e.to_str()).unwrap_or("");
            let backend = match ext {
                "yaml" | "yml" => "conda",
                "toml" => "pixi",
                "lock" => "conda",
                _ => {
                    eprintln!(
                        "{} Unknown environment spec format: '{}'",
                        "Warning:".yellow(),
                        spec.display()
                    );
                    eprintln!(
                        "  Supported formats: .yaml/.yml (conda), .toml (pixi), .lock (conda-lock)"
                    );
                    anyhow::bail!("Unsupported environment spec format");
                }
            };

            eprintln!(
                "{} Creating {} environment '{}' from '{}'...",
                "Info:".bold().cyan(),
                backend,
                name_str,
                spec.display()
            );

            match backend {
                "conda" => {
                    // Use conda/mamba to create environment
                    // Prefer mamba for speed, fall back to conda
                    let tool = {
                        let mamba_exists = std::process::Command::new("mamba")
                            .arg("--version")
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .is_ok();
                        let micromamba_exists = std::process::Command::new("micromamba")
                            .arg("--version")
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .is_ok();
                        if mamba_exists {
                            "mamba"
                        } else if micromamba_exists {
                            "micromamba"
                        } else {
                            "conda"
                        }
                    };

                    let status = std::process::Command::new(tool)
                        .args([
                            "env",
                            "create",
                            "-f",
                            &spec.to_string_lossy(),
                            "-n",
                            &name_str,
                        ])
                        .status()
                        .with_context(|| format!("failed to run {} env create", tool))?;

                    if !status.success() {
                        anyhow::bail!(
                            "{} env create failed with exit code {:?}",
                            tool,
                            status.code()
                        );
                    }
                    eprintln!(
                        "  {} Environment '{}' created successfully.",
                        "✓".green(),
                        name_str
                    );
                    eprintln!("  Activate with: conda activate {}", name_str);
                }
                "pixi" => {
                    // For pixi, create a project using the spec as a base
                    let _pixi_toml = std::fs::read_to_string(&spec)
                        .with_context(|| format!("cannot read pixi spec: {}", spec.display()))?;

                    // Create a temporary directory for pixi project
                    let temp_dir = std::env::temp_dir().join(format!("oxo-flow-{}", name_str));
                    std::fs::create_dir_all(&temp_dir)?;

                    let status = std::process::Command::new("pixi")
                        .args(["init", "-q"])
                        .current_dir(&temp_dir)
                        .status()
                        .with_context(|| "failed to run pixi init")?;

                    if !status.success() {
                        anyhow::bail!("pixi init failed");
                    }

                    // Install packages from the spec
                    let status = std::process::Command::new("pixi")
                        .args(["install"])
                        .current_dir(&temp_dir)
                        .status()
                        .with_context(|| "failed to run pixi install")?;

                    if !status.success() {
                        anyhow::bail!("pixi install failed");
                    }

                    eprintln!(
                        "  {} Pixi project created at: {}",
                        "✓".green(),
                        temp_dir.display()
                    );
                    eprintln!("  Activate with: cd {} && pixi shell", temp_dir.display());
                }
                other => {
                    anyhow::bail!(
                        "Environment backend '{}' not supported for env create yet",
                        other
                    );
                }
            }
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
            "docker" => oxo_flow_core::container::ContainerFormat::Docker,
            "singularity" => oxo_flow_core::container::ContainerFormat::Singularity,
            "compose" => oxo_flow_core::container::ContainerFormat::Compose,
            other => anyhow::bail!(
                "unsupported package format '{}'. Supported formats: docker, singularity, compose",
                other
            ),
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
        oxo_flow_core::container::ContainerFormat::Compose => {
            oxo_flow_core::container::generate_compose_file(&config, &pkg_config)?
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

// ── AI-powered environment spec generation ────────────────────────────────

/// Generate a conda environment YAML from a natural-language description
/// using the AI provider + built-in tool reference table.
async fn create_env_from_ai(description: &std::path::Path, name: Option<String>) -> Result<()> {
    let description = description.to_string_lossy().to_string();
    let provider = crate::commands::ai_template::resolve_ai_provider()?;

    println!("{}", "AI Environment Generator".bold().green());
    println!(
        "  Model: {}",
        provider.model().unwrap_or_else(|| "default".into())
    );
    println!("  Description: {description}\n");

    let system = format!(
        r#"## Role
You are a bioinformatics environment specialist. Generate a conda environment
YAML file from the user's natural-language description.

## Tool Reference
{}

## Output Requirements
Generate ONLY valid conda environment YAML inside ```yaml code fences:

```yaml
# envs/<name>.yaml
name: <kebab-case-env-name>
channels:
  - bioconda
  - conda-forge
dependencies:
  - <tool>=<pinned-version>
```

Rules:
- Pin EVERY tool version (from the reference table where available)
- Include all tools the user mentions; add common companions if clearly needed
- channels order: bioconda first, conda-forge second
- Do NOT add comments beyond the file header
"#,
        oxo_flow_ai::knowledge::builtin::format_tool_table()
    );

    println!("{}", "  Generating...".bold().cyan());
    use oxo_flow_ai::types::Message;
    let messages = vec![Message::system(&system), Message::user(&description)];
    let response = provider.chat_with_tools(&messages, &[]).await?;
    let response_text = response.content.unwrap_or_default();

    // Extract YAML from code fence
    let yaml_content = extract_yaml(&response_text)
        .ok_or_else(|| anyhow::anyhow!("AI response did not contain valid conda YAML"))?;

    // Basic structural validation
    if !yaml_content.contains("dependencies") {
        anyhow::bail!("generated YAML missing 'dependencies' section");
    }

    // Determine output path: -n <name> → envs/<name>.yaml; else envs/generated.yaml
    let env_name = name.unwrap_or_else(|| {
        yaml_content
            .lines()
            .find_map(|l| l.strip_prefix("name:"))
            .map(|n| n.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "generated".to_string())
    });
    let out_path = std::path::PathBuf::from("envs").join(format!("{env_name}.yaml"));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &yaml_content)?;

    println!(
        "{} Environment spec written to {}",
        "✓".green(),
        out_path.display()
    );
    println!();
    println!(
        "  Review the spec: {}",
        out_path.display().to_string().dimmed()
    );
    println!(
        "  Then create it with: oxo-flow env create {}",
        out_path.display().to_string().dimmed()
    );
    Ok(())
}

/// Extract YAML content from an AI response (```yaml fence or raw).
fn extract_yaml(response: &str) -> Option<String> {
    if let Some(start) = response.find("```yaml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            let content = response[start..start + end].trim().to_string();
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    if let Some(pos) = response.find("name:") {
        return Some(response[pos..].trim().to_string());
    }
    None
}
