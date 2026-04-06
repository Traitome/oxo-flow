//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use crate::environment::EnvironmentResolver;
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

/// Structured event emitted during workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionEvent {
    /// Workflow execution started.
    WorkflowStarted {
        workflow_name: String,
        total_rules: usize,
    },
    /// A rule started execution.
    RuleStarted {
        rule: String,
        command: Option<String>,
    },
    /// A rule completed.
    RuleCompleted {
        rule: String,
        status: JobStatus,
        duration_ms: u64,
    },
    /// A rule was skipped.
    RuleSkipped { rule: String, reason: String },
    /// Workflow execution completed.
    WorkflowCompleted {
        total_duration_ms: u64,
        succeeded: usize,
        failed: usize,
        skipped: usize,
    },
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

    /// Number of retries attempted so far.
    #[serde(default)]
    pub retries: u32,

    /// Timeout configured for this job (not serializable; lives only in memory).
    #[serde(skip)]
    pub timeout: Option<std::time::Duration>,
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

    /// Number of times to retry a failed job before giving up.
    pub retry_count: u32,

    /// Optional timeout per job.
    pub timeout: Option<std::time::Duration>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::current_dir().unwrap_or_default(),
            keep_going: false,
            retry_count: 0,
            timeout: None,
        }
    }
}

/// Local process executor for running workflow rules.
pub struct LocalExecutor {
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
    env_resolver: EnvironmentResolver,
}

