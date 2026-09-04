use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::backend::BackendJobStatus;
use oxo_flow_core::backend::ExecutorBackend;
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use crate::ClusterAction;
use crate::commands::print_banner;

/// Resolve a `--backend` value, refusing to guess.
///
/// An unrecognised name used to fall back to SLURM, so a typo (`-b slrm`,
/// `-b pbs-torque`) submitted real jobs to, or queried, the wrong scheduler.
fn parse_backend(name: &str) -> Result<ClusterBackend> {
    ClusterBackend::from_str(name).map_err(|_| {
        anyhow::anyhow!("unknown cluster backend '{name}' — expected slurm, pbs, sge, or lsf")
    })
}

/// Resolve the backend for a cluster action: the `--backend` flag wins,
/// then `$OXO_FLOW_CLUSTER_BACKEND`, then SLURM — the overwhelmingly
/// common scheduler, and the one `-b` used to be required for. An EXPLICIT
/// value keeps [`parse_backend`]'s strict error, so a typo can never
/// silently route to SLURM; only an omitted flag falls back.
fn resolve_backend(flag: Option<&str>) -> Result<ClusterBackend> {
    let env = std::env::var("OXO_FLOW_CLUSTER_BACKEND").ok();
    resolve_backend_with(flag, env.as_deref())
}

/// The pure decision behind [`resolve_backend`], with the environment value
/// injected — the test crate forbids `unsafe`, which rules out the
/// `set_var` dance, and a pure function pins the precedence table anyway.
fn resolve_backend_with(flag: Option<&str>, env: Option<&str>) -> Result<ClusterBackend> {
    if let Some(name) = flag {
        return parse_backend(name);
    }
    match env.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => parse_backend(name),
        None => Ok(ClusterBackend::Slurm),
    }
}

/// Human one-liner for the parsed status of one job.
fn status_line(id: &str, status: BackendJobStatus) -> String {
    match status {
        BackendJobStatus::Unknown => {
            format!("  {id}: not in the queue (finished, or an unknown id)")
        }
        other => format!("  {id}: {}", other.label()),
    }
}

/// Ask the scheduler after `job_ids` and answer one status per id.
///
/// The scheduler's output is CAPTURED and parsed rather than streamed: a bare
/// passthrough gave the caller a transcript when it asked a question, and
/// nothing a machine could read. `run` is the execution seam — the command
/// hands in the real scheduler binary, tests hand in a stub.
fn query_status(
    backend: &ClusterBackend,
    job_ids: &[String],
    run: impl Fn(&str, &[String]) -> Result<String>,
) -> Result<Vec<(String, BackendJobStatus)>> {
    let mut report: Vec<(String, BackendJobStatus)> = job_ids
        .iter()
        .map(|id| (id.clone(), BackendJobStatus::Unknown))
        .collect();
    for (program, args) in oxo_flow_core::backend::cluster::status_invocations(backend, job_ids) {
        for (id, status) in
            oxo_flow_core::backend::cluster::status_report(backend, job_ids, &run(program, &args)?)
        {
            // A scheduler that does not know an id stays silent about it:
            // keep the earlier verdict rather than overwriting it with
            // unknown.
            if status != BackendJobStatus::Unknown
                && let Some(entry) = report.iter_mut().find(|(rid, _)| rid == &id)
            {
                entry.1 = status;
            }
        }
    }
    Ok(report)
}

/// Ask the scheduler for the current user's live jobs and answer one
/// `(id, status)` per listed row — the no-arguments `cluster status` view.
/// Same execution seam as [`query_status`].
fn query_my_jobs(
    backend: &ClusterBackend,
    run: impl Fn(&str, &[String]) -> Result<String>,
) -> Result<Vec<(String, BackendJobStatus)>> {
    // Every backend currently yields exactly one listing invocation, so
    // this is a plain single call — the first answer is the answer.
    let Some((program, args)) = oxo_flow_core::backend::cluster::my_jobs_invocations(backend)
        .into_iter()
        .next()
    else {
        return Ok(Vec::new());
    };
    eprintln!(
        "{} Executing '{} {}'...",
        "Cluster:".bold().cyan(),
        program,
        args.join(" ")
    );
    // A user listing has no per-requested-id contract: the rows the
    // scheduler returns ARE the answer, so run the invocations inline
    // rather than threading through the overwrite-only-non-Unknown
    // machinery of query_status.
    let listing = run(program, &args)?;
    Ok(oxo_flow_core::backend::cluster::parse_job_listing(
        backend, &listing,
    ))
}

