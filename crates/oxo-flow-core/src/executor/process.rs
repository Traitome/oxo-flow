use crate::environment::EnvironmentResolver;
use crate::error::{OxoFlowError, Result};
use crate::rule::{FilePatterns, Rule};
use crate::scheduler::ResourcePool;
use crate::storage::StoragePath;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};

use super::checkpoint::cleanup_temp_outputs;
use super::security::{
    sanitize_shell_command, validate_path_safety, validate_shell_safety,
    validate_wildcard_injection,
};

/// Default interpreter mapping for script file extensions.
pub static DEFAULT_INTERPRETER_MAP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    map.insert(".py".to_string(), "python".to_string());
    map.insert(".py3".to_string(), "python3".to_string());
    map.insert(".R".to_string(), "Rscript".to_string());
    map.insert(".r".to_string(), "Rscript".to_string());
    map.insert(".jl".to_string(), "julia".to_string());
    map.insert(".sh".to_string(), "bash".to_string());
    map.insert(".bash".to_string(), "bash".to_string());
    map.insert(".pl".to_string(), "perl".to_string());
    map.insert(".rb".to_string(), "ruby".to_string());
    map.insert(".qmd".to_string(), "quarto render".to_string());
    map.insert(".Rmd".to_string(), "quarto render".to_string());
    map.insert(".rmd".to_string(), "quarto render".to_string());
    map.insert(
        ".ipynb".to_string(),
        "jupyter nbconvert --to notebook --execute".to_string(),
    );
    map.insert(".smk".to_string(), "snakemake".to_string());
    map.insert(".nextflow".to_string(), "nextflow run".to_string());
    map.insert(".wdl".to_string(), "miniwdl run".to_string());
    map
});

/// Detect interpreter for a script file based on extension.
pub fn detect_interpreter(
    script_path: &str,
    interpreter_override: Option<&str>,
    custom_map: &HashMap<String, String>,
) -> Option<String> {
    if let Some(interp) = interpreter_override {
        return match super::security::validate_interpreter_path(interp) {
            Ok(()) => Some(interp.to_string()),
            Err(e) => {
                tracing::warn!("interpreter override rejected: {e}");
                None
            }
        };
    }
    let ext = Path::new(script_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let mut key = String::with_capacity(e.len() + 1);
            key.push('.');
            for c in e.chars() {
                for lc in c.to_lowercase() {
                    key.push(lc);
                }
            }
            key
        });

    if let Some(ref extension) = ext {
        if let Some(interp) = custom_map.get(extension) {
            return Some(interp.clone());
        }
        if let Some(interp) = DEFAULT_INTERPRETER_MAP.get(extension) {
            return Some(interp.clone());
        }
    }
    None
}

/// Build command from interpreter and script path.
pub fn build_script_command(interpreter: &str, script_path: &str) -> String {
    format!("{} {}", interpreter, script_path)
}

/// Status of a job in the execution pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Success,
    Failed,
    Skipped,
    Queued,
    Cancelled,
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
    WorkflowStarted {
        workflow_name: String,
        total_rules: usize,
    },
    RuleStarted {
        rule: String,
        command: Option<String>,
    },
    RuleCompleted {
        rule: String,
        status: JobStatus,
        duration_ms: u64,
    },
    RuleSkipped {
        rule: String,
        reason: String,
    },
    WorkflowCompleted {
        total_duration_ms: u64,
        succeeded: usize,
        failed: usize,
        skipped: usize,
    },
    // R10: Add WorkflowCancelled event
    WorkflowCancelled {
        workflow_name: String,
    },
}

