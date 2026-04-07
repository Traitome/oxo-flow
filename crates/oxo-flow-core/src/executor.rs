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
    /// Waiting in the scheduler queue (submitted but not yet running).
    Queued,
    /// Cancelled by user or system.
    Cancelled,
    /// Exceeded the configured timeout.
    TimedOut,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
            Self::Queued => write!(f, "queued"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::TimedOut => write!(f, "timed_out"),
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

impl ExecutionEvent {
    /// Serialize the event to a JSON string suitable for structured logging.
    ///
    /// Produces a single-line JSON object (NDJSON format) with a `timestamp`
    /// field added automatically. This format is compatible with log
    /// aggregation tools like Elasticsearch, Datadog, and CloudWatch.
    pub fn to_json_log(&self) -> String {
        let timestamp = chrono::Utc::now().to_rfc3339();
        match self {
            ExecutionEvent::WorkflowStarted {
                workflow_name,
                total_rules,
            } => {
                format!(
                    r#"{{"timestamp":"{}","event":"workflow_started","workflow":"{}","total_rules":{}}}"#,
                    timestamp, workflow_name, total_rules
                )
            }
            ExecutionEvent::RuleStarted { rule, command } => {
                let cmd = command.as_deref().unwrap_or("");
                format!(
                    r#"{{"timestamp":"{}","event":"rule_started","rule":"{}","command":"{}"}}"#,
                    timestamp,
                    rule,
                    cmd.replace('"', "\\\"")
                )
            }
            ExecutionEvent::RuleCompleted {
                rule,
                status,
                duration_ms,
            } => {
                format!(
                    r#"{{"timestamp":"{}","event":"rule_completed","rule":"{}","status":"{}","duration_ms":{}}}"#,
                    timestamp, rule, status, duration_ms
                )
            }
            ExecutionEvent::RuleSkipped { rule, reason } => {
                format!(
                    r#"{{"timestamp":"{}","event":"rule_skipped","rule":"{}","reason":"{}"}}"#,
                    timestamp,
                    rule,
                    reason.replace('"', "\\\"")
                )
            }
            ExecutionEvent::WorkflowCompleted {
                total_duration_ms,
                succeeded,
                failed,
                skipped,
            } => {
                format!(
                    r#"{{"timestamp":"{}","event":"workflow_completed","total_duration_ms":{},"succeeded":{},"failed":{},"skipped":{}}}"#,
                    timestamp, total_duration_ms, succeeded, failed, skipped
                )
            }
        }
    }

    /// Returns the event type name as a string.
    pub fn event_type(&self) -> &'static str {
        match self {
            ExecutionEvent::WorkflowStarted { .. } => "workflow_started",
            ExecutionEvent::RuleStarted { .. } => "rule_started",
            ExecutionEvent::RuleCompleted { .. } => "rule_completed",
            ExecutionEvent::RuleSkipped { .. } => "rule_skipped",
            ExecutionEvent::WorkflowCompleted { .. } => "workflow_completed",
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
            Some(cmd) => render_shell_command(cmd, rule, wildcard_values),
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

    /// Save checkpoint state to a file.
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(|e| OxoFlowError::Config {
            message: format!("failed to save checkpoint to {}: {e}", path.display()),
        })
    }

    /// Load checkpoint state from a file.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Config {
            message: format!("failed to read checkpoint from {}: {e}", path.display()),
        })?;
        Self::from_json(&json)
    }

    /// Returns the default checkpoint file path for a workflow.
    pub fn default_path(workdir: &Path) -> std::path::PathBuf {
        workdir.join(".oxo-flow").join("checkpoint.json")
    }

    /// Generate Prometheus-style text metrics from checkpoint state.
    ///
    /// Returns metrics in the Prometheus text exposition format suitable
    /// for scraping by Prometheus or compatible monitoring tools.
    pub fn to_prometheus_metrics(&self) -> String {
        let mut output = String::new();

        output.push_str(
            "# HELP oxo_flow_rules_completed_total Number of rules completed successfully.\n",
        );
        output.push_str("# TYPE oxo_flow_rules_completed_total gauge\n");
        output.push_str(&format!(
            "oxo_flow_rules_completed_total {}\n",
            self.completed_rules.len()
        ));

        output.push_str("# HELP oxo_flow_rules_failed_total Number of rules that failed.\n");
        output.push_str("# TYPE oxo_flow_rules_failed_total gauge\n");
        output.push_str(&format!(
            "oxo_flow_rules_failed_total {}\n",
            self.failed_rules.len()
        ));

        output.push_str("# HELP oxo_flow_rule_duration_seconds Wall-clock time per rule.\n");
        output.push_str("# TYPE oxo_flow_rule_duration_seconds gauge\n");
        for (rule, benchmark) in &self.benchmarks {
            output.push_str(&format!(
                "oxo_flow_rule_duration_seconds{{rule=\"{}\"}} {:.3}\n",
                rule, benchmark.wall_time_secs
            ));
        }

        if !self.benchmarks.is_empty() {
            let total_time: f64 = self.benchmarks.values().map(|b| b.wall_time_secs).sum();
            output.push_str("# HELP oxo_flow_total_duration_seconds Total execution time.\n");
            output.push_str("# TYPE oxo_flow_total_duration_seconds gauge\n");
            output.push_str(&format!(
                "oxo_flow_total_duration_seconds {:.3}\n",
                total_time
            ));
        }

        output
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

/// Compute a checksum of a file for integrity and non-determinism detection.
///
/// Uses a simple hash of the file contents. Returns the hex-encoded hash string,
/// or an error if the file cannot be read.
pub fn compute_file_checksum(path: &Path) -> Result<String> {
    let content = std::fs::read(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("failed to read {} for checksum: {e}", path.display()),
    })?;
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// Verify output file checksums match previously recorded values.
///
/// Returns a list of (file_path, expected, actual) tuples for any mismatches.
pub fn verify_output_checksums(
    checksums: &HashMap<String, String>,
    workdir: &Path,
) -> Vec<(String, String, String)> {
    let mut mismatches = Vec::new();
    for (file, expected) in checksums {
        let path = workdir.join(file);
        match compute_file_checksum(&path) {
            Ok(actual) if actual != *expected => {
                mismatches.push((file.clone(), expected.clone(), actual));
            }
            Err(_) => {
                mismatches.push((file.clone(), expected.clone(), "<unreadable>".to_string()));
            }
            _ => {}
        }
    }
    mismatches
}

/// Check if a rule should be skipped based on content-aware caching.
///
/// Unlike [`should_skip_rule`] which only checks file modification times,
/// this function also considers file content checksums. This avoids
/// unnecessary re-execution when a file's mtime changes but its content
/// does not (e.g., after `touch` or a no-op rebuild).
///
/// `known_checksums` maps file paths to their previously recorded checksums.
/// If a file's current checksum matches its known checksum, the file is
/// considered unchanged even if its mtime is newer.
pub fn should_skip_rule_content_aware(
    rule: &Rule,
    workdir: &Path,
    known_checksums: &HashMap<String, String>,
) -> bool {
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
        return true;
    }

    // First check mtime (fast path)
    let mtime_fresh = rule.input.iter().all(|input| {
        let input_path = workdir.join(input);
        rule.output.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    });

    if mtime_fresh {
        return true;
    }

    // Mtime says stale — check content checksums as fallback
    // If all input files have unchanged content (matching known checksums),
    // we can still skip the rule
    rule.input.iter().all(|input| {
        let input_path = workdir.join(input);
        if let Some(known) = known_checksums.get(input) {
            match compute_file_checksum(&input_path) {
                Ok(current) => current == *known,
                Err(_) => false,
            }
        } else {
            false // No known checksum — can't verify content
        }
    })
}