/// Emit the `oxo_submit` shell helper: submits a script and echoes the bare
/// scheduler job id.
///
/// Only PBS's `qsub` prints an id a dependency flag can consume directly.
/// SLURM needs `--parsable`; SGE and LSF print sentences. Chaining raw
/// submit output into `--dependency=afterok:` produced
/// `afterok:Submitted batch job 12345` and broke silently (issue #74
/// phase-1 item 1). The patterns mirror `parse_job_id` in
/// `oxo_flow_core::backend::cluster` so the wrapper and the live submit
/// path agree on what a job id is.
fn generate_submit_helper(backend: &ClusterBackend) -> String {
    let body = match backend {
        // `--parsable` prints a bare id, or `<id>;<cluster>` on federated
        // clusters — keep the id.
        ClusterBackend::Slurm => {
            "  out=$(sbatch --parsable \"$@\") || return $?\n  id=${out%%;*}\n"
        }
        // qsub already prints a bare id (`12345.headnode` is accepted
        // verbatim by `-W depend=`). Exactly one token, or fall through to
        // the error path — mashing multi-token output together would feed a
        // garbage id to the next job's `-W depend=`.
        ClusterBackend::Pbs => {
            "  out=$(qsub \"$@\") || return $?\n  set -- $out\n  if [ $# -eq 1 ]; then id=\"$1\"; else id=\"\"; fi\n"
        }
        // "Your job 12345 (\"align\") has been submitted"
        ClusterBackend::Sge => {
            "  out=$(qsub \"$@\") || return $?\n  id=$(printf '%s' \"$out\" | grep -oE 'Your job(-array)? [0-9]+' | grep -oE '[0-9]+$')\n"
        }
        // "Job <12345> is submitted to queue <normal>."
        ClusterBackend::Lsf => {
            "  out=$(bsub \"$@\") || return $?\n  id=$(printf '%s' \"$out\" | grep -oE 'Job <[0-9]+>' | grep -oE '[0-9]+')\n"
        }
    };

    let mut helper = String::new();
    helper.push_str("# Submit one script and echo its scheduler job id.\n");
    helper.push_str("oxo_submit() {\n");
    helper.push_str("  local out id\n");
    helper.push_str(body);
    helper.push_str("  if [ -z \"$id\" ]; then\n");
    helper.push_str("    echo \"oxo-flow: cannot parse job id from: $out\" >&2\n");
    helper.push_str("    return 1\n");
    helper.push_str("  fi\n");
    helper.push_str("  printf '%s' \"$id\"\n");
    helper.push_str("}\n\n");
    helper
}