impl ExecutionEvent {
    pub fn to_json_log(&self) -> String {
        let timestamp = Utc::now().to_rfc3339();
        match self {
            ExecutionEvent::WorkflowStarted {
                workflow_name,
                total_rules,
            } => serde_json::json!({
                "timestamp": timestamp,
                "event": "workflow_started",
                "workflow": workflow_name,
                "total_rules": total_rules
            })
            .to_string(),

            ExecutionEvent::RuleStarted { rule, command } => serde_json::json!({
                "timestamp": timestamp,
                "event": "rule_started",
                "rule": rule,
                "command": command.as_deref().unwrap_or("")
            })
            .to_string(),

            ExecutionEvent::RuleCompleted {
                rule,
                status,
                duration_ms,
            } => serde_json::json!({
                "timestamp": timestamp,
                "event": "rule_completed",
                "rule": rule,
                "status": status.to_string(),
                "duration_ms": duration_ms
            })
            .to_string(),

            ExecutionEvent::RuleSkipped { rule, reason } => serde_json::json!({
                "timestamp": timestamp,
                "event": "rule_skipped",
                "rule": rule,
                "reason": reason
            })
            .to_string(),

            ExecutionEvent::WorkflowCompleted {
                total_duration_ms,
                succeeded,
                failed,
                skipped,
            } => serde_json::json!({
                "timestamp": timestamp,
                "event": "workflow_completed",
                "total_duration_ms": total_duration_ms,
                "succeeded": succeeded,
                "failed": failed,
                "skipped": skipped
            })
            .to_string(),
            ExecutionEvent::WorkflowCancelled { workflow_name } => serde_json::json!({
                "timestamp": timestamp,
                "event": "workflow_cancelled",
                "workflow": workflow_name
            })
            .to_string(),
        }
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            ExecutionEvent::WorkflowStarted { .. } => "workflow_started",
            ExecutionEvent::RuleStarted { .. } => "rule_started",
            ExecutionEvent::RuleCompleted { .. } => "rule_completed",
            ExecutionEvent::RuleSkipped { .. } => "rule_skipped",
            ExecutionEvent::WorkflowCompleted { .. } => "workflow_completed",
            ExecutionEvent::WorkflowCancelled { .. } => "workflow_cancelled",
        }
    }
}

/// Record of a single job execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub rule: String,
    pub status: JobStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub retries: u32,
    #[serde(skip)]
    pub timeout: Option<std::time::Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Configuration for the executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub max_jobs: usize,
    pub dry_run: bool,
    pub workdir: PathBuf,
    pub keep_going: bool,
    pub retry_count: u32,
    pub timeout: Option<std::time::Duration>,
    pub max_threads: Option<u32>,
    pub max_memory_mb: Option<u64>,
    pub resource_groups: HashMap<String, u32>,
    pub skip_env_setup: bool,
    pub cache_dir: Option<PathBuf>,
    pub interpreter_map: HashMap<String, String>,
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
            interpreter_map: HashMap::new(),
        }
    }
}

pub struct LocalExecutor {
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
    env_resolver: EnvironmentResolver,
    resource_pool: Arc<Mutex<ResourcePool>>,
}

