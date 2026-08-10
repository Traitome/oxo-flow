use crate::commands::{print_banner, resolve_workflow};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{CheckpointState, ExecutorConfig, LocalExecutor};
use oxo_flow_core::rule::parse_duration_secs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Validate a config value against its ConfigDef declaration.
fn validate_config_value(
    name: &str,
    value: &str,
    def: &oxo_flow_core::config::ConfigDef,
) -> anyhow::Result<()> {
    // ── choices ──
    if let Some(ref choices) = def.choices
        && !choices.iter().any(|c| c == value)
    {
        anyhow::bail!(
            "invalid value '{}' for config '{}'. Allowed: [{}]\n  {}",
            value,
            name,
            choices.join(", "),
            def.help.as_deref().unwrap_or("")
        );
    }

    // ── type validation ──
    if let Some(ref type_) = def.type_ {
        match type_.as_str() {
            "int" => {
                value.parse::<i64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects an integer, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "float" => {
                value.parse::<f64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects a float, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "bool" => {
                if value != "true" && value != "false" {
                    anyhow::bail!(
                        "config '{}' expects a boolean (true/false), got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    );
                }
            }
            "path" => {
                if value.is_empty() {
                    anyhow::bail!(
                        "config '{}' expects a path, got empty string\n  {}",
                        name,
                        def.help.as_deref().unwrap_or("")
                    );
                }
                // must_exist check
                if def.must_exist {
                    let p = std::path::Path::new(value);
                    if !p.exists() {
                        anyhow::bail!(
                            "config '{}' path does not exist: '{}'\n  {}",
                            name,
                            value,
                            def.help.as_deref().unwrap_or("")
                        );
                    }
                }
            }
            _ => { /* "string" or unknown — no validation */ }
        }
    }

    // ── range validation (requires type = int or float) ──
    if let Some(ref range) = def.range
        && let Some((min_str, max_str)) = range.split_once("..")
    {
        let min: f64 = min_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range min '{}' for config '{}'", min_str, name)
        })?;
        let max: f64 = max_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range max '{}' for config '{}'", max_str, name)
        })?;
        let val: f64 = value.parse().map_err(|_| {
            anyhow::anyhow!(
                "config '{}' has range '{range}' but value '{}' is not numeric\n  {}",
                name,
                value,
                def.help.as_deref().unwrap_or("")
            )
        })?;
        if val < min || val > max {
            anyhow::bail!(
                "config '{}' value {} is outside range {}..{}\n  {}",
                name,
                val,
                min,
                max,
                def.help.as_deref().unwrap_or("")
            );
        }
    }

    Ok(())
}

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
    skip_ref_build: bool,
    cache_dir: Option<PathBuf>,
    provenance: bool,
    json: bool,
    cli_args: Vec<String>,
    extra_samples: Vec<String>,
    ai_recover: bool,
    _ai_max_retries: Option<u32>,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();

    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // ── Parse and merge CLI config overrides ────────────────────────────
    // Accepted forms (all map to `config.<key>` values):
    //   KEY=VALUE            direct positional form
    //   --KEY=VALUE          long-flag form
    //   --KEY VALUE          long-flag form with separate value
    //   --arg KEY=VALUE      legacy `--arg` form (backward compatible)

    let mut cli_arg_values: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut cli_args_iter = cli_args.into_iter().peekable();
    while let Some(arg_str) = cli_args_iter.next() {
        let (k, v) = if let Some(eq) = arg_str.find('=') {
            let k = arg_str[..eq].trim_start_matches('-').to_string();
            (k, arg_str[eq + 1..].to_string())
        } else if let Some(k) = arg_str.strip_prefix("--") {
            // `--KEY VALUE` — consume the next argument as the value
            let v = cli_args_iter.next().with_context(|| {
                format!("invalid config flag: '{arg_str}' — expected --KEY=VALUE or --KEY VALUE")
            })?;
            (k.to_string(), v)
        } else {
            anyhow::bail!(
                "invalid config value format: '{arg_str}' — expected KEY=VALUE, --KEY=VALUE, or --KEY VALUE"
            );
        };
        if k.is_empty() || v.is_empty() {
            anyhow::bail!(
                "invalid config value format: '{arg_str}' — KEY and VALUE must be non-empty"
            );
        }
        cli_arg_values.insert(k, v);
    }

    // Apply CLI overrides and defaults from declarative config entries.
    // config_meta holds the declaration (required/default/help) for keys
    // that use the `key = { default, required, … }` syntax in [config].
    for (name, cfg_def) in &config.config_meta {
        let effective_value = if let Some(val) = cli_arg_values.get(name) {
            validate_config_value(name, val, cfg_def)?;
            config
                .config
                .insert(name.clone(), toml::Value::String(val.clone()));
            val.clone()
        } else if let Some(ref default) = cfg_def.default {
            validate_config_value(name, default, cfg_def)?;
            config
                .config
                .entry(name.clone())
                .or_insert_with(|| toml::Value::String(default.clone()));
            default.clone()
        } else if cfg_def.required {
            let help_suffix = cfg_def
                .help
                .as_deref()
                .map(|h| format!("\n  {h}"))
                .unwrap_or_default();
            let display_name = if cfg_def.sensitive {
                format!("{name} (sensitive)")
            } else {
                name.clone()
            };
            anyhow::bail!(
                "required config '{display_name}' not set. Use --{name} <value>{help_suffix}"
            );
        } else {
            continue;
        };
        // Mask sensitive values in all non-execution output
        if cfg_def.sensitive {
            tracing::info!("config '{}' = **** (sensitive)", name);
        } else {
            tracing::debug!("config '{}' = '{}'", name, effective_value);
        }
    }

    // Also inject any undeclared --arg values as {config.xxx}
    for (k, v) in &cli_arg_values {
        config
            .config
            .entry(k.clone())
            .or_insert_with(|| toml::Value::String(v.clone()));
    }

    // ── Merge --sample CLI flags into sample groups and samples_list ───

    if !extra_samples.is_empty() {
        if let Some(group) = config
            .sample_groups
            .iter_mut()
            .find(|g| g.name == "auto-discovered")
        {
            for s in &extra_samples {
                if !group.samples.contains(s) {
                    group.samples.push(s.clone());
                }
            }
        } else {
            config
                .sample_groups
                .push(oxo_flow_core::config::SampleGroup {
                    name: "cli-specified".to_string(),
                    samples: extra_samples.clone(),
                    metadata: std::collections::HashMap::new(),
                });
        }
        let existing = config
            .config
            .get("samples_list")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut merged: Vec<String> = existing
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        for s in &extra_samples {
            if !merged.contains(s) {
                merged.push(s.clone());
            }
        }
        config.config.insert(
            "samples_list".to_string(),
            toml::Value::String(merged.join(",")),
        );
        eprintln!(
            "  {} Added {} sample(s) via --sample flag",
            "Samples:".cyan(),
            extra_samples.len()
        );
    }

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

    let executor = Arc::new(LocalExecutor::new(exec_config));
    let success_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_rules_set: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let failures: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    // Rules that never ran because an upstream dependency failed, paired with the
    // dependency that blocked them. Reported separately from genuine failures so
    // the root cause stays distinguishable from the fallout.
    let blocked: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    let checkpoint_path = workdir
        .as_ref()
        .unwrap_or(&workflow_dir)
        .join(".oxo-flow/checkpoint.json");
    let checkpoint: Arc<Mutex<CheckpointState>> = if checkpoint_path.exists() {
        Arc::new(Mutex::new(
            CheckpointState::load_from_file(&checkpoint_path).unwrap_or_default(),
        ))
    } else {
        Arc::new(Mutex::new(CheckpointState::default()))
    };

    // Store workflow path in checkpoint for resume support
    {
        let mut ck = checkpoint.lock().await;
        ck.set_workflow_path(&workflow);
    }

    // When --resume-failed is set, clear failed rules from checkpoint so they re-execute.
    if resume_failed && checkpoint_path.exists() {
        let mut ck = checkpoint.lock().await;
        let failed_count = ck.failed_rules.len();
        let completed_count = ck.completed_rules.len();
        ck.failed_rules.clear();
        eprintln!(
            "{} Resuming {} completed, re-running {} failed rules",
            "Resume:".bold().cyan(),
            completed_count,
            failed_count
        );
    }

    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    // All config values (including CLI --arg overrides) become {config.key} in templates.
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }
    let wildcard_values: Arc<HashMap<String, String>> = Arc::new(wildcard_values);
    let workdir_actual = Arc::new(workdir.as_ref().unwrap_or(&workflow_dir).clone());

    // ── Auto-build references (indexes, data files) ──────────────────────

    if !skip_ref_build && !config.references.is_empty() {
        let ref_workdir = workdir.as_ref().unwrap_or(&workflow_dir);
        let ref_checkpoint = checkpoint_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(".oxo-flow/reference-checkpoint.json");
        let mut ref_state: std::collections::HashSet<String> = if ref_checkpoint.exists() {
            std::fs::read_to_string(&ref_checkpoint)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };

        for ref_def in &config.references {
            let output_path = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                &ref_def.output,
                &wildcard_values,
            );
            let output_full = ref_workdir.join(&output_path);

            if output_full.exists() && ref_state.contains(&ref_def.name) {
                continue; // Already built and tracked
            }

            // Check freshness: source newer than output → warn
            if let Some(ref source) = ref_def.source {
                let source_path =
                    ref_workdir.join(oxo_flow_core::executor::checkpoint::expand_config_in_path(
                        source,
                        &wildcard_values,
                    ));
                if source_path.exists()
                    && output_full.exists()
                    && oxo_flow_core::executor::checkpoint::file_is_newer(
                        &source_path,
                        &output_full,
                    )
                {
                    eprintln!(
                        "  {} {}: source is newer than output, rebuilding...",
                        "↻".yellow(),
                        ref_def.name
                    );
                }
            }

            if !output_full.exists() {
                let build_cmd = oxo_flow_core::executor::process::render_shell_command(
                    &ref_def.build,
                    &oxo_flow_core::rule::Rule {
                        name: format!("ref:{}", ref_def.name),
                        output: vec![output_path.clone()].into(),
                        ..Default::default()
                    },
                    &wildcard_values,
                );
                eprintln!(
                    "  {} Building {}: {}",
                    "⚙".cyan().bold(),
                    ref_def.name,
                    ref_def.description.as_deref().unwrap_or(&ref_def.output)
                );
                let status = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&build_cmd)
                    .current_dir(ref_workdir)
                    .status();
                match status {
                    Ok(s) if s.success() => {
                        ref_state.insert(ref_def.name.clone());
                        if let Some(parent) = ref_checkpoint.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(
                            &ref_checkpoint,
                            serde_json::to_string(&ref_state).unwrap_or_default(),
                        );
                        eprintln!("    {} {} built", "✓".green(), ref_def.name);
                    }
                    Ok(s) => {
                        anyhow::bail!(
                            "failed to build reference '{}' (exit {}). Command: {}",
                            ref_def.name,
                            s.code().unwrap_or(-1),
                            build_cmd
                        );
                    }
                    Err(e) => {
                        anyhow::bail!("failed to run build command for '{}': {}", ref_def.name, e);
                    }
                }
            } else if !ref_state.contains(&ref_def.name) {
                // Output exists but not tracked — mark as built
                ref_state.insert(ref_def.name.clone());
                let _ = std::fs::write(
                    &ref_checkpoint,
                    serde_json::to_string(&ref_state).unwrap_or_default(),
                );
            }
        }
    }

    // Rule `when` conditions are evaluated by the executor itself, which reports
    // them back as JobStatus::Skipped. Pre-evaluating them here as well would
    // count every condition-skipped rule twice.

    // Execute rules in parallel groups (topological levels).
    // Rules within a group have no dependencies on each other and can run concurrently.
    // Filter groups to only include rules in the execution order (respects --target).
    let order_set: std::collections::HashSet<&str> = order.iter().map(String::as_str).collect();
    let groups: Vec<Vec<String>> = dag
        .parallel_groups()
        .context("failed to compute parallel groups")?
        .into_iter()
        .map(|g| {
            g.into_iter()
                .filter(|name| order_set.contains(name.as_str()))
                .collect()
        })
        .filter(|g: &Vec<String>| !g.is_empty())
        .collect();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs.max(1)));

    for group in &groups {
        let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        for rule_name in group {
            // Skip if any direct dependency failed or was itself blocked.
            // `dag.dependencies()` returns direct predecessors only, so a blocked
            // rule must join the failed set for the block to reach its own
            // dependents. Groups are processed in topological order, so a rule's
            // dependencies are always resolved before the rule is considered.
            let blocked_by = {
                let frs = failed_rules_set.lock().await;
                dag.dependencies(rule_name)
                    .ok()
                    .and_then(|deps| deps.into_iter().find(|d| frs.contains(d.as_str())))
            };

            if let Some(dep) = blocked_by {
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                blocked.lock().await.push((rule_name.clone(), dep));
                if !is_tty {
                    eprintln!(
                        "  {} {} (blocked by failed dependency)",
                        "⊘".yellow(),
                        rule_name
                    );
                }
                progress.inc(1);
                continue;
            }

            // Check if already checkpoint-completed
            let should_skip = {
                let ck = checkpoint.lock().await;
                if ck.is_completed(rule_name) {
                    // Verify outputs still exist
                    if let Some(rule) = config.get_rule(rule_name) {
                        let mut outputs_exist = true;
                        for output in &rule.output {
                            if !output.contains('{') {
                                let expanded =
                                    oxo_flow_core::executor::checkpoint::expand_config_in_path(
                                        output,
                                        &wildcard_values,
                                    );
                                if !workdir_actual.join(&expanded).exists() {
                                    outputs_exist = false;
                                    break;
                                }
                            }
                        }
                        outputs_exist
                    } else {
                        true
                    }
                } else {
                    false
                }
            };

            if should_skip {
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                progress.set_message("skipping already completed");
                if !is_tty {
                    eprintln!("  {} {} (already completed)", "⊝".dimmed(), rule_name);
                }
                progress.inc(1);
                continue;
            }

            // Clone shared state for the spawned task
            let rule = config
                .get_rule(rule_name)
                .ok_or_else(|| anyhow::anyhow!("rule '{}' not found in workflow", rule_name))
                .unwrap()
                .clone();
            let rule_name = rule_name.clone();
            let executor = executor.clone();
            let checkpoint = checkpoint.clone();
            let checkpoint_path = checkpoint_path.clone();
            let failed_rules_set = failed_rules_set.clone();
            let failures = failures.clone();
            let success_count = success_count.clone();
            let fail_count = fail_count.clone();
            let skipped_count = skipped_count.clone();
            let wildcard_values = wildcard_values.clone();
            let workdir_actual = workdir_actual.clone();
            let semaphore = semaphore.clone();
            let progress = progress.clone();

            progress.set_message(format!("executing {}", rule_name));
            if !is_tty {
                eprintln!("  {} {}", "Running:".bold().cyan(), rule_name);
            }

            let typed_config = config.config.clone();
            let handle = tokio::spawn(async move {
                let _permit = semaphore.acquire().await;

                match executor
                    .execute_rule_with_config(&rule, &wildcard_values, &typed_config)
                    .await
                {
                    Ok(record) => {
                        let duration = record
                            .finished_at
                            .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
                            .map(|d| d.num_milliseconds() as f64 / 1000.0)
                            .unwrap_or(0.0);

                        if record.status == oxo_flow_core::executor::JobStatus::Success {
                            success_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                            let mut ck = checkpoint.lock().await;
                            ck.mark_completed(&rule_name, benchmark);
                            if provenance {
                                for output in &rule.output {
                                    let output_path = workdir_actual.join(output);
                                    if output_path.exists()
                                        && let Ok(checksum) = oxo_flow_core::executor::checkpoint::compute_file_checksum(&output_path)
                                    {
                                        ck.record_checksum(output, checksum);
                                    }
                                }
                            }
                            if let Err(e) = ck.save_to_file(&checkpoint_path) {
                                tracing::warn!("Failed to save checkpoint: {e}");
                            }
                        } else if record.status == oxo_flow_core::executor::JobStatus::Skipped {
                            skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            {
                                let mut frs = failed_rules_set.lock().await;
                                frs.insert(rule_name.clone());
                            }
                            let mut ck = checkpoint.lock().await;
                            ck.mark_failed(&rule_name);
                            if let Err(e) = ck.save_to_file(&checkpoint_path) {
                                tracing::warn!("Failed to save checkpoint: {e}");
                            }
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
                            if !keep_going {
                                eprintln!("  {} {}", "✗".red(), err_msg);
                            } else {
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
                                let mut f = failures.lock().await;
                                f.push((rule_name.clone(), reason));
                            }
                        }
                    }
                    Err(e) => {
                        fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        {
                            let mut frs = failed_rules_set.lock().await;
                            frs.insert(rule_name.clone());
                        }
                        let mut ck = checkpoint.lock().await;
                        ck.mark_failed(&rule_name);
                        if let Err(e) = ck.save_to_file(&checkpoint_path) {
                            tracing::warn!("Failed to save checkpoint: {e}");
                        }
                        if !keep_going {
                            eprintln!("  {} rule '{}' failed: {}", "✗".red(), rule_name, e);
                        } else {
                            let mut f = failures.lock().await;
                            f.push((rule_name.clone(), e.to_string()));
                        }
                    }
                }
                progress.inc(1);
            });
            handles.push(handle);
        }

        // Wait for all rules in this group before proceeding to next group
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!("Task panicked: {e}");
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // After each group, check if we should abort (non-keep-going mode)
        let fc = fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if fc > 0 && !keep_going {
            progress.finish_and_clear();

            // AI error recovery: auto-detect from workflow [ai] or explicit --ai-recover
            let should_recover =
                ai_recover || crate::commands::ai_template::should_use_ai(Some(&workflow), false);
            if should_recover {
                let failures_guard = failures.lock().await;
                if let Some((rule, error)) = failures_guard.first()
                    && let Some(provider) =
                        crate::commands::ai_template::try_resolve_ai(Some(&workflow), true)
                {
                    let result = crate::commands::ai_recover::diagnose_failure(
                        &workflow, rule, -1, error, &provider,
                    )
                    .await;
                    match result {
                        Ok(diag) => {
                            if let Some(ref toml) = diag.modified_toml {
                                let _ = crate::commands::ai_recover::apply_fix(
                                    &workflow, toml, "recovery",
                                );
                            }
                        }
                        Err(e) => eprintln!("  AI diagnosis failed: {e}"),
                    }
                }
            }

            return Err(anyhow::anyhow!("workflow execution failed"));
        }
    }

    let success_count = success_count.load(std::sync::atomic::Ordering::Relaxed);
    let fail_count = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let skipped_count = skipped_count.load(std::sync::atomic::Ordering::Relaxed);
    let checkpoint = checkpoint.lock().await;
    let failures = failures.lock().await;
    let blocked = blocked.lock().await;

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
        for (name, reason) in failures.iter() {
            eprintln!("  {} {} — {}", "✗".red(), name.bold(), reason);
        }
    }

    // Blocked rules did not run and produced no outputs. Listing them keeps the
    // fallout of a failure visible without disguising it as a genuine failure.
    if !blocked.is_empty() {
        eprintln!("\n{}", "Blocked rules (did not run):".bold().yellow());
        for (name, dep) in blocked.iter() {
            eprintln!("  {} {} — depends on '{}'", "⊘".yellow(), name.bold(), dep);
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
                "blocked": blocked.len(),
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
    ai: bool,
    _ai_max_retries: Option<u32>,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        crate::commands::ai_check::analyze_workflow(&workflow, &provider, "dry-run").await?;
        println!();
    }
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

pub async fn debug_command(workflow: PathBuf, rule_name: Option<String>, ai: bool) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        let expanded: Vec<String> = config
            .rules
            .iter()
            .map(|r| {
                let shell_cmd = r.shell.as_deref().unwrap_or("(no shell command)");
                let shell = oxo_flow_core::executor::process::render_shell_command(
                    shell_cmd,
                    r,
                    &std::collections::HashMap::new(),
                );
                format!(
                    "## {}\nthreads={}, memory={}, env={:?}\n```bash\n{}\n```",
                    r.name,
                    r.resources.threads,
                    r.resources.memory.as_deref().unwrap_or("1G"),
                    r.environment,
                    shell
                )
            })
            .collect();
        let joined = expanded.join("\n\n");

        let system = r#"## Role
You are a bioinformatics pipeline debugger. Explain each expanded shell command in plain language,
flagging potential issues: parameter misuse, resource mismatch, missing required flags, or
logical errors. Output format per rule:

**rule_name**: <what this command does — 1 sentence>
⚠ <any issues found — 1 line per issue, or "No issues" if clean>
"#
        .to_string();
        let user = format!(
            "## Workflow: {}\n\n## Expanded Commands\n\n{joined}\n\n## Task\nExplain each command and flag issues.",
            workflow.display()
        );

        println!("{}", "  Analyzing commands...".bold().cyan());
        match provider.chat(&system, &user).await {
            Ok(response) => {
                println!("\n{}\n{response}", "AI Analysis".bold().green().underline());
                return Ok(());
            }
            Err(e) => eprintln!("  AI analysis failed: {e}"),
        }
    }

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

pub async fn resume_command(
    checkpoint: PathBuf,
    jobs: usize,
    _ai_recover: bool,
    _ai_max_retries: Option<u32>,
) -> Result<()> {
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
        true,            // skip_ref_build (resume: refs already built)
        None,            // cache_dir
        false,           // provenance (checkpoint already has checksums)
        false,           // json (resume defaults to human-readable)
        Vec::new(),      // cli_args (resume reuses checkpoint state)
        Vec::new(),      // extra_samples (resume uses existing groups)
        false,           // ai_recover (resume doesn't support recovery)
        None,            // ai_max_retries
    )
    .await
}
