#![allow(deprecated)]
//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use crate::environment::EnvironmentResolver;
use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use crate::scheduler::ResourcePool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};

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

    /// Reason for skipping (if status is Skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
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

    /// Optional timeout per job (global default).
    pub timeout: Option<std::time::Duration>,

    /// Maximum total CPU threads across all running jobs.
    /// If set, jobs requiring more threads than available will be blocked.
    pub max_threads: Option<u32>,

    /// Maximum total memory in MB across all running jobs.
    /// If set, jobs requiring more memory than available will be blocked.
    pub max_memory_mb: Option<u64>,

    /// Resource group capacities.
    pub resource_groups: HashMap<String, u32>,

    /// Skip environment setup (assume environments are already ready).
    pub skip_env_setup: bool,

    /// Cache directory for environments (default: ~/.cache/oxo-flow/).
    pub cache_dir: Option<std::path::PathBuf>,
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
            max_threads: None,
            max_memory_mb: None,
            resource_groups: HashMap::new(),
            skip_env_setup: false,
            cache_dir: None,
        }
    }
}

/// Local process executor for running workflow rules.
pub struct LocalExecutor {
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
    env_resolver: EnvironmentResolver,
    /// Resource pool for tracking available CPU/memory.
    resource_pool: Arc<Mutex<ResourcePool>>,
}