/// Compute checksums for all non-wildcard input files of a rule.
///
/// Returns a map from file path (relative) to hex-encoded checksum.
/// Files that cannot be read are silently skipped.
pub fn compute_input_checksums(rule: &Rule, workdir: &Path) -> HashMap<String, String> {
    let mut checksums = HashMap::new();
    for input in &rule.input {
        if crate::wildcard::has_wildcards(input) {
            continue;
        }
        let path = workdir.join(input);
        if let Ok(checksum) = compute_file_checksum(&path) {
            checksums.insert(input.clone(), checksum);
        }
    }
    checksums
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

    /// Parent run ID for tracking re-execution lineage.
    #[serde(default)]
    pub parent_run_id: Option<String>,

    /// Input file checksums for reproducibility verification.
    #[serde(default)]
    pub input_checksums: HashMap<String, String>,

    /// Output file checksums computed after execution.
    #[serde(default)]
    pub output_checksums: HashMap<String, String>,

    /// Software versions used during execution (tool name → version).
    #[serde(default)]
    pub software_versions: HashMap<String, String>,
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
            parent_run_id: None,
            input_checksums: HashMap::new(),
            output_checksums: HashMap::new(),
            software_versions: HashMap::new(),
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

/// Render a shell command template by substituting all placeholder variables.
///
/// Supports the following placeholders:
/// - `{input}` — all input files joined with a space
/// - `{input[N]}` — the Nth input file (0-indexed)
/// - `{output}` — all output files joined with a space
/// - `{output[N]}` — the Nth output file (0-indexed)
/// - `{threads}` — the rule's effective thread count
/// - `{config.key}` — a workflow config variable (passed via `wildcard_values` with `config.` prefix)
/// - `{key}` — a user-defined wildcard value from `wildcard_values`
///
/// Missing indexed accesses (out-of-range) are left unreplaced so that the
/// shell command will fail with a clear error rather than silently producing
/// empty substitutions.
pub fn render_shell_command(
    cmd: &str,
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
) -> String {
    let mut expanded = cmd.to_string();

    // Expand {output} → all outputs space-joined
    expanded = expanded.replace("{output}", &rule.output.join(" "));

    // Expand {output[N]} → Nth output (0-indexed)
    for (i, out) in rule.output.iter().enumerate() {
        expanded = expanded.replace(&format!("{{output[{i}]}}"), out);
    }

    // Expand {input} → all inputs space-joined
    expanded = expanded.replace("{input}", &rule.input.join(" "));

    // Expand {input[N]} → Nth input (0-indexed)
    for (i, inp) in rule.input.iter().enumerate() {
        expanded = expanded.replace(&format!("{{input[{i}]}}"), inp);
    }

    // Expand {threads}
    expanded = expanded.replace("{threads}", &rule.effective_threads().to_string());

    // Expand user wildcards and config values (e.g. {sample}, {config.reference})
    for (key, value) in wildcard_values {
        expanded = expanded.replace(&format!("{{{key}}}"), value);
    }

    expanded
}

/// Get the system hostname, returning "unknown" if unavailable.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Summary statistics for a workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// Total number of rules.
    pub total_rules: usize,
    /// Rules that completed successfully.
    pub succeeded: usize,
    /// Rules that failed.
    pub failed: usize,
    /// Rules that were skipped.
    pub skipped: usize,
    /// Total wall-clock time in seconds.
    pub total_duration_secs: f64,
    /// Per-rule timing data (rule name → wall-clock seconds).
    pub rule_durations: HashMap<String, f64>,
    /// Longest rule execution time in seconds.
    pub max_rule_duration_secs: f64,
    /// Name of the longest-running rule.
    pub bottleneck_rule: Option<String>,
}

