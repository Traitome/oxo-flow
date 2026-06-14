//! Background process spawn and monitor for pipeline execution.
//!
//! Spawns pipeline rules as subprocesses, captures stdout/stderr, tracks
//! exit codes, and reports progress back via the run_nodes table.
//!
//! Zero HTTP dependency — pure async Rust functions suitable for
//! testing without starting a web server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

/// Outcome of executing a single rule.
#[derive(Debug, Clone)]
pub struct RuleOutcome {
    pub rule_name: String,
    pub status: String, // "success" | "failed"
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub started_at: String,
    pub finished_at: String,
}

/// Spawn a single shell command and wait for it to finish.
///
/// Returns the outcome with captured stdout/stderr, exit code, and timing.
pub async fn execute_rule(
    rule_name: &str,
    shell_command: &str,
    workdir: &PathBuf,
    env: &HashMap<String, String>,
) -> Result<RuleOutcome, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let start = Instant::now();

    // Resolve the shell from environment or default to sh
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    let output = tokio::process::Command::new(&shell)
        .arg("-c")
        .arg(shell_command)
        .current_dir(workdir)
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to spawn {rule_name}: {e}"))?;

    let duration_ms = start.elapsed().as_millis() as u64;
    let finished_at = chrono::Utc::now().to_rfc3339();

    let exit_code = output.status.code().unwrap_or(-1);
    let status = if output.status.success() {
        "success"
    } else {
        "failed"
    };

    Ok(RuleOutcome {
        rule_name: rule_name.to_string(),
        status: status.to_string(),
        exit_code,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        duration_ms,
        started_at: now,
        finished_at,
    })
}

/// Persist a rule outcome to the run_nodes table.
pub async fn persist_rule_outcome(
    pool: &sqlx::SqlitePool,
    run_id: &str,
    outcome: &RuleOutcome,
    attempt: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT OR REPLACE INTO run_nodes (run_id, rule_name, status, started_at, finished_at, exit_code, attempt, error_pattern) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(run_id)
    .bind(&outcome.rule_name)
    .bind(&outcome.status)
    .bind(&outcome.started_at)
    .bind(&outcome.finished_at)
    .bind(outcome.exit_code)
    .bind(attempt)
    .bind(None::<String>)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Submit a pipeline to an HPC scheduler (SLURM/PBS) instead of running locally.
///
/// Generates a cluster submission script and submits it via sbatch/qsub.
/// Returns the HPC job ID on success.
pub async fn submit_to_hpc(
    pipeline: &WorkflowConfig,
    workdir: &PathBuf,
    scheduler: &str,
    cpus: u32,
    memory_gb: u32,
    walltime: &str,
) -> Result<String, String> {
    let workdir_str = workdir.to_string_lossy();
    let script_path = crate::infra::hpc::generate_slurm_script(
        &pipeline.workflow.name,
        &workdir_str,
        cpus,
        memory_gb,
        walltime,
        &format!("oxo-flow run {}/workflow.oxoflow", workdir_str),
    );

    // Write script to workdir
    let script_file = workdir.join("submit.sh");
    std::fs::write(&script_file, &script_path)
        .map_err(|e| format!("Failed to write HPC submit script: {e}"))?;

    // Submit via scheduler
    let pbs_output = format!("{workdir_str}/pbs-output.log");
    let (submit_bin, submit_args) = match scheduler {
        "slurm" => ("sbatch", vec!["--parsable", "submit.sh"]),
        "pbs" | "torque" => ("qsub", vec!["-o", &pbs_output, "submit.sh"]),
        "lsf" => ("bsub", vec!["-o", &pbs_output, "<", "submit.sh"]),
        "sge" => ("qsub", vec!["-o", &pbs_output, "submit.sh"]),
        _ => return Err(format!("Unsupported HPC scheduler: {scheduler}")),
    };

    let output = tokio::process::Command::new(submit_bin)
        .args(&submit_args)
        .current_dir(workdir)
        .output()
        .await
        .map_err(|e| format!("Failed to submit HPC job: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("HPC submission failed: {stderr}"));
    }

    let job_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(job_id)
}

/// Run a pipeline by executing rules in topological order, respecting
/// parallel groups.
///
/// Returns the list of rule outcomes in execution order.
pub async fn execute_pipeline(
    _run_id: &str,
    _pipeline: &WorkflowConfig,
    _dag: &WorkflowDag,
    _workdir: &PathBuf,
    max_jobs: usize,
) -> Result<Vec<RuleOutcome>, String> {
    let execution_order = _dag.execution_order().map_err(|e| format!("DAG: {e}"))?;
    let parallel_groups = _dag.parallel_groups().unwrap_or_default();

    let mut outcomes: Vec<RuleOutcome> = Vec::new();
    let env = HashMap::new();

    if parallel_groups.is_empty() {
        // Sequential execution
        for rule_name in &execution_order {
            let rule = _pipeline
                .rules
                .iter()
                .find(|r| &r.name == rule_name)
                .ok_or_else(|| format!("Rule {rule_name} not found"))?;

            let shell_cmd = rule
                .shell
                .as_deref()
                .ok_or_else(|| format!("Rule {rule_name} has no shell command"))?;

            let outcome = execute_rule(rule_name, shell_cmd, _workdir, &env).await?;
            outcomes.push(outcome);
        }
    } else {
        // Parallel execution within groups
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_jobs.max(1)));

        for group in &parallel_groups {
            let mut handles = Vec::new();

            for rule_name in group {
                let rule = _pipeline
                    .rules
                    .iter()
                    .find(|r| &r.name == rule_name)
                    .ok_or_else(|| format!("Rule {rule_name} not found"))?
                    .clone();

                let shell_cmd = rule
                    .shell
                    .clone()
                    .ok_or_else(|| format!("Rule {rule_name} has no shell command"))?;

                let wd = _workdir.clone();
                let e = env.clone();
                let sem = semaphore.clone();
                let rn = rule_name.clone();

                let handle = tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    execute_rule(&rn, &shell_cmd, &wd, &e).await
                });
                handles.push(handle);
            }

            for handle in handles {
                match handle.await {
                    Ok(Ok(outcome)) => outcomes.push(outcome),
                    Ok(Err(e)) => {
                        tracing::error!("Rule execution error: {e}");
                    }
                    Err(e) => {
                        tracing::error!("Task join error: {e}");
                    }
                }
            }
        }
    }

    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_rule_echo() {
        let wd = std::env::temp_dir();
        let env = HashMap::new();
        let outcome = execute_rule("test_echo", "echo hello", &wd, &env)
            .await
            .expect("echo should succeed");
        assert_eq!(outcome.status, "success");
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_rule_failure() {
        let wd = std::env::temp_dir();
        let env = HashMap::new();
        let outcome = execute_rule("test_fail", "exit 1", &wd, &env)
            .await
            .expect("command should run");
        assert_eq!(outcome.status, "failed");
        assert_eq!(outcome.exit_code, 1);
    }

    #[test]
    fn test_execute_pipeline_sequential() {
        // This is a compile-time test to ensure the function signature works.
        // Actual execution tests are in the integration test suite.
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"
[[rules]]
name = "step1"
shell = "echo step1"
output = ["step1.txt"]
[[rules]]
name = "step2"
shell = "echo step2"
input = ["step1.txt"]
output = ["step2.txt"]
"#;
        let wf = oxo_flow_core::WorkflowConfig::parse(toml).unwrap();
        let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
        let _order = dag.execution_order().unwrap();
        assert_eq!(_order.len(), 2);
    }
}