impl LocalExecutor {
    /// Create a new local executor with the given configuration.
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_jobs));

        // Create environment resolver with optional cache directory
        let env_resolver = match &config.cache_dir {
            Some(cache_dir) => EnvironmentResolver::with_cache_dir(cache_dir),
            None => EnvironmentResolver::new(),
        };

        // Initialize resource pool with system limits or configured limits
        let (max_threads, max_memory_mb) = Self::detect_system_resources(&config);
        let mut resource_pool = ResourcePool::new(max_threads, max_memory_mb);
        resource_pool.set_groups(config.resource_groups.clone());
        let resource_pool = Arc::new(Mutex::new(resource_pool));

        Self {
            config,
            semaphore,
            env_resolver,
            resource_pool,
        }
    }

    /// Detect system resources or use configured limits.
    fn detect_system_resources(config: &ExecutorConfig) -> (u32, u64) {
        let max_threads = config.max_threads.unwrap_or_else(|| {
            // Use num_cpus as default
            num_cpus::get() as u32
        });

        let max_memory_mb = config.max_memory_mb.unwrap_or_else(|| {
            // Use sysinfo for cross-platform memory detection
            use sysinfo::System;
            let mut sys = System::new_all();
            sys.refresh_memory();
            let total_bytes = sys.total_memory();
            let detected_mb = total_bytes / 1024 / 1024; // bytes -> KB -> MB

            // Fallback to 8GB if detection fails (shouldn't happen on real systems)
            if detected_mb > 0 {
                detected_mb
            } else {
                tracing::warn!("sysinfo returned 0 memory, falling back to 8GB default");
                8192
            }
        });

        tracing::debug!(
            max_threads = %max_threads,
            max_memory_mb = %max_memory_mb,
            "initialized resource pool using sysinfo"
        );

        (max_threads, max_memory_mb)
    }

    /// Kill a process and all its children by terminating the process group.
    /// Only available on Unix systems.
    #[cfg(unix)]
    fn kill_process_tree(pid: u32) -> std::io::Result<()> {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::{Pid, getpgid};

        let nix_pid = Pid::from_raw(pid as i32);
        let pgid = getpgid(Some(nix_pid)).map_err(|e| std::io::Error::other(e.to_string()))?;

        // Kill entire process group with SIGKILL
        kill(pgid, Signal::SIGKILL).map_err(|e| std::io::Error::other(e.to_string()))?;

        tracing::debug!(pid = %pid, pgid = %pgid, "killed process group");
        Ok(())
    }

    /// Stub for non-Unix systems (no process group support).
    #[cfg(not(unix))]
    fn kill_process_tree(_pid: u32) -> std::io::Result<()> {
        // On non-Unix, we rely on the normal timeout behavior
        Ok(())
    }

    /// Ensure the environment for a rule is ready before execution.
    /// Creates/pulls environments as needed unless skip_env_setup is set.
    async fn ensure_environment_ready(&self, rule: &Rule) -> Result<()> {
        if self.config.skip_env_setup {
            tracing::debug!(rule = %rule.name, "skipping environment setup");
            return Ok(());
        }

        let env_spec = &rule.environment;
        if env_spec.is_empty() {
            return Ok(()); // System environment, no setup needed
        }

        // Get cache key for this environment
        let key = self.env_resolver.cache_key(env_spec);

        // Check if already ready
        if self.env_resolver.cache_is_ready(&key).await {
            tracing::debug!(rule = %rule.name, env_key = %key, "environment already ready");
            return Ok(());
        }

        // Run setup command
        let setup_cmd = self.env_resolver.setup_command(env_spec)?;
        tracing::info!(rule = %rule.name, setup_cmd = %setup_cmd, "setting up environment");

        let output = Command::new("sh")
            .arg("-c")
            .arg(&setup_cmd)
            .current_dir(&self.config.workdir)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                self.env_resolver.cache_mark_ready(&key).await;
                tracing::info!(rule = %rule.name, env_key = %key, "environment ready");
                Ok(())
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                tracing::error!(rule = %rule.name, stderr = %stderr, "environment setup failed");
                Err(OxoFlowError::Environment {
                    kind: env_spec.kind().to_string(),
                    message: format!("setup failed: {}", stderr),
                })
            }
            Err(e) => {
                tracing::error!(rule = %rule.name, error = %e, "environment setup command failed");
                Err(OxoFlowError::Environment {
                    kind: env_spec.kind().to_string(),
                    message: format!("setup command failed: {}", e),
                })
            }
        }
    }

    /// Wrap a command through the environment resolver, falling back to the
    /// original command on error and emitting a warning.
    fn resolve_command(&self, command: &str, rule: &Rule) -> String {
        match self
            .env_resolver
            .wrap_command(command, &rule.environment, Some(&rule.resources))
        {
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

    /// Check if resources are available for this rule.
    async fn check_resources(&self, rule: &Rule) -> Result<()> {
        let pool = self.resource_pool.lock().await;

        if !pool.can_accommodate(rule) {
            let required_threads = rule.effective_threads();
            let required_memory = rule
                .effective_memory()
                .and_then(crate::scheduler::parse_memory_mb)
                .unwrap_or(0);

            tracing::warn!(
                rule = %rule.name,
                threads_required = required_threads,
                threads_available = pool.threads,
                memory_required_mb = required_memory,
                memory_available_mb = pool.memory_mb,
                "insufficient resources, job will wait"
            );

            return Err(OxoFlowError::ResourceExhausted {
                rule: rule.name.clone(),
                required_threads,
                available_threads: pool.threads,
                required_memory_mb: required_memory,
                available_memory_mb: pool.memory_mb,
            });
        }

        tracing::debug!(
            rule = %rule.name,
            threads_available = pool.threads,
            memory_available_mb = pool.memory_mb,
            "resources available"
        );

        Ok(())
    }

    /// Reserve resources for a rule before execution.
    async fn reserve_resources(&self, rule: &Rule) {
        let mut pool = self.resource_pool.lock().await;
        pool.reserve(rule);

        let req_threads = rule.effective_threads();
        let req_memory_mb = rule
            .effective_memory()
            .and_then(crate::scheduler::parse_memory_mb)
            .unwrap_or(0);

        tracing::info!(
            rule = %rule.name,
            threads_requested = req_threads,
            threads_available = pool.threads,
            memory_requested_mb = req_memory_mb,
            memory_available_mb = pool.memory_mb,
            "reserved resources"
        );
    }

    /// Release resources after rule completion.
    async fn release_resources(&self, rule: &Rule) {
        let (max_threads, max_memory_mb) = Self::detect_system_resources(&self.config);
        let mut pool = self.resource_pool.lock().await;
        pool.release(
            rule,
            max_threads,
            max_memory_mb,
            &self.config.resource_groups,
        );

        let req_threads = rule.effective_threads();
        let req_memory_mb = rule
            .effective_memory()
            .and_then(crate::scheduler::parse_memory_mb)
            .unwrap_or(0);

        tracing::info!(
            rule = %rule.name,
            threads_released = req_threads,
            threads_available = pool.threads,
            memory_released_mb = req_memory_mb,
            memory_available_mb = pool.memory_mb,
            "released resources"
        );
    }

    /// Clean up temporary output files when a rule fails.
    async fn cleanup_temp_outputs(&self, rule: &Rule, wildcard_values: &HashMap<String, String>) {
        if rule.temp_output.is_empty() {
            return;
        }

        for temp_pattern in &rule.temp_output {
            let expanded = render_shell_command(temp_pattern, rule, wildcard_values);
            let temp_path = self.config.workdir.join(&expanded);

            if tokio::fs::try_exists(&temp_path).await.ok() == Some(true) {
                if let Err(e) = tokio::fs::remove_file(&temp_path).await {
                    tracing::warn!(
                        rule = %rule.name,
                        path = %temp_path.display(),
                        error = %e,
                        "failed to cleanup temp output"
                    );
                } else {
                    tracing::debug!(
                        rule = %rule.name,
                        path = %temp_path.display(),
                        "cleaned up temp output"
                    );
                }
            }
        }

        // Clean up transform chunk outputs if cleanup is enabled
        if let Some(ref transform) = rule.transform
            && transform.cleanup
        {
            // Chunk outputs are in .oxo-flow/chunks/{split_var}/
            let split_var = &transform.split.by;
            let chunk_dir = self.config.workdir.join(".oxo-flow/chunks").join(split_var);
            if tokio::fs::try_exists(&chunk_dir).await.ok() == Some(true)
                && let Err(e) = tokio::fs::remove_dir_all(&chunk_dir).await
            {
                tracing::warn!(
                    rule = %rule.name,
                    dir = %chunk_dir.display(),
                    error = %e,
                    "failed to cleanup transform chunk directory"
                );
            }
        }
    }

    /// Get effective timeout for a rule (rule-specific overrides global default).
    fn get_timeout(&self, rule: &Rule) -> Option<std::time::Duration> {
        // Rule-specific time_limit overrides global timeout
        if let Some(ref time_limit) = rule.resources.time_limit
            && let Some(secs) = crate::rule::parse_duration_secs(time_limit)
        {
            return Some(std::time::Duration::from_secs(secs));
        }
        self.config.timeout
    }

    /// Execute a single rule as a local process.
    #[must_use = "executing a rule returns a Result that must be used"]
    pub async fn execute_rule(
        &self,
        rule: &Rule,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<JobRecord> {
        // Get per-rule timeout (rule-specific overrides global default)
        let timeout = self.get_timeout(rule);

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
            timeout,
            skip_reason: None,
        };

        let shell_cmd = match &rule.shell {
            Some(cmd) => render_shell_command(cmd, rule, wildcard_values),
            None => {
                record.status = JobStatus::Skipped;
                record.finished_at = Some(Utc::now());
                return Ok(record);
            }
        };

        // Evaluate the `when` condition — skip execution if it resolves to false.
        if let Some(ref condition) = rule.when {
            // Build a config-value map from wildcard entries prefixed with "config."
            let config_values: std::collections::HashMap<String, toml::Value> = wildcard_values
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix("config.")
                        .map(|key| (key.to_string(), toml::Value::String(v.clone())))
                })
                .collect();
            if !evaluate_condition(condition, &config_values) {
                tracing::info!(rule = %rule.name, condition = %condition, "skipping rule: condition evaluated to false");
                record.status = JobStatus::Skipped;
                record.skip_reason = Some("condition evaluated to false".to_string());
                record.finished_at = Some(Utc::now());
                return Ok(record);
            }
        }

        // Skip rule if outputs are already up-to-date (all outputs newer than all inputs).
        if should_skip_rule(rule, &self.config.workdir, wildcard_values) {
            tracing::info!(rule = %rule.name, "outputs up-to-date, skipping");
            record.status = JobStatus::Skipped;
            record.skip_reason = Some("outputs up-to-date".to_string());
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Wrap the command through the environment resolver
        let shell_cmd = self.resolve_command(&shell_cmd, rule);

        record.command = Some(shell_cmd.clone());

        // Warn about any dangerous patterns detected in the expanded command.
        for warning in sanitize_shell_command(&shell_cmd) {
            tracing::warn!(rule = %rule.name, "{warning}");
        }

        if self.config.dry_run {
            tracing::info!(rule = %rule.name, command = %shell_cmd, "dry-run");
            record.status = JobStatus::Skipped;
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Ensure environment is ready before execution
        self.ensure_environment_ready(rule).await?;

        // Check resources before execution
        self.check_resources(rule).await?;

        // Pre-flight disk space check (warning only)
        let disk_warnings = crate::scheduler::validate_disk_requirements(
            std::slice::from_ref(rule),
            &self.config.workdir,
        );
        for warning in disk_warnings {
            tracing::warn!("{}", warning);
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

        // NEW: Reserve resources before execution
        self.reserve_resources(rule).await;

        // Execute pre_exec hook if defined
        if let Some(ref pre_cmd) = rule.pre_exec {
            tracing::info!(rule = %rule.name, hook = %pre_cmd, "executing pre_exec hook");
            let pre_result = Command::new("sh")
                .arg("-c")
                .arg(pre_cmd)
                .current_dir(&self.config.workdir)
                .envs(&rule.envvars)
                .output()
                .await;
            match pre_result {
                Ok(output) => {
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        self.release_resources(rule).await;
                        return Err(OxoFlowError::Execution {
                            rule: rule.name.clone(),
                            message: format!(
                                "pre_exec hook failed with code {}: {}",
                                output.status.code().unwrap_or(-1),
                                stderr.trim()
                            ),
                        });
                    }
                }
                Err(e) => {
                    self.release_resources(rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn pre_exec hook: {e}"),
                    });
                }
            }
        }

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

            // Spawn the process to get access to the PID for process group killing
            // Use process_group(0) to create a new process group where the shell
            // is the leader, ensuring all child processes are in the same group.
            #[cfg(unix)]
            let child = {
                Command::new("sh")
                    .arg("-c")
                    .arg(&shell_cmd)
                    .current_dir(&self.config.workdir)
                    .envs(&rule.envvars)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .process_group(0)
                    .spawn()
                    .map_err(|e| OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn command: {e}"),
                    })?
            };

            #[cfg(not(unix))]
            let child = {
                Command::new("sh")
                    .arg("-c")
                    .arg(&shell_cmd)
                    .current_dir(&self.config.workdir)
                    .envs(&rule.envvars)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn command: {e}"),
                    })?
            };

            let child_pid = child.id().unwrap_or(0);

            // Use per-rule timeout
            let cmd_result = if let Some(duration) = timeout {
                match tokio::time::timeout(duration, child.wait_with_output()).await {
                    Ok(inner) => inner,
                    Err(_) => {
                        // Timeout occurred - kill the process group
                        if child_pid > 0
                            && let Err(e) = Self::kill_process_tree(child_pid)
                        {
                            tracing::warn!(
                                rule = %rule.name,
                                pid = %child_pid,
                                error = %e,
                                "failed to kill process group on timeout"
                            );
                        }
                        record.finished_at = Some(Utc::now());
                        record.status = JobStatus::Failed;
                        record.stderr = Some(format!(
                            "command timed out after {duration:?} for rule '{}'",
                            rule.name
                        ));
                        tracing::error!(
                            rule = %rule.name,
                            timeout = ?duration,
                            pid = %child_pid,
                            "command timed out"
                        );
                        // NEW: Release resources even on timeout
                        self.release_resources(rule).await;
                        // Clean up temp outputs on failure
                        self.cleanup_temp_outputs(rule, wildcard_values).await;
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
                child.wait_with_output().await
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

                // NEW: Release resources after success
                self.release_resources(rule).await;

                // Execute on_success hook if defined
                if let Some(ref hook_cmd) = rule.on_success {
                    tracing::info!(rule = %rule.name, hook = %hook_cmd, "executing on_success hook");
                    let hook_result = Command::new("sh")
                        .arg("-c")
                        .arg(hook_cmd)
                        .current_dir(&self.config.workdir)
                        .envs(&rule.envvars)
                        .output()
                        .await;
                    if let Ok(hook_output) = hook_result
                        && !hook_output.status.success()
                    {
                        tracing::warn!(
                            rule = %rule.name,
                            hook = %hook_cmd,
                            code = %hook_output.status.code().unwrap_or(-1),
                            "on_success hook failed"
                        );
                    }
                }

                if !self.config.dry_run {
                    let missing = validate_outputs(rule, &self.config.workdir, wildcard_values);
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
                // Continue to next iteration to retry the command
                continue;
            } else {
                record.status = JobStatus::Failed;
                // NEW: Release resources after final failure
                self.release_resources(rule).await;
                // Clean up temp outputs on failure
                self.cleanup_temp_outputs(rule, wildcard_values).await;
                tracing::error!(rule = %rule.name, code = %code, "failed");

                // Execute on_failure hook if defined (after all retries exhausted)
                if let Some(ref hook_cmd) = rule.on_failure {
                    tracing::info!(rule = %rule.name, hook = %hook_cmd, "executing on_failure hook");
                    let hook_result = Command::new("sh")
                        .arg("-c")
                        .arg(hook_cmd)
                        .current_dir(&self.config.workdir)
                        .envs(&rule.envvars)
                        .output()
                        .await;
                    if let Ok(hook_output) = hook_result
                        && !hook_output.status.success()
                    {
                        tracing::warn!(
                            rule = %rule.name,
                            hook = %hook_cmd,
                            code = %hook_output.status.code().unwrap_or(-1),
                            "on_failure hook failed"
                        );
                    }
                }

                if !self.config.keep_going {
                    return Err(OxoFlowError::TaskFailed {
                        rule: rule.name.clone(),
                        code,
                    });
                }
            }
        }

        // Ensure resources are released if we exit the loop without success
        self.release_resources(rule).await;
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
                    timeout: self.get_timeout(rule),
                    skip_reason: None,
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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| OxoFlowError::Config {
                message: format!("failed to create checkpoint directory: {e}"),
            })?;
        }
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
        if json.trim().is_empty() {
            return Ok(Self::default());
        }
        Self::from_json(&json).map_err(|e| OxoFlowError::Config {
            message: format!(
                "failed to deserialize checkpoint from {}: {}",
                path.display(),
                e
            ),
        })
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
        output.push_str("# TYPE oxo_flow_rules_completed_total counter\n");
        output.push_str(&format!(
            "oxo_flow_rules_completed_total {}\n",
            self.completed_rules.len()
        ));

        output.push_str("# HELP oxo_flow_rules_failed_total Number of rules that failed.\n");
        output.push_str("# TYPE oxo_flow_rules_failed_total counter\n");
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

