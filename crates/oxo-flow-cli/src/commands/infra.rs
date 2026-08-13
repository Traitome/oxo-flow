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
        EnvAction::Create {
            spec,
            name,
            ai,
            backend,
        } => {
            // AI mode: SPEC is a natural-language description, not a file path.
            if ai {
                return create_env_from_ai(&spec, name, &backend).await;
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
                    // The generated spec is itself a complete pixi project
                    // manifest ([project] + [dependencies]) — install it
                    // directly. `pixi init` would create an empty project
                    // and install nothing.
                    let pixi_toml = std::fs::read_to_string(&spec)
                        .with_context(|| format!("cannot read pixi spec: {}", spec.display()))?;

                    let project_dir = std::env::temp_dir().join(format!("oxo-flow-{}", name_str));
                    std::fs::create_dir_all(&project_dir)?;
                    std::fs::write(project_dir.join("pixi.toml"), &pixi_toml)
                        .with_context(|| "failed to write pixi.toml")?;

                    let status = std::process::Command::new("pixi")
                        .args(["install"])
                        .current_dir(&project_dir)
                        .status()
                        .with_context(|| "failed to run pixi install")?;

                    if !status.success() {
                        anyhow::bail!("pixi install failed");
                    }

                    eprintln!(
                        "  {} Pixi project created at: {}",
                        "✓".green(),
                        project_dir.display()
                    );
                    eprintln!(
                        "  Activate with: cd {} && pixi shell",
                        project_dir.display()
                    );
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

/// Generate an environment spec (conda YAML or pixi TOML) from a
/// natural-language description using the AI provider + built-in tool table.
async fn create_env_from_ai(
    description: &std::path::Path,
    name: Option<String>,
    backend: &str,
) -> Result<()> {
    let description = description.to_string_lossy().to_string();
    let provider = crate::commands::ai_template::resolve_ai_provider()?;

    let is_pixi = backend.eq_ignore_ascii_case("pixi");
    if !is_pixi && !backend.eq_ignore_ascii_case("conda") {
        anyhow::bail!("unsupported backend '{}'. Use 'conda' or 'pixi'.", backend);
    }

    println!("{}", "AI Environment Generator".bold().green());
    println!(
        "  Model: {}",
        provider.model().unwrap_or_else(|| "default".into())
    );
    println!("  Backend: {}", if is_pixi { "pixi" } else { "conda" });
    println!("  Description: {description}\n");

    // Pre-resolve tools mentioned in the description against the embedded
    // Bioconda database — inject real names + current versions so the model
    // pins accurately instead of guessing. Generic words and loose summary
    // matches are filtered out so unrelated tools don't pollute the prompt.
    const STOP_WORDS: &[&str] = &[
        "and", "the", "for", "with", "using", "into", "from", "your", "pipeline", "workflow",
        "analysis", "data", "files", "output", "input", "all", "new", "this", "that", "via",
        "then", "also", "based", "need", "want", "please",
    ];
    let mut matched = String::new();
    let mut seen = std::collections::HashSet::new();
    for word in description
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.to_ascii_lowercase().as_str()))
    {
        let word_lower = word.to_ascii_lowercase();
        for tool in oxo_flow_ai::knowledge::bioconda::search_tools(word, 3) {
            // Keep only matches where the tool name actually contains the
            // word — summary-only matches are too loose to inject.
            if !tool.name.to_ascii_lowercase().contains(&word_lower) {
                continue;
            }
            if seen.insert(tool.name.clone()) {
                matched.push_str(&format!(
                    "- {} {} — {}\n",
                    tool.name,
                    tool.version,
                    if tool.summary.is_empty() {
                        "(no description)"
                    } else {
                        &tool.summary
                    }
                ));
            }
        }
    }
    if !matched.is_empty() {
        eprintln!("{}", "  Matched Bioconda tools:".bold().cyan());
        for line in matched.lines() {
            eprintln!("    {line}");
        }
    }

    let system = if is_pixi {
        format!(
            r#"## Role
You are a bioinformatics environment specialist. Generate a pixi environment
TOML file from the user's natural-language description.

## Tool Reference
{}

## Matched Bioconda Tools (real names + current versions)
{}

## Output Requirements
Generate ONLY valid pixi.toml inside ```toml code fences:

```toml
[project]
name = "env-name"
channels = ["conda-forge", "bioconda"]
platforms = ["linux-64"]

[dependencies]
tool = "version"
```

Rules:
- Pin every tool version using the Matched Bioconda Tools above when present; otherwise use your knowledge of current stable versions
- Include all tools the user mentions; add common companions if clearly needed
- channels order: conda-forge first, bioconda second (Bioconda's recommended channel setup)
- Do NOT add comments beyond the file header
"#,
            oxo_flow_ai::knowledge::builtin::format_tool_table(),
            if matched.is_empty() {
                "(none matched)"
            } else {
                &matched
            }
        )
    } else {
        format!(
            r#"## Role
You are a bioinformatics environment specialist. Generate a conda environment
YAML file from the user's natural-language description.

## Tool Reference
{}

## Matched Bioconda Tools (real names + current versions)
{}

## Output Requirements
Generate ONLY valid conda environment YAML inside ```yaml code fences:

```yaml
# envs/<name>.yaml
name: <kebab-case-env-name>
channels:
  - conda-forge
  - bioconda
dependencies:
  - <tool>=<pinned-version>
```

Rules:
- Pin every tool version using the Matched Bioconda Tools above when present; otherwise use your knowledge of current stable versions
- Include all tools the user mentions; add common companions if clearly needed
- channels order: conda-forge first, bioconda second (Bioconda's recommended channel setup)
- Do NOT add comments beyond the file header
"#,
            oxo_flow_ai::knowledge::builtin::format_tool_table(),
            if matched.is_empty() {
                "(none matched)"
            } else {
                &matched
            }
        )
    };

    println!("{}", "  Generating...".bold().cyan());
    use oxo_flow_ai::types::Message;
    let messages = vec![Message::system(&system), Message::user(&description)];
    let response = provider.chat_with_tools(&messages, &[]).await?;
    let response_text = response.content.unwrap_or_default();

    // Extract spec from code fence (YAML for conda, TOML for pixi)
    let spec_content = if is_pixi {
        extract_toml(&response_text)
            .ok_or_else(|| anyhow::anyhow!("AI response did not contain valid pixi TOML"))?
    } else {
        extract_yaml(&response_text)
            .ok_or_else(|| anyhow::anyhow!("AI response did not contain valid conda YAML"))?
    };

    // Basic structural validation
    let has_deps = spec_content.contains("dependencies");
    if !has_deps {
        anyhow::bail!("generated spec missing 'dependencies' section");
    }

    // Determine output path: -n <name> → envs/<name>.<ext>
    let ext = if is_pixi { "toml" } else { "yaml" };
    let env_name = name.unwrap_or_else(|| {
        let name_key = if is_pixi {
            spec_content.lines().find_map(|l| l.strip_prefix("name = "))
        } else {
            spec_content.lines().find_map(|l| l.strip_prefix("name:"))
        };
        name_key
            .map(|n| n.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "generated".to_string())
    });
    let out_path = std::path::PathBuf::from("envs").join(format!("{env_name}.{ext}"));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &spec_content)?;

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

/// Extract TOML content from an AI response (```toml fence or raw).
fn extract_toml(response: &str) -> Option<String> {
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            let content = response[start..start + end].trim().to_string();
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    if let Some(pos) = response.find("[project]") {
        return Some(response[pos..].trim().to_string());
    }
    None
}