/// Generate a submit wrapper script that handles job dependencies.
/// This script tracks job IDs and sets up proper dependency chains.
fn generate_submit_wrapper(
    backend: &ClusterBackend,
    order: &[String],
    dag: &WorkflowDag,
    output_dir: &Path,
) -> Result<String> {
    let mut script = String::new();
    script.push_str("#!/bin/bash\n");
    script.push_str("# Auto-generated dependency-aware submit script\n");
    script.push_str("# Generated by oxo-flow\n\n");
    script.push_str("set -e\n\n");
    script.push_str("# Track job IDs\ndeclare -A JOB_IDS\n\n");
    script.push_str(&generate_submit_helper(backend));

    // Generate submit commands for each rule in order
    for rule_name in order {
        let script_name = format!("{}.sh", rule_name);
        let script_path = output_dir.join(&script_name);

        // Get dependencies for this rule
        let deps = dag.dependencies(rule_name).unwrap_or_default();
        let dep_job_refs: Vec<String> = deps
            .iter()
            .map(|d| format!("${{JOB_IDS[{}]}}", d))
            .collect();

        script.push_str(&format!("echo 'Submitting {}...'\n", rule_name));

        // Add dependency specification if there are dependencies
        if !dep_job_refs.is_empty() {
            match backend {
                ClusterBackend::Slurm => {
                    let dep_str = dep_job_refs.join(":");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit --dependency=afterok:{} {})\n",
                        rule_name,
                        dep_str,
                        script_path.display()
                    ));
                }
                ClusterBackend::Pbs => {
                    // PBS uses -W depend=afterok:jobid
                    let dep_str = dep_job_refs.join(":");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -W depend=afterok:{} {})\n",
                        rule_name,
                        dep_str,
                        script_path.display()
                    ));
                }
                ClusterBackend::Sge => {
                    // SGE takes one comma-separated -hold_jid list; a repeated
                    // flag would keep only the last dependency.
                    let hold_jid = dep_job_refs.join(",");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -hold_jid {} {})\n",
                        rule_name,
                        hold_jid,
                        script_path.display()
                    ));
                }
                ClusterBackend::Lsf => {
                    // LSF uses -w 'ended(jobid)'. DOUBLE quotes in the
                    // generated script: single quotes would keep the
                    // ${JOB_IDS[..]} reference literal and the dependency
                    // chain would silently never chain (caught live on
                    // tx-ubuntu's mock bsub).
                    let dep_str = dep_job_refs
                        .iter()
                        .map(|d| format!("ended({})", d))
                        .collect::<Vec<_>>()
                        .join(" && ");
                    script.push_str(&format!(
                        "JOB_IDS[{}]=$(oxo_submit -w \"{}\" {})\n",
                        rule_name,
                        dep_str,
                        script_path.display()
                    ));
                }
            }
        } else {
            // No dependencies
            script.push_str(&format!(
                "JOB_IDS[{}]=$(oxo_submit {})\n",
                rule_name,
                script_path.display()
            ));
        }

        // Double quotes: the id has to expand, not print literally.
        script.push_str(&format!(
            "echo \"  Submitted {} as job ID: ${{JOB_IDS[{}]}}\"\n\n",
            rule_name, rule_name
        ));
    }

    script.push_str("echo 'All jobs submitted successfully!'\n");
    script.push_str("echo 'Job ID mapping:'\n");
    script.push_str("for name in \"${!JOB_IDS[@]}\"; do\n");
    script.push_str("  echo \"  $name: ${JOB_IDS[$name]}\"\n");
    script.push_str("done\n");

    Ok(script)
}