/// Evaluate a `when` condition string against workflow config values.
///
/// # Supported syntax
///
/// | Syntax | Description |
/// |--------|-------------|
/// | `true` / `false` | Literal boolean values |
/// | `config.<key>` | Truthy check — true when key is present and non-empty/non-zero |
/// | `config.<key> == "<value>"` | String equality |
/// | `config.<key> != "<value>"` | String inequality |
/// | `config.<key> == true\|false` | Boolean equality |
/// | `config.<key> > N` | Numeric comparison (`>`, `>=`, `<`, `<=`) |
/// | `file_exists("<path>")` | True when the file exists on the filesystem |
/// | `!<expr>` | Logical negation |
/// | `<expr> && <expr>` | Logical AND (short-circuit) |
/// | `<expr> \|\| <expr>` | Logical OR (short-circuit) |
/// | `(<expr>)` | Parenthesised grouping |
///
/// Unknown or syntactically invalid expressions default to `true`.
pub fn evaluate_condition(
    condition: &str,
    config_values: &std::collections::HashMap<String, toml::Value>,
) -> bool {
    evaluate_condition_inner(condition.trim(), config_values)
}

/// Internal recursive evaluator for `when` expressions.
fn evaluate_condition_inner(
    s: &str,
    config_values: &std::collections::HashMap<String, toml::Value>,
) -> bool {
    let s = s.trim();
    if s.is_empty() || s == "true" {
        return true;
    }
    if s == "false" {
        return false;
    }

    // ------------------------------------------------------------------
    // Parentheses grouping
    // ------------------------------------------------------------------
    if s.starts_with('(') && s.ends_with(')') {
        // Unwrap only if the outer parens form a balanced pair
        if balanced_parens(s) {
            return evaluate_condition_inner(&s[1..s.len() - 1], config_values);
        }
    }

    // ------------------------------------------------------------------
    // Logical OR  (lowest precedence — split on outermost `||`)
    // ------------------------------------------------------------------
    if let Some(idx) = find_top_level_op(s, "||") {
        let left = &s[..idx];
        let right = &s[idx + 2..];
        return evaluate_condition_inner(left, config_values)
            || evaluate_condition_inner(right, config_values);
    }

    // ------------------------------------------------------------------
    // Logical AND  (split on outermost `&&`)
    // ------------------------------------------------------------------
    if let Some(idx) = find_top_level_op(s, "&&") {
        let left = &s[..idx];
        let right = &s[idx + 2..];
        return evaluate_condition_inner(left, config_values)
            && evaluate_condition_inner(right, config_values);
    }

    // ------------------------------------------------------------------
    // Logical NOT
    // ------------------------------------------------------------------
    if let Some(rest) = s.strip_prefix('!') {
        return !evaluate_condition_inner(rest.trim(), config_values);
    }

    // ------------------------------------------------------------------
    // file_exists("<path>")
    // ------------------------------------------------------------------
    if let Some(inner) = s
        .strip_prefix("file_exists(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let path = inner.trim().trim_matches('"').trim_matches('\'');
        return std::path::Path::new(path).exists();
    }

    // ------------------------------------------------------------------
    // Comparison operators: config.<key> op <rhs>
    // Try operators from longest to shortest to avoid ambiguous prefix matches.
    // ------------------------------------------------------------------
    for op in &["==", "!=", ">=", "<=", ">", "<"] {
        if let Some(idx) = find_top_level_op(s, op) {
            let lhs = s[..idx].trim();
            let rhs = s[idx + op.len()..].trim();
            if let Some(key) = lhs.strip_prefix("config.") {
                let val = config_values.get(key);
                return compare_config_value(val, op, rhs);
            }
        }
    }

    // ------------------------------------------------------------------
    // config.<key>  — truthy check
    // ------------------------------------------------------------------
    if let Some(key) = s.strip_prefix("config.") {
        return match config_values.get(key) {
            Some(toml::Value::Boolean(b)) => *b,
            Some(toml::Value::String(sv)) => !sv.is_empty() && sv != "false" && sv != "0",
            Some(toml::Value::Integer(i)) => *i != 0,
            Some(toml::Value::Float(f)) => *f != 0.0,
            Some(_) => true,
            None => false,
        };
    }

    // Default: treat unknown expressions as truthy
    true
}