impl ExecutionStats {
    /// Compute execution statistics from job records.
    pub fn from_records(records: &HashMap<String, JobRecord>) -> Self {
        let mut succeeded = 0;
        let mut failed = 0;
        let mut skipped = 0;
        let mut rule_durations = HashMap::new();
        let mut max_duration = 0.0_f64;
        let mut bottleneck = None;

        for (name, record) in records {
            match record.status {
                JobStatus::Success => succeeded += 1,
                JobStatus::Failed | JobStatus::TimedOut => failed += 1,
                JobStatus::Skipped => skipped += 1,
                _ => {}
            }

            if let (Some(start), Some(end)) = (record.started_at, record.finished_at) {
                let duration = end.signed_duration_since(start).num_milliseconds() as f64 / 1000.0;
                rule_durations.insert(name.clone(), duration);
                if duration > max_duration {
                    max_duration = duration;
                    bottleneck = Some(name.clone());
                }
            }
        }

        let total_duration_secs: f64 = rule_durations.values().sum();

        Self {
            total_rules: records.len(),
            succeeded,
            failed,
            skipped,
            total_duration_secs,
            rule_durations,
            max_rule_duration_secs: max_duration,
            bottleneck_rule: bottleneck,
        }
    }
}

/// Clean up old checkpoint and cache files from the .oxo-flow directory.
///
/// Removes checkpoint files older than `max_age_days` and returns the
/// number of files removed.
pub fn cleanup_cache(workdir: &Path, max_age_days: u64) -> usize {
    let cache_dir = workdir.join(".oxo-flow");
    if !cache_dir.exists() {
        return 0;
    }

    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);
    let now = std::time::SystemTime::now();
    let mut removed = 0;

    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Ok(metadata) = std::fs::metadata(&path)
            && let Ok(modified) = metadata.modified()
            && let Ok(age) = now.duration_since(modified)
            && age > max_age
        {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to remove old cache file"
                );
            } else {
                tracing::debug!(
                    path = %path.display(),
                    "removed old cache file"
                );
                removed += 1;
            }
        }
    }

    removed
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

    // -- render_shell_command tests -------------------------------------------

    #[test]
    fn render_shell_output_indexed() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["in.txt".to_string()],
            output: vec!["out.txt".to_string(), "out2.txt".to_string()],
            shell: None,
            ..Default::default()
        };
        let result = render_shell_command("cat {input[0]} > {output[0]}", &rule, &HashMap::new());
        assert_eq!(result, "cat in.txt > out.txt");
    }

    #[test]
    fn render_shell_output_all() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["a.txt".to_string(), "b.txt".to_string()],
            output: vec!["out.txt".to_string()],
            shell: None,
            ..Default::default()
        };
        let result = render_shell_command("cat {input} > {output}", &rule, &HashMap::new());
        assert_eq!(result, "cat a.txt b.txt > out.txt");
    }

    #[test]
    fn render_shell_threads() {
        let rule = Rule {
            name: "test".to_string(),
            threads: Some(8),
            output: vec!["out.bam".to_string()],
            ..Default::default()
        };
        let result = render_shell_command(
            "bwa mem -t {threads} ref.fa > {output[0]}",
            &rule,
            &HashMap::new(),
        );
        assert_eq!(result, "bwa mem -t 8 ref.fa > out.bam");
    }

    #[test]
    fn render_shell_config_values() {
        let rule = Rule {
            name: "test".to_string(),
            output: vec!["hello.txt".to_string()],
            ..Default::default()
        };
        let mut values = HashMap::new();
        values.insert("config.reference".to_string(), "/data/ref.fa".to_string());
        let result =
            render_shell_command("bwa mem {config.reference} > {output[0]}", &rule, &values);
        assert_eq!(result, "bwa mem /data/ref.fa > hello.txt");
    }

    #[tokio::test]
    async fn execute_output_index_expansion() {
        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: false,
            workdir: std::env::temp_dir(),
            keep_going: false,
            retry_count: 0,
            timeout: None,
        };
        let executor = LocalExecutor::new(config);
        let rule = Rule {
            name: "output_test".to_string(),
            input: vec![],
            output: vec!["hello_output.txt".to_string()],
            shell: Some("echo hello_oxoflow_{output[0]}".to_string()),
            ..Default::default()
        };
        let record = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();
        assert_eq!(record.status, JobStatus::Success);
        let stdout = record.stdout.unwrap();
        assert!(
            stdout.contains("hello_oxoflow_hello_output.txt"),
            "stdout was: {stdout}"
        );
    }

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

    #[test]
    fn sanitize_shell_safe_command() {
        let warnings =
            sanitize_shell_command("bwa mem ref.fa reads.fq | samtools sort > aligned.bam");
        assert!(warnings.iter().any(|w| w.contains("Pipe")));
    }

    #[test]
    fn sanitize_shell_dangerous_command() {
        let warnings = sanitize_shell_command("rm -rf / && curl evil.com");
        assert!(warnings.iter().any(|w| w.contains("recursive deletion")));
        assert!(warnings.iter().any(|w| w.contains("curl")));
    }

    #[test]
    fn validate_path_safe() {
        let workdir = std::path::Path::new("/work");
        assert!(validate_path_safety(workdir, "output/result.txt").is_ok());
    }

    #[test]
    fn validate_path_traversal() {
        let workdir = std::path::Path::new("/work");
        assert!(validate_path_safety(workdir, "../../etc/passwd").is_err());
    }

    #[test]
    fn job_status_display_all_variants() {
        assert_eq!(JobStatus::Pending.to_string(), "pending");
        assert_eq!(JobStatus::Success.to_string(), "success");
        assert_eq!(JobStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn execution_provenance_display() {
        let prov = ExecutionProvenance {
            oxo_flow_version: "0.1.0".to_string(),
            config_checksum: "abc123".to_string(),
            started_at: Utc::now(),
            finished_at: None,
            hostname: "testhost".to_string(),
            workdir: "/work".to_string(),
            operator_id: None,
            instrument_id: None,
            reagent_lot: None,
            specimen_id: None,
            parent_run_id: None,
            input_checksums: HashMap::new(),
            output_checksums: HashMap::new(),
            software_versions: HashMap::new(),
        };
        let s = prov.to_string();
        assert!(s.contains("0.1.0"));
        assert!(s.contains("testhost"));
    }

    #[test]
    fn job_status_new_variants_display() {
        assert_eq!(JobStatus::Queued.to_string(), "queued");
        assert_eq!(JobStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(JobStatus::TimedOut.to_string(), "timed_out");
    }

    #[test]
    fn checkpoint_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_checkpoint.json");

        let mut state = CheckpointState::new();
        state.mark_completed(
            "rule_a",
            BenchmarkRecord {
                rule: "rule_a".to_string(),
                wall_time_secs: 10.5,
                max_memory_mb: Some(1024),
                cpu_seconds: Some(9.0),
            },
        );
        state.mark_failed("rule_b");

        state.save_to_file(&path).unwrap();
        let loaded = CheckpointState::load_from_file(&path).unwrap();

        assert!(loaded.is_completed("rule_a"));
        assert!(loaded.failed_rules.contains("rule_b"));
    }

    #[test]
    fn checkpoint_default_path() {
        let path = CheckpointState::default_path(Path::new("/work"));
        assert_eq!(path, Path::new("/work/.oxo-flow/checkpoint.json"));
    }

    #[test]
    fn compute_file_checksum_basic() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let checksum1 = compute_file_checksum(&file).unwrap();
        let checksum2 = compute_file_checksum(&file).unwrap();
        assert_eq!(checksum1, checksum2); // Same content = same checksum
        assert!(!checksum1.is_empty());

        // Different content = different checksum
        std::fs::write(&file, "different content").unwrap();
        let checksum3 = compute_file_checksum(&file).unwrap();
        assert_ne!(checksum1, checksum3);
    }

    #[test]
    fn verify_output_checksums_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("out.txt");
        std::fs::write(&file, "original").unwrap();
        let original = compute_file_checksum(&file).unwrap();

        let mut checksums = HashMap::new();
        checksums.insert("out.txt".to_string(), original.clone());

        // No mismatches when content unchanged
        assert!(verify_output_checksums(&checksums, dir.path()).is_empty());

        // Mismatch after content change
        std::fs::write(&file, "modified").unwrap();
        let mismatches = verify_output_checksums(&checksums, dir.path());
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].0, "out.txt");
    }

    #[test]
    fn content_aware_skip_no_outputs() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["in.txt".to_string()],
            output: vec![],
            ..Default::default()
        };
        let checksums = HashMap::new();
        assert!(!should_skip_rule_content_aware(
            &rule,
            Path::new("/nonexistent"),
            &checksums
        ));
    }

    #[test]
    fn content_aware_skip_wildcard_patterns() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["{sample}.txt".to_string()],
            output: vec!["{sample}.bam".to_string()],
            ..Default::default()
        };
        let checksums = HashMap::new();
        assert!(!should_skip_rule_content_aware(
            &rule,
            Path::new("/nonexistent"),
            &checksums
        ));
    }

    #[test]
    fn compute_input_checksums_skips_wildcards() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["{sample}.txt".to_string()],
            ..Default::default()
        };
        let checksums = compute_input_checksums(&rule, Path::new("/nonexistent"));
        assert!(checksums.is_empty());
    }

    #[test]
    fn execution_provenance_with_lineage() {
        let prov = ExecutionProvenance::new("abc123", Path::new("/work"));
        assert!(prov.parent_run_id.is_none());
        assert!(prov.input_checksums.is_empty());
        assert!(prov.output_checksums.is_empty());
        assert!(prov.software_versions.is_empty());
    }

    #[test]
    fn execution_event_json_log() {
        let event = ExecutionEvent::WorkflowStarted {
            workflow_name: "test-pipeline".to_string(),
            total_rules: 5,
        };
        let json = event.to_json_log();
        assert!(json.contains("\"event\":\"workflow_started\""));
        assert!(json.contains("\"workflow\":\"test-pipeline\""));
        assert!(json.contains("\"total_rules\":5"));
        assert!(json.contains("\"timestamp\":"));
    }

    #[test]
    fn execution_event_type() {
        let event = ExecutionEvent::RuleStarted {
            rule: "align".to_string(),
            command: Some("bwa mem".to_string()),
        };
        assert_eq!(event.event_type(), "rule_started");
    }

    #[test]
    fn checkpoint_prometheus_metrics() {
        let mut state = CheckpointState::new();
        state.mark_completed(
            "align",
            BenchmarkRecord {
                rule: "align".to_string(),
                wall_time_secs: 120.5,
                max_memory_mb: None,
                cpu_seconds: None,
            },
        );
        state.mark_failed("variant_call");

        let metrics = state.to_prometheus_metrics();
        assert!(metrics.contains("oxo_flow_rules_completed_total 1"));
        assert!(metrics.contains("oxo_flow_rules_failed_total 1"));
        assert!(metrics.contains("oxo_flow_rule_duration_seconds{rule=\"align\"} 120.500"));
        assert!(metrics.contains("oxo_flow_total_duration_seconds"));
    }

    #[test]
    fn execution_stats_from_records() {
        let now = chrono::Utc::now();
        let mut records = HashMap::new();
        records.insert(
            "fast".to_string(),
            JobRecord {
                rule: "fast".to_string(),
                status: JobStatus::Success,
                started_at: Some(now - chrono::Duration::seconds(10)),
                finished_at: Some(now),
                exit_code: Some(0),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
            },
        );
        records.insert(
            "slow".to_string(),
            JobRecord {
                rule: "slow".to_string(),
                status: JobStatus::Success,
                started_at: Some(now - chrono::Duration::seconds(60)),
                finished_at: Some(now),
                exit_code: Some(0),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
            },
        );
        records.insert(
            "skipped".to_string(),
            JobRecord {
                rule: "skipped".to_string(),
                status: JobStatus::Skipped,
                started_at: None,
                finished_at: None,
                exit_code: None,
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
            },
        );

        let stats = ExecutionStats::from_records(&records);
        assert_eq!(stats.total_rules, 3);
        assert_eq!(stats.succeeded, 2);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.bottleneck_rule.as_deref(), Some("slow"));
        assert!(stats.max_rule_duration_secs > 50.0);
    }

    #[test]
    fn cleanup_cache_nonexistent_dir() {
        let count = cleanup_cache(Path::new("/nonexistent-oxo-flow-test-dir"), 7);
        assert_eq!(count, 0);
    }
}
