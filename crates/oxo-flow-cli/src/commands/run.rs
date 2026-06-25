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
    provenance: bool,
    json: bool,
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

    // indicatif's stderr draw target auto-hides when stderr is not a terminal,
    // which makes every per-rule progress message silently disappear under pipes,
    // redirects, nohup, CI, or schedulers. When that happens, fall back to plain
    // eprintln lines so the run is never silent.
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());

    let progress = indicatif::ProgressBar::new(order.len() as u64);
    progress.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ETA:{eta} ({msg})",
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

    // Fail fast if any rule's declared request can never fit an explicit
    // --max-memory / --max-threads cap. Otherwise the run would execute earlier
    // rules and only discover the impossible one mid-pipeline.
    let scheduled: Vec<&oxo_flow_core::rule::Rule> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .collect();
    let breaches = oxo_flow_core::scheduler::check_budget_feasibility(
        &scheduled,
        exec_config.max_threads,
        exec_config.max_memory_mb,
    );
    if !breaches.is_empty() {
        progress.finish_and_clear();
        let detail = breaches
            .iter()
            .map(|b| format!("  - {b}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow::anyhow!(
            "resource budget too small for {} rule(s); no rules were run:\n{}",
            breaches.len(),
            detail
        ));
    }

    let executor = LocalExecutor::new(exec_config);
    let mut success_count = 0;
    let mut fail_count = 0;
    let mut skipped_count = 0;
    let mut completed_rules = std::collections::HashSet::new();
    let mut failed_rules_set = std::collections::HashSet::new();
    // Collected so `--keep-going` can print a consolidated failure summary at the
    // end instead of leaving the user to scroll back through interleaved output.
    let mut failures: Vec<(String, String)> = Vec::new();

    let checkpoint_path = workdir
        .as_ref()
        .unwrap_or(&workflow_dir)
        .join(".oxo-flow/checkpoint.json");
    let mut checkpoint = if checkpoint_path.exists() {
        CheckpointState::load_from_file(&checkpoint_path).unwrap_or_default()
    } else {
        CheckpointState::default()
    };

    // Store workflow path in checkpoint for resume support
    checkpoint.set_workflow_path(&workflow);

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

    let total_rules = order.len();
    for (idx, rule_name) in order.iter().enumerate() {
        if completed_rules.contains(rule_name) {
            progress.inc(1);
            continue;
        }

        if checkpoint.is_completed(rule_name) {
            // Verify output files still exist before skipping — user may have
            // cleaned the output directory since the last run.
            let mut outputs_exist = true;
            if let Some(rule) = config.get_rule(rule_name) {
                let workdir_actual = workdir.as_ref().unwrap_or(&workflow_dir);
                for output in &rule.output {
                    if !output.contains('{') {
                        let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            output,
                            &wildcard_values,
                        );
                        if !workdir_actual.join(&expanded).exists() {
                            outputs_exist = false;
                            tracing::info!(
                                rule = rule_name,
                                output = expanded,
                                "Output file missing, will re-execute rule"
                            );
                            break;
                        }
                    }
                }
            }
            if outputs_exist {
                skipped_count += 1;
                progress.set_message("skipping already completed");
                if !is_tty {
                    eprintln!("  {} {} (already completed)", "⊝".dimmed(), rule_name);
                }
                progress.inc(1);
                continue;
            }
        }

        // Check if any upstream dependency failed — skip with clear message
        if !failed_rules_set.is_empty()
            && let Ok(deps) = dag.dependencies(rule_name)
            && deps.iter().any(|d| failed_rules_set.contains(d.as_str()))
        {
            let failed_deps: Vec<_> = deps
                .iter()
                .filter(|d| failed_rules_set.contains(d.as_str()))
                .collect();
            skipped_count += 1;
            let failed_deps_str = failed_deps
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            progress.set_message(format!(
                "skipping {} (dependency failed: {})",
                rule_name, failed_deps_str
            ));
            if !is_tty {
                eprintln!(
                    "  {} {} (dependency failed: {})",
                    "⊝".yellow(),
                    rule_name,
                    failed_deps_str
                );
            }
            progress.inc(1);
            if !keep_going {
                // Build a clear error about the cascade
                progress.finish_and_clear();
                return Err(anyhow::anyhow!(
                    "Stopping: rule '{}' depends on failed rule '{}'",
                    rule_name,
                    failed_deps[0]
                ));
            }
            tracing::warn!(
                rule = rule_name,
                failed_deps = ?failed_deps,
                "Skipping rule because upstream dependency failed"
            );
            continue;
        }

        let rule = config
            .get_rule(rule_name)
            .ok_or_else(|| anyhow::anyhow!("rule '{}' not found in workflow", rule_name))?
            .clone();
        progress.set_message(format!("executing {}", rule_name));
        if !is_tty {
            eprintln!(
                "{} [{}/{}] {}",
                "Running:".bold().cyan(),
                idx + 1,
                total_rules,
                rule_name
            );
        }

        match executor.execute_rule(&rule, &wildcard_values).await {
            Ok(record) => {
                let duration = record
                    .finished_at
                    .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
                    .map(|d| d.num_milliseconds() as f64 / 1000.0)
                    .unwrap_or(0.0);

                if record.status == oxo_flow_core::executor::JobStatus::Success {
                    success_count += 1;
                    if !is_tty {
                        eprintln!("  {} {} ({:.1}s)", "✓".green(), rule_name, duration);
                    }
                    let benchmark = oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                        rule: rule_name.clone(),
                        wall_time_secs: duration,
                        max_memory_mb: None,
                        cpu_seconds: None,
                        retries: record.retries,
                    };
                    checkpoint.mark_completed(rule_name, benchmark);
                    if provenance {
                        for output in &rule.output {
                            let output_path =
                                workdir.as_ref().unwrap_or(&workflow_dir).join(output);
                            if output_path.exists()
                                && let Ok(checksum) =
                                    oxo_flow_core::executor::checkpoint::compute_file_checksum(
                                        &output_path,
                                    )
                            {
                                checkpoint.record_checksum(output, checksum);
                            }
                        }
                    }
                    if let Err(e) = checkpoint.save_to_file(&checkpoint_path) {
                        tracing::warn!("Failed to save checkpoint: {e}");
                    }
                } else if record.status == oxo_flow_core::executor::JobStatus::Skipped {
                    skipped_count += 1;
                } else {
                    fail_count += 1;
                    failed_rules_set.insert(rule_name.clone());
                    checkpoint.mark_failed(rule_name);
                    if let Err(e) = checkpoint.save_to_file(&checkpoint_path) {
                        tracing::warn!("Failed to save checkpoint: {e}");
                    }
                    // Build detailed error message with stderr and exit code
                    let mut err_msg = format!("rule '{}' failed", rule_name);
                    if let Some(ref stderr) = record.stderr {
                        let trimmed = stderr.trim();
                        if !trimmed.is_empty() {
                            err_msg.push_str(&format!("\nstderr: {}", trimmed));
                        }
                    }
                    if let Some(code) = record.exit_code {
                        err_msg.push_str(&format!("\nexit code: {}", code));
                    }
                    if let Some(ref cmd) = record.command {
                        err_msg.push_str(&format!("\ncommand: {}", cmd));
                    }
                    if !keep_going {
                        progress.finish_and_clear();
                        return Err(anyhow::anyhow!(err_msg));
                    }
                    // In keep_going mode, still print the error and record a concise
                    // one-line reason for the end-of-run failure summary.
                    eprintln!("  {} {}", "✗".red(), err_msg);
                    let mut reason = String::new();
                    if let Some(code) = record.exit_code {
                        reason.push_str(&format!("exit code {}", code));
                    }
                    if let Some(ref stderr) = record.stderr
                        && let Some(last) =
                            stderr.trim().lines().next_back().filter(|l| !l.is_empty())
                    {
                        if !reason.is_empty() {
                            reason.push_str(" — ");
                        }
                        reason.push_str(last);
                    }
                    if reason.is_empty() {
                        reason.push_str("failed");
                    }
                    failures.push((rule_name.clone(), reason));
                }
            }
            Err(e) => {
                fail_count += 1;
                failed_rules_set.insert(rule_name.clone());
                checkpoint.mark_failed(rule_name);
                if let Err(e) = checkpoint.save_to_file(&checkpoint_path) {
                    tracing::warn!("Failed to save checkpoint: {e}");
                }
                if !keep_going {
                    progress.finish_and_clear();
                    return Err(e.into());
                }
                // Previously this branch swallowed the error entirely in keep_going
                // mode — surface it inline and record it for the summary.
                let reason = e.to_string();
                eprintln!("  {} rule '{}' failed: {}", "✗".red(), rule_name, reason);
                failures.push((rule_name.clone(), reason));
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

    // With --keep-going, execution continues past failures, so list every failed
    // rule (and why) in one place rather than making the user hunt for them.
    if !failures.is_empty() {
        eprintln!("\n{}", "Failed rules:".bold().red());
        for (name, reason) in &failures {
            eprintln!("  {} {} — {}", "✗".red(), name.bold(), reason);
        }
    }

    // Verify output files exist for completed rules
    if success_count > 0 {
        let workdir_actual = workdir.as_ref().unwrap_or(&workflow_dir);
        let mut missing_outputs = Vec::new();
        let mut verified = 0usize;
        let mut total_size: u64 = 0;
        for rule_name in &order {
            if checkpoint.is_completed(rule_name)
                && let Some(rule) = config.get_rule(rule_name)
            {
                for output in &rule.output {
                    if !output.contains('{') {
                        let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            output,
                            &wildcard_values,
                        );
                        let resolved = workdir_actual.join(&expanded);
                        if resolved.exists() {
                            verified += 1;
                            if let Ok(meta) = std::fs::metadata(&resolved) {
                                total_size += meta.len();
                            }
                        } else if !workdir_actual.join(output).exists() {
                            missing_outputs.push(format!("  {}: {}", rule_name, output));
                        } else {
                            verified += 1;
                        }
                    }
                }
            }
        }
        if verified > 0 {
            let size_str = if total_size > 1_073_741_824 {
                format!("{:.1}GB", total_size as f64 / 1_073_741_824.0)
            } else if total_size > 1_048_576 {
                format!("{:.1}MB", total_size as f64 / 1_048_576.0)
            } else {
                format!("{}B", total_size)
            };
            eprintln!(
                "{} {} output files verified ({} total)",
                "✓".green(),
                verified,
                size_str
            );
        }
        if !missing_outputs.is_empty() {
            eprintln!(
                "{} {} output file(s) were not found:",
                "⚠".yellow(),
                missing_outputs.len()
            );
            for m in &missing_outputs {
                eprintln!("{}", m.dimmed());
            }
        }
    }

    // JSON output mode
    if json {
        let wf_path = Some(workflow.to_string_lossy().to_string());
        let output = serde_json::json!({
            "command": "run",
            "status": if fail_count > 0 { "failed" } else { "completed" },
            "workflow": wf_path,
            "results": serde_json::json!({
                "succeeded": success_count,
                "skipped": skipped_count,
                "failed": fail_count,
            }),
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    if fail_count > 0 && !keep_going {
        return Err(anyhow::anyhow!("workflow execution failed"));
    }

    Ok(())
}

pub async fn dry_run_command(
    workflow: Option<PathBuf>,
    target: Vec<String>,
    verbose: bool,
    json: bool,
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
        let rule = config
            .get_rule(rule_name)
            .ok_or_else(|| anyhow::anyhow!("rule '{}' not found", rule_name))?;
        eprintln!("  {}. {}", i + 1, rule_name.bold().cyan());

        let threads = rule.effective_threads();
        eprintln!("     threads={}", threads);

        if !rule.environment.is_empty() {
            eprintln!("     env={}", rule.environment.kind());
        }

        if let Some(ref mem) = rule.effective_memory() {
            eprintln!("     memory={}", mem);
        }

        if rule.checkpoint {
            eprintln!("     checkpoint=true");
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

        // Show input file status for concrete (non-wildcard) paths
        for inp in &rule.input {
            let s = inp.to_string();
            if !s.contains('{') && !s.contains('*') && !s.starts_with('/') {
                let exists = std::path::Path::new(&s).exists();
                let icon = if exists { "✓" } else { "✗" };
                eprintln!("     input {}: {}", icon, s);
            }
        }

        if verbose {
            // Additional verbose info
        }
    }

    // Resource summary
    let total_threads: u32 = config.rules.iter().map(|r| r.effective_threads()).sum();
    let max_threads: u32 = config
        .rules
        .iter()
        .map(|r| r.effective_threads())
        .max()
        .unwrap_or(1);
    let memory_values: Vec<&str> = config
        .rules
        .iter()
        .filter_map(|r| r.effective_memory())
        .collect();
    eprintln!();
    eprintln!(
        "{} {} rules, total {} threads declared, max {} threads/rule",
        "Summary:".bold(),
        order.len(),
        total_threads,
        max_threads,
    );
    if !memory_values.is_empty() {
        eprintln!(
            "         {} rule(s) with memory requirements",
            memory_values.len()
        );
    }
    if !config.sample_groups.is_empty() {
        eprintln!(
            "         {} sample group(s), {} pair(s)",
            config.sample_groups.len(),
            config.pairs.len()
        );
    }

    let suggested_jobs = std::thread::available_parallelism()
        .map(|n| n.get().min(16).to_string())
        .unwrap_or_else(|_| "4".to_string());
    eprintln!(
        "\n{}  oxo-flow run {} -j {}",
        "To execute:".bold().cyan(),
        workflow.display(),
        suggested_jobs
    );

    // JSON output mode
    if json {
        let order_list = order.clone();
        let rule_list: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|name| config.get_rule(name))
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "threads": r.effective_threads(),
                    "environment": r.environment.kind(),
                    "memory": r.effective_memory(),
                    "checkpoint": r.checkpoint,
                })
            })
            .collect();

        let output = serde_json::json!({
            "command": "dry-run",
            "workflow": workflow.display().to_string(),
            "total_rules": order.len(),
            "execution_order": order_list,
            "rules": rule_list,
            "summary": {
                "total_threads": total_threads,
                "max_threads_per_rule": max_threads,
                "memory_rules": memory_values.len(),
            },
            "suggested_jobs": suggested_jobs,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    Ok(())
}

pub async fn debug_command(workflow: PathBuf, rule_name: Option<String>) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

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

pub async fn handle_status(checkpoint_path: PathBuf, json: bool) -> Result<()> {
    let _ = &json;
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

pub async fn resume_command(checkpoint: PathBuf, jobs: usize) -> Result<()> {
    // resume does not produce structured JSON output
    print_banner();

    // Load checkpoint state
    let state = CheckpointState::load_from_file(&checkpoint).with_context(|| {
        format!(
            "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).",
            checkpoint.display()
        )
    })?;

    // Get workflow path from checkpoint
    let workflow_path = match &state.workflow_path {
        Some(p) => PathBuf::from(p),
        None => {
            anyhow::bail!(
                "Checkpoint does not contain a workflow reference.\n  \
                 The checkpoint at '{}' was generated by an older version of oxo-flow.\n  \
                 To resume manually, run: oxo-flow run <workflow.oxoflow>\n  \
                 (oxo-flow run automatically resumes from the checkpoint)",
                checkpoint.display()
            );
        }
    };

    if !workflow_path.exists() {
        anyhow::bail!(
            "Workflow file '{}' referenced by checkpoint no longer exists.\n  \
             The workflow may have been moved or deleted.",
            workflow_path.display()
        );
    }

    let completed = state.completed_rules.len();
    let failed = state.failed_rules.len();

    eprintln!(
        "{} Resuming workflow '{}'",
        "Resume:".bold().cyan(),
        workflow_path.display()
    );
    eprintln!("  Checkpoint: {}", checkpoint.display());
    eprintln!(
        "  State: {} completed, {} failed, {} remaining",
        completed,
        failed,
        state
            .completed_rules
            .len()
            .saturating_sub(completed.saturating_sub(failed))
    );

    if completed == 0 && failed == 0 {
        eprintln!(
            "  {} No rules have been executed yet. Use 'oxo-flow run' instead.",
            "Note:".yellow()
        );
        return Ok(());
    }

    // Re-run the workflow — the checkpoint with completed rules will cause
    // already-finished rules to be skipped automatically
    eprintln!();
    eprintln!(
        "{} Launching executor with {} parallel job(s)...",
        "Info:".bold().cyan(),
        jobs
    );

    run_command(
        Some(workflow_path),
        jobs,
        false,           // keep_going
        None,            // workdir
        Vec::new(),      // target
        0,               // retry
        "0".to_string(), // timeout
        false,           // resume_failed (user can re-run with --resume-failed in 'run')
        None,            // profile
        0,               // max_threads
        0,               // max_memory
        false,           // skip_env_setup
        None,            // cache_dir
        false,           // provenance (checkpoint already has checksums)
        false,           // json (resume defaults to human-readable)
    )
    .await
}
