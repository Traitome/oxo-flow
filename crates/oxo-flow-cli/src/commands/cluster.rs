use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::collections::HashMap;

use crate::ClusterAction;
use crate::commands::print_banner;

pub async fn cluster_command(action: ClusterAction) -> Result<()> {
    print_banner();
    match action {
        ClusterAction::Submit {
            workflow,
            backend,
            queue,
            account,
            output,
            target: _,
            dry_run,
        } => {
            let config = WorkflowConfig::from_file(&workflow)
                .with_context(|| format!("failed to parse {}", workflow.display()))?;

            let dag =
                WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

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

            if dry_run {
                eprintln!(
                    "{} (dry-run) would generate {} job scripts for {} rules",
                    "Cluster:".bold().yellow(),
                    backend,
                    order.len()
                );
                return Ok(());
            }

            std::fs::create_dir_all(&output)?;

            eprintln!(
                "{} Generating {} job scripts for {} rules",
                "Cluster:".bold().cyan(),
                backend,
                order.len()
            );

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
                let rule = config.get_rule(rule_name).unwrap();

                let shell_cmd = match oxo_flow_core::executor::process::build_execution_command(
                    rule,
                    &wildcard_values,
                    &config.workflow.interpreter_map,
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

                // Generate script with environment wrapping
                let script = oxo_flow_core::cluster::generate_submit_script_with_env(
                    &cluster_backend,
                    rule,
                    &shell_cmd,
                    &cluster_config,
                    &env_resolver,
                )
                .map_err(|e| anyhow::anyhow!("environment wrapping failed: {}", e))?;

                let script_path = output.join(format!("{rule_name}.sh"));
                std::fs::write(&script_path, &script)?;
                eprintln!("  {} {}", "✓".green(), script_path.display());
            }

            eprintln!(
                "\n{} {} scripts written to {}",
                "Done:".bold(),
                order.len(),
                output.display()
            );
            eprintln!(
                "  Submit with: {} {}/*.sh",
                oxo_flow_core::cluster::submit_command(&cluster_backend),
                output.display()
            );
        }

        ClusterAction::Status { backend, job_ids } => {
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
            let mut args: Vec<&str> = parts.collect();

            for id in &job_ids {
                args.push(id);
            }

            match std::process::Command::new(program).args(&args).status() {
                Ok(status) => {
                    if !status.success() {
                        anyhow::bail!(
                            "Command failed with exit code: {}",
                            status.code().unwrap_or(-1)
                        );
                    }
                }
                Err(e) => {
                    eprintln!("  Is {} installed on this system?", program);
                    anyhow::bail!("Failed to execute status command: {}", e);
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

        ClusterAction::Logs { backend: _, job_id } => {
            eprintln!(
                "{} Logs for job ID {} not yet implemented",
                "⚠️".yellow(),
                job_id
            );
        }
    }
    Ok(())
}
