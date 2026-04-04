//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;

/// Status of a job in the execution pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Waiting for dependencies to complete.
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Success,
    /// Failed with an error.
    Failed,
    /// Skipped (e.g., outputs already up-to-date).
    Skipped,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Record of a single job execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    /// Rule name.
    pub rule: String,

    /// Job status.
    pub status: JobStatus,

    /// Start time.
    pub started_at: Option<DateTime<Utc>>,

    /// End time.
    pub finished_at: Option<DateTime<Utc>>,

    /// Exit code (if completed).
    pub exit_code: Option<i32>,

    /// Standard output (truncated if large).
    pub stdout: Option<String>,

    /// Standard error (truncated if large).
    pub stderr: Option<String>,

    /// Shell command that was executed.
    pub command: Option<String>,
}

/// Configuration for the executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of concurrent jobs.
    pub max_jobs: usize,

    /// Whether to run in dry-run mode (no actual execution).
    pub dry_run: bool,

    /// Working directory for execution.
    pub workdir: std::path::PathBuf,

    /// Whether to keep going on errors.
    pub keep_going: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::current_dir().unwrap_or_default(),
            keep_going: false,
        }
    }
}

/// Local process executor for running workflow rules.
pub struct LocalExecutor {
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
}

impl LocalExecutor {
    /// Create a new local executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_jobs));
        Self { config, semaphore }
    }

    /// Execute a single rule as a local process.
    pub async fn execute_rule(
        &self,
        rule: &Rule,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<JobRecord> {
        let mut record = JobRecord {
            rule: rule.name.clone(),
            status: JobStatus::Running,
            started_at: Some(Utc::now()),
            finished_at: None,
            exit_code: None,
            stdout: None,
            stderr: None,
            command: None,
        };

        let shell_cmd = match &rule.shell {
            Some(cmd) => {
                // Expand wildcards in the command
                let mut expanded = cmd.clone();
                for (key, value) in wildcard_values {
                    expanded = expanded.replace(&format!("{{{key}}}"), value);
                }
                expanded
            }
            None => {
                record.status = JobStatus::Skipped;
                record.finished_at = Some(Utc::now());
                return Ok(record);
            }
        };

        record.command = Some(shell_cmd.clone());

        if self.config.dry_run {
            tracing::info!(rule = %rule.name, command = %shell_cmd, "dry-run");
            record.status = JobStatus::Skipped;
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Acquire concurrency permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: rule.name.clone(),
                message: format!("semaphore error: {e}"),
            })?;

        tracing::info!(rule = %rule.name, threads = %rule.effective_threads(), "executing");

        let output = Command::new("sh")
            .arg("-c")
            .arg(&shell_cmd)
            .current_dir(&self.config.workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: rule.name.clone(),
                message: e.to_string(),
            })?;

        record.finished_at = Some(Utc::now());
        record.exit_code = output.status.code();
        record.stdout = Some(String::from_utf8_lossy(&output.stdout).to_string());
        record.stderr = Some(String::from_utf8_lossy(&output.stderr).to_string());

        if output.status.success() {
            record.status = JobStatus::Success;
            tracing::info!(rule = %rule.name, "completed successfully");
        } else {
            record.status = JobStatus::Failed;
            let code = output.status.code().unwrap_or(-1);
            tracing::error!(rule = %rule.name, code = %code, "failed");
            if !self.config.keep_going {
                return Err(OxoFlowError::TaskFailed {
                    rule: rule.name.clone(),
                    code,
                });
            }
        }

        Ok(record)
    }

    /// Dry-run a list of rules, printing what would be executed.
    pub fn dry_run_rules(&self, rules: &[Rule]) -> Vec<JobRecord> {
        rules
            .iter()
            .map(|rule| {
                let command = rule.shell.clone();
                tracing::info!(
                    rule = %rule.name,
                    command = ?command,
                    threads = %rule.effective_threads(),
                    env = %rule.environment.kind(),
                    "would execute"
                );

                JobRecord {
                    rule: rule.name.clone(),
                    status: JobStatus::Skipped,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};

    fn make_rule(name: &str, shell: &str) -> Rule {
        Rule {
            name: name.to_string(),
            input: vec![],
            output: vec![],
            shell: Some(shell.to_string()),
            script: None,
            threads: None,
            memory: None,
            resources: Resources::default(),
            environment: EnvironmentSpec::default(),
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
        }
    }

    #[test]
    fn job_status_display() {
        assert_eq!(JobStatus::Pending.to_string(), "pending");
        assert_eq!(JobStatus::Running.to_string(), "running");
        assert_eq!(JobStatus::Success.to_string(), "success");
        assert_eq!(JobStatus::Failed.to_string(), "failed");
        assert_eq!(JobStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn dry_run_rules() {
        let config = ExecutorConfig {
            dry_run: true,
            ..Default::default()
        };
        let executor = LocalExecutor::new(config);
        let rules = vec![
            make_rule("step1", "echo hello"),
            make_rule("step2", "echo world"),
        ];

        let records = executor.dry_run_rules(&rules);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, JobStatus::Skipped);
        assert_eq!(records[1].status, JobStatus::Skipped);
    }

    #[tokio::test]
    async fn execute_echo() {
        let config = ExecutorConfig {
            max_jobs: 2,
            dry_run: false,
            workdir: std::env::temp_dir(),
            keep_going: false,
        };
        let executor = LocalExecutor::new(config);
        let rule = make_rule("echo_test", "echo hello_oxoflow");

        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(record.status, JobStatus::Success);
        assert!(record.stdout.unwrap().contains("hello_oxoflow"));
    }

    #[tokio::test]
    async fn execute_dry_run() {
        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: true,
            workdir: std::env::temp_dir(),
            keep_going: false,
        };
        let executor = LocalExecutor::new(config);
        let rule = make_rule("dry_test", "echo should_not_run");

        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(record.status, JobStatus::Skipped);
    }

    #[tokio::test]
    async fn execute_failing_command() {
        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::temp_dir(),
            keep_going: true,
        };
        let executor = LocalExecutor::new(config);
        let rule = make_rule("fail_test", "exit 42");

        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(record.status, JobStatus::Failed);
        assert_eq!(record.exit_code, Some(42));
    }

    #[tokio::test]
    async fn execute_wildcard_expansion() {
        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::temp_dir(),
            keep_going: false,
        };
        let executor = LocalExecutor::new(config);
        let rule = make_rule("wildcard_test", "echo {sample}");

        let mut values = HashMap::new();
        values.insert("sample".to_string(), "TUMOR_01".to_string());

        let record = executor.execute_rule(&rule, &values).await.unwrap();
        assert_eq!(record.status, JobStatus::Success);
        assert!(record.stdout.unwrap().contains("TUMOR_01"));
    }
}