impl LocalExecutor {
    /// Create a new local executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_jobs));
        let env_resolver = EnvironmentResolver::new();
        Self {
            config,
            semaphore,
            env_resolver,
        }
    }

    /// Wrap a command through the environment resolver, falling back to the
    /// original command on error and emitting a warning.
    fn resolve_command(&self, command: &str, rule: &Rule) -> String {
        match self.env_resolver.wrap_command(command, &rule.environment) {
            Ok(wrapped) => wrapped,
            Err(e) => {
                tracing::warn!(
                    rule = %rule.name,
                    error = %e,
                    "environment wrapping failed, falling back to original command"
                );
                command.to_string()
            }
        }
    }

    /// Execute a single rule as a local process.
    #[must_use = "executing a rule returns a Result that must be used"]
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
            retries: 0,
            timeout: self.config.timeout,
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

        // Wrap the command through the environment resolver
        let shell_cmd = self.resolve_command(&shell_cmd, rule);

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

        let max_attempts = 1 + self.config.retry_count;
        for attempt in 0..max_attempts {
            if attempt > 0 {
                tracing::warn!(
                    rule = %rule.name,
                    attempt = attempt + 1,
                    max_attempts = max_attempts,
                    "retrying failed command"
                );
                record.retries = attempt;
            }

            let cmd_future = Command::new("sh")
                .arg("-c")
                .arg(&shell_cmd)
                .current_dir(&self.config.workdir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();

            let cmd_result = if let Some(duration) = self.config.timeout {
                match tokio::time::timeout(duration, cmd_future).await {
                    Ok(inner) => inner,
                    Err(_) => {
                        record.finished_at = Some(Utc::now());
                        record.status = JobStatus::Failed;
                        record.stderr = Some(format!(
                            "command timed out after {duration:?} for rule '{}'",
                            rule.name
                        ));
                        tracing::error!(
                            rule = %rule.name,
                            timeout = ?duration,
                            "command timed out"
                        );
                        if !self.config.keep_going {
                            return Err(OxoFlowError::Execution {
                                rule: rule.name.clone(),
                                message: format!("command timed out after {duration:?}"),
                            });
                        }
                        return Ok(record);
                    }
                }
            } else {
                cmd_future.await
            };

            let output = cmd_result.map_err(|e| OxoFlowError::Execution {
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

                if !self.config.dry_run {
                    let missing = validate_outputs(rule, &self.config.workdir);
                    for path in &missing {
                        tracing::warn!(
                            rule = %rule.name,
                            path = %path,
                            "expected output file not found after execution"
                        );
                    }
                }

                return Ok(record);
            }

            // Command failed — retry if attempts remain
            let code = output.status.code().unwrap_or(-1);
            if attempt + 1 < max_attempts {
                tracing::warn!(
                    rule = %rule.name,
                    code = %code,
                    attempt = attempt + 1,
                    max_attempts = max_attempts,
                    "command failed, will retry"
                );
            } else {
                record.status = JobStatus::Failed;
                tracing::error!(rule = %rule.name, code = %code, "failed");
                if !self.config.keep_going {
                    return Err(OxoFlowError::TaskFailed {
                        rule: rule.name.clone(),
                        code,
                    });
                }
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
                let wrapped = command
                    .as_deref()
                    .map(|cmd| self.resolve_command(cmd, rule));
                tracing::info!(
                    rule = %rule.name,
                    command = ?command,
                    wrapped_command = ?wrapped,
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
                    command: wrapped.or(command),
                    retries: 0,
                    timeout: self.config.timeout,
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
    #[must_use = "serialization returns a Result that must be used"]
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| OxoFlowError::Config {
            message: format!("failed to serialize checkpoint: {e}"),
        })
    }

    /// Deserialize a checkpoint state from a JSON string.
    #[must_use = "deserialization returns a Result that must be used"]
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

/// Evaluate a simple `when` condition string against workflow config values.
///
/// Supports:
///  - `"config.<key>"` → true if key exists and is truthy
///  - `"true"` / `"false"` literal
///  - `"!<expr>"` → negation
pub fn evaluate_condition(
    condition: &str,
    config_values: &std::collections::HashMap<String, toml::Value>,
) -> bool {
    let condition = condition.trim();
    if condition.is_empty() || condition == "true" {
        return true;
    }
    if condition == "false" {
        return false;
    }
    if let Some(rest) = condition.strip_prefix('!') {
        return !evaluate_condition(rest, config_values);
    }
    if let Some(key) = condition.strip_prefix("config.") {
        return match config_values.get(key) {
            Some(toml::Value::Boolean(b)) => *b,
            Some(toml::Value::String(s)) => !s.is_empty(),
            Some(_) => true,
            None => false,
        };
    }
    // Default: treat as truthy
    true
}

/// Clean up temporary output files produced by a rule.
pub fn cleanup_temp_outputs(rule: &Rule, workdir: &Path) {
    for temp in &rule.temp_output {
        let path = workdir.join(temp);
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(file = %path.display(), error = %e, "failed to remove temp output");
            } else {
                tracing::debug!(file = %path.display(), "removed temp output");
            }
        }
    }
}

/// Check if a rule should be skipped based on output freshness.
///
/// Returns true if all outputs exist and are newer than all inputs.
pub fn should_skip_rule(rule: &Rule, workdir: &Path) -> bool {
    if rule.output.is_empty() {
        return false;
    }
    // Skip check for wildcard patterns
    if rule.output.iter().any(|o| o.contains('{')) || rule.input.iter().any(|i| i.contains('{')) {
        return false;
    }
    let all_outputs_exist = rule.output.iter().all(|o| workdir.join(o).exists());
    if !all_outputs_exist {
        return false;
    }
    if rule.input.is_empty() {
        return true; // No inputs to check freshness against
    }
    // Check if all outputs are newer than all inputs
    rule.input.iter().all(|input| {
        let input_path = workdir.join(input);
        rule.output.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    })
}

/// Validate that declared output files exist after execution.
/// Returns a list of missing output file paths.
pub fn validate_outputs(rule: &Rule, workdir: &Path) -> Vec<String> {
    rule.output
        .iter()
        .filter(|output| {
            // Skip wildcard patterns — they can't be checked without expansion
            if crate::wildcard::has_wildcards(output) {
                return false;
            }
            let path = workdir.join(output);
            !path.exists()
        })
        .cloned()
        .collect()
}

/// Provenance metadata for a workflow execution.
///
/// Captures the oxo-flow version, configuration checksum, and execution
/// timestamps for reproducibility and audit trail purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionProvenance {
    /// oxo-flow version that performed the execution.
    pub oxo_flow_version: String,

    /// SHA-256 checksum of the workflow configuration.
    pub config_checksum: String,

    /// Execution start time.
    pub started_at: DateTime<Utc>,

    /// Execution end time (set when complete).
    pub finished_at: Option<DateTime<Utc>>,

    /// Hostname where execution occurred.
    pub hostname: String,

    /// Working directory.
    pub workdir: String,

    /// Operator / analyst ID (for clinical compliance).
    #[serde(default)]
    pub operator_id: Option<String>,
    /// Instrument ID (for lab tracking).
    #[serde(default)]
    pub instrument_id: Option<String>,
    /// Reagent lot number.
    #[serde(default)]
    pub reagent_lot: Option<String>,
    /// Specimen / accession number.
    #[serde(default)]
    pub specimen_id: Option<String>,
}

