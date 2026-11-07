//! `dry-run` preview execution: static DAG validation, profile merging,
//! checkpoint impact preview, and structured JSON output.

use super::*;
use crate::commands::{print_banner, resolve_workflow, samples};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use std::collections::HashMap;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn dry_run_command(
    workflow: Option<PathBuf>,
    target: Vec<String>,
    module: Vec<String>,
    _verbose: bool,
    json: bool,
    ai: bool,
    ai_max_retries: Option<u32>,
    samples_filter: Vec<String>,
    workdir: Option<PathBuf>,
    profile: Option<String>,
    skip_ref_build: bool,
    _cache_dir: Option<PathBuf>,
    cli_args: Vec<String>,
    rerun: bool,
    resume_failed: bool,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    // A#2 (persona campaign): auto-discovery silently picked a file when no
    // path was given — name the pick so `oxo-flow dry-run` in a multi-file
    // directory never operates on a surprise file.
    diagnostic_narrate(
        format_args!(
            "{} Using workflow file: {}",
            "Info:".bold().cyan(),
            workflow.display()
        ),
        None,
    );
    // Path resolution base: --workdir wins, default is the workflow file's
    // directory — the same base the executor uses for run (issue #68).
    let base_dir = workdir.as_deref().unwrap_or(&workflow_dir);

    // Issue #751: same discovery fallback as run — bundle extractions live
    // under the temp dir, and --workdir may hold the data. The fallback is
    // a no-op unless the two roots differ.
    let discovery_fallback =
        if workflow.starts_with(std::env::temp_dir()) && base_dir != workflow_dir {
            Some(base_dir)
        } else {
            None
        };
    let mut config =
        WorkflowConfig::from_file_with_discovery_fallback(&workflow, discovery_fallback)
            .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // ── CLI config overrides (the shared phase, overrides.rs) ─────
    // Applied in run's exact order: overrides first, then
    // the --samples subset filter — so the preview config is structurally
    // identical to what `run` would execute.
    apply_cli_override_phase(&mut config, &workflow, cli_args, "dry-run")?;

    // ── Filter to a sample subset (--samples first:N / names / ready) ──
    // `ready` resolves against a scratch expanded clone; the report covers
    // the full cohort so the readiness section stays informative even when
    // the listing is filtered (issue #63).
    let ready_report = if !samples_filter.is_empty() {
        samples::apply_samples_filter(&mut config, &samples_filter, false, base_dir)?
    } else {
        None
    };

    // ── Execution profile (shared with run) ──────────────────────────────
    // The SAME merge helper `run` uses: preview and execution can never
    // drift apart on profile semantics.
    if let Some(ref profile_name) = profile {
        crate::commands::run_preview::merge_profile(&mut config, profile_name, &workflow_dir)?;
    }

    // ── Scientific preflight (deterministic, evidence-backed) ────────────
    // Printed for every dry-run; with --ai the findings are also passed to
    // the model for a plain-language explanation.
    let scientific = oxo_flow_core::scientific_preflight::analyze_scientific_constraints(&config);
    let scientific_context = if scientific.is_empty() {
        String::new()
    } else {
        let mut text = String::new();
        for w in &scientific {
            text.push_str(&format!(
                "- [{}] {}: {}\n  Fix: {}\n",
                w.code, w.rule, w.message, w.suggestion
            ));
        }
        text
    };

    // AI: auto-detect from workflow [ai] or explicit --ai flag. Suppressed
    // under --json (#539): same stdout-contract reasoning as validate/lint.
    if !json
        && let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai)
    {
        // `--ai-max-retries` bounds the attempts, same contract as `run`'s
        // recovery (only a FAILED analysis call is retried).
        let attempts = ai_attempts(ai_max_retries);
        let mut result = Err(anyhow::anyhow!("AI analysis did not run"));
        for attempt in 1..=attempts {
            result = crate::commands::ai_check::analyze_workflow(
                &workflow,
                &provider,
                "dry-run",
                &scientific_context,
            )
            .await;
            if result.is_ok() || attempt == attempts {
                break;
            }
            if let Err(e) = &result {
                eprintln!("  AI analysis attempt {attempt}/{attempts} failed: {e} — retrying");
            }
        }
        result?;
        println!();
    }

    if !scientific.is_empty() {
        eprintln!("\n{}", "Scientific preflight:".bold().yellow());
        for w in &scientific {
            eprintln!("  ⚠ [{}] {}: {}", w.code, w.rule, w.message);
            eprintln!("    {} {}", "→".bold(), w.suggestion);
        }
    }

    config.apply_defaults();
    // Local-execution advice only: a profile that routes to a scheduler runs
    // on cluster nodes, so the submit host's capacity is irrelevant — the
    // same rule the `run` local path applies (journey-power P7). Emitted
    // before wildcard expansion so warnings stay per template rule.
    if cluster_path::routed_cluster_profile(profile.as_deref(), config.cluster.as_ref()).is_none() {
        config.warn_host_resource_fit();
    }
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // ── Undefined `{config.*}` gate (issue #142 H1) — the same gate run
    // applies: preview must refuse a typo'd key exactly like execution.
    // Same gate and resolution as `run` (issue #142 H1, #112).
    let (mut dag, target) = build_dag_and_targets(&config, target, &module)?;
    // Default targets (issue #469): same seeding as `run`.
    let target = resolve_default_targets(&config, &target);
    // Targeted dry-runs close over the INSTANTIATED DAG (issue #247) —
    // the same when-gate filter `run` applies, so the preview cannot list
    // instances that would never execute.
    let dry_wildcard_values: HashMap<String, String> = config_placeholder_values(&config.config);
    let when_false_rules =
        crate::commands::run_preview::when_false_rules(&config, &dry_wildcard_values);

    // Compute the execution set (respects --target), then display it in
    // parallel-group order — the same grouping `run` uses for scheduling.
    // Independent rules at the same level appear adjacent, so the listing
    // reflects actual concurrency instead of an arbitrary topological order.
    let mut order = if target.is_empty() {
        parallel_display_order(&dag, &target, &when_false_rules, base_dir, None, true)?
    } else {
        execution_order_skipping_with_notes(&dag, &target, &when_false_rules, base_dir, None, true)?
    };

    // All config values (including CLI --arg overrides) become {config.key} in templates.
    let wildcard_values = config_placeholder_values(&config.config);

    // ── Checkpoint-aware rerun preview (issue #66) ────────────────────────
    // Read-only: the preview classifies every rule in the execution set
    // exactly as `run` would — invalidation sources, downstream cascade,
    // and the protected remainder — without touching the checkpoint on disk.
    let checkpoint_path = CheckpointState::default_path(base_dir);
    // The preview is ALWAYS computed — with an empty state when no
    // checkpoint exists. `when` conditions can skip rules even on a fresh
    // run, so "every rule will execute" would be wrong without it.
    // The shared load phase (findings.rs) in its read-only form: a missing
    // file warns and plans fresh; a CORRUPT file is named as corrupt (the
    // old catch-all "no checkpoint" message mislabeled it) and the plan
    // degrades to empty — dry-run stays usable as the recovery tool while
    // run refuses to start. Nothing is persisted.
    let (mut checkpoint_state, _stale_running) = load_checkpoint_for_command(
        &checkpoint_path,
        &workflow,
        CheckpointLoadMode::ReadOnlyPreview,
        None,
    )?;
    let sensitive_keys = sensitive_keys_of(&config);
    // The verbose plan print must not leak sensitive values (issue #99 B1).
    let sensitive_values = sensitive_values_of(&config);
    let interpreter_map = config.workflow.interpreter_map.clone();
    let mut preview = crate::commands::run_preview::preview_run_plan(
        &checkpoint_state,
        &config,
        &dag,
        &order,
        base_dir,
        &wildcard_values,
        &sensitive_keys,
        &interpreter_map,
        &checkpoint_path,
        rerun,
        resume_failed,
    );

    // ── Checkpoint re-entry replay (issue #78 P3) ────────────────────────
    // The round-0 preview determines which checkpoint rules are up-to-date
    // (Skipped); their recorded re-entries replay and the preview recomputes
    // on the re-expanded plan — the same static plan a run would execute.
    if !checkpoint_state.reentries.is_empty() {
        use crate::commands::run_preview::RuleStatus;
        let valid: std::collections::HashSet<String> = preview
            .plan
            .iter()
            .filter(|p| p.status == RuleStatus::Skipped)
            .map(|p| p.name.clone())
            .collect();
        let replayed = replay_checkpoint_reentries(
            &mut ReplayPlan {
                config: &mut config,
                dag: &mut dag,
                order: &mut order,
                target: &target,
                when_false_rules: &when_false_rules,
                workdir: base_dir,
                run_log: None,
            },
            &checkpoint_state.reentries,
            &valid,
            ReplayPlanOrdering::ParallelDisplay,
            "dry-run",
            true,
        )?;
        if replayed > 0 {
            preview = crate::commands::run_preview::preview_run_plan(
                &checkpoint_state,
                &config,
                &dag,
                &order,
                base_dir,
                &wildcard_values,
                &sensitive_keys,
                &interpreter_map,
                &checkpoint_path,
                rerun,
                resume_failed,
            );
        }
    }
    // ── Protected-tree drift (issue #935 P1) — read-only ──────────────────
    // Same detection `run` applies, on the in-memory checkpoint copy: the
    // preview never persists baselines (dry-run stays read-only), so first
    // observation is reported as "will be baselined" instead of "recorded".
    // Unlike `run`, dry-run reports but does not fail (only `run` gates).
    let (dry_protected_drift_lines, dry_protected_baselined, dry_protected_warnings) =
        crate::commands::run_preview::detect_protected_tree_drift(
            &mut checkpoint_state,
            &config,
            base_dir,
        );

    // Pixi-manifest preflight (issue #848): mirror run's pending_envs —
    // every non-skip rule's env spec, resolved against the workflow dir.
    // plan/dry-run previously passed while a declared manifest was missing
    // and only the run failed after the plan was already committed to.
    let env_manifest_issues: Vec<(String, String)> = {
        let mut issues = Vec::new();
        for entry in preview.plan.iter().filter(|entry| !entry.status.is_skip()) {
            let Some(rule) = config.get_rule(&entry.name) else {
                continue;
            };
            let resolved = rule.environment.resolve_relative_paths(Some(&workflow_dir));
            if let Some(spec) = resolved.pixi
                && oxo_flow_core::environment::validate_pixi_manifest(&spec).is_err()
            {
                issues.push((entry.name.clone(), spec));
            }
        }
        issues
    };
    {
        let p = &preview;
        let modified = p
            .checkpoint_modified
            .map(|t| {
                chrono::DateTime::<chrono::Local>::from(t)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown time".to_string());
        eprintln!(
            "{} {} (modified {})",
            "Checkpoint:".bold().cyan(),
            p.checkpoint_path.display(),
            modified.dimmed()
        );
        // Issue #432(b): the headline must answer "what happens next", not
        // "how big is the DAG". `order.len()` is DAG size — shown, but
        // labelled as such and secondary to the run/skip/completed split.
        let will_run = p.plan.len() - p.will_skip;
        eprintln!(
            "{} would run: {} | skip: {} | completed: {} (DAG size: {})",
            "Plan:".bold().yellow(),
            will_run.to_string().green(),
            p.will_skip.to_string().dimmed(),
            p.completed_total.to_string().cyan(),
            order.len()
        );
        for chain in &p.cascade_chains {
            eprintln!("  {} {}", "rerun cascade:".yellow(), chain.join(" → "));
        }
    }

    // ── Reference auto-builds (mirrors run's --skip-ref-build semantics) ──
    // run builds any reference whose output is missing before scheduling;
    // the preview surfaces that cost instead of hiding it.
    let reference_builds: Vec<String> = if skip_ref_build {
        Vec::new()
    } else {
        config
            .references
            .iter()
            .filter(|r| !base_dir.join(&r.output).exists())
            .map(|r| r.name.clone())
            .collect()
    };
    if !reference_builds.is_empty() {
        eprintln!(
            "{} {} would run: {}",
            "References:".bold().cyan(),
            oxo_flow_core::plural(
                reference_builds.len(),
                "reference build",
                "reference builds"
            ),
            reference_builds.join(", ")
        );
        eprintln!(
            "  {} pass --skip-ref-build to assume they are pre-built.",
            "ℹ".dimmed()
        );
    }

    // Sample readiness (issue #63). With `--samples ready` the report covers
    // the full cohort (computed pre-filter); otherwise it is computed on the
    // (possibly filtered) expanded config.
    let readiness_report = match ready_report {
        Some(report) => Some(report),
        None => Some(oxo_flow_core::readiness::compute_readiness(
            &config, base_dir,
        )),
    };
    if let Some(report) = &readiness_report {
        samples::print_readiness_section(report);
    }

    for (i, rule_name) in order.iter().enumerate() {
        let rule = config
            .get_rule(rule_name)
            .ok_or_else(|| anyhow::anyhow!("rule '{}' not found", rule_name))?;
        let status_text = preview
            .plan
            .iter()
            .find(|r| r.name == *rule_name)
            .map(|r| {
                let (word, _, reason, _) = r.status.render_parts();
                let text = format!("[{word}: {reason}]");
                // Skips stay quiet; changed inputs / missing outputs and the
                // cascades they trigger are the attention-worthy cases
                // (loud yellow); everything else runs bold.
                if word == "skip" {
                    format!("{}", text.dimmed())
                } else if word == "rerun" || r.status.is_warning() {
                    format!("{}", text.bold().yellow())
                } else {
                    format!("{}", text.bold())
                }
            })
            .unwrap_or_default();
        eprintln!("  {}. {}  {}", i + 1, rule_name.bold().cyan(), status_text);

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
            let expanded = oxo_flow_core::executor::process::render_shell_command(
                cmd,
                rule,
                &wildcard_values,
                oxo_flow_core::scheduler::detect_system_limits(),
            );
            // Mask sensitive values (issue #99 B1): the plan print is a log
            // surface like any other.
            let expanded =
                oxo_flow_core::executor::process::mask_sensitive(&expanded, &sensitive_values);
            // Workdir-aware blocking preview (issue #428): E011 only applies
            // when the deletion target lies outside the run workdir, so an
            // absolute {config.out_dir} cleanup is not labeled "blocked".
            let blocked_label = if let Some((_category, description)) =
                oxo_flow_core::format::shell_blocking_pattern(&expanded)
            {
                match oxo_flow_core::executor::validate_shell_safety_in_workdir(&expanded, base_dir)
                {
                    Err(e) => Some(e.to_string()),
                    Ok(()) => None,
                }
                .map(|_| format!("(blocked: E011 — {description})"))
            } else {
                None
            };
            if let Some(label) = blocked_label {
                eprintln!("     command: {}  {}", expanded, label.red().bold());
            } else {
                eprintln!("     command: {}", expanded);
            }
        }

        // Show input file status for concrete (non-wildcard) paths.
        // Relative paths resolve against the workflow's directory — the same
        // place the executor runs shells from and readiness checks (issue #63
        // follow-up: CWD-based checks contradicted the readiness report when
        // invoked from another directory).
        for inp in &rule.input {
            let s = inp.to_string();
            if !s.contains('{') && !s.contains('*') && !s.starts_with('/') {
                let exists = base_dir.join(&s).exists();
                let icon = if exists { "✓" } else { "✗" };
                eprintln!("     input {}: {}", icon, s);
            }
        }
    }

    // Fail-fast pixi-manifest block (issue #848): printed after the rule
    // listing so the plan context stays visible, before the JSON document.
    if !env_manifest_issues.is_empty() {
        eprintln!(
            "\n  {} {} declared by pending rules do not exist:",
            "✗".red().bold(),
            oxo_flow_core::plural(env_manifest_issues.len(), "pixi manifest", "pixi manifests")
        );
        for (rule, spec) in &env_manifest_issues {
            eprintln!("    - {rule}: {spec} (resolved relative to the workflow directory)");
        }
        eprintln!(
            "  {} create the manifest at the resolved path or correct the rule's `pixi` path",
            "hint:".yellow().bold()
        );
    }

    // Protected-tree drift section (issue #935 P1): printed after the rule
    // listing, before the JSON document — same placement as the pixi block.
    if !dry_protected_warnings.is_empty() {
        for line in &dry_protected_warnings {
            eprintln!("  {} {}", "⚠".yellow(), line);
        }
    }
    if dry_protected_baselined > 0 {
        eprintln!(
            "  {} {} — future changes will be flagged by plan/dry-run/run",
            "Note:".bold().cyan(),
            oxo_flow_core::plural(
                dry_protected_baselined,
                "new protected tree observed (no baseline persisted by dry-run)",
                "new protected trees observed (no baseline persisted by dry-run)"
            )
        );
    }
    if !dry_protected_drift_lines.is_empty() {
        eprintln!(
            "\n  {} PROTECTED TREE DRIFT detected in {} file(s):",
            "✗".red().bold(),
            dry_protected_drift_lines.len()
        );
        for line in &dry_protected_drift_lines {
            eprintln!("    {} {}", "·".red(), line);
        }
        eprintln!(
            "  {} restore the files or pass --allow-protected-drift to run anyway",
            "hint:".yellow().bold()
        );
    }

    // Resource summary — computed over the execution order (the same set
    // the listing prints), not the whole config, so the totals match the
    // rules actually listed (e.g. `--rule` filtered runs).
    let rules_in_order = order.iter().filter_map(|name| config.get_rule(name));
    let total_threads: u32 = rules_in_order.clone().map(|r| r.effective_threads()).sum();
    let max_threads: u32 = rules_in_order
        .clone()
        .map(|r| r.effective_threads())
        .max()
        .unwrap_or(1);
    let memory_values: Vec<&str> = rules_in_order
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
            "         {} with memory requirements",
            oxo_flow_core::plural(memory_values.len(), "rule", "rules")
        );
    }
    if !config.sample_groups.is_empty() {
        eprintln!(
            "         {}, {}",
            oxo_flow_core::plural(config.sample_groups.len(), "sample group", "sample groups"),
            oxo_flow_core::plural(config.pairs.len(), "pair", "pairs")
        );
    }

    // Suggest -j per parallel group (issue #361): each wave is evaluated
    // against its own heaviest rule, so a light wide wave is not starved
    // by an unrelated heavy wave elsewhere in the DAG. The engine's
    // resource pool enforces declared threads, so this is a performance
    // hint, not an oversubscription gate.
    let system_threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    let groups = dag.parallel_groups().unwrap_or_default();
    let suggested_jobs = suggest_jobs(&config, &groups, system_threads).to_string();
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

        // Machine-readable sample readiness (issue #63).
        let samples_block = readiness_report.as_ref().map(|report| {
            let ready_names: Vec<&str> = report.ready.iter().map(|s| s.name.as_str()).collect();
            let waiting: Vec<serde_json::Value> = report
                .waiting
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "missing": s.missing,
                    })
                })
                .collect();
            serde_json::json!({
                "total": report.total,
                "ready": report.ready.len(),
                "waiting_count": report.waiting.len(),
                "ready_names": ready_names,
                "waiting": waiting,
                "missing_global": report.missing_global,
            })
        });

        // Machine-readable checkpoint preview (issue #66): the predicted
        // execution plan with per-rule status + reason.
        let checkpoint_block = Some(&preview).map(|p| {
            let plan: Vec<serde_json::Value> = p
                .plan
                .iter()
                .map(|r| {
                    let (_, machine_tag, _, cascaded_from) = r.status.render_parts();
                    serde_json::json!({
                        "name": r.name,
                        "status": machine_tag,
                        "cascaded_from": cascaded_from,
                    })
                })
                .collect();
            serde_json::json!({
                "path": p.checkpoint_path.display().to_string(),
                "modified": p.checkpoint_modified.map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                }),
                "completed_total": p.completed_total,
                "summary": {
                    "will_run": p.plan.len() - p.will_skip,
                    "will_skip": p.will_skip,
                    "protected_outside": p.protected_outside,
                },
                "plan": plan,
                "cascade_chains": p.cascade_chains,
            })
        });

        // Stable per-rule plan (schema_version 1 — the first release to
        // ship this surface): status is the same bracket word the human
        // listing prints ([run:/[skip:/[rerun:), reason is the human
        // status text after the colon — so the JSON is a 1:1 structured
        // mirror of the stderr plan the ecosystem greps.
        let plan_entries: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|name| {
                let rule = config.get_rule(name)?;
                let (status, reason, cascaded_from) = match preview
                    .plan
                    .iter()
                    .find(|r| r.name == *name)
                    .map(|r| &r.status)
                {
                    Some(status) => {
                        let (word, _, reason, cascaded_from) = status.render_parts();
                        (word, reason, cascaded_from)
                    }
                    // The preview classifies every rule in the execution
                    // set, so this branch is unreachable; keep it loud.
                    None => ("unknown", "unknown".to_string(), None),
                };
                let expanded_shell = rule.shell.as_ref().map(|cmd| {
                    oxo_flow_core::executor::process::render_shell_command(
                        cmd,
                        rule,
                        &wildcard_values,
                        oxo_flow_core::scheduler::detect_system_limits(),
                    )
                });
                // Declared patterns stay raw in inputs/outputs (the stable
                // contract); the expanded variants give consumers the exact
                // per-instance paths the engine will touch — the same
                // {config.x} expansion the human stderr listing shows.
                let inputs_expanded: Vec<String> = rule
                    .input
                    .to_vec()
                    .iter()
                    .map(|p| {
                        oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            p,
                            &wildcard_values,
                        )
                    })
                    .collect();
                let outputs_expanded: Vec<String> = rule
                    .output
                    .to_vec()
                    .iter()
                    .map(|p| {
                        oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            p,
                            &wildcard_values,
                        )
                    })
                    .collect();
                Some(serde_json::json!({
                    "name": rule.name,
                    "status": status,
                    "reason": reason,
                    "cascaded_from": cascaded_from,
                    "threads": rule.effective_threads(),
                    "memory": rule.effective_memory(),
                    "environment": rule.environment.kind(),
                    "command": expanded_shell,
                    "inputs": serde_json::to_value(&rule.input).unwrap_or(serde_json::Value::Null),
                    "outputs": serde_json::to_value(&rule.output).unwrap_or(serde_json::Value::Null),
                    "inputs_expanded": inputs_expanded,
                    "outputs_expanded": outputs_expanded,
                }))
            })
            .collect();

        let output = serde_json::json!({
            "command": "dry-run",
            "schema_version": 1,
            "workflow": workflow.display().to_string(),
            "total_rules": order.len(),
            "execution_order": order_list,
            "rules": rule_list,
            "plan": plan_entries,
            "summary": {
                "total_threads": total_threads,
                "max_threads_per_rule": max_threads,
                "memory_rules": memory_values.len(),
                "would_execute": preview.plan.len() - preview.will_skip,
                "will_skip": preview.will_skip,
                "total_rules": order.len(),
            },
            "sample_groups": config
                .sample_groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "name": g.name,
                        "samples": g.samples,
                    })
                })
                .collect::<Vec<_>>(),
            "pairs": config
                .pairs
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "pair_id": p.pair_id,
                        "experiment": p.experiment,
                        "control": p.control,
                    })
                })
                .collect::<Vec<_>>(),
            "suggested_jobs": suggested_jobs,
            "samples": samples_block,
            "checkpoint_preview": checkpoint_block,
            "reentry": {
                // Recorded re-entries (checkpoint.json) — replayed when their
                // checkpoint rule is up-to-date; revoked otherwise (issue #78 P3).
                "recorded": checkpoint_state.reentries,
                // Checkpoint rules that may add instances at runtime.
                "possible": config
                    .rules
                    .iter()
                    .filter(|r| r.checkpoint)
                    .map(|r| serde_json::json!({
                        "rule": r.name,
                        "manifest": r.checkpoint_manifest,
                    }))
                    .collect::<Vec<_>>(),
            },
            "profile": profile,
            "reference_builds": reference_builds,
            // Issue #848: declared pixi manifests that do not exist (resolved
            // against the workflow directory). Empty unless a pending rule's
            // manifest is missing — the command still exits non-zero in that
            // case after the document is printed.
            "env_manifest_issues": env_manifest_issues
                .iter()
                .map(|(rule, spec)| serde_json::json!({"rule": rule, "spec": spec}))
                .collect::<Vec<_>>(),
            // Issue #935 P1: protected-tree drift, read-only (dry-run never
            // persists baselines). `drifted_files` carries one describe()
            // line per changed file; `new_trees` counts first observations.
            "protected_tree_drift": {
                "drifted_files": dry_protected_drift_lines,
                "new_trees": dry_protected_baselined,
                "warnings": dry_protected_warnings,
            },
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    // Bail AFTER the JSON document so callers still get machine-readable
    // output on this failure path (same contract as `validate --json`).
    if !env_manifest_issues.is_empty() {
        anyhow::bail!(
            "{} declared by pending rules do not exist — \
             create the manifest at the resolved path or correct the rule's \
             `pixi` path (resolved relative to the workflow directory)",
            oxo_flow_core::plural(env_manifest_issues.len(), "pixi manifest", "pixi manifests")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    // ── Issue #432: dry-run headline + status --workdir / staleness ──────

    #[test]
    fn dry_run_headline_reports_run_skip_completed_not_dag_size() {
        // The headline must answer "what happens next" (issue #432b): the
        // old `DAG: (dry-run) N rules would execute` read as "everything
        // re-runs" while the checkpoint said most rules were up to date.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "headline"

[[rules]]
name = "step"
input = []
output = ["out.txt"]
shell = "touch out.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["dry-run", wf.to_str().unwrap()])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
        assert!(
            stderr.contains("Plan: would run: 1 | skip: 0 | completed: 0 (DAG size: 1)"),
            "headline must lead with would-run/skip/completed, got: {stderr}"
        );
        assert!(
            !stderr.contains("rules would execute"),
            "the old misleading headline must be gone, got: {stderr}"
        );
    }
}
