use crate::commands::{print_banner, resolve_workflow};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{CheckpointState, ExecutorConfig, LocalExecutor};
use oxo_flow_core::rule::parse_duration_secs;
use std::collections::HashMap;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    workflow: Option<PathBuf>,
    jobs: usize,
    keep_going: bool,
    workdir: Option<PathBuf>,
    target: Vec<String>,
    retry: u32,
    timeout: String,
    resume_failed: bool,
    profile: Option<String>,
    max_threads: u32,
    max_memory: u64,
    skip_env_setup: bool,
    cache_dir: Option<PathBuf>,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();

    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    let dag = WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

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

    // Load profile if specified and merge config values.
    if let Some(ref profile_name) = profile {
        let profile_paths = [
            workflow_dir
                .join("profiles")
                .join(format!("{profile_name}.toml")),
            workflow_dir
                .join("profiles")
                .join(format!("{profile_name}.oxoflow")),
        ];
        let profile_path = profile_paths.iter().find(|p| p.exists());
        if let Some(path) = profile_path {
            let profile_content = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read profile {}", path.display()))?;
            let profile_toml: toml::Value = profile_content
                .parse()
                .with_context(|| format!("failed to parse profile {}", path.display()))?;
            if let Some(config_table) = profile_toml.get("config").and_then(toml::Value::as_table) {
                for (key, value) in config_table {
                    config
                        .config
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
                eprintln!(
                    "{} Merged {} config values from profile '{}'",
                    "Profile:".bold().cyan(),
                    config_table.len(),
                    profile_name
                );
            }
        } else {
            eprintln!(
                "{} Profile '{}' not found in profiles/ directory",
                "Warning:".bold().yellow(),
                profile_name
            );
        }
    }
    for (i, rule_name) in order.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, rule_name);
    }

    let progress = indicatif::ProgressBar::new(order.len() as u64);
    progress.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({msg})",
            )?
            .progress_chars("#>-"),
    );

    let timeout_secs: u64 = if timeout == "0" {
        0
    } else if let Ok(n) = timeout.parse::<u64>() {
        n
    } else {
        parse_duration_secs(&timeout).unwrap_or_else(|| {
            eprintln!(
                "{} Invalid timeout format '{}', defaulting to no timeout",
                "Warning:".bold().yellow(),
                timeout
            );
            0
        })
    };

    let exec_config = ExecutorConfig {
        max_jobs: jobs,
        dry_run: false,
        workdir: workdir.clone().unwrap_or_else(|| workflow_dir.clone()),
        keep_going,
        retry_count: retry,
        timeout: if timeout_secs > 0 {
            Some(std::time::Duration::from_secs(timeout_secs))
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
        resource_groups: config
            .resource_groups
            .iter()
            .map(|(k, v)| (k.clone(), v.max))
            .collect(),
        skip_env_setup,
        cache_dir,
        interpreter_map: config.workflow.interpreter_map.clone(),
    };

    let executor = LocalExecutor::new(exec_config);
    let mut success_count = 0;
    let mut fail_count = 0;
    let mut skipped_count = 0;
    let mut completed_rules = std::collections::HashSet::new();

    let checkpoint_path = workdir
        .as_ref()
        .unwrap_or(&workflow_dir)
        .join(".oxo-flow/checkpoint.json");
    let mut checkpoint = if checkpoint_path.exists() {
        CheckpointState::load_from_file(&checkpoint_path).unwrap_or_default()
    } else {
        CheckpointState::default()
    };

    // When --resume-failed is set, clear failed rules from checkpoint so they re-execute.
    if resume_failed && checkpoint_path.exists() {
        let failed_count = checkpoint.failed_rules.len();
        let completed_count = checkpoint.completed_rules.len();
        checkpoint.failed_rules.clear();
        eprintln!(
            "{} Resuming {} completed, re-running {} failed rules",
            "Resume:".bold().cyan(),
            completed_count,
            failed_count
        );
    }

    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    for rule in config.rules.iter() {
        if !order.contains(&rule.name) {
            continue;
        }

        if let Some(ref condition) = rule.when {
            let config_values: HashMap<String, toml::Value> = config.config.clone();
            if !oxo_flow_core::executor::process::evaluate_condition(condition, &config_values) {
                skipped_count += 1;
                completed_rules.insert(rule.name.clone());
                continue;
            }
        }
    }

    for rule_name in &order {
        if completed_rules.contains(rule_name) {
            progress.inc(1);
            continue;
        }

        if checkpoint.is_completed(rule_name) {
            skipped_count += 1;
            progress.set_message("skipping already completed");
            progress.inc(1);
            continue;
        }

        let rule = config.get_rule(rule_name).unwrap().clone();
        progress.set_message(format!("executing {}", rule_name));

        match executor.execute_rule(&rule, &wildcard_values).await {
            Ok(record) => {
                let duration = record
                    .finished_at
                    .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
                    .map(|d| d.num_milliseconds() as f64 / 1000.0)
                    .unwrap_or(0.0);

                if record.status == oxo_flow_core::executor::JobStatus::Success {
                    success_count += 1;
                    let benchmark = oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                        rule: rule_name.clone(),
                        wall_time_secs: duration,
                        max_memory_mb: None,
                        cpu_seconds: None,
                    };
                    checkpoint.mark_completed(rule_name, benchmark);
                    let _ = checkpoint.save_to_file(&checkpoint_path);
                } else if record.status == oxo_flow_core::executor::JobStatus::Skipped {
                    skipped_count += 1;
                } else {
                    fail_count += 1;
                    checkpoint.mark_failed(rule_name);
                    let _ = checkpoint.save_to_file(&checkpoint_path);
                    if !keep_going {
                        progress.finish_and_clear();
                        return Err(anyhow::anyhow!("rule '{}' failed", rule_name));
                    }
                }
            }
            Err(e) => {
                fail_count += 1;
                checkpoint.mark_failed(rule_name);
                let _ = checkpoint.save_to_file(&checkpoint_path);
                if !keep_going {
                    progress.finish_and_clear();
                    return Err(e.into());
                }
            }
        }
        progress.inc(1);
    }

    progress.finish_and_clear();
    eprintln!(
        "\n{} {} succeeded, {} skipped, {} failed",
        "Done:".bold(),
        success_count,
        skipped_count,
        fail_count
    );

    if fail_count > 0 && !keep_going {
        return Err(anyhow::anyhow!("workflow execution failed"));
    }

    Ok(())
}