impl ExecutionProvenance {
    /// Create a new provenance record for the current execution.
    pub fn new(config_checksum: &str, workdir: &Path) -> Self {
        Self {
            oxo_flow_version: env!("CARGO_PKG_VERSION").to_string(),
            config_checksum: config_checksum.to_string(),
            started_at: Utc::now(),
            finished_at: None,
            hostname: hostname(),
            workdir: workdir.display().to_string(),
            operator_id: None,
            instrument_id: None,
            reagent_lot: None,
            specimen_id: None,
        }
    }

    /// Mark the execution as complete.
    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }
}

impl std::fmt::Display for ExecutionProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "oxo-flow {} on {} (config checksum: {})",
            self.oxo_flow_version, self.hostname, self.config_checksum
        )
    }
}

/// Check a shell command for potentially dangerous patterns.
///
/// Returns a list of warnings for any suspicious patterns found.
/// This is a best-effort heuristic, not a security guarantee.
#[must_use]
pub fn sanitize_shell_command(cmd: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let dangerous_patterns = [
        ("$(", "Command substitution detected"),
        ("`", "Backtick command substitution detected"),
        ("&&", "Command chaining detected"),
        ("||", "Conditional command chaining detected"),
        (";", "Command separator detected"),
        ("|", "Pipe detected"),
        (">/dev/", "Redirect to /dev/ detected"),
        ("rm -rf /", "Dangerous recursive deletion detected"),
        ("chmod 777", "Overly permissive chmod detected"),
        ("curl ", "Network access via curl detected"),
        ("wget ", "Network access via wget detected"),
        ("eval ", "eval usage detected"),
    ];
    for (pattern, warning) in &dangerous_patterns {
        if cmd.contains(pattern) {
            warnings.push(format!("Shell command warning: {} in '{}'", warning, cmd));
        }
    }
    warnings
}

/// Validate that a file path does not escape the working directory
/// (path traversal prevention).
///
/// Returns `Ok(())` if the path is safe, or an error if traversal is detected.
#[must_use = "path safety validation returns a Result that must be checked"]
pub fn validate_path_safety(workdir: &std::path::Path, path: &str) -> crate::Result<()> {
    let resolved = workdir.join(path);
    // Normalize: just check the string doesn't contain ..
    if path.contains("..") {
        // Attempt canonicalization to see if it escapes
        if let Ok(canonical) = resolved.canonicalize() {
            if !canonical.starts_with(workdir) {
                return Err(crate::OxoFlowError::Validation {
                    message: format!("Path '{}' escapes the working directory", path),
                    rule: None,
                    suggestion: Some(
                        "Use relative paths within the workflow directory".to_string(),
                    ),
                });
            }
        } else {
            // Path doesn't exist yet, but contains ".." which is suspicious
            return Err(crate::OxoFlowError::Validation {
                message: format!(
                    "Path '{}' contains '..' which may escape the working directory",
                    path
                ),
                rule: None,
                suggestion: Some("Avoid using '..' in output paths".to_string()),
            });
        }
    }
    Ok(())
}

