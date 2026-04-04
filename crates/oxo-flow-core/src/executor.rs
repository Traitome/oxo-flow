//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
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

// ---------------------------------------------------------------------------
// Benchmark & checkpoint support
// ---------------------------------------------------------------------------

/// Performance metrics recorded after executing a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRecord {
    /// Name of the rule that was benchmarked.
    pub rule: String,
    /// Wall-clock time in seconds.
    pub wall_time_secs: f64,
    /// Peak resident memory in megabytes (placeholder — not yet measured).
    pub max_memory_mb: Option<u64>,
    /// Total CPU seconds consumed (placeholder — not yet measured).
    pub cpu_seconds: Option<f64>,
}

/// Persistent checkpoint state for resumable workflow execution.
///
/// Tracks which rules have completed or failed so that a restarted workflow
/// can skip already-finished work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointState {
    /// Rules that completed successfully.
    pub completed_rules: HashSet<String>,
    /// Rules that failed during execution.
    pub failed_rules: HashSet<String>,
    /// Benchmark records keyed by rule name.
    pub benchmarks: HashMap<String, BenchmarkRecord>,
}

impl CheckpointState {
    /// Create a new, empty checkpoint state.
    pub fn new() -> Self {
        Self {
            completed_rules: HashSet::new(),
            failed_rules: HashSet::new(),
            benchmarks: HashMap::new(),
        }
    }

    /// Mark a rule as successfully completed and store its benchmark.
    pub fn mark_completed(&mut self, rule: &str, benchmark: BenchmarkRecord) {
        self.completed_rules.insert(rule.to_string());
        self.failed_rules.remove(rule);
        self.benchmarks.insert(rule.to_string(), benchmark);
    }

    /// Mark a rule as failed.
    pub fn mark_failed(&mut self, rule: &str) {
        self.failed_rules.insert(rule.to_string());
        self.completed_rules.remove(rule);
    }

    /// Returns `true` if the rule finished successfully.
    pub fn is_completed(&self, rule: &str) -> bool {
        self.completed_rules.contains(rule)
    }

    /// Returns `true` if the rule should be skipped (i.e., it already completed).
    pub fn should_skip(&self, rule: &str) -> bool {
        self.is_completed(rule)
    }

    /// Serialize the checkpoint state to a JSON string.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| OxoFlowError::Config {
            message: format!("failed to serialize checkpoint: {e}"),
        })
    }

    /// Deserialize a checkpoint state from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| OxoFlowError::Config {
            message: format!("failed to deserialize checkpoint: {e}"),
        })
    }
}