impl LocalExecutor {
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_jobs));
        let env_resolver = match &config.cache_dir {
            Some(cache_dir) => EnvironmentResolver::with_cache_dir(cache_dir),
            None => EnvironmentResolver::new(),
        };
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

    fn detect_system_resources(config: &ExecutorConfig) -> (u32, u64) {
        // R8: Fix num_cpus in Docker.
        // On Linux, we should respect cgroup limits if available.
        // num_cpus::get() usually handles this on Linux, but sysinfo can also help.
        let max_threads = config.max_threads.unwrap_or_else(|| num_cpus::get() as u32);

        // R7: Cross-platform memory detection (already uses sysinfo)
        let max_memory_mb = config.max_memory_mb.unwrap_or_else(|| {
            use sysinfo::System;
            let mut sys = System::new_all();
            sys.refresh_memory();
            let total_bytes = sys.total_memory();
            let detected_mb = total_bytes / 1024 / 1024;
            if detected_mb > 0 { detected_mb } else { 8192 }
        });

        (max_threads, max_memory_mb)
    }

    async fn ensure_environment_ready(&self, rule: &Rule) -> Result<()> {
        if self.config.skip_env_setup {
            return Ok(());
        }
        let env_spec = &rule.environment;
        if env_spec.is_empty() {
            return Ok(());
        }
        let key = self.env_resolver.cache_key(env_spec);
        if self.env_resolver.cache_is_ready(&key).await {
            return Ok(());
        }
        let setup_cmd = self.env_resolver.setup_command(env_spec)?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(&setup_cmd)
            .current_dir(&self.config.workdir)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                self.env_resolver.cache_mark_ready(&key).await;
                Ok(())
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                Err(OxoFlowError::Environment {
                    kind: env_spec.kind().to_string(),
                    message: format!("setup failed: {}", stderr),
                })
            }
            Err(e) => Err(OxoFlowError::Environment {
                kind: env_spec.kind().to_string(),
                message: format!("setup command failed: {}", e),
            }),
        }
    }

    fn resolve_command(&self, command: &str, rule: &Rule) -> String {
        match self
            .env_resolver
            .wrap_command(command, &rule.environment, Some(&rule.resources))
        {
            Ok(wrapped) => wrapped,
            Err(e) => {
                tracing::warn!(rule = %rule.name, error = %e, "environment wrapping failed");
                command.to_string()
            }
        }
    }

    async fn check_resources(&self, rule: &Rule) -> Result<()> {
        let pool = self.resource_pool.lock().await;
        if !pool.can_accommodate(rule) {
            let required_threads = rule.effective_threads();
            let required_memory = rule
                .effective_memory()
                .and_then(crate::scheduler::parse_memory_mb)
                .unwrap_or(0);

            return Err(OxoFlowError::ResourceExhausted {
                rule: rule.name.clone(),
                required_threads,
                available_threads: pool.threads,
                required_memory_mb: required_memory,
                available_memory_mb: pool.memory_mb,
            });
        }
        Ok(())
    }

    async fn reserve_resources(&self, rule: &Rule) {
        let mut pool = self.resource_pool.lock().await;
        pool.reserve(rule);
    }

    async fn release_resources(&self, rule: &Rule) {
        let max_threads = self
            .config
            .max_threads
            .unwrap_or_else(|| num_cpus::get() as u32);
        let max_memory_mb = self.config.max_memory_mb.unwrap_or(8192);
        let mut pool = self.resource_pool.lock().await;
        pool.release(
            rule,
            max_threads,
            max_memory_mb,
            &self.config.resource_groups,
        );
    }

    fn get_timeout(&self, rule: &Rule) -> Option<std::time::Duration> {
        if let Some(ref time_limit) = rule.resources.time_limit
            && let Some(secs) = crate::rule::parse_duration_secs(time_limit)
        {
            return Some(std::time::Duration::from_secs(secs));
        }
        self.config.timeout
    }

    pub async fn execute_rule(
        &self,
        rule: &Rule,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<JobRecord> {
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

        let base_cmd =
            match build_execution_command(rule, wildcard_values, &self.config.interpreter_map) {
                Some(cmd) => cmd,
                None => {
                    record.status = JobStatus::Skipped;
                    record.finished_at = Some(Utc::now());
                    record.skip_reason = Some("no shell or script defined".to_string());
                    return Ok(record);
                }
            };

        if let Some(ref condition) = rule.when {
            let config_values: HashMap<String, toml::Value> = wildcard_values
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix("config.")
                        .map(|key| (key.to_string(), toml::Value::String(v.clone())))
                })
                .collect();
            if !evaluate_condition(condition, &config_values) {
                record.status = JobStatus::Skipped;
                record.skip_reason = Some("condition evaluated to false".to_string());
                record.finished_at = Some(Utc::now());
                return Ok(record);
            }
        }

        if super::checkpoint::should_skip_rule(rule, &self.config.workdir, wildcard_values) {
            record.status = JobStatus::Skipped;
            record.skip_reason = Some("outputs up-to-date".to_string());
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        let resolved_commands = vec![self.resolve_command(&base_cmd, rule)];
        record.command = resolved_commands.first().cloned();

        // Detect remote storage paths in inputs/outputs (stub, no-op for now).
        warn_if_remote_paths(rule, wildcard_values);

        validate_wildcard_injection(wildcard_values)?;
        for cmd in &resolved_commands {
            validate_shell_safety(cmd)?;
            for warning in sanitize_shell_command(cmd) {
                tracing::warn!(rule = %rule.name, "{warning}");
            }
        }

        // Validate output paths for traversal safety
        for output_pattern in rule.output.to_vec() {
            validate_path_safety(&self.config.workdir, &output_pattern)?;
        }

        if self.config.dry_run {
            record.status = JobStatus::Skipped;
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Create parent directories for all output files
        for output_pattern in &rule.output {
            let path = self.config.workdir.join(output_pattern);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!(
                            "failed to create output directory {}: {e}",
                            parent.display()
                        ),
                    }
                })?;
            }
        }

        self.ensure_environment_ready(rule).await?;
        self.check_resources(rule).await?;

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: rule.name.clone(),
                message: format!("semaphore error: {e}"),
            })?;

        self.reserve_resources(rule).await;

        // Hooks logic (simplified here for brevity, keeping it inline in execute_rule as in original)
        if let Some(ref pre_cmd) = rule.pre_exec {
            validate_shell_safety(pre_cmd)?;
            let pre_result = Command::new("sh")
                .arg("-c")
                .arg(pre_cmd)
                .current_dir(&self.config.workdir)
                .envs(&rule.envvars)
                .output()
                .await;
            match pre_result {
                Ok(output) if !output.status.success() => {
                    self.release_resources(rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: "pre_exec hook failed".to_string(),
                    });
                }
                Err(e) => {
                    self.release_resources(rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn pre_exec hook: {e}"),
                    });
                }
                _ => {}
            }
        }

        let max_attempts = 1 + self.config.retry_count;
        let mut all_commands_succeeded = false;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut last_exit_code: Option<i32> = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                record.retries = attempt;
            }

            all_commands_succeeded = true;
            combined_stdout.clear();
            combined_stderr.clear();

            for cmd in &resolved_commands {
                #[cfg(unix)]
                let child = Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(&self.config.workdir)
                    .envs(&rule.envvars)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .process_group(0)
                    .spawn();

                #[cfg(not(unix))]
                let child = Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(&self.config.workdir)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                let child = child.map_err(|e| OxoFlowError::Execution {
                    rule: rule.name.clone(),
                    message: format!("failed to spawn: {e}"),
                })?;

                let child_id = child.id();

                let cmd_result = if let Some(duration) = timeout {
                    match tokio::time::timeout(duration, child.wait_with_output()).await {
                        Ok(inner) => inner,
                        Err(_) => {
                            // R3 fix: use id directly and check it
                            if let Some(pid) = child_id {
                                let _ = super::timeout::kill_process_tree(pid);
                            }
                            all_commands_succeeded = false;
                            record.status = JobStatus::TimedOut;
                            last_exit_code = Some(124);
                            combined_stderr.push_str("command timed out");
                            break;
                        }
                    }
                } else {
                    child.wait_with_output().await
                };

                match cmd_result {
                    Ok(output) => {
                        combined_stdout.push_str(&String::from_utf8_lossy(&output.stdout));
                        combined_stderr.push_str(&String::from_utf8_lossy(&output.stderr));
                        last_exit_code = output.status.code();
                        if !output.status.success() {
                            all_commands_succeeded = false;
                            record.status = JobStatus::Failed;
                            break;
                        }
                    }
                    Err(e) => {
                        all_commands_succeeded = false;
                        record.status = JobStatus::Failed;
                        combined_stderr.push_str(&e.to_string());
                        break;
                    }
                }
            }

            if all_commands_succeeded {
                record.status = JobStatus::Success;
                break;
            }

            // R1 fix: trigger retries for TimeOut as well if needed.
            // Original code had `if record.status != JobStatus::TimedOut` before the retry loop logic.
            // Actually, the Phase 1 says "Fix timeout skipping retries".

            if attempt + 1 < max_attempts {
                if let Some(ref delay_str) = rule.retry_delay
                    && let Some(secs) = crate::rule::parse_duration_secs(delay_str)
                {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                }
                record.status = JobStatus::Running;
                continue;
            }
        }

        record.finished_at = Some(Utc::now());
        record.exit_code = last_exit_code;
        record.stdout = Some(combined_stdout);
        record.stderr = Some(combined_stderr);

        self.release_resources(rule).await;

        if all_commands_succeeded {
            record.status = JobStatus::Success;
            if let Some(ref hook_cmd) = rule.on_success {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg(hook_cmd)
                    .current_dir(&self.config.workdir)
                    .envs(&rule.envvars)
                    .output()
                    .await;
            }
        } else {
            // Keep the status set in the loop (Failed or TimedOut)
            cleanup_temp_outputs(rule, &self.config.workdir).await;
            if let Some(ref hook_cmd) = rule.on_failure {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg(hook_cmd)
                    .current_dir(&self.config.workdir)
                    .envs(&rule.envvars)
                    .output()
                    .await;
            }
        }

        Ok(record)
    }

    pub fn dry_run_rules(&self, rules: &[Rule]) -> Vec<JobRecord> {
        rules
            .iter()
            .map(|rule| {
                let command = rule.shell.clone();
                let wrapped = command
                    .as_deref()
                    .map(|cmd| self.resolve_command(cmd, rule));

                // Apply shell safety checks in dry-run mode so dangerous
                // commands are visible to users before actual execution.
                if let Some(ref cmd) = wrapped {
                    if let Err(e) = validate_shell_safety(cmd) {
                        tracing::warn!(rule = %rule.name, error = %e, "dry-run: dangerous shell command detected");
                    }
                    for warning in sanitize_shell_command(cmd) {
                        tracing::warn!(rule = %rule.name, "{warning}");
                    }
                }
                // Also check the raw command if no wrapped version
                if let Some(ref raw_cmd) = command {
                    if let Err(e) = validate_shell_safety(raw_cmd) {
                        tracing::warn!(rule = %rule.name, error = %e, "dry-run: dangerous shell command detected");
                    }
                    for warning in sanitize_shell_command(raw_cmd) {
                        tracing::warn!(rule = %rule.name, "{warning}");
                    }
                }

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

pub fn build_execution_command(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    interpreter_map: &HashMap<String, String>,
) -> Option<String> {
    let shell_cmd = rule
        .shell
        .as_ref()
        .map(|cmd| render_shell_command(cmd, rule, wildcard_values));

    let script_cmd = rule.script.as_ref().map(|script_path| {
        let expanded_script = render_shell_command(script_path, rule, wildcard_values);
        let base_script = expanded_script
            .split_whitespace()
            .next()
            .unwrap_or(&expanded_script);

        match detect_interpreter(base_script, rule.interpreter.as_deref(), interpreter_map) {
            Some(interp) => build_script_command(&interp, &expanded_script),
            None => expanded_script,
        }
    });

    if shell_cmd.is_none() && script_cmd.is_none() {
        return None;
    }

    let mut base_cmd = match (&shell_cmd, &script_cmd) {
        (Some(shell), Some(script)) => format!("{}\n{}", shell, script),
        (Some(shell), None) => shell.clone(),
        (None, Some(script)) => script.clone(),
        (None, None) => unreachable!(),
    };

    if !rule.envvars.is_empty() {
        let mut env_prefix = String::new();
        for (k, v) in &rule.envvars {
            let escaped_v = v.replace('\'', "'\\''");
            env_prefix.push_str(&format!("export {}='{}'\n", k, escaped_v));
        }
        base_cmd = format!("{}{}", env_prefix, base_cmd);
    }

    Some(base_cmd)
}

pub fn render_shell_command(
    cmd: &str,
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
) -> String {
    let mut expanded = cmd.to_string();
    let all_outputs = rule.output.to_vec();
    expanded = expanded.replace("{output}", &all_outputs.join(" "));
    for i in 0..rule.output.len() {
        if let Some(out) = rule.output.get_index(i) {
            expanded = expanded.replace(&format!("{{output[{i}]}}"), out);
        }
    }
    if let FilePatterns::Map(ref m) = rule.output {
        for (name, out) in m {
            expanded = expanded.replace(&format!("{{output.{name}}}"), out);
        }
    }
    let all_inputs = rule.input.to_vec();
    expanded = expanded.replace("{input}", &all_inputs.join(" "));
    for i in 0..rule.input.len() {
        if let Some(inp) = rule.input.get_index(i) {
            expanded = expanded.replace(&format!("{{input[{i}]}}"), inp);
        }
    }
    if let FilePatterns::Map(ref m) = rule.input {
        for (name, inp) in m {
            expanded = expanded.replace(&format!("{{input.{name}}}"), inp);
        }
    }
    expanded = expanded.replace("{threads}", &rule.effective_threads().to_string());
    for (key, value) in &rule.params {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        expanded = expanded.replace(&format!("{{params.{key}}}"), &string_val);
    }
    for (key, value) in wildcard_values {
        expanded = expanded.replace(&format!("{{{key}}}"), value);
    }
    expanded
}

pub fn evaluate_condition(condition: &str, config_values: &HashMap<String, toml::Value>) -> bool {
    evaluate_condition_inner(condition.trim(), config_values)
}

fn evaluate_condition_inner(s: &str, config_values: &HashMap<String, toml::Value>) -> bool {
    let s = s.trim();
    if s.is_empty() || s == "true" {
        return true;
    }
    if s == "false" {
        return false;
    }
    if s.starts_with('(') && s.ends_with(')') && balanced_parens(s) {
        return evaluate_condition_inner(&s[1..s.len() - 1], config_values);
    }
    if let Some(idx) = find_top_level_op(s, "||") {
        return evaluate_condition_inner(&s[..idx], config_values)
            || evaluate_condition_inner(&s[idx + 2..], config_values);
    }
    if let Some(idx) = find_top_level_op(s, "&&") {
        return evaluate_condition_inner(&s[..idx], config_values)
            && evaluate_condition_inner(&s[idx + 2..], config_values);
    }
    if let Some(rest) = s.strip_prefix('!') {
        return !evaluate_condition_inner(rest.trim(), config_values);
    }
    if let Some(inner) = s
        .strip_prefix("file_exists(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let path = inner.trim().trim_matches('"').trim_matches('\'');
        return Path::new(path).exists();
    }
    for op in &["==", "!=", ">=", "<=", ">", "<"] {
        if let Some(idx) = find_top_level_op(s, op) {
            let lhs = s[..idx].trim();
            let rhs = s[idx + op.len()..].trim();
            if let Some(key) = lhs.strip_prefix("config.") {
                return compare_config_value(config_values.get(key), op, rhs);
            }
        }
    }
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
    true
}

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
        if depth == 0 && i < bytes.len() - 1 {
            return false;
        }
    }
    true
}

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
                    "==" => (f - rhs_num).abs() < 1e-9,
                    "!=" => (f - rhs_num).abs() >= 1e-9,
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
                "==" => sv == rhs_str,
                "!=" => sv != rhs_str,
                _ => false,
            }
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    pub total_rules: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_duration_secs: f64,
    pub rule_durations: HashMap<String, f64>,
    pub max_rule_duration_secs: f64,
    pub bottleneck_rule: Option<String>,
}