/// Returns the byte index of `op` in `s` that sits at the top level
/// (i.e., not inside any parentheses or string literal).
/// Returns `None` if `op` is not found at the top level.
fn find_top_level_op(s: &str, op: &str) -> Option<usize> {
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut in_double = false;
    let mut in_single = false;
    let n = bytes.len();

    let mut i = 0usize;
    while i < n {
        let b = bytes[i];
        match b {
            b'"' if !in_single => in_double = !in_double,
            b'\'' if !in_double => in_single = !in_single,
            b'(' if !in_double && !in_single => depth += 1,
            b')' if !in_double && !in_single => depth -= 1,
            _ => {}
        }
        if !in_double
            && !in_single
            && depth == 0
            && i + op_len <= n
            && &bytes[i..i + op_len] == op_bytes
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Returns `true` if the outer `(…)` in `s` form a balanced pair
/// (i.e., the closing `)` is the very last character).
fn balanced_parens(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
        return false;
    }
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
        }
        // If depth drops to 0 before the last char, the parens are not outer
        if depth == 0 && i < bytes.len() - 1 {
            return false;
        }
    }
    true
}

/// Compare a config value against `rhs` using the given comparison operator.
fn compare_config_value(val: Option<&toml::Value>, op: &str, rhs: &str) -> bool {
    match val {
        Some(toml::Value::Boolean(b)) => {
            let rhs_bool = match rhs {
                "true" => true,
                "false" => false,
                _ => return false,
            };
            match op {
                "==" => *b == rhs_bool,
                "!=" => *b != rhs_bool,
                _ => false,
            }
        }
        Some(toml::Value::Integer(i)) => {
            if let Ok(rhs_num) = rhs.parse::<i64>() {
                match op {
                    "==" => *i == rhs_num,
                    "!=" => *i != rhs_num,
                    ">=" => *i >= rhs_num,
                    "<=" => *i <= rhs_num,
                    ">" => *i > rhs_num,
                    "<" => *i < rhs_num,
                    _ => false,
                }
            } else {
                false
            }
        }
        Some(toml::Value::Float(f)) => {
            if let Ok(rhs_num) = rhs.parse::<f64>() {
                match op {
                    // Use a relative tolerance scaled to the larger magnitude so
                    // that both small and large floating-point values are handled
                    // correctly (f64::EPSILON only covers values near 1.0).
                    "==" => {
                        let tol = (f.abs().max(rhs_num.abs())) * 1e-9;
                        (*f - rhs_num).abs() <= tol.max(f64::MIN_POSITIVE)
                    }
                    "!=" => {
                        let tol = (f.abs().max(rhs_num.abs())) * 1e-9;
                        (*f - rhs_num).abs() > tol.max(f64::MIN_POSITIVE)
                    }
                    ">=" => *f >= rhs_num,
                    "<=" => *f <= rhs_num,
                    ">" => *f > rhs_num,
                    "<" => *f < rhs_num,
                    _ => false,
                }
            } else {
                false
            }
        }
        Some(toml::Value::String(sv)) => {
            let rhs_str = rhs.trim_matches('"').trim_matches('\'');
            match op {
                "==" => sv.as_str() == rhs_str,
                "!=" => sv.as_str() != rhs_str,
                // For numeric comparison operators, try to parse string as number
                ">=" | "<=" | ">" | "<" => {
                    // Try integer comparison first
                    if let (Ok(sv_num), Ok(rhs_num)) = (sv.parse::<i64>(), rhs_str.parse::<i64>()) {
                        match op {
                            ">=" => sv_num >= rhs_num,
                            "<=" => sv_num <= rhs_num,
                            ">" => sv_num > rhs_num,
                            "<" => sv_num < rhs_num,
                            _ => false,
                        }
                    } else if let (Ok(sv_num), Ok(rhs_num)) =
                        (sv.parse::<f64>(), rhs_str.parse::<f64>())
                    {
                        // Fall back to float comparison
                        match op {
                            ">=" => sv_num >= rhs_num,
                            "<=" => sv_num <= rhs_num,
                            ">" => sv_num > rhs_num,
                            "<" => sv_num < rhs_num,
                            _ => false,
                        }
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
        None => {
            // Key not present — treat as empty / false
            match op {
                "==" => {
                    let rhs_str = rhs.trim_matches('"').trim_matches('\'');
                    rhs_str.is_empty() || rhs_str == "false"
                }
                "!=" => {
                    let rhs_str = rhs.trim_matches('"').trim_matches('\'');
                    !rhs_str.is_empty() && rhs_str != "false"
                }
                _ => false,
            }
        }
        _ => false,
    }
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
/// Config variable placeholders (e.g. `{config.sample}`) are expanded using
/// `wildcard_values` before the path existence check.
pub fn should_skip_rule(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    if rule.output.is_empty() {
        return false;
    }

    // Expand config vars in output paths (e.g. {config.sample} → SAMPLE001)
    let expanded_outputs: Vec<String> = rule
        .output
        .iter()
        .map(|o| expand_config_in_path(o, wildcard_values))
        .collect();

    // Skip if any expanded output still contains a wildcard pattern ({sample} etc.)
    if expanded_outputs.iter().any(|o| o.contains('{')) {
        return false;
    }
    // Expand config vars in inputs too (for freshness comparison)
    let expanded_inputs: Vec<String> = rule
        .input
        .iter()
        .map(|i| expand_config_in_path(i, wildcard_values))
        .collect();
    if expanded_inputs.iter().any(|i| i.contains('{')) {
        return false;
    }

    let all_outputs_exist = expanded_outputs.iter().all(|o| workdir.join(o).exists());
    if !all_outputs_exist {
        return false;
    }
    if expanded_inputs.is_empty() {
        return true; // No inputs to check freshness against
    }
    // Check if all outputs are newer than all inputs
    expanded_inputs.iter().all(|input| {
        let input_path = workdir.join(input);
        expanded_outputs.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    })
}

/// Expand `{key}` placeholders in a path string using the provided values map.
///
/// Only performs simple key-value substitution (no `{input[N]}` / `{output[N]}` logic).
/// Used for checking output file existence after expansion of config variables.
pub fn expand_config_in_path(path: &str, wildcard_values: &HashMap<String, String>) -> String {
    let mut result = path.to_string();
    for (key, value) in wildcard_values {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

/// Validate that declared output files exist after execution.
/// Returns a list of missing output file paths (after expanding config variables).
pub fn validate_outputs(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    rule.output
        .iter()
        .filter_map(|output| {
            // Expand config variables (e.g. {config.sample}) before checking
            let expanded = expand_config_in_path(output, wildcard_values);
            // Skip paths that still contain wildcard patterns (e.g. {sample} from wildcard rules)
            if crate::wildcard::has_wildcards(&expanded) {
                return None;
            }
            let path = workdir.join(&expanded);
            if path.exists() { None } else { Some(expanded) }
        })
        .collect()
}

/// Compute a checksum of a file for integrity and non-determinism detection.
///
/// Uses SHA-256 for clinical-grade integrity verification.
///
/// Returns the hex-encoded SHA-256 hash string prefixed with "sha256:",
/// or an error if the file cannot be read.
pub fn compute_file_checksum(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let content = std::fs::read(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("failed to read {} for checksum: {e}", path.display()),
    })?;

    // SHA-256 for clinical-grade integrity verification (CLIA/CAP requirement)
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hasher.finalize();
    Ok(format!("sha256:{:x}", hash))
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

    /// Persist provenance to `.oxo-flow/provenance.json` for clinical compliance.
    ///
    /// This is required for CLIA/CAP audit trail documentation.
    pub fn persist(&self, workdir: &Path) -> Result<()> {
        let provenance_dir = workdir.join(".oxo-flow");
        std::fs::create_dir_all(&provenance_dir).map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("failed to create provenance directory: {e}"),
        })?;

        let provenance_file = provenance_dir.join("provenance.json");
        let json = serde_json::to_string_pretty(self).map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("failed to serialize provenance: {e}"),
        })?;

        std::fs::write(&provenance_file, json).map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("failed to write provenance file: {e}"),
        })?;

        Ok(())
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
/// Returns a list of warnings for suspicious patterns that could indicate
/// shell injection or destructive operations.  Common bioinformatics idioms
/// such as pipes (`|`), command chaining (`&&`), and semicolons (`;`) are
/// intentionally **not** flagged because they appear in virtually every
/// genomics shell template.
///
/// This function checks the *literal* command string after wildcard expansion.
/// Call it on the expanded shell command (post `render_shell_command`) to catch
/// any dangerous content injected via wildcard values.
///
/// This is a best-effort heuristic, not a security guarantee.
#[must_use]
pub fn sanitize_shell_command(cmd: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let dangerous_patterns = [
        ("$(", "Command substitution detected"),
        ("`", "Backtick command substitution detected"),
        (">/dev/", "Redirect to /dev/ detected"),
        ("rm -rf /", "Dangerous recursive deletion detected"),
        ("chmod 777", "Overly permissive chmod detected"),
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

    // Expand {params.key}
    for (key, value) in &rule.params {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        expanded = expanded.replace(&format!("{{params.{key}}}"), &string_val);
    }

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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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

        let missing = validate_outputs(&rule, dir.path(), &HashMap::new());
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

        let missing = validate_outputs(&rule, dir.path(), &HashMap::new());
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
        assert!(!should_skip_rule(&rule, dir.path(), &HashMap::new()));
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
        assert!(!should_skip_rule(&rule, dir.path(), &HashMap::new()));
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
        assert!(!should_skip_rule(&rule, dir.path(), &HashMap::new()));
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
        assert!(should_skip_rule(&rule, dir.path(), &HashMap::new()));
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
        assert!(!should_skip_rule(&rule, dir.path(), &HashMap::new()));
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
        assert!(should_skip_rule(&rule, dir.path(), &HashMap::new()));
    }

    #[test]
    fn should_skip_rule_config_vars_expanded() {
        // Outputs with {config.sample} should be expanded using wildcard_values
        // before checking whether the files exist on disk.
        let dir = tempfile::tempdir().unwrap();

        // Create the file under its expanded name
        let output = dir.path().join("results_SAMPLE001.txt");
        std::fs::write(&output, "done").unwrap();

        let rule = Rule {
            name: "test".to_string(),
            output: vec!["results_{config.sample}.txt".to_string()],
            shell: Some("echo done".to_string()),
            ..Default::default()
        };

        let mut values = HashMap::new();
        values.insert("config.sample".to_string(), "SAMPLE001".to_string());

        // With config vars expanded, output exists → should skip
        assert!(should_skip_rule(&rule, dir.path(), &values));
        // Without config vars, treat path as literal → output not found → don't skip
        assert!(!should_skip_rule(&rule, dir.path(), &HashMap::new()));
    }

    #[test]
    fn validate_outputs_config_vars_expanded() {
        // validate_outputs must expand {config.xxx} before checking file existence.
        let dir = tempfile::tempdir().unwrap();

        // The actual file is at the expanded path
        let output = dir.path().join("aligned_NA12878.bam");
        std::fs::write(&output, "bam_data").unwrap();

        let rule = Rule {
            name: "align".to_string(),
            output: vec!["aligned_{config.sample}.bam".to_string()],
            shell: Some("bwa mem ...".to_string()),
            ..Default::default()
        };

        let mut values = HashMap::new();
        values.insert("config.sample".to_string(), "NA12878".to_string());

        // With config vars expanded the file is found → no missing outputs
        let missing = validate_outputs(&rule, dir.path(), &values);
        assert!(
            missing.is_empty(),
            "file should be found after config var expansion, but missing: {:?}",
            missing
        );

        // Without config vars the literal path is not found → reported as missing
        let missing_no_vals = validate_outputs(&rule, dir.path(), &HashMap::new());
        assert_eq!(missing_no_vals.len(), 1);
    }

    #[test]
    fn expand_config_in_path_substitutes_values() {
        let mut values = HashMap::new();
        values.insert("config.sample".to_string(), "NA12878".to_string());
        values.insert("config.threads".to_string(), "8".to_string());

        assert_eq!(
            expand_config_in_path("aligned/{config.sample}.bam", &values),
            "aligned/NA12878.bam"
        );
        assert_eq!(
            expand_config_in_path("logs/{config.sample}_t{config.threads}.log", &values),
            "logs/NA12878_t8.log"
        );
        // No substitution when keys are absent
        assert_eq!(
            expand_config_in_path("data/{config.missing}.txt", &HashMap::new()),
            "data/{config.missing}.txt"
        );
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
        // Standard bioinformatics idioms (pipes, &&, ;) must NOT produce warnings.
        let warnings =
            sanitize_shell_command("bwa mem ref.fa reads.fq | samtools sort > aligned.bam");
        assert!(
            warnings.is_empty(),
            "pipe in a bioinformatics command should not be flagged: {warnings:?}"
        );
    }

    #[test]
    fn sanitize_shell_dangerous_command() {
        let warnings = sanitize_shell_command("eval rm -rf /");
        assert!(warnings.iter().any(|w| w.contains("recursive deletion")));
        assert!(warnings.iter().any(|w| w.contains("eval")));
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

        // SHA-256 format verification for clinical compliance
        assert!(checksum1.starts_with("sha256:"));
        assert_eq!(checksum1.len(), 71); // "sha256:" (7 chars) + 64 hex chars

        // Different content = different checksum
        std::fs::write(&file, "different content").unwrap();
        let checksum3 = compute_file_checksum(&file).unwrap();
        assert_ne!(checksum1, checksum3);
    }

    #[test]
    fn sha256_checksum_known_value() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let checksum = compute_file_checksum(&file).unwrap();
        // SHA-256 of "hello world" is known: b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert!(
            checksum.contains("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
        );
    }

    #[test]
    fn provenance_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut prov = ExecutionProvenance::new("abc123", dir.path());
        prov.finish();

        // Persist provenance
        prov.persist(dir.path()).unwrap();

        // Verify file was created
        let provenance_file = dir.path().join(".oxo-flow/provenance.json");
        assert!(provenance_file.exists());

        // Verify content is valid JSON
        let content = std::fs::read_to_string(&provenance_file).unwrap();
        let parsed: ExecutionProvenance = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.config_checksum, "abc123");
        assert!(parsed.finished_at.is_some());
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
                skip_reason: None,
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
                skip_reason: None,
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
                skip_reason: None,
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

    // -----------------------------------------------------------------------
    // WF-01: Enhanced evaluate_condition tests
    // -----------------------------------------------------------------------

    #[test]
    fn evaluate_condition_string_equality() {
        let mut config = HashMap::new();
        config.insert(
            "mode".to_string(),
            toml::Value::String("tumor_normal".to_string()),
        );
        assert!(evaluate_condition(
            r#"config.mode == "tumor_normal""#,
            &config
        ));
        assert!(!evaluate_condition(r#"config.mode == "germline""#, &config));
        assert!(evaluate_condition(r#"config.mode != "germline""#, &config));
    }

    #[test]
    fn evaluate_condition_boolean_equality() {
        let mut config = HashMap::new();
        config.insert("annotate".to_string(), toml::Value::Boolean(true));
        assert!(evaluate_condition("config.annotate == true", &config));
        assert!(!evaluate_condition("config.annotate == false", &config));
        assert!(evaluate_condition("config.annotate != false", &config));
    }

    #[test]
    fn evaluate_condition_integer_comparisons() {
        let mut config = HashMap::new();
        config.insert("min_coverage".to_string(), toml::Value::Integer(30));
        assert!(evaluate_condition("config.min_coverage > 20", &config));
        assert!(evaluate_condition("config.min_coverage >= 30", &config));
        assert!(!evaluate_condition("config.min_coverage > 30", &config));
        assert!(evaluate_condition("config.min_coverage < 50", &config));
        assert!(evaluate_condition("config.min_coverage <= 30", &config));
        assert!(!evaluate_condition("config.min_coverage < 30", &config));
        assert!(evaluate_condition("config.min_coverage == 30", &config));
        assert!(evaluate_condition("config.min_coverage != 10", &config));
    }

    #[test]
    fn evaluate_condition_logical_and() {
        let mut config = HashMap::new();
        config.insert("run_qc".to_string(), toml::Value::Boolean(true));
        config.insert("run_align".to_string(), toml::Value::Boolean(true));
        config.insert("skip".to_string(), toml::Value::Boolean(false));

        assert!(evaluate_condition(
            "config.run_qc && config.run_align",
            &config
        ));
        assert!(!evaluate_condition("config.run_qc && config.skip", &config));
        assert!(!evaluate_condition("config.skip && config.run_qc", &config));
    }

    #[test]
    fn evaluate_condition_logical_or() {
        let mut config = HashMap::new();
        config.insert("run_qc".to_string(), toml::Value::Boolean(true));
        config.insert("skip".to_string(), toml::Value::Boolean(false));

        assert!(evaluate_condition("config.run_qc || config.skip", &config));
        assert!(evaluate_condition("config.skip || config.run_qc", &config));
        assert!(!evaluate_condition("config.skip || false", &config));
    }

    #[test]
    fn evaluate_condition_parentheses() {
        let mut config = HashMap::new();
        config.insert("a".to_string(), toml::Value::Boolean(true));
        config.insert("b".to_string(), toml::Value::Boolean(false));
        config.insert("c".to_string(), toml::Value::Boolean(true));

        // Without parens: a || (b && c) = true || false = true
        assert!(evaluate_condition(
            "config.a || config.b && config.c",
            &config
        ));
        // Parenthesised: (a || b) && c = true && true = true
        assert!(evaluate_condition(
            "(config.a || config.b) && config.c",
            &config
        ));
    }

    #[test]
    fn evaluate_condition_file_exists_nonexistent() {
        let config = HashMap::new();
        assert!(!evaluate_condition(
            r#"file_exists("/nonexistent-path-oxo-flow-test")"#,
            &config
        ));
    }

    #[test]
    fn evaluate_condition_file_exists_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sentinel.txt");
        std::fs::write(&file, "exists").unwrap();
        let condition = format!(r#"file_exists("{}")"#, file.display());
        let config = HashMap::new();
        assert!(evaluate_condition(&condition, &config));
    }

    #[test]
    fn evaluate_condition_negated_comparison() {
        let mut config = HashMap::new();
        config.insert("mode".to_string(), toml::Value::String("wgs".to_string()));
        assert!(evaluate_condition(r#"!(config.mode == "wes")"#, &config));
        assert!(!evaluate_condition(r#"!(config.mode == "wgs")"#, &config));
    }

    #[test]
    fn evaluate_condition_complex_expression() {
        let mut config = HashMap::new();
        config.insert("run_qc".to_string(), toml::Value::Boolean(true));
        config.insert("threads".to_string(), toml::Value::Integer(8));
        config.insert(
            "mode".to_string(),
            toml::Value::String("tumor_normal".to_string()),
        );

        // run_qc == true && threads >= 4 && mode == "tumor_normal"
        assert!(evaluate_condition(
            r#"config.run_qc == true && config.threads >= 4 && config.mode == "tumor_normal""#,
            &config
        ));
    }

    // -- Retry and Hooks tests ------------------------------------------------

    #[test]
    fn rule_has_hooks_fields() {
        let rule = Rule {
            name: "test".to_string(),
            on_success: Some("echo success".to_string()),
            on_failure: Some("echo failure".to_string()),
            ..Default::default()
        };
        assert!(rule.on_success.is_some());
        assert!(rule.on_failure.is_some());
    }

    #[test]
    fn rule_retries_field() {
        let rule = Rule {
            name: "retry_test".to_string(),
            retries: 3,
            ..Default::default()
        };
        assert_eq!(rule.retries, 3);
    }

    #[test]
    fn executor_config_retry_count() {
        let config = ExecutorConfig {
            retry_count: 2,
            ..Default::default()
        };
        assert_eq!(config.retry_count, 2);
    }

    #[test]
    fn detect_system_memory_returns_valid_value() {
        use sysinfo::System;
        let mut sys = System::new_all();
        sys.refresh_memory();
        let total = sys.total_memory();
        // Should return at least some memory (systems always have >0)
        assert!(total > 0, "sysinfo should detect system memory");
        // Convert to MB should work
        let mb = total / 1024 / 1024;
        assert!(mb > 0, "memory in MB should be positive");
    }

    #[tokio::test]
    async fn cleanup_temp_outputs_on_failure() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let temp_file = dir.path().join("temp_output.txt");
        tokio::fs::write(&temp_file, "partial data").await.unwrap();

        let rule = Rule {
            name: "test_rule".to_string(),
            input: vec!["input.txt".to_string()],
            output: vec!["output.txt".to_string()],
            shell: Some("exit 1".to_string()),
            temp_output: vec![temp_file.to_str().unwrap().to_string()],
            ..Default::default()
        };

        let config = ExecutorConfig {
            max_jobs: 1,
            dry_run: false,
            workdir: dir.path().to_path_buf(),
            keep_going: true,
            retry_count: 0,
            timeout: None,
            max_threads: None,
            max_memory_mb: None,
            resource_groups: HashMap::new(),
            skip_env_setup: true,
            cache_dir: None,
        };

        let executor = LocalExecutor::new(config);
        let result = executor.execute_rule(&rule, &HashMap::new()).await.unwrap();

        assert_eq!(result.status, JobStatus::Failed);
        // Temp file should be cleaned up
        assert!(!tokio::fs::try_exists(&temp_file).await.unwrap());
    }
}