impl Default for CheckpointState {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if `source` is newer than `target` (Make-style freshness check).
///
/// If either file does not exist or its metadata cannot be read, returns `false`.
pub fn file_is_newer(source: &Path, target: &Path) -> bool {
    let source_modified = match std::fs::metadata(source).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let target_modified = match std::fs::metadata(target).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    source_modified > target_modified
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

    // -- BenchmarkRecord tests -----------------------------------------------

    #[test]
    fn benchmark_record_creation() {
        let b = BenchmarkRecord {
            rule: "fastqc".to_string(),
            wall_time_secs: 42.5,
            max_memory_mb: Some(1024),
            cpu_seconds: Some(38.0),
        };
        assert_eq!(b.rule, "fastqc");
        assert!((b.wall_time_secs - 42.5).abs() < f64::EPSILON);
        assert_eq!(b.max_memory_mb, Some(1024));
        assert_eq!(b.cpu_seconds, Some(38.0));
    }

    #[test]
    fn benchmark_record_placeholder_fields() {
        let b = BenchmarkRecord {
            rule: "bwa".to_string(),
            wall_time_secs: 10.0,
            max_memory_mb: None,
            cpu_seconds: None,
        };
        assert!(b.max_memory_mb.is_none());
        assert!(b.cpu_seconds.is_none());
    }

    // -- CheckpointState tests -----------------------------------------------

    #[test]
    fn checkpoint_mark_completed() {
        let mut state = CheckpointState::new();
        let bench = BenchmarkRecord {
            rule: "step1".to_string(),
            wall_time_secs: 5.0,
            max_memory_mb: None,
            cpu_seconds: None,
        };
        state.mark_completed("step1", bench);
        assert!(state.is_completed("step1"));
        assert!(state.should_skip("step1"));
        assert!(!state.failed_rules.contains("step1"));
    }

    #[test]
    fn checkpoint_mark_failed() {
        let mut state = CheckpointState::new();
        state.mark_failed("step2");
        assert!(!state.is_completed("step2"));
        assert!(!state.should_skip("step2"));
        assert!(state.failed_rules.contains("step2"));
    }

    #[test]
    fn checkpoint_completed_clears_failed() {
        let mut state = CheckpointState::new();
        state.mark_failed("step1");
        assert!(state.failed_rules.contains("step1"));

        let bench = BenchmarkRecord {
            rule: "step1".to_string(),
            wall_time_secs: 3.0,
            max_memory_mb: None,
            cpu_seconds: None,
        };
        state.mark_completed("step1", bench);
        assert!(state.is_completed("step1"));
        assert!(!state.failed_rules.contains("step1"));
    }

    #[test]
    fn checkpoint_failed_clears_completed() {
        let mut state = CheckpointState::new();
        let bench = BenchmarkRecord {
            rule: "step1".to_string(),
            wall_time_secs: 1.0,
            max_memory_mb: None,
            cpu_seconds: None,
        };
        state.mark_completed("step1", bench);
        state.mark_failed("step1");
        assert!(!state.is_completed("step1"));
        assert!(state.failed_rules.contains("step1"));
    }

    #[test]
    fn checkpoint_json_round_trip() {
        let mut state = CheckpointState::new();
        state.mark_completed(
            "align",
            BenchmarkRecord {
                rule: "align".to_string(),
                wall_time_secs: 120.0,
                max_memory_mb: Some(8192),
                cpu_seconds: Some(110.0),
            },
        );
        state.mark_failed("variant_call");

        let json = state.to_json().unwrap();
        let restored = CheckpointState::from_json(&json).unwrap();

        assert!(restored.is_completed("align"));
        assert!(restored.failed_rules.contains("variant_call"));
        assert!(!restored.is_completed("variant_call"));

        let bench = restored.benchmarks.get("align").unwrap();
        assert!((bench.wall_time_secs - 120.0).abs() < f64::EPSILON);
        assert_eq!(bench.max_memory_mb, Some(8192));
    }

    #[test]
    fn checkpoint_default_is_empty() {
        let state = CheckpointState::default();
        assert!(state.completed_rules.is_empty());
        assert!(state.failed_rules.is_empty());
        assert!(state.benchmarks.is_empty());
    }

    // -- file_is_newer tests -------------------------------------------------

    #[test]
    fn file_is_newer_nonexistent_returns_false() {
        let source = Path::new("nonexistent_src_12345.txt");
        let target = Path::new("nonexistent_tgt_12345.txt");
        assert!(!file_is_newer(source, target));
    }

    #[test]
    fn file_is_newer_with_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("older.txt");
        let newer = dir.path().join("newer.txt");

        std::fs::write(&older, "old").unwrap();
        // Sleep briefly to ensure different modification times.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&newer, "new").unwrap();

        assert!(file_is_newer(&newer, &older));
        assert!(!file_is_newer(&older, &newer));
    }

    #[test]
    fn file_is_newer_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("exists.txt");
        std::fs::write(&existing, "data").unwrap();

        assert!(!file_is_newer(Path::new("no_such_file.txt"), &existing));
    }

    #[test]
    fn file_is_newer_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("exists.txt");
        std::fs::write(&existing, "data").unwrap();

        assert!(!file_is_newer(&existing, Path::new("no_such_file.txt")));
    }
}