/// Get the system hostname, returning "unknown" if unavailable.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
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
            ..Default::default()
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
            retry_count: 0,
            timeout: None,
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
            retry_count: 0,
            timeout: None,
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
            retry_count: 0,
            timeout: None,
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
            retry_count: 0,
            timeout: None,
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

    #[tokio::test]
    async fn execute_with_timeout() {
        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::temp_dir(),
            keep_going: true,
            retry_count: 0,
            timeout: Some(std::time::Duration::from_millis(100)),
        };
        let executor = LocalExecutor::new(config);
        let rule = make_rule("timeout_test", "sleep 30");

        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(record.status, JobStatus::Failed);
        assert!(record.stderr.unwrap().contains("timed out"));
    }

    #[test]
    fn validate_outputs_finds_missing() {
        let dir = tempfile::tempdir().unwrap();
        let rule = Rule {
            name: "missing_out".to_string(),
            input: vec![],
            output: vec![
                "does_not_exist.txt".to_string(),
                "also_missing.bam".to_string(),
            ],
            shell: Some("echo hi".to_string()),
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
            ..Default::default()
        };

        let missing = validate_outputs(&rule, dir.path());
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"does_not_exist.txt".to_string()));
        assert!(missing.contains(&"also_missing.bam".to_string()));
    }

    #[test]
    fn validate_outputs_skips_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let rule = Rule {
            name: "wildcard_out".to_string(),
            input: vec![],
            output: vec!["{sample}.bam".to_string(), "fixed_output.txt".to_string()],
            shell: Some("echo hi".to_string()),
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
            ..Default::default()
        };

        let missing = validate_outputs(&rule, dir.path());
        // {sample}.bam should be skipped (wildcard), only fixed_output.txt reported
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], "fixed_output.txt");
    }

    // -- evaluate_condition tests --------------------------------------------

    #[test]
    fn evaluate_condition_empty_is_true() {
        let config = HashMap::new();
        assert!(evaluate_condition("", &config));
    }

    #[test]
    fn evaluate_condition_literal_true() {
        let config = HashMap::new();
        assert!(evaluate_condition("true", &config));
    }

    #[test]
    fn evaluate_condition_literal_false() {
        let config = HashMap::new();
        assert!(!evaluate_condition("false", &config));
    }

    #[test]
    fn evaluate_condition_negation() {
        let config = HashMap::new();
        assert!(!evaluate_condition("!true", &config));
        assert!(evaluate_condition("!false", &config));
    }

    #[test]
    fn evaluate_condition_config_bool_true() {
        let mut config = HashMap::new();
        config.insert("enable_qc".to_string(), toml::Value::Boolean(true));
        assert!(evaluate_condition("config.enable_qc", &config));
    }

    #[test]
    fn evaluate_condition_config_bool_false() {
        let mut config = HashMap::new();
        config.insert("enable_qc".to_string(), toml::Value::Boolean(false));
        assert!(!evaluate_condition("config.enable_qc", &config));
    }

    #[test]
    fn evaluate_condition_config_string_nonempty() {
        let mut config = HashMap::new();
        config.insert(
            "reference".to_string(),
            toml::Value::String("/path/to/ref.fa".to_string()),
        );
        assert!(evaluate_condition("config.reference", &config));
    }

    #[test]
    fn evaluate_condition_config_string_empty() {
        let mut config = HashMap::new();
        config.insert("reference".to_string(), toml::Value::String(String::new()));
        assert!(!evaluate_condition("config.reference", &config));
    }

    #[test]
    fn evaluate_condition_config_integer_is_truthy() {
        let mut config = HashMap::new();
        config.insert("threads".to_string(), toml::Value::Integer(4));
        assert!(evaluate_condition("config.threads", &config));
    }

    #[test]
    fn evaluate_condition_config_missing_key() {
        let config = HashMap::new();
        assert!(!evaluate_condition("config.nonexistent", &config));
    }

    #[test]
    fn evaluate_condition_negated_config() {
        let mut config = HashMap::new();
        config.insert("skip".to_string(), toml::Value::Boolean(true));
        assert!(!evaluate_condition("!config.skip", &config));
    }

    #[test]
    fn evaluate_condition_unknown_treated_as_truthy() {
        let config = HashMap::new();
        assert!(evaluate_condition("some_expression", &config));
    }

    #[test]
    fn evaluate_condition_trimmed_whitespace() {
        let config = HashMap::new();
        assert!(evaluate_condition("  true  ", &config));
        assert!(!evaluate_condition("  false  ", &config));
    }

    // -- cleanup_temp_outputs tests ------------------------------------------

    #[test]
    fn cleanup_temp_outputs_removes_files() {
        let dir = tempfile::tempdir().unwrap();
        let temp_file = dir.path().join("temp.bam");
        std::fs::write(&temp_file, "temporary").unwrap();
        assert!(temp_file.exists());

        let rule = Rule {
            name: "test".to_string(),
            temp_output: vec!["temp.bam".to_string()],
            ..Default::default()
        };

        cleanup_temp_outputs(&rule, dir.path());
        assert!(!temp_file.exists());
    }

    #[test]
    fn cleanup_temp_outputs_ignores_missing() {
        let dir = tempfile::tempdir().unwrap();

        let rule = Rule {
            name: "test".to_string(),
            temp_output: vec!["nonexistent.bam".to_string()],
            ..Default::default()
        };

        // Should not panic
        cleanup_temp_outputs(&rule, dir.path());
    }

    #[test]
    fn cleanup_temp_outputs_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.tmp");
        let f2 = dir.path().join("b.tmp");
        std::fs::write(&f1, "a").unwrap();
        std::fs::write(&f2, "b").unwrap();

        let rule = Rule {
            name: "test".to_string(),
            temp_output: vec!["a.tmp".to_string(), "b.tmp".to_string()],
            ..Default::default()
        };

        cleanup_temp_outputs(&rule, dir.path());
        assert!(!f1.exists());
        assert!(!f2.exists());
    }

    // -- should_skip_rule tests ----------------------------------------------

    #[test]
    fn should_skip_rule_no_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let rule = Rule {
            name: "test".to_string(),
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(!should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn should_skip_rule_outputs_missing() {
        let dir = tempfile::tempdir().unwrap();
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["in.txt".to_string()],
            output: vec!["out.txt".to_string()],
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(!should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn should_skip_rule_wildcard_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["{sample}.fastq".to_string()],
            output: vec!["{sample}.bam".to_string()],
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(!should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn should_skip_rule_outputs_up_to_date() {
        let dir = tempfile::tempdir().unwrap();

        // Create input first
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "data").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Create output after input (newer)
        let output = dir.path().join("output.txt");
        std::fs::write(&output, "result").unwrap();

        let rule = Rule {
            name: "test".to_string(),
            input: vec!["input.txt".to_string()],
            output: vec!["output.txt".to_string()],
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn should_skip_rule_outputs_stale() {
        let dir = tempfile::tempdir().unwrap();

        // Create output first
        let output = dir.path().join("output.txt");
        std::fs::write(&output, "old_result").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Create input after output (input is newer → need re-run)
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "new_data").unwrap();

        let rule = Rule {
            name: "test".to_string(),
            input: vec!["input.txt".to_string()],
            output: vec!["output.txt".to_string()],
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(!should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn should_skip_rule_no_inputs_all_outputs_exist() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output.txt");
        std::fs::write(&output, "result").unwrap();

        let rule = Rule {
            name: "test".to_string(),
            output: vec!["output.txt".to_string()],
            shell: Some("echo hi".to_string()),
            ..Default::default()
        };
        assert!(should_skip_rule(&rule, dir.path()));
    }

    #[test]
    fn provenance_creation() {
        let prov = ExecutionProvenance::new("abc123", Path::new("/work"));
        assert_eq!(prov.oxo_flow_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(prov.config_checksum, "abc123");
        assert!(prov.finished_at.is_none());
    }

    #[test]
    fn provenance_finish() {
        let mut prov = ExecutionProvenance::new("abc123", Path::new("/work"));
        prov.finish();
        assert!(prov.finished_at.is_some());
    }

    #[test]
    fn execution_event_workflow_started_serialization() {
        let event = ExecutionEvent::WorkflowStarted {
            workflow_name: "test-pipeline".to_string(),
            total_rules: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("WorkflowStarted"));
        assert!(json.contains("test-pipeline"));
        assert!(json.contains("5"));
        let deserialized: ExecutionEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ExecutionEvent::WorkflowStarted {
                workflow_name,
                total_rules,
            } => {
                assert_eq!(workflow_name, "test-pipeline");
                assert_eq!(total_rules, 5);
            }
            _ => panic!("expected WorkflowStarted"),
        }
    }

    #[test]
    fn execution_event_rule_started_serialization() {
        let event = ExecutionEvent::RuleStarted {
            rule: "fastqc".to_string(),
            command: Some("fastqc input.fq".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ExecutionEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ExecutionEvent::RuleStarted { rule, command } => {
                assert_eq!(rule, "fastqc");
                assert_eq!(command.as_deref(), Some("fastqc input.fq"));
            }
            _ => panic!("expected RuleStarted"),
        }
    }

    #[test]
    fn execution_event_rule_completed_serialization() {
        let event = ExecutionEvent::RuleCompleted {
            rule: "align".to_string(),
            status: JobStatus::Success,
            duration_ms: 12345,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ExecutionEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ExecutionEvent::RuleCompleted {
                rule,
                status,
                duration_ms,
            } => {
                assert_eq!(rule, "align");
                assert_eq!(status, JobStatus::Success);
                assert_eq!(duration_ms, 12345);
            }
            _ => panic!("expected RuleCompleted"),
        }
    }

    #[test]
    fn execution_event_rule_skipped_serialization() {
        let event = ExecutionEvent::RuleSkipped {
            rule: "qc".to_string(),
            reason: "outputs up to date".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ExecutionEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ExecutionEvent::RuleSkipped { rule, reason } => {
                assert_eq!(rule, "qc");
                assert_eq!(reason, "outputs up to date");
            }
            _ => panic!("expected RuleSkipped"),
        }
    }

    #[test]
    fn execution_event_workflow_completed_serialization() {
        let event = ExecutionEvent::WorkflowCompleted {
            total_duration_ms: 99999,
            succeeded: 3,
            failed: 1,
            skipped: 2,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ExecutionEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ExecutionEvent::WorkflowCompleted {
                total_duration_ms,
                succeeded,
                failed,
                skipped,
            } => {
                assert_eq!(total_duration_ms, 99999);
                assert_eq!(succeeded, 3);
                assert_eq!(failed, 1);
                assert_eq!(skipped, 2);
            }
            _ => panic!("expected WorkflowCompleted"),
        }
    }
}