pub async fn dry_run_command(
    workflow: Option<PathBuf>,
    target: Vec<String>,
    verbose: bool,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    let dag = WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

    let order = if target.is_empty() {
        dag.execution_order()?
    } else {
        let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
        dag.execution_order_for_targets(&target_refs)
            .with_context(|| "failed to resolve target rules")?
    };

    eprintln!(
        "{} (dry-run) {} rules would execute",
        "DAG:".bold().yellow(),
        order.len()
    );

    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    for (i, rule_name) in order.iter().enumerate() {
        let rule = config.get_rule(rule_name).unwrap();
        eprintln!("  {}. {}", i + 1, rule_name.bold().cyan());

        let threads = rule.effective_threads();
        eprintln!("     threads={}", threads);

        if !rule.environment.is_empty() {
            eprintln!("     env={}", rule.environment.kind());
        }

        if !rule.output.is_empty() {
            let expanded_outputs: Vec<String> = rule
                .output
                .iter()
                .map(|o| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(o, &wildcard_values)
                })
                .collect();
            eprintln!("     outputs: {:?}", expanded_outputs);
        }

        if let Some(ref cmd) = rule.shell {
            let expanded =
                oxo_flow_core::executor::process::render_shell_command(cmd, rule, &wildcard_values);
            eprintln!("     command: {}", expanded);
        }

        if verbose {
            // Additional verbose info
        }
    }

    Ok(())
}