pub async fn cluster_command(action: ClusterAction) -> Result<()> {
    print_banner();
    match action {
        ClusterAction::Submit {
            workflow,
            backend,
            queue,
            account,
            walltime,
            extra_args,
            output,
            target,
            module,
            dry_run,
            with_dependencies,
        } => {
            // The backend is validated before any workflow is read: a typo
            // must not cost a parse, let alone reach a queue.
            let cluster_backend = resolve_backend(backend.as_deref())?;

            let mut config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            // Expand wildcards before the DAG is built, exactly as `run`
            // does (issue #74 phase 1). Without this a scatter rule stays a
            // single template and the generated script submits a literal
            // `{sample}` to the scheduler.
            config.apply_defaults();
            config
                .expand_wildcards()
                .context("failed to expand wildcard rules")?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

            // --module partial runs (issue #112 elasticity) — the same
            // resolution `run` and `dry-run` use: each module name resolves
            // to its rules plus the host producers of its declared concrete
            // inputs, unioned into --target for the ordering machinery below.
            let mut target = target;
            for m in &module {
                match config.module_closure(m) {
                    Some(names) => target.extend(names),
                    None => {
                        let known: Vec<&String> = config.module_rules.keys().collect();
                        return Err(anyhow::anyhow!(
                            "unknown module '{m}' — known modules: {}",
                            if known.is_empty() {
                                "(none — no [[include]] modules)".to_string()
                            } else {
                                known
                                    .iter()
                                    .map(|k| k.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        ));
                    }
                }
            }

            // Targeted cluster runs close over the INSTANTIATED DAG
            // (issue #247): when-gated instances never enter the set.
            let when_false_rules: std::collections::HashSet<String> = if target.is_empty() {
                std::collections::HashSet::new()
            } else {
                let wildcard_values: std::collections::HashMap<String, String> = config
                    .config
                    .iter()
                    .map(|(key, value)| {
                        let string_val = match value {
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (format!("config.{key}"), string_val)
                    })
                    .collect();
                config
                    .rules
                    .iter()
                    .filter(|rule| {
                        crate::commands::run_preview::when_condition_false(
                            rule,
                            &config,
                            &wildcard_values,
                        )
                    })
                    .map(|rule| rule.name.clone())
                    .collect()
            };
            let order = if target.is_empty() {
                dag.execution_order()?
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                let (filtered, skipped_targets) = dag
                    .execution_order_for_targets_skipping(&target_refs, &when_false_rules)
                    .with_context(|| "failed to resolve target rules")?;
                for skipped in &skipped_targets {
                    eprintln!(
                        "{} target '{skipped}' is when-gated false — it never runs; removed from the execution set (its upstream was pruned too)",
                        "Note:".yellow()
                    );
                }
                filtered
            };

            let cluster_config = oxo_flow_core::cluster::ClusterJobConfig {
                backend: cluster_backend,
                queue: queue.clone(),
                account: account.clone(),
                walltime: walltime.clone(),
                extra_args: extra_args.clone(),
            };

            // A dry run writes the very scripts a real submit would: an HPC
            // user asks for one to read the #SBATCH headers before the queue
            // sees them. Nothing is submitted — the wrapper is left out and
            // the summary says so (the old dry run generated nothing at all,
            // so there was nothing to review).
            std::fs::create_dir_all(&output)?;

            // The rendered scripts declare `#SBATCH --output=logs/<rule>.out`
            // and slurmd opens that file at job launch — before the script
            // body's `mkdir -p logs` runs. Create the directory now (issue #74
            // phase-1 note 2).
            if let Some(wf_dir) = workflow.parent() {
                std::fs::create_dir_all(wf_dir.join("logs"))?;
            }

            if dry_run {
                eprintln!(
                    "{} (dry-run) generating {} job scripts for {} rule instances — nothing is submitted",
                    "Cluster:".bold().yellow(),
                    cluster_backend,
                    order.len()
                );
            } else {
                eprintln!(
                    "{} Generating {} job scripts for {} rule instances",
                    "Cluster:".bold().cyan(),
                    cluster_backend,
                    order.len()
                );
            }

            // Create environment resolver for command wrapping
            let env_resolver = oxo_flow_core::environment::EnvironmentResolver::new();

            // Build config variable map for placeholder expansion
            let mut wildcard_values: HashMap<String, String> = HashMap::new();
            for (key, value) in &config.config {
                let string_val = match value {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                wildcard_values.insert(format!("config.{key}"), string_val);
            }

            for rule_name in &order {
                let rule = config
                    .get_rule(rule_name)
                    .ok_or_else(|| anyhow::anyhow!("rule '{}' not found in workflow", rule_name))?;

                let shell_cmd = match oxo_flow_core::executor::process::build_execution_command(
                    rule,
                    &wildcard_values,
                    &config.workflow.interpreter_map,
                    oxo_flow_core::scheduler::detect_system_limits(),
                ) {
                    Some(cmd) => cmd,
                    None => {
                        eprintln!(
                            "  {} {} — no shell command or script, skipping",
                            "⊘".yellow(),
                            rule_name
                        );
                        continue;
                    }
                };

                // Render through the ExecutorBackend trait (issue #78): the
                // command stays a thin render layer over the same directive
                // generator the live submit path uses.
                let wrapped_cmd = env_resolver
                    .wrap_command(
                        &shell_cmd,
                        &rule.environment,
                        Some(&rule.resources),
                        Path::new("."),
                    )
                    .map_err(|e| anyhow::anyhow!("environment wrapping failed: {}", e))?;
                let scheduled = oxo_flow_core::backend::ScheduledRule {
                    rule: rule.clone(),
                    // The standalone `cluster submit` command has no
                    // expansion templates to consult — an instance maps to
                    // itself.
                    template: rule_name.clone(),
                    shell_cmd: wrapped_cmd,
                    workdir: std::path::PathBuf::from("."),
                    dependencies: dag.dependencies(rule_name).unwrap_or_default(),
                    wildcard_values: wildcard_values.clone(),
                };
                let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                    cluster_backend,
                    cluster_config.clone(),
                );
                let script = executor.render_script(&scheduled)?;

                let script_path = output.join(format!("{rule_name}.sh"));
                std::fs::write(&script_path, &script)?;
                eprintln!("  {} {}", "✓".green(), script_path.display());
            }

            // Generate dependency-aware submit script if requested
            if with_dependencies && !dry_run {
                let submit_script =
                    generate_submit_wrapper(&cluster_backend, &order, &dag, &output)?;
                let submit_path = output.join("submit.sh");
                std::fs::write(&submit_path, submit_script)?;
                eprintln!(
                    "  {} {} (dependency-aware submit script)",
                    "✓".green(),
                    submit_path.display()
                );

                eprintln!(
                    "\n{} {} scripts written to {}",
                    "Done:".bold(),
                    order.len() + 1,
                    output.display()
                );
                eprintln!("  Submit with: bash {}", submit_path.display());
                eprintln!(
                    "  Or manually: {} {}/*.sh",
                    oxo_flow_core::cluster::submit_command(&cluster_backend),
                    output.display()
                );
            } else {
                eprintln!(
                    "\n{} {} scripts written to {}",
                    "Done:".bold(),
                    order.len(),
                    output.display()
                );
                if dry_run {
                    eprintln!("  DRY-RUN: review the scripts, then submit with:");
                    eprintln!(
                        "    {} {}/*.sh",
                        oxo_flow_core::cluster::submit_command(&cluster_backend),
                        output.display()
                    );
                } else {
                    eprintln!(
                        "  Submit with: {} {}/*.sh",
                        oxo_flow_core::cluster::submit_command(&cluster_backend),
                        output.display()
                    );
                }
            }
        }

        ClusterAction::Status { backend, job_ids } => {
            let cluster_backend = resolve_backend(backend.as_deref())?;

            if job_ids.is_empty() {
                // No ids: answer "what of MINE is in the queue right now" —
                // the natural first question after a submit, and the one the
                // old hard error left unanswered (the command used to bail
                // with "requires at least one job ID").
                let listing = query_my_jobs(&cluster_backend, |program, args| {
                    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let output = match std::process::Command::new(program).args(&arg_refs).output()
                    {
                        Ok(out) => out,
                        Err(e) => {
                            eprintln!("  Is {program} installed on this system?");
                            anyhow::bail!("Failed to execute status command: {e}");
                        }
                    };
                    if !output.status.success() {
                        // bjobs exits non-zero when the queue is empty;
                        // treat that as an empty answer, not a failure.
                        eprintln!(
                            "{} exited {} (no jobs listed)",
                            program,
                            output.status.code().unwrap_or(-1)
                        );
                        return Ok(String::new());
                    }
                    Ok(String::from_utf8_lossy(&output.stdout).to_string())
                })?;

                eprintln!(
                    "{} {} queued job(s)",
                    "Cluster:".bold().cyan(),
                    listing.len()
                );
                for (id, status) in &listing {
                    eprintln!("{}", status_line(id, *status));
                }
                // Machine-readable form on stdout, one `<id>\t<state>` per
                // job — human output stays on stderr, the codebase-wide
                // convention.
                for (id, status) in &listing {
                    println!("{id}\t{}", status.label());
                }
                return Ok(());
            }
            for (program, args) in
                oxo_flow_core::backend::cluster::status_invocations(&cluster_backend, &job_ids)
            {
                eprintln!(
                    "{} Executing '{} {}'...",
                    "Cluster:".bold().cyan(),
                    program,
                    args.join(" ")
                );
            }

            let mut report = query_status(&cluster_backend, &job_ids, |program, args| {
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let output = match std::process::Command::new(program).args(&arg_refs).output() {
                    Ok(out) => out,
                    Err(e) => {
                        eprintln!("  Is {program} installed on this system?");
                        anyhow::bail!("Failed to execute status command: {e}");
                    }
                };
                if !output.status.success() {
                    // A scheduler exits non-zero for "no such job" (qstat -j)
                    // while still naming the others; what it printed still
                    // answers for the ids it did list.
                    eprintln!("{} exited {}", program, output.status.code().unwrap_or(-1));
                }
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            })?;

            eprintln!("{} {} job(s):", "Cluster:".bold().cyan(), report.len());
            // The live queue only answers for jobs still in it. A finished
            // job is invisible to squeue/qstat, yet that is exactly the
            // state a user asks about most — layer the accounting store
            // over the Unknown verdicts, the same way the run driver does
            // (see the executor's terminal_status and issue #244's
            // no-slurmdbd story). A job with no accounting record either
            // keeps its Unknown ("finished, or an unknown id") below.
            let unknown: Vec<String> = report
                .iter()
                .filter(|(_, s)| *s == BackendJobStatus::Unknown)
                .map(|(id, _)| id.clone())
                .collect();
            if !unknown.is_empty() {
                let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                    cluster_backend,
                    ClusterJobConfig {
                        backend: cluster_backend,
                        queue: None,
                        account: None,
                        walltime: None,
                        extra_args: Vec::new(),
                    },
                );
                for id in &unknown {
                    if let Some(record) = executor.terminal_status(id).await {
                        let entry = report
                            .iter_mut()
                            .find(|(rid, _)| rid == id)
                            .expect("report came from the same id list");
                        entry.1 = record.status;
                    }
                }
            }
            for (id, status) in &report {
                eprintln!("{}", status_line(id, *status));
            }
            // Machine-readable form on stdout, one `<id>\t<state>` per job —
            // human output stays on stderr, the codebase-wide convention.
            for (id, status) in &report {
                println!("{id}\t{}", status.label());
            }
        }

        ClusterAction::Cancel { backend, job_ids } => {
            let cancel_cmd = oxo_flow_core::backend::cluster::cancel_command(&resolve_backend(
                backend.as_deref(),
            )?);

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
                            anyhow::bail!(
                                "Command failed with exit code: {}",
                                status.code().unwrap_or(-1)
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("  Is {} installed on this system?", cancel_cmd);
                        anyhow::bail!("Failed to execute cancel command: {}", e);
                    }
                }
            }
        }

        ClusterAction::Logs { backend, job_id } => {
            // Issue #67 §4: the last CLI stub. SLURM fetches a precise
            // accounting record (`sacct --format=JobID,State,ExitCode,
            // Elapsed,MaxRSS`); PBS/SGE/LSF stay best-effort (qstat -f /
            // qacct / bacct) — the same per-scheduler contract the
            // BackendDriver uses.
            let cluster_backend = resolve_backend(backend.as_deref())?;
            let executor = oxo_flow_core::backend::cluster::ClusterExecutor::new(
                cluster_backend,
                oxo_flow_core::cluster::ClusterJobConfig {
                    backend: cluster_backend,
                    queue: None,
                    account: None,
                    walltime: None,
                    extra_args: Vec::new(),
                },
            );
            let logs = executor
                .logs(&job_id)
                .await
                .context("failed to fetch job logs")?;
            if logs.trim().is_empty() {
                eprintln!(
                    "{} No accounting records found for job ID {}",
                    "Warning:".bold().yellow(),
                    job_id
                );
            } else {
                println!("{logs}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{cluster_command, query_my_jobs, query_status, resolve_backend_with, status_line};
    use crate::ClusterAction;
    use oxo_flow_core::backend::BackendJobStatus;
    use oxo_flow_core::cluster::ClusterBackend;
    use std::path::PathBuf;

    fn run(action: ClusterAction) -> anyhow::Result<()> {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(cluster_command(action))
    }

    fn workflow_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("wf.oxoflow");
        std::fs::write(
            &path,
            "[workflow]\nname = \"wf\"\n[[rules]]\nname = \"align\"\n\
             shell = \"true\"\noutput = [\"out.txt\"]\n",
        )
        .unwrap();
        path
    }

    fn submit_action(
        workflow: PathBuf,
        output: PathBuf,
        backend: &str,
        dry_run: bool,
    ) -> ClusterAction {
        ClusterAction::Submit {
            workflow,
            backend: Some(backend.to_string()),
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            output,
            target: Vec::new(),
            module: Vec::new(),
            dry_run,
            with_dependencies: true,
        }
    }

    #[test]
    fn unknown_backend_is_rejected_for_every_action() {
        // A typo used to fall back to SLURM silently — submit scripts for the
        // wrong scheduler, scancel where the user asked for qdel. An EXPLICIT
        // bad value must still be rejected even though an omitted flag now
        // defaults to slurm.
        let cases = vec![
            ClusterAction::Status {
                backend: Some("slrm".to_string()),
                job_ids: vec!["1".to_string()],
            },
            ClusterAction::Cancel {
                backend: Some("torque".to_string()),
                job_ids: vec!["1".to_string()],
            },
            ClusterAction::Logs {
                backend: Some("ge".to_string()),
                job_id: "1".to_string(),
            },
        ];
        for action in cases {
            let msg = run(action).unwrap_err().to_string();
            assert!(
                msg.starts_with("unknown cluster backend '"),
                "must name the offending backend: {msg}"
            );
            assert!(
                msg.contains("expected slurm, pbs, sge, or lsf"),
                "must list the accepted values: {msg}"
            );
        }
    }

    #[test]
    fn submit_rejects_an_unknown_backend_before_reading_the_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.oxoflow");
        let err = run(submit_action(
            missing,
            dir.path().join("scripts"),
            "slurm2",
            true,
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown cluster backend 'slurm2'"),
            "the backend is validated before anything else: {err}"
        );
    }

    #[test]
    fn parse_backend_accepts_every_documented_name() {
        for name in ["slurm", "SLURM", "pbs", "sge", "lsf"] {
            assert!(
                resolve_backend_with(Some(name), None).is_ok(),
                "{name} must resolve"
            );
        }
    }

    #[test]
    fn resolve_backend_falls_back_to_slurm_when_the_flag_is_omitted() {
        // The -b flag used to be required on every cluster action; typing it
        // on a SLURM site is pure ceremony. Omitted = slurm, unless the
        // environment names another scheduler.
        assert_eq!(
            resolve_backend_with(None, None).unwrap(),
            ClusterBackend::Slurm
        );
        assert_eq!(
            resolve_backend_with(Some("sge"), Some("pbs")).unwrap(),
            ClusterBackend::Sge
        );
    }

    #[test]
    fn resolve_backend_honors_the_environment_default() {
        // A PBS-site user sets OXO_FLOW_CLUSTER_BACKEND once and never types
        // -b again. Whitespace around the value is tolerated (export typos),
        // an empty value falls through to the slurm default, and an explicit
        // flag still wins over the environment.
        assert_eq!(
            resolve_backend_with(None, Some("pbs")).unwrap(),
            ClusterBackend::Pbs
        );
        assert_eq!(
            resolve_backend_with(Some("lsf"), Some("pbs")).unwrap(),
            ClusterBackend::Lsf
        );
        assert_eq!(
            resolve_backend_with(None, Some("  sge  ")).unwrap(),
            ClusterBackend::Sge
        );
        assert_eq!(
            resolve_backend_with(None, Some("")).unwrap(),
            ClusterBackend::Slurm
        );
        assert_eq!(
            resolve_backend_with(None, Some("   ")).unwrap(),
            ClusterBackend::Slurm
        );

        // And a garbage environment value is an error naming the value —
        // never a silent slurm fallback.
        let msg = resolve_backend_with(None, Some("torque"))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("unknown cluster backend 'torque'"), "{msg}");
    }

    #[test]
    fn query_my_jobs_parses_the_listing() {
        // The seam question: exactly the user-listing invocation, and the
        // returned rows are parsed (id, status) — header lines skipped.
        let listing = query_my_jobs(&ClusterBackend::Slurm, |program, args| {
            assert_eq!(program, "squeue");
            assert_eq!(args[0], "-u");
            assert_eq!(args[2], "--noheader");
            Ok("101|PENDING\n202|RUNNING\n".to_string())
        })
        .unwrap();
        assert_eq!(
            listing,
            vec![
                ("101".to_string(), BackendJobStatus::Pending),
                ("202".to_string(), BackendJobStatus::Running),
            ]
        );
    }

    #[test]
    fn query_my_jobs_surfaces_scheduler_failures() {
        let err = query_my_jobs(&ClusterBackend::Slurm, |_, _| {
            Err(anyhow::anyhow!("squeue: slurm_load_jobs error"))
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("slurm_load_jobs"),
            "the scheduler's own words must reach the user: {err}"
        );
    }

    #[test]
    fn status_answers_every_requested_id() {
        // SGE asks once per job: two invocations must not lose an answer, and
        // an id the scheduler is silent about reads as unknown — not missing.
        let report = query_status(
            &oxo_flow_core::cluster::ClusterBackend::Sge,
            &["1".to_string(), "2".to_string()],
            |program, args| {
                assert_eq!(program, "qstat");
                // Job 1 is running; job 2 already left the queue — qstat
                // prints nothing for it.
                match args[1].as_str() {
                    "1" => Ok("job_number: 1\nstate: r\n".to_string()),
                    _ => Ok(String::new()),
                }
            },
        )
        .unwrap();
        assert_eq!(report.len(), 2);
        assert_eq!(report[0], ("1".to_string(), BackendJobStatus::Running));
        assert_eq!(report[1], ("2".to_string(), BackendJobStatus::Unknown));
    }

    #[test]
    fn status_surfaces_scheduler_failures() {
        let err = query_status(
            &oxo_flow_core::cluster::ClusterBackend::Slurm,
            &["1".to_string()],
            |_, _| Err(anyhow::anyhow!("squeue: slurm_load_jobs error")),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("slurm_load_jobs"),
            "the scheduler's own words must reach the user: {err}"
        );
    }

    #[test]
    fn status_line_explains_a_missing_job() {
        assert_eq!(
            status_line("123", BackendJobStatus::Unknown),
            "  123: not in the queue (finished, or an unknown id)"
        );
        assert_eq!(
            status_line("123", BackendJobStatus::Running),
            "  123: running"
        );
    }

    #[test]
    fn dry_run_writes_the_scripts_but_no_wrapper() {
        // A dry run exists so an HPC user can read the #SBATCH headers before
        // the queue does — it has to leave them on disk, and must leave
        // nothing behind that would submit them.
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        let workflow = workflow_file(dir.path());
        run(submit_action(
            workflow.clone(),
            scripts.clone(),
            "slurm",
            true,
        ))
        .unwrap();
        let body = std::fs::read_to_string(scripts.join("align.sh")).unwrap();
        assert!(
            body.contains("#SBATCH --job-name=align"),
            "the generated header must be reviewable: {body}"
        );
        assert!(
            body.contains("#SBATCH --chdir="),
            "the generated header pins the working directory: {body}"
        );
        assert!(
            !scripts.join("submit.sh").exists(),
            "a dry run must not leave a submit wrapper behind"
        );

        // The real path keeps the wrapper it always wrote.
        std::fs::remove_dir_all(&scripts).unwrap();
        run(submit_action(workflow, scripts.clone(), "slurm", false)).unwrap();
        assert!(scripts.join("submit.sh").exists());
    }
}