impl ExecutionStats {
    pub fn from_records(records: &HashMap<String, JobRecord>) -> Self {
        let mut succeeded = 0;
        let mut failed = 0;
        let mut skipped = 0;
        let mut rule_durations = HashMap::new();
        let mut max_duration = 0.0;
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
        Self {
            total_rules: records.len(),
            succeeded,
            failed,
            skipped,
            total_duration_secs: rule_durations.values().sum(),
            rule_durations,
            max_rule_duration_secs: max_duration,
            bottleneck_rule: bottleneck,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionProvenance {
    pub oxo_flow_version: String,
    pub config_checksum: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub hostname: String,
    pub workdir: String,
    #[serde(default)]
    pub operator_id: Option<String>,
    #[serde(default)]
    pub instrument_id: Option<String>,
    #[serde(default)]
    pub reagent_lot: Option<String>,
    #[serde(default)]
    pub specimen_id: Option<String>,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub input_checksums: HashMap<String, String>,
    #[serde(default)]
    pub output_checksums: HashMap<String, String>,
    #[serde(default)]
    pub software_versions: HashMap<String, String>,
}

impl ExecutionProvenance {
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
    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }
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

pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn cleanup_cache(workdir: &Path, max_age_days: u64) -> usize {
    let cache_dir = workdir.join(".oxo-flow");
    if !cache_dir.exists() {
        return 0;
    }
    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Ok(metadata) = std::fs::metadata(&path)
                && let Ok(modified) = metadata.modified()
                && let Ok(age) = now.duration_since(modified)
                && age > max_age
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

/// Check rule input/output paths for remote storage URIs and log a warning
/// if any are found. Full remote-file integration is not yet implemented, so
/// this serves as an early-detection signal to users.
///
/// This is a placeholder -- future work will wire up `StorageResolver` to
/// transparently stage remote inputs and upload outputs.
pub fn warn_if_remote_paths(rule: &Rule, wildcard_values: &HashMap<String, String>) {
    let check_path = |path: &str| {
        let rendered = render_shell_command(path, rule, wildcard_values);
        let sp = StoragePath::parse(&rendered);
        if sp.is_remote() {
            tracing::warn!(
                rule = %rule.name,
                path = %rendered,
                scheme = ?sp.scheme,
                "remote storage path detected but full integration is not yet implemented; \
                 this path may not be accessible"
            );
        }
    };
    for input in rule.input.iter() {
        check_path(input);
    }
    for output in rule.output.iter() {
        check_path(output);
    }
}