pub async fn debug_command(workflow: PathBuf, rule_name: Option<String>) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    oxo_flow_core::config::resolve_rule_templates(&mut config.rules)
        .context("failed to resolve rule templates")?;

    let dag = WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

    let rules_to_show: Vec<&oxo_flow_core::rule::Rule> = if let Some(ref name) = rule_name {
        match config.rules.iter().find(|r| r.name == *name) {
            Some(r) => vec![r],
            None => {
                eprintln!("{} rule '{}' not found", "error:".bold().red(), name);
                return Err(anyhow::anyhow!("rule not found"));
            }
        }
    } else {
        config.rules.iter().collect()
    };

    eprintln!(
        "{} Debugging {} rules",
        "Debug:".bold().cyan(),
        rules_to_show.len()
    );

    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    for rule in &rules_to_show {
        eprintln!("{}", format!("── Rule: {} ──", rule.name).bold().cyan());

        if let Some(ref desc) = rule.description {
            eprintln!("  {} {}", "Description:".dimmed(), desc);
        }

        if !rule.output.is_empty() {
            let expanded_outputs: Vec<String> = rule
                .output
                .iter()
                .map(|o| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(o, &wildcard_values)
                })
                .collect();
            eprintln!("  {} {:?}", "Outputs:".dimmed(), expanded_outputs);
        }

        if let Some(ref cmd) = rule.shell {
            let expanded =
                oxo_flow_core::executor::process::render_shell_command(cmd, rule, &wildcard_values);
            eprintln!("  {} {}", "Shell (expanded):".dimmed(), expanded);
        }

        if let Ok(deps) = dag.dependencies(&rule.name)
            && !deps.is_empty()
        {
            eprintln!("  {} {:?}", "Dependencies:".dimmed(), deps);
        }

        eprintln!();
    }

    Ok(())
}

pub async fn handle_status(checkpoint_path: PathBuf) -> Result<()> {
    print_banner();

    // Detect common mistake: user passes a .oxoflow file instead of checkpoint
    if checkpoint_path
        .extension()
        .is_some_and(|ext| ext == "oxoflow")
    {
        eprintln!(
            "{} '{}' appears to be a workflow file, not a checkpoint.",
            "Warning:".bold().yellow(),
            checkpoint_path.display()
        );
        eprintln!(
            "  The 'status' command expects a checkpoint file (e.g., .oxo-flow/checkpoint.json)."
        );
        eprintln!(
            "  Run 'oxo-flow run {}' first to generate a checkpoint.",
            checkpoint_path.display()
        );
        anyhow::bail!("Cannot read workflow file as checkpoint");
    }

    let state = CheckpointState::load_from_file(&checkpoint_path).with_context(|| {
        format!(
            "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).\n  \
             Checkpoint files are generated automatically by 'oxo-flow run' in .oxo-flow/checkpoint.json.",
            checkpoint_path.display()
        )
    })?;

    eprintln!(
        "{} Status for checkpoint: {}",
        "Status:".bold().cyan(),
        checkpoint_path.display()
    );
    eprintln!("  Completed: {}", state.completed_rules.len());
    eprintln!("  Failed:    {}", state.failed_rules.len());

    if !state.completed_rules.is_empty() {
        eprintln!("\n{}", "Completed rules:".bold().green());
        for rule in &state.completed_rules {
            eprintln!("  {} {}", "✓".green(), rule);
        }
    }

    if !state.failed_rules.is_empty() {
        eprintln!("\n{}", "Failed rules:".bold().red());
        for rule in &state.failed_rules {
            eprintln!("  {} {}", "✗".red(), rule);
        }
    }

    Ok(())
}

pub async fn resume_command(checkpoint: Option<PathBuf>, jobs: usize) -> Result<()> {
    print_banner();
    eprintln!(
        "{} The 'resume' command is not yet fully implemented as a standalone command.",
        "Note:".bold().cyan()
    );
    eprintln!("  By default, 'oxo-flow run' will automatically resume if a checkpoint exists.");

    if let Some(path) = checkpoint {
        eprintln!("  Resuming from: {} with {} jobs", path.display(), jobs);
    }
    Ok(())
}
