use crate::environment::EnvironmentResolver;
use crate::error::{OxoFlowError, Result};
use crate::rule::{FilePatterns, Rule};
use crate::scheduler::ResourcePool;
use crate::storage::StorageResolver;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};

use super::checkpoint::{cleanup_temp_outputs, validate_outputs};
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
    /// Sampled peak RSS of the rule's process subtree in MiB
    /// (`None` when no child was spawned; issue #67 §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rss_mb: Option<u64>,
    /// Sampled CPU time of the rule's process in seconds (all its
    /// threads; child processes are not accumulated; `None` when the
    /// sampler never observed the child; issue #83 P1-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<f64>,
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
    /// Force re-execution of every rule selected for this run, even when
    /// outputs are up to date (CLI `--rerun`). Checkpoint records for
    /// rules outside this run are untouched.
    pub force_rerun: bool,
    /// Rules forced to re-execute even when their outputs are fresh.
    ///
    /// Populated by config-change impact analysis (issue #62): rules
    /// invalidated in the checkpoint must bypass the mtime freshness gate,
    /// otherwise their stale outputs would be silently reused.
    pub force_rules: std::collections::HashSet<String>,
    pub skip_env_setup: bool,
    pub cache_dir: Option<PathBuf>,
    pub interpreter_map: HashMap<String, String>,
    /// Storage backends for remote input staging / output upload
    /// (issue #80 item 2). Defaults to the local backend only.
    pub storage_resolver: StorageResolver,
    /// Values of `[config_meta.*] sensitive = true` config keys — masked out
    /// of captured stdout/stderr and the recorded command before they reach
    /// the checkpoint/report (issue #99 B1). Empty = no masking.
    pub sensitive_values: Vec<String>,
    /// Workflow-global shell prelude prepended to every rule command and
    /// hook on its own line (issue #92), e.g. `set -euo pipefail`.
    /// Opt-in: `None` keeps the historical exact command text.
    pub shell_prelude: Option<String>,
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
            force_rerun: false,
            force_rules: std::collections::HashSet::new(),
            skip_env_setup: false,
            cache_dir: None,
            interpreter_map: HashMap::new(),
            storage_resolver: StorageResolver::with_local(),
            sensitive_values: Vec::new(),
            shell_prelude: None,
        }
    }
}

pub struct LocalExecutor {
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
    env_resolver: EnvironmentResolver,
    resource_pool: Arc<Mutex<ResourcePool>>,
    /// Wakes waiters when resources are released back to the pool.
    resource_notify: Arc<tokio::sync::Notify>,
    /// Detected system thread count (respects cgroup limits on Linux).
    system_threads: u32,
    /// Detected system total memory in MB (respects cgroup limits on Linux).
    system_memory_mb: u64,
    /// Shared per-run peak-RSS sampler (issue #67 §4).
    rss_sampler: Arc<super::rss::RssSampler>,
}

/// Detect total system memory in MB using the most reliable method available
/// on the current platform. On Linux, falls back to parsing `/proc/meminfo`
/// if the sysinfo crate returns unexpected results.
pub(crate) fn detect_total_memory_mb() -> u64 {
    // Primary: sysinfo crate (cross-platform)
    if let Ok(mb) = std::panic::catch_unwind(|| {
        use sysinfo::System;
        // Only memory is needed; avoid `System::new_all()` scanning all of /proc.
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory() / 1024 / 1024
    }) && mb > 0
    {
        return mb;
    }

    // Fallback for Linux: read /proc/meminfo directly
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                // Format: "MemTotal:       1056640524 kB"
                if let Some(kb_str) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    let mb = kb_str / 1024;
                    if mb > 0 {
                        return mb;
                    }
                }
            }
        }
    }

    0
}

/// Detect total swap in MB (0 when absent). Swap is backable memory the
/// kernel will use under pressure — the effective resource ceiling is
/// RAM + swap, so tools sized by `{effective_memory_mb}` and the
/// container `--memory` clamp use the whole backable budget on
/// small-memory boxes (live: tx-ubuntu's 3.7G RAM + 6G swap).
pub(crate) fn detect_swap_mb() -> u64 {
    if let Ok(mb) = std::panic::catch_unwind(|| {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_swap() / 1024 / 1024
    }) && mb > 0
    {
        return mb;
    }
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("SwapTotal:")
                && let Some(kb_str) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok())
            {
                let mb = kb_str / 1024;
                if mb > 0 {
                    return mb;
                }
            }
        }
    }
    0
}

impl LocalExecutor {
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_jobs));
        let env_resolver = {
            let cache_dir = config
                .cache_dir
                .clone()
                .unwrap_or_else(|| config.workdir.join(".oxo-flow").join("env-cache"));
            EnvironmentResolver::with_cache_dir(&cache_dir)
        };
        let (max_threads, max_memory_mb) = Self::detect_system_resources(&config);
        tracing::debug!(
            threads = max_threads,
            memory_gb = format!("{:.1}", max_memory_mb as f64 / 1024.0),
            "Detected system resources"
        );
        let mut resource_pool = ResourcePool::new(max_threads, max_memory_mb);
        resource_pool.set_groups(config.resource_groups.clone());
        let resource_pool = Arc::new(Mutex::new(resource_pool));

        Self {
            config,
            semaphore,
            env_resolver,
            resource_pool,
            resource_notify: Arc::new(tokio::sync::Notify::new()),
            system_threads: max_threads,
            system_memory_mb: max_memory_mb,
            rss_sampler: Arc::new(super::rss::RssSampler::new()),
        }
    }

    fn detect_system_resources(config: &ExecutorConfig) -> (u32, u64) {
        let max_threads = config.max_threads.unwrap_or_else(|| {
            // Respect cgroup CPU limits on Linux via num_cpus
            num_cpus::get() as u32
        });

        // Cross-platform memory detection with Linux /proc/meminfo fallback.
        // The ceiling is RAM + swap: swap is backable memory the kernel
        // will use under pressure, and ignoring it leaves real capacity
        // unused on small boxes (live: 3.7G RAM + 6G swap runs better
        // sized to ~9.7G). Override with --max-memory when the swap
        // should not count (e.g. latency-sensitive lanes).
        let max_memory_mb = config.max_memory_mb.unwrap_or_else(|| {
            let detected = detect_total_memory_mb() + detect_swap_mb();
            if detected > 0 {
                detected
            } else {
                tracing::warn!("Could not detect system memory; defaulting to 8192MB. Set --max-memory to override.");
                8192
            }
        });

        (max_threads, max_memory_mb)
    }

    /// Provide helpful suggestions for common environment setup failures.
    fn env_setup_hint(kind: &str, stderr: &str) -> Option<String> {
        let stderr_lower = stderr.to_lowercase();
        match kind {
            "mamba" => {
                if stderr_lower.contains("command not found")
                    || stderr_lower.contains("no such file")
                {
                    Some("mamba / micromamba is not installed or not in PATH. Install Mambaforge: https://github.com/conda-forge/miniforge".into())
                } else if stderr_lower.contains("prefix already exists") {
                    Some("environment already exists — this should have been caught by the cache. Try running with a clean cache directory.".into())
                } else if stderr_lower.contains("solver") || stderr_lower.contains("conflict") {
                    Some("dependency solver conflict. Try relaxing version pins in the environment YAML, or add 'conda-forge' channel.".into())
                } else if stderr_lower.contains("environmentfilenotfound")
                    || stderr_lower.contains("no such file")
                {
                    Some("environment YAML file not found. Check that the path is correct and relative to the workflow file.".into())
                } else if stderr_lower.contains("permission denied")
                    || stderr_lower.contains("operation not permitted")
                {
                    Some(
                        "permission denied creating environment. If using a project-local prefix, check that the parent directory is writable."
                            .into(),
                    )
                } else {
                    None
                }
            }
            "conda" => {
                if stderr_lower.contains("command not found")
                    || stderr_lower.contains("no such file")
                {
                    Some("conda is not installed or not in PATH. Install Miniconda: https://docs.conda.io/en/latest/miniconda.html".into())
                } else if stderr_lower.contains("prefix already exists") {
                    Some("environment already exists — this should have been caught by the cache. Try running with a clean cache directory.".into())
                } else if stderr_lower.contains("solver") || stderr_lower.contains("conflict") {
                    Some("dependency solver conflict. Try relaxing version pins in the environment YAML, or add 'conda-forge' channel.".into())
                } else if stderr_lower.contains("environmentfilenotfound")
                    || stderr_lower.contains("no such file")
                {
                    Some("environment YAML file not found. Check that the path is correct and relative to the workflow file.".into())
                } else if stderr_lower.contains("permission denied")
                    || stderr_lower.contains("operation not permitted")
                {
                    Some(
                        "permission denied creating conda environment. If using a project-local prefix, check that the parent directory is writable."
                            .into(),
                    )
                } else {
                    None
                }
            }
            "docker" => {
                if stderr_lower.contains("command not found")
                    || stderr_lower.contains("no such file")
                {
                    Some("Docker is not installed or not in PATH. Install Docker: https://docs.docker.com/engine/install/".into())
                } else if stderr_lower.contains("permission denied") {
                    Some("Docker requires permission. Add your user to the 'docker' group or use sudo.".into())
                } else if stderr_lower.contains("pull") && stderr_lower.contains("error") {
                    Some("Docker image pull failed. Check image name, network connectivity, or registry authentication.".into())
                } else {
                    None
                }
            }
            "singularity" => {
                if stderr_lower.contains("command not found")
                    || stderr_lower.contains("no such file")
                {
                    Some("Singularity/Apptainer is not installed. Install: https://apptainer.org/docs/admin/main/installation.html".into())
                } else {
                    None
                }
            }
            "pixi" => {
                if stderr_lower.contains("command not found") {
                    Some(
                        "pixi is not installed. Install: https://pixi.sh/latest/#installation"
                            .into(),
                    )
                } else {
                    None
                }
            }
            "venv" => {
                if stderr_lower.contains("command not found") {
                    Some("python3 is not installed. Install Python 3: https://www.python.org/downloads/".into())
                } else {
                    None
                }
            }
            _ => None,
        }
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
        // Cold cache but the env may already exist (checkpoint wipe, a
        // previous run, an external `conda create`): verify it in place
        // instead of re-running the setup. The setup's fallback
        // `conda env update --prune` re-resolves every dependency — live:
        // tcasia's majiq==2.5 pip resolution failed on a flaky mirror even
        // though the env was fully installed.
        if let Ok(Some(verify)) = self.env_resolver.verify_command(env_spec)
            && self.env_verify(&verify).await
        {
            self.env_resolver.cache_mark_ready(&key).await;
            return Ok(());
        }
        // Serialize setup per environment: concurrent rule instances sharing
        // an env (e.g. S1 + S2 instances of the same rule) used to run two
        // `conda env create` in parallel — the loser's transaction removes
        // the winner's just-installed packages (live evidence: rnaseq's
        // env history shows `+fq` followed by `-fq` 12s later, leaving an
        // empty env that the cache then marked ready).
        let setup_lock = self.env_resolver.setup_lock(&key);
        let _setup_guard = setup_lock.lock().await;
        // Double-check: another task may have completed the setup while we
        // waited for the lock.
        if self.env_resolver.cache_is_ready(&key).await {
            return Ok(());
        }
        let setup_cmd = self.env_resolver.setup_command(env_spec)?;
        // Some packages' post-link scripts download data during
        // `conda env create` (bioconductor-genomeinfodbdata fetches its
        // annotation tarballs) — before the new env's own ca-certificates
        // bundle is linked, so their curls die with SSL 77 (live evidence:
        // clindet + enrichment region_enrichment_analysis envs). Export
        // the base conda CA bundle for the setup command when available.
        let kind = env_spec.kind();
        let setup_cmd = if kind == "conda" || kind == "mamba" {
            format!(
                "CB=\"$(dirname \"$(dirname \"$(command -v conda)\")\")/ssl/cacert.pem\"; \
                 [ -f \"$CB\" ] && export SSL_CERT_FILE=\"$CB\"; {setup_cmd}"
            )
        } else {
            setup_cmd
        };
        // Cross-process serialization, PER ENVIRONMENT: another run on this
        // machine creating the SAME env would corrupt the transaction
        // (rnaseq +fq/-fq live evidence); DIFFERENT envs are independent by
        // conda/pixi semantics and must not contend (the old global lock
        // let a 3.5h bioconductor solve stall every env setup). The wait
        // is bounded — a stuck holder fails fast with a diagnostic instead
        // of hanging the queue (OXO_ENV_LOCK_TIMEOUT_SECS to tune).
        let lock_key = key.clone();
        let cross_process_guard = tokio::task::spawn_blocking(move || {
            super::env_create_lock::EnvCreateLock::acquire(&lock_key)
        })
        .await;
        let cross_process_guard = match cross_process_guard {
            Ok(Ok(guard)) => guard,
            Ok(Err(e)) => {
                tracing::error!(error = %e, "env create lock acquisition failed");
                return Err(OxoFlowError::Environment {
                    kind: kind.to_string(),
                    message: format!("env create lock acquisition failed: {e}"),
                });
            }
            Err(join_err) => {
                tracing::warn!(error = %join_err, "env lock task join failed — env setup proceeds unlocked");
                None
            }
        };
        if cross_process_guard.is_none() {
            tracing::warn!(
                "env-create cross-process lock unavailable — env setup proceeds unlocked"
            );
        }
        let output = Command::new("sh")
            .arg("-c")
            .arg(&setup_cmd)
            .current_dir(&self.config.workdir)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                // Setup reported success — but conda/mamba can exit 0 while
                // leaving a broken env behind (interrupted transactions on
                // loaded machines; live evidence: tx-ubuntu, where
                // `conda env update --prune` left an env with no bin/ and
                // exit code 0). Verify usability; on failure, tear the env
                // down and retry setup once before reporting an error.
                match self.env_resolver.verify_command(env_spec)? {
                    None => {
                        self.env_resolver.cache_mark_ready(&key).await;
                        Ok(())
                    }
                    Some(verify) => {
                        let mut verified = Self::env_verify(self, &verify).await;
                        if !verified
                            && let Ok(Some(teardown)) = self.env_resolver.teardown_command(env_spec)
                        {
                            tracing::warn!(
                                rule = %rule.name,
                                "environment setup exited 0 but verification failed — tearing down and retrying once"
                            );
                            let _ = Command::new("sh")
                                .arg("-c")
                                .arg(&teardown)
                                .current_dir(&self.config.workdir)
                                .output()
                                .await;
                            if let Ok(retry) = Command::new("sh")
                                .arg("-c")
                                .arg(&setup_cmd)
                                .current_dir(&self.config.workdir)
                                .output()
                                .await
                                && retry.status.success()
                            {
                                verified = Self::env_verify(self, &verify).await;
                            }
                        }
                        if verified {
                            self.env_resolver.cache_mark_ready(&key).await;
                            Ok(())
                        } else {
                            Err(OxoFlowError::Environment {
                                kind: env_spec.kind().to_string(),
                                message:
                                    "environment setup exited 0 but verification failed — the \
                                     environment is broken (a previous interrupted creation \
                                     may have left a partial prefix)"
                                        .to_string(),
                            })
                        }
                    }
                }
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                let hint = Self::env_setup_hint(env_spec.kind(), &stderr);
                Err(OxoFlowError::Environment {
                    kind: env_spec.kind().to_string(),
                    message: if let Some(h) = hint {
                        format!("setup failed: {}\nHint: {}", stderr.trim(), h)
                    } else {
                        format!("setup failed: {}", stderr.trim())
                    },
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                let hint = Self::env_setup_hint(env_spec.kind(), &err_msg);
                Err(OxoFlowError::Environment {
                    kind: env_spec.kind().to_string(),
                    message: if let Some(h) = hint {
                        format!("setup command failed: {}\nHint: {}", err_msg, h)
                    } else {
                        format!("setup command failed: {}", err_msg)
                    },
                })
            }
        }
    }

    /// Run an environment verification command (success = usable env).
    async fn env_verify(&self, verify: &str) -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(verify)
            .current_dir(&self.config.workdir)
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    }

    fn resolve_command(&self, command: &str, rule: &Rule, scratch_dir: Option<&Path>) -> String {
        // Container backends use the declared memory as their cgroup limit.
        // Cap it at the PHYSICAL RAM (or the explicit --max-memory override):
        // a cgroup above physical RAM lets a tool allocate past what the
        // kernel has and the OOM killer then shoots the container (live:
        // nanoplot exit 137 under a 9.7G RAM+swap cgroup on a 3.7G box).
        // Swap counts for the SCHEDULING pool, not for container limits.
        // cgroup-aware tools (cellranger, picard, STAR) still see an honest
        // limit. Live: eager's MarkDuplicates got --memory 4096M on a
        // 3723MB machine.
        let mut resources = rule.resources.clone();
        let container_ceiling_mb = self
            .config
            .max_memory_mb
            .unwrap_or_else(detect_total_memory_mb);
        if let Some(declared) = resources
            .memory
            .as_deref()
            .and_then(crate::scheduler::parse_memory_mb)
            && declared > container_ceiling_mb
        {
            resources.memory = Some(format!("{container_ceiling_mb}M"));
        }
        match self.env_resolver.wrap_command(
            command,
            &rule.environment,
            Some(&resources),
            &self.config.workdir,
        ) {
            Ok(wrapped) => match scratch_dir {
                Some(scratch) => fixup_container_wrapper(
                    &wrapped,
                    rule.environment.kind(),
                    &self.config.workdir,
                    scratch,
                ),
                None => wrapped,
            },
            Err(e) => {
                tracing::warn!(rule = %rule.name, error = %e, "environment wrapping failed");
                command.to_string()
            }
        }
    }

    /// Wait until the resource pool can accommodate the rule, then reserve
    /// atomically.  Rules that can never fit in the total pool capacity fail
    /// fast instead of waiting forever.
    pub(crate) async fn check_resources(&self, rule: &Rule) -> Result<()> {
        let required_threads = rule.effective_threads();
        let required_memory = rule
            .effective_memory()
            .and_then(crate::scheduler::parse_memory_mb)
            .unwrap_or(0);

        // Requests beyond the machine's total capacity are clamped for pool
        // accounting: the declared value is the tool's upper bound (often an
        // upstream HPC label copied verbatim by a port), not a hard scheduling
        // requirement. An over-capacity rule reserves the whole pool and
        // therefore runs alone.
        if required_threads > self.system_threads {
            tracing::warn!(
                rule = %rule.name,
                required_threads,
                available_threads = self.system_threads,
                "rule requests more threads than the machine provides — clamping the pool reservation (the rule will run alone)"
            );
        }
        if required_memory > self.system_memory_mb {
            tracing::warn!(
                rule = %rule.name,
                required_memory_mb = required_memory,
                available_memory_mb = self.system_memory_mb,
                "rule requests more memory than the machine provides — clamping the pool reservation (the rule will run alone)"
            );
        }

        // Fast-fail: a group requirement above the declared capacity (or an
        // undeclared group) can never be satisfied — the wait loop below
        // would otherwise hang forever. Group capacities are explicit user
        // declarations, so mismatches stay hard errors.
        for (group_name, &required) in &rule.resources.groups {
            let available = self
                .config
                .resource_groups
                .get(group_name)
                .copied()
                .unwrap_or(0);
            if required > available {
                return Err(OxoFlowError::ResourceGroupExhausted {
                    rule: rule.name.clone(),
                    group: group_name.clone(),
                    required,
                    available,
                });
            }
        }

        loop {
            {
                let mut pool = self.resource_pool.lock().await;
                if pool.can_accommodate(rule, self.system_threads, self.system_memory_mb) {
                    pool.reserve(rule, self.system_threads, self.system_memory_mb);
                    return Ok(());
                }
            }
            // Resources busy — wait for a release notification.
            self.resource_notify.notified().await;
        }
    }

    async fn release_resources(&self, rule: &Rule) {
        let max_threads = self.config.max_threads.unwrap_or(self.system_threads);
        let max_memory_mb = self.config.max_memory_mb.unwrap_or(self.system_memory_mb);
        let mut pool = self.resource_pool.lock().await;
        pool.release(
            rule,
            max_threads,
            max_memory_mb,
            &self.config.resource_groups,
        );
        drop(pool);
        // Wake all waiters — released capacity may satisfy multiple rules.
        self.resource_notify.notify_waiters();
    }

    fn get_timeout(&self, rule: &Rule) -> Option<std::time::Duration> {
        if let Some(ref time_limit) = rule.resources.time_limit
            && let Some(secs) = crate::rule::parse_duration_secs(time_limit)
        {
            return Some(std::time::Duration::from_secs(secs));
        }
        self.config.timeout
    }

    /// Build the per-instance scratch directory path for a scratch rule.
    ///
    /// The name combines a sanitized rule name with a millisecond timestamp
    /// and a process-local counter so parallel instances of the same rule
    /// never collide. The directory itself is created only right before the
    /// command spawns — pre-flight failures must not leak empty dirs.
    fn scratch_dir_for(&self, rule_name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.config
            .workdir
            .join(".oxo-flow")
            .join("scratch")
            .join(format!(
                "{}-{millis}-{n}",
                sanitize_dir_component(rule_name)
            ))
    }

    /// Move every declared output that was produced inside the scratch
    /// directory back to its declared location in the main workdir.
    ///
    /// Outputs that never appeared in the scratch (e.g. a tool wrote an
    /// absolute path directly) are left where they are — validation runs
    /// against the main workdir either way.
    async fn move_scratch_outputs(
        &self,
        rule: &Rule,
        scratch_dir: &Path,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<()> {
        for output in rule.output.to_vec() {
            let expanded = super::checkpoint::expand_config_in_path(&output, wildcard_values);
            // Unresolved wildcards have no single declared location and
            // validation skips them too — leave them in scratch.
            if crate::wildcard::has_wildcards(&expanded) {
                continue;
            }
            let src = scratch_dir.join(&expanded);
            if !src.exists() {
                continue;
            }
            let dest = self.config.workdir.join(&expanded);
            // Parent directories mirror the pre-execution creation logic.
            if let Some(parent) = dest.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                create_rule_dir(parent, rule, "output")?;
            }
            move_path(&src, &dest)
                .await
                .map_err(|e| OxoFlowError::Execution {
                    rule: rule.name.clone(),
                    message: format!(
                        "failed to move scratch output '{expanded}' back to the workdir: {e}"
                    ),
                })?;
            tracing::debug!(rule = %rule.name, output = %expanded, "moved scratch output to workdir");
        }
        Ok(())
    }

    pub async fn execute_rule(
        &self,
        rule: &Rule,
        wildcard_values: &HashMap<String, String>,
    ) -> Result<JobRecord> {
        self.execute_rule_with_config(rule, wildcard_values, &HashMap::new())
            .await
    }

    /// Fold one handle's sampled CPU seconds into the running total:
    /// values are summed per attempt, and `None` contributions (the
    /// sampler never observed the process) leave the total untouched.
    fn fold_cpu_seconds(total: Option<f64>, handle: &super::rss::RssHandle) -> Option<f64> {
        match (total, handle.cpu_seconds()) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// Execute a single rule with typed config values for condition
    /// evaluation.
    ///
    /// `config` should be the original `WorkflowConfig.config` map (preserving
    /// TOML types). When provided, `when` conditions use typed comparisons
    /// (e.g., `config.min_qual >= 20` for integers). When empty, falls back to
    /// string-only comparisons from `wildcard_values` (backward compatible).
    pub async fn execute_rule_with_config(
        &self,
        rule: &Rule,
        wildcard_values: &HashMap<String, String>,
        config: &HashMap<String, toml::Value>,
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
            max_rss_mb: None,
            cpu_seconds: None,
        };

        // Condition evaluation happens before any remote staging so a rule
        // whose `when` is false never triggers downloads.
        if let Some(ref condition) = rule.when {
            // Build condition values from typed config first (preserves
            // int/float/bool for numeric comparisons), then fall back to
            // wildcard_values (strings from CLI args and template expansion).
            let mut config_values: HashMap<String, toml::Value> =
                config.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (k, v) in wildcard_values {
                if let Some(key) = k.strip_prefix("config.") {
                    config_values
                        .entry(key.to_string())
                        .or_insert_with(|| toml::Value::String(v.clone()));
                }
            }
            if !evaluate_condition(condition, &config_values) {
                record.status = JobStatus::Skipped;
                record.skip_reason = Some("condition evaluated to false".to_string());
                record.finished_at = Some(Utc::now());
                return Ok(record);
            }
        }

        // Dry runs stay read-only: no staging, no downloads.
        if self.config.dry_run {
            record.status = JobStatus::Skipped;
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Stage remote inputs and redirect remote outputs to local
        // upload-stage paths (issue #80 item 2). The substitution lives on
        // a copy of the rule — `config.rules` is never mutated and
        // checkpoints keep recording the original remote URIs.
        let (rule, uploads) = match super::staging::stage_remote_io(
            rule,
            &self.config.workdir,
            wildcard_values,
            &self.config.storage_resolver,
        )
        .await
        {
            Ok(Some(prep)) => {
                if prep.missing_optional_input && rule.optional {
                    record.status = JobStatus::Skipped;
                    record.skip_reason = Some("optional inputs missing".to_string());
                    record.finished_at = Some(Utc::now());
                    return Ok(record);
                }
                (prep.rule, prep.uploads)
            }
            Ok(None) => (rule.clone(), Vec::new()),
            Err(e) => {
                record.status = JobStatus::Failed;
                record.finished_at = Some(Utc::now());
                record.exit_code = Some(-1);
                // Staging errors embed config-expanded URIs — mask them
                // like every other captured surface (issue #99 B1).
                record.stderr = Some(mask_sensitive(
                    &format!("\n[oxo-flow] {e}"),
                    &self.config.sensitive_values,
                ));
                return Ok(record);
            }
        };

        // Scratch rules render inputs (and scripts) as absolute paths into
        // the main workdir while keeping outputs relative — the shell runs
        // with its cwd in the scratch dir, so relative outputs land there
        // and are collected afterwards.
        let base_cmd = if rule.scratch {
            build_execution_command_in_scratch(
                &rule,
                wildcard_values,
                &self.config.interpreter_map,
                &self.config.workdir,
                crate::scheduler::ResourceLimits {
                    threads: self.system_threads,
                    memory_mb: self.system_memory_mb,
                },
            )
        } else {
            build_execution_command(
                &rule,
                wildcard_values,
                &self.config.interpreter_map,
                crate::scheduler::ResourceLimits {
                    threads: self.system_threads,
                    memory_mb: self.system_memory_mb,
                },
            )
        };
        let base_cmd = match base_cmd {
            Some(cmd) => cmd,
            None => {
                record.status = JobStatus::Skipped;
                record.finished_at = Some(Utc::now());
                record.skip_reason = Some("no shell or script defined".to_string());
                return Ok(record);
            }
        };
        // Workflow-global shell prelude (issue #92): prepended BEFORE the
        // environment wrapper resolves, so the prelude runs INSIDE the
        // container/conda wrapper — the local and cluster paths share one
        // semantics.
        let base_cmd =
            crate::config::prepend_shell_prelude(&base_cmd, self.config.shell_prelude.as_deref());

        // Optional rules skip (no error) when their declared inputs are
        // absent — e.g. analysis steps that only apply to some samples
        // (issue #75). Remote inputs were already resolved by staging
        // (missing optional remote inputs skipped above); this checks the
        // remaining local paths. Evaluated before the freshness gate so a
        // missing input never falls through to a failing shell command.
        if super::checkpoint::optional_inputs_missing(&rule, &self.config.workdir, wildcard_values)
        {
            record.status = JobStatus::Skipped;
            record.skip_reason = Some("optional inputs missing".to_string());
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Freshness gate: outputs up-to-date. For remote outputs the
        // uploaded objects are authoritative for existence — a locally
        // cached upload stage is not proof the cloud still has the file.
        if !self.config.force_rerun
            && !self.config.force_rules.contains(&rule.name)
            && super::checkpoint::should_skip_rule(&rule, &self.config.workdir, wildcard_values)
            && self.remote_outputs_present(&uploads).await
        {
            record.status = JobStatus::Skipped;
            record.skip_reason = Some("outputs up-to-date".to_string());
            record.finished_at = Some(Utc::now());
            return Ok(record);
        }

        // Scratch rules run in an isolated directory. The path is decided
        // here (environment wrapping needs it for container mounts) but the
        // directory itself is only created right before the command spawns,
        // so pre-flight failures never leak empty dirs.
        let scratch_dir = rule.scratch.then(|| self.scratch_dir_for(&rule.name));

        // Pre-execution snapshot of the declared outputs: on failure, only
        // files this attempt creates or modifies are invalidated (issue
        // #118) — a failed rule must never leave partial outputs that the
        // freshness gate would treat as up-to-date on the next run.
        let output_snapshot = super::output_invalidation::snapshot_outputs(
            &rule,
            &self.config.workdir,
            wildcard_values,
        );

        let resolved_commands =
            vec![self.resolve_command(&base_cmd, &rule, scratch_dir.as_deref())];
        record.command = resolved_commands
            .first()
            .cloned()
            .map(|c| mask_sensitive(&c, &self.config.sensitive_values));

        validate_wildcard_injection(wildcard_values)?;
        for cmd in &resolved_commands {
            validate_shell_safety(cmd)?;
            for warning in sanitize_shell_command(cmd) {
                tracing::info!(rule = %rule.name, "{warning} (common in bioinformatics scripts)");
            }
        }

        // Validate output paths for traversal safety
        for output_pattern in rule.output.to_vec() {
            validate_path_safety(&self.config.workdir, &output_pattern)?;
        }

        // Warn if GPU is requested but running on local executor (no GPU scheduling)
        if rule.resources.gpu_spec.is_some() {
            tracing::warn!(
                rule = %rule.name,
                "GPU spec declared but running on local executor — GPU resources will not be verified. \
                 Use a cluster backend (slurm, pbs, sge, lsf) for GPU scheduling."
            );
        }

        // Create parent directories for all output files
        for output_pattern in &rule.output {
            // Expand config variables (e.g. {config.results_dir}) before
            // creating parent directories — otherwise a literal
            // `{config.…}` directory is created and the shell command
            // later fails to find it.
            let expanded =
                super::checkpoint::expand_config_in_path(output_pattern, wildcard_values);
            let path = self.config.workdir.join(expanded);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                create_rule_dir(parent, &rule, "output")?;
            }
        }

        // Create the log file's parent directory with the same expansion
        // and failure semantics as output directories — `2> {log}` must
        // not fail because `logs/` does not exist yet.
        if let Some(log_pattern) = &rule.log {
            let expanded = super::checkpoint::expand_config_in_path(log_pattern, wildcard_values);
            let path = self.config.workdir.join(expanded);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                create_rule_dir(parent, &rule, "log")?;
            }
        }

        self.ensure_environment_ready(&rule).await?;
        // check_resources waits for availability AND reserves atomically.
        self.check_resources(&rule).await?;

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: rule.name.clone(),
                message: format!("semaphore error: {e}"),
            })?;

        // Create the scratch working directory now that execution is
        // certain; its name was already decided for env wrapping above.
        if let Some(scratch) = &scratch_dir {
            create_rule_dir(scratch, &rule, "scratch")?;
        }
        // The rule's shell cwd: scratch for scratch rules, main workdir
        // otherwise (docker/singularity run with `-w`/inherited cwd in the
        // scratch after the wrapper fixup above).
        let rule_cwd: &Path = scratch_dir.as_deref().unwrap_or(&self.config.workdir);

        // Hooks run the same placeholder rendering as the main command
        // ({config.x}, {input}, {output}) so hook commands see expanded
        // values instead of literal braces (issue #75).
        if let Some(ref pre_cmd) = rule.pre_exec {
            // Scratch rules render inputs absolute into the MAIN workdir
            // (the scratch dir is only the shell's cwd).
            let rendered = if scratch_dir.is_some() {
                render_shell_command_in_scratch(
                    pre_cmd,
                    &rule,
                    wildcard_values,
                    &self.config.workdir,
                    crate::scheduler::ResourceLimits {
                        threads: self.system_threads,
                        memory_mb: self.system_memory_mb,
                    },
                )
            } else {
                render_shell_command(
                    pre_cmd,
                    &rule,
                    wildcard_values,
                    crate::scheduler::ResourceLimits {
                        threads: self.system_threads,
                        memory_mb: self.system_memory_mb,
                    },
                )
            };
            if let Err(e) = validate_shell_safety(&rendered) {
                // Nothing ran yet — drop the empty scratch dir rather than
                // leaking one per attempt (the lifecycle contract: no
                // leftover dirs on paths where the shell never started).
                discard_scratch(scratch_dir.as_deref()).await;
                self.release_resources(&rule).await;
                return Err(e);
            }
            // pre_exec takes the same prelude as the other hooks (issue #92).
            let rendered_pre = crate::config::prepend_shell_prelude(
                &rendered,
                self.config.shell_prelude.as_deref(),
            );
            let pre_child = spawn_rule_shell(&rendered_pre, rule_cwd, &rule.envvars);
            let pre_result = match pre_child {
                Ok(child) => child.wait_with_output().await,
                Err(e) => {
                    discard_scratch(scratch_dir.as_deref()).await;
                    self.release_resources(&rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn pre_exec hook: {e}"),
                    });
                }
            };
            match pre_result {
                Ok(output) if !output.status.success() => {
                    // The hook ran and may have written diagnostic files —
                    // preserve the scratch dir and name its path (matches
                    // the "preserved on failure" contract).
                    let note = scratch_dir
                        .as_deref()
                        .map(scratch_preserved_note)
                        .unwrap_or_default();
                    self.release_resources(&rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("pre_exec hook failed{note}"),
                    });
                }
                Err(e) => {
                    discard_scratch(scratch_dir.as_deref()).await;
                    self.release_resources(&rule).await;
                    return Err(OxoFlowError::Execution {
                        rule: rule.name.clone(),
                        message: format!("failed to spawn pre_exec hook: {e}"),
                    });
                }
                _ => {}
            }
        }

        let max_attempts = 1 + std::cmp::max(self.config.retry_count, rule.retries);
        let mut all_commands_succeeded = false;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut last_exit_code: Option<i32> = None;
        // Sampled peak RSS across every attempt (issue #67 §4).
        let mut peak_bytes: u64 = 0;
        // Sampled CPU seconds across every attempt (issue #83 P1-13);
        // `None` until the sampler observes the child alive in a tick.
        let mut cpu_seconds: Option<f64> = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                record.retries = attempt;
            }

            all_commands_succeeded = true;
            combined_stdout.clear();
            combined_stderr.clear();

            for cmd in &resolved_commands {
                // Rules deliberately inherit the caller's process group instead
                // of starting their own (`process_group(0)`): one run = one
                // process group, so a supervisor (web cancel/pause/resume) or a
                // terminal Ctrl+C signals the run as a whole and no rule is ever
                // orphaned. Timeout enforcement kills the rule's subtree instead
                // (see timeout::kill_process_tree), so per-rule semantics are
                // unchanged.
                let child = match spawn_rule_shell(cmd, rule_cwd, &rule.envvars) {
                    Ok(child) => child,
                    Err(e) => {
                        // The shell never started — no diagnostic files can
                        // exist in the scratch dir; drop it instead of
                        // leaking one per failed attempt.
                        discard_scratch(scratch_dir.as_deref()).await;
                        self.release_resources(&rule).await;
                        return Err(OxoFlowError::Execution {
                            rule: rule.name.clone(),
                            message: format!("failed to spawn: {e}"),
                        });
                    }
                };

                let child_id = child.id();

                let rss_handle = child_id.map(|pid| self.rss_sampler.track(pid));

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
                            if let Some(handle) = rss_handle {
                                cpu_seconds = Self::fold_cpu_seconds(cpu_seconds, &handle);
                                peak_bytes = peak_bytes.max(handle.finish());
                            }
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
                            if let Some(handle) = rss_handle {
                                cpu_seconds = Self::fold_cpu_seconds(cpu_seconds, &handle);
                                peak_bytes = peak_bytes.max(handle.finish());
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        all_commands_succeeded = false;
                        record.status = JobStatus::Failed;
                        combined_stderr.push_str(&e.to_string());
                        if let Some(handle) = rss_handle {
                            cpu_seconds = Self::fold_cpu_seconds(cpu_seconds, &handle);
                            peak_bytes = peak_bytes.max(handle.finish());
                        }
                        break;
                    }
                }

                if let Some(handle) = rss_handle {
                    cpu_seconds = Self::fold_cpu_seconds(cpu_seconds, &handle);
                    peak_bytes = peak_bytes.max(handle.finish());
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
        // Mask sensitive values at the capture boundary (issue #99 B1):
        // everything downstream — checkpoint stderr_tail, report, AI
        // recovery, web UI — derives from this record.
        record.stdout = Some(mask_sensitive(
            &combined_stdout,
            &self.config.sensitive_values,
        ));
        record.stderr = Some(mask_sensitive(
            &combined_stderr,
            &self.config.sensitive_values,
        ));
        if peak_bytes > 0 {
            record.max_rss_mb = Some(peak_bytes.div_ceil(1024 * 1024));
        }
        record.cpu_seconds = cpu_seconds;

        self.release_resources(&rule).await;

        if all_commands_succeeded {
            // Scratch rules: collect declared outputs produced inside the
            // scratch dir back to their main-workdir locations BEFORE the
            // freshness validation below.
            if let Some(scratch) = &scratch_dir
                && let Err(e) = self
                    .move_scratch_outputs(&rule, scratch, wildcard_values)
                    .await
            {
                record.status = JobStatus::Failed;
                record.exit_code = Some(-1);
                push_stderr_note(&mut record, &format!("\n[oxo-flow] {e}"));
                push_stderr_note(&mut record, &scratch_preserved_note(scratch));
                cleanup_temp_outputs(&rule, &self.config.workdir).await;
                super::output_invalidation::invalidate_failed_outputs(&output_snapshot).await;
                if let Some(ref hook_cmd) = rule.on_failure {
                    run_hook(
                        &render_shell_command(
                            hook_cmd,
                            &rule,
                            wildcard_values,
                            crate::scheduler::ResourceLimits {
                                threads: self.system_threads,
                                memory_mb: self.system_memory_mb,
                            },
                        ),
                        &rule,
                        &self.config.workdir,
                        self.config.shell_prelude.as_deref(),
                    )
                    .await;
                }
                return Ok(record);
            }
            // Verify declared output files actually exist before marking success.
            // Shell commands can exit 0 even when tools fail internally
            // (e.g. "tool_a; rm -f temp" — rm succeeds, masking tool_a failure).
            let missing = validate_outputs(&rule, &self.config.workdir, wildcard_values);
            if missing.is_empty() {
                record.status = JobStatus::Success;
                // Remote outputs land in the cloud only after the local
                // copies passed validation (issue #80 item 2). An upload
                // failure fails the rule — a declared remote output that
                // did not land is a broken contract.
                let upload_ok =
                    if let Err(e) = self.upload_remote_outputs(&rule.name, &uploads).await {
                        record.status = JobStatus::Failed;
                        record.exit_code = Some(-1);
                        push_stderr_note(
                            &mut record,
                            &format!("\n[oxo-flow] remote output upload failed: {e}"),
                        );
                        if let Some(ref hook_cmd) = rule.on_failure {
                            run_hook(
                                &render_shell_command(
                                    hook_cmd,
                                    &rule,
                                    wildcard_values,
                                    crate::scheduler::ResourceLimits {
                                        threads: self.system_threads,
                                        memory_mb: self.system_memory_mb,
                                    },
                                ),
                                &rule,
                                &self.config.workdir,
                                self.config.shell_prelude.as_deref(),
                            )
                            .await;
                        }
                        false
                    } else {
                        if let Some(ref hook_cmd) = rule.on_success {
                            run_hook(
                                &render_shell_command(
                                    hook_cmd,
                                    &rule,
                                    wildcard_values,
                                    crate::scheduler::ResourceLimits {
                                        threads: self.system_threads,
                                        memory_mb: self.system_memory_mb,
                                    },
                                ),
                                &rule,
                                &self.config.workdir,
                                self.config.shell_prelude.as_deref(),
                            )
                            .await;
                        }
                        true
                    };
                // Scratch is transient: a fully successful rule drops it.
                // Failures (command, validation, upload) keep it around for
                // debugging, and the error message names its path.
                if upload_ok
                    && let Some(scratch) = &scratch_dir
                    && let Err(e) = tokio::fs::remove_dir_all(scratch).await
                {
                    tracing::warn!(
                        rule = %rule.name,
                        error = %e,
                        scratch = %scratch.display(),
                        "failed to remove scratch directory after success"
                    );
                }
            } else {
                record.status = JobStatus::Failed;
                record.exit_code = Some(-1);
                let missing_list = missing.join(", ");
                tracing::warn!(
                    rule = %rule.name,
                    missing = %missing_list,
                    "Shell exited 0 but declared outputs are missing — marking as failed"
                );
                let msg = format!(
                    "\n[oxo-flow] output validation failed: {} declared output(s) not found: {}",
                    missing.len(),
                    missing_list
                );
                push_stderr_note(&mut record, &msg);
                if let Some(scratch) = &scratch_dir {
                    push_stderr_note(&mut record, &scratch_preserved_note(scratch));
                }
                cleanup_temp_outputs(&rule, &self.config.workdir).await;
                super::output_invalidation::invalidate_failed_outputs(&output_snapshot).await;
                if let Some(ref hook_cmd) = rule.on_failure {
                    run_hook(
                        &render_shell_command(
                            hook_cmd,
                            &rule,
                            wildcard_values,
                            crate::scheduler::ResourceLimits {
                                threads: self.system_threads,
                                memory_mb: self.system_memory_mb,
                            },
                        ),
                        &rule,
                        &self.config.workdir,
                        self.config.shell_prelude.as_deref(),
                    )
                    .await;
                }
            }
        } else {
            // Keep the status set in the loop (Failed or TimedOut)
            if let Some(scratch) = &scratch_dir {
                push_stderr_note(&mut record, &scratch_preserved_note(scratch));
            }
            cleanup_temp_outputs(&rule, &self.config.workdir).await;
            super::output_invalidation::invalidate_failed_outputs(&output_snapshot).await;
            if let Some(ref hook_cmd) = rule.on_failure {
                run_hook(
                    &render_shell_command(
                        hook_cmd,
                        &rule,
                        wildcard_values,
                        crate::scheduler::ResourceLimits {
                            threads: self.system_threads,
                            memory_mb: self.system_memory_mb,
                        },
                    ),
                    &rule,
                    &self.config.workdir,
                    self.config.shell_prelude.as_deref(),
                )
                .await;
            }
        }

        Ok(record)
    }

    /// A rule whose outputs are remote is only "up to date" when both the
    /// local upload-stage copies and the remote objects exist — the cloud
    /// copy is authoritative for existence (issue #80 item 2).
    async fn remote_outputs_present(
        &self,
        uploads: &[(crate::storage::StoragePath, PathBuf)],
    ) -> bool {
        for (remote, local) in uploads {
            if !local.exists() {
                return false;
            }
            let Some(backend) = self.config.storage_resolver.get_backend(&remote.scheme) else {
                return false;
            };
            if !matches!(backend.head(remote).await, Ok(Some(_))) {
                return false;
            }
        }
        true
    }

    /// Upload every prepared remote output; the first failure aborts.
    async fn upload_remote_outputs(
        &self,
        rule_name: &str,
        uploads: &[(crate::storage::StoragePath, PathBuf)],
    ) -> Result<()> {
        for (remote, local) in uploads {
            let backend = self
                .config
                .storage_resolver
                .get_backend(&remote.scheme)
                .ok_or_else(|| OxoFlowError::Config {
                    message: format!(
                        "no storage backend registered for '{}' (rule '{rule_name}')",
                        remote.raw
                    ),
                })?;
            backend
                .upload(local, remote)
                .await
                .map_err(|e| OxoFlowError::Execution {
                    rule: rule_name.to_string(),
                    message: format!("failed to upload remote output '{}': {e}", remote.raw),
                })?;
        }
        Ok(())
    }

    pub fn dry_run_rules(&self, rules: &[Rule]) -> Vec<JobRecord> {
        rules
            .iter()
            .map(|rule| {
                let command = rule.shell.clone();
                // Dry-run is read-only: no scratch dir exists, so the
                // container wrapper is shown unmodified (a preview).
                let wrapped = command
                    .as_deref()
                    .map(|cmd| self.resolve_command(cmd, rule, None));

                // Apply shell safety checks in dry-run mode so dangerous
                // commands are visible to users before actual execution.
                if let Some(ref cmd) = wrapped {
                    if let Err(e) = validate_shell_safety(cmd) {
                        tracing::warn!(rule = %rule.name, error = %e, "dry-run: dangerous shell command detected");
                    }
                    for warning in sanitize_shell_command(cmd) {
                        tracing::info!(rule = %rule.name, "{warning} (common in bioinformatics scripts)");
                    }
                }
                // Also check the raw command if no wrapped version
                if let Some(ref raw_cmd) = command {
                    if let Err(e) = validate_shell_safety(raw_cmd) {
                        tracing::warn!(rule = %rule.name, error = %e, "dry-run: dangerous shell command detected");
                    }
                    for warning in sanitize_shell_command(raw_cmd) {
                        tracing::info!(rule = %rule.name, "{warning} (common in bioinformatics scripts)");
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
                    max_rss_mb: None,
                    cpu_seconds: None,
                }
            })
            .collect()
    }
}

pub fn build_execution_command(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    interpreter_map: &HashMap<String, String>,
    limits: crate::scheduler::ResourceLimits,
) -> Option<String> {
    build_execution_command_inner(rule, wildcard_values, interpreter_map, None, limits)
}

/// Scratch-mode variant: input and script paths render absolute (they live
/// in the main workdir), outputs stay relative so they land in the scratch
/// working directory.
pub(crate) fn build_execution_command_in_scratch(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    interpreter_map: &HashMap<String, String>,
    workdir: &Path,
    limits: crate::scheduler::ResourceLimits,
) -> Option<String> {
    build_execution_command_inner(
        rule,
        wildcard_values,
        interpreter_map,
        Some(workdir),
        limits,
    )
}

fn build_execution_command_inner(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    interpreter_map: &HashMap<String, String>,
    abs_root: Option<&Path>,
    limits: crate::scheduler::ResourceLimits,
) -> Option<String> {
    let shell_cmd = rule
        .shell
        .as_ref()
        .map(|cmd| render_shell_command_inner(cmd, rule, wildcard_values, abs_root, limits));

    let script_cmd = rule.script.as_ref().map(|script_path| {
        let expanded_script =
            render_shell_command_inner(script_path, rule, wildcard_values, abs_root, limits);
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

    // Auto-create output directories to eliminate mkdir -p boilerplate in shells
    let mut dirs_to_create: Vec<String> = Vec::new();
    for output in &rule.output {
        let expanded = render_shell_command_inner(output, rule, wildcard_values, abs_root, limits);
        // Only create dirs for paths with directory separators, skip wildcards
        if expanded.contains('/')
            && !expanded.contains('{')
            && let Some(parent) = std::path::Path::new(&expanded).parent()
        {
            let dir = parent.to_string_lossy().to_string();
            if !dir.is_empty() && dir != "." && !dirs_to_create.contains(&dir) {
                dirs_to_create.push(dir);
            }
        }
    }
    if !dirs_to_create.is_empty() {
        let mkdir_cmd = format!("mkdir -p {}", dirs_to_create.join(" "));
        base_cmd = format!("{}\n{}", mkdir_cmd, base_cmd);
    }

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

/// Spawn a rule command under `bash -c`, falling back to `sh -c` on
/// systems without bash (minimal containers). Upstream workflows rely on
/// bash features (process substitution `<(…)`, brace expansion) that
/// POSIX `sh` rejects; the bare execution path (no conda/docker/singularity
/// wrapping) must run under bash when available.
///
/// The fallback is logged once per process so operators can spot it.
pub(super) fn spawn_rule_shell(
    cmd: &str,
    workdir: &std::path::Path,
    envs: &HashMap<String, String>,
) -> std::io::Result<tokio::process::Child> {
    fn shell_command(
        shell: &str,
        cmd: &str,
        workdir: &std::path::Path,
        envs: &HashMap<String, String>,
    ) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(shell);
        c.arg("-c").arg(cmd).current_dir(workdir).envs(envs);
        // stdin is explicitly null (issue #101): tokio's default is INHERIT,
        // so a TTY-launched `oxo-flow run` would hand the terminal to every
        // rule — stdin-polling tools can park forever on it. Null stdin is
        // the batch-tool contract (what cluster submit scripts do with
        // </dev/null).
        c.stdin(Stdio::null());
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        c
    }
    match shell_command("bash", cmd, workdir, envs).spawn() {
        Ok(child) => Ok(child),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn_shell_fallback_once();
            shell_command("sh", cmd, workdir, envs).spawn()
        }
        Err(e) => Err(e),
    }
}

/// Emit the bash→sh fallback warning at most once per process.
fn warn_shell_fallback_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("bash not found on PATH — falling back to sh -c for shell commands");
    }
}

/// Create a rule's output/log/scratch parent directory synchronously.
///
/// Directory creation is a handful of fast syscalls; routing it through
/// tokio's blocking pool meant failures surfaced as the opaque
/// "background task failed" (observed repeatedly in live runs), hiding
/// the real error (ENOSPC, EACCES, …) from operators.
fn create_rule_dir(path: &Path, rule: &Rule, what: &str) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| OxoFlowError::Execution {
        rule: rule.name.clone(),
        message: format!("failed to create {what} directory {}: {e}", path.display()),
    })
}

/// Directory-name-safe rule names: alphanumerics plus `. _ -` survive,
/// everything else becomes `_` (rule names admit `:` per `Rule::validate`,
/// which is not a safe directory character everywhere).
fn sanitize_dir_component(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Make a container wrapper (docker / singularity) for a scratch rule see
/// both the main workdir (absolute input paths) and the scratch dir, and
/// run inside the scratch.
///
/// The container backends wrap commands as plain shell strings that
/// bind-mount only the executor workdir (environment.rs). Scratch rules
/// need an additional scratch bind and — for docker — the `-w` working
/// directory switched to the scratch dir. Only the prefix before the
/// first ` sh -c '` is touched (with the bash re-exec shim that is the
/// shim's own `sh -c`, which still precedes all mounts), so a user command
/// that coincidentally contains the same tokens is never rewritten.
/// Non-container wrappers pass through unchanged.
pub(super) fn fixup_container_wrapper(
    wrapped: &str,
    kind: &str,
    workdir: &Path,
    scratch: &Path,
) -> String {
    let workdir_str = workdir.display().to_string();
    let scratch_str = scratch.display().to_string();
    let (prefix, rest) = match wrapped.find(" sh -c '") {
        Some(i) => (&wrapped[..i], &wrapped[i..]),
        None => (wrapped, ""),
    };
    let fixed = match kind {
        "docker" => {
            let main_mount = format!("-v {workdir_str}:{workdir_str}");
            if !prefix.contains(&main_mount) {
                tracing::warn!(
                    wrapper = %prefix,
                    "docker wrapper lacks the expected workdir mount — scratch dir will not be mounted"
                );
                return wrapped.to_string();
            }
            prefix
                .replace(
                    &main_mount,
                    &format!("{main_mount} -v {scratch_str}:{scratch_str}"),
                )
                .replace(&format!("-w {workdir_str}"), &format!("-w {scratch_str}"))
        }
        "singularity" => {
            let main_bind = format!("--bind {workdir_str}:{workdir_str}");
            if !prefix.contains(&main_bind) {
                tracing::warn!(
                    wrapper = %prefix,
                    "singularity wrapper lacks the expected workdir bind — scratch dir will not be mounted"
                );
                return wrapped.to_string();
            }
            prefix.replace(
                &main_bind,
                &format!("{main_bind} --bind {scratch_str}:{scratch_str}"),
            )
        }
        _ => return wrapped.to_string(),
    };
    format!("{fixed}{rest}")
}

/// Move a file or directory, falling back to copy+delete when a plain
/// rename fails (e.g. scratch and workdir live on different filesystems).
async fn move_path(src: &Path, dest: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(src, dest).await {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_tree(src, dest)?;
            remove_tree(src).await
        }
    }
}

/// Recursively copy a file or directory tree (synchronous: output moves
/// touch bounded local files; `std::fs` avoids an infinitely sized future
/// that a recursive `async fn` would need to box).
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

/// Remove a file or directory tree; a missing path is not an error.
async fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    }
}

/// Append a diagnostic note to a job record's stderr, creating the field
/// when the rule produced no stderr at all.
/// Mask sensitive values in captured output (issue #99 B1).
///
/// Runner-side masking, GitHub-Actions-style: every occurrence of each
/// value is replaced with `***` before the text lands in the job record
/// (checkpoint, report, AI recovery, web). Values shorter than 4
/// characters are not masked — the same threshold GitHub Actions uses, so
/// common short strings (thresholds, counts) are not mass-redacted.
/// Structured forms of a secret (JSON/YAML serialization, base64) do not
/// match the exact value and pass through — the documented limitation of
/// exact-match masking.
pub fn mask_sensitive(text: &str, values: &[String]) -> String {
    if text.is_empty() || values.is_empty() {
        return text.to_string();
    }
    let mut masked = text.to_string();
    for value in values {
        if value.chars().count() >= 4 {
            masked = masked.replace(value.as_str(), "***");
        }
    }
    masked
}

fn push_stderr_note(record: &mut JobRecord, note: &str) {
    if let Some(ref mut stderr) = record.stderr {
        stderr.push_str(note);
    } else {
        record.stderr = Some(note.to_string());
    }
}

/// The note appended to a failing scratch rule's stderr so operators can
/// find the preserved working directory.
fn scratch_preserved_note(scratch: &Path) -> String {
    format!(
        "\n[oxo-flow] scratch directory preserved for debugging: {}",
        scratch.display()
    )
}

/// Remove a scratch directory on paths where the rule's shell never
/// started (spawn/safety failures): nothing diagnostic can exist inside,
/// so a leftover dir is a pure leak. Best-effort — removal failure only
/// warns, it never changes the rule's outcome.
async fn discard_scratch(scratch: Option<&Path>) {
    if let Some(dir) = scratch
        && let Err(e) = tokio::fs::remove_dir_all(dir).await
    {
        tracing::warn!(
            scratch = %dir.display(),
            error = %e,
            "failed to remove scratch directory after early failure"
        );
    }
}

/// Execute a rendered rule hook (on_success / on_failure) in the rule's
/// environment. Hooks are best-effort: their failure never changes the
/// rule's own status (pre_exec is the exception and aborts the rule).
async fn run_hook(cmd: &str, rule: &Rule, workdir: &std::path::Path, shell_prelude: Option<&str>) {
    // Hooks take the same prelude as rule commands (issue #92).
    let cmd = crate::config::prepend_shell_prelude(cmd, shell_prelude);
    let child = match spawn_rule_shell(&cmd, workdir, &rule.envvars) {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(rule = %rule.name, error = %e, "failed to spawn rule hook");
            return;
        }
    };
    let result = child.wait_with_output().await;
    match result {
        Ok(output) if !output.status.success() => {
            tracing::warn!(
                rule = %rule.name,
                code = %output.status.code().unwrap_or(-1),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "rule hook failed (best-effort)"
            );
        }
        Err(e) => {
            tracing::warn!(rule = %rule.name, error = %e, "failed to spawn rule hook");
        }
        _ => {}
    }
}

pub fn render_shell_command(
    cmd: &str,
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    limits: crate::scheduler::ResourceLimits,
) -> String {
    render_shell_command_inner(cmd, rule, wildcard_values, None, limits)
}

/// Scratch-mode rendering: `{input}` and `{log}` render as absolute paths
/// under `workdir` (they live outside the scratch directory), while
/// `{output}` keeps its declared relative form so the shell writes it into
/// the scratch working directory for later collection.
pub(crate) fn render_shell_command_in_scratch(
    cmd: &str,
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    workdir: &Path,
    limits: crate::scheduler::ResourceLimits,
) -> String {
    render_shell_command_inner(cmd, rule, wildcard_values, Some(workdir), limits)
}

fn render_shell_command_inner(
    cmd: &str,
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    abs_root: Option<&Path>,
    limits: crate::scheduler::ResourceLimits,
) -> String {
    let mut expanded = cmd.to_string();
    // `{log}` resolves to the rule's log path with the same wildcard and
    // config expansion as every other path (W004's suggestion was unwired
    // before: `2> {log}` silently created a literal "{log}" file).
    if let Some(log_path) = &rule.log {
        // Recursing on the log path itself (or a log path referencing
        // `{log}`) would loop forever — keep the literal instead.
        let expanded_log = if cmd == log_path || log_path.contains("{log}") {
            log_path.clone()
        } else {
            render_shell_command_inner(log_path, rule, wildcard_values, abs_root, limits)
        };
        let rendered_log = absolute_path(abs_root, &expanded_log);
        expanded = expanded.replace("{log}", &rendered_log);
        // `{log[0]}` for snakemake ports (log is a scalar here, so the
        // indexed form maps to the same path).
        expanded = expanded.replace("{log[0]}", &rendered_log);
    }
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
    // Inputs expand their `{config.x}` / wildcard placeholders here so the
    // absolute form can be computed; in non-scratch mode the result is
    // byte-identical to the historical raw-pattern pass.
    let all_inputs: Vec<String> = rule
        .input
        .iter()
        .map(|inp| absolute_path(abs_root, &expand_wildcards_in_pattern(inp, wildcard_values)))
        .collect();
    expanded = expanded.replace("{input}", &all_inputs.join(" "));
    for i in 0..rule.input.len() {
        if let Some(inp) = rule.input.get_index(i) {
            let rendered =
                absolute_path(abs_root, &expand_wildcards_in_pattern(inp, wildcard_values));
            expanded = expanded.replace(&format!("{{input[{i}]}}"), &rendered);
        }
    }
    if let FilePatterns::Map(ref m) = rule.input {
        for (name, inp) in m {
            let rendered =
                absolute_path(abs_root, &expand_wildcards_in_pattern(inp, wildcard_values));
            expanded = expanded.replace(&format!("{{input.{name}}}"), &rendered);
        }
    }
    expanded = expanded.replace("{threads}", &rule.effective_threads().to_string());
    if let Some(mem) = rule.effective_memory() {
        expanded = expanded.replace("{memory}", mem);
    }
    // Tool-facing effective resources: the declared request clamped to the
    // machine, so tools can size their own flags (e.g. -Xmx{effective_memory_mb}m)
    // instead of hardcoding HPC-scale values that OOM small boxes.
    let (eff_threads, eff_mem_mb) = crate::scheduler::effective_tool_resources(rule, limits);
    expanded = expanded.replace("{effective_threads}", &eff_threads.to_string());
    expanded = expanded.replace("{effective_memory_mb}", &eff_mem_mb.to_string());
    for (key, value) in &rule.params {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        expanded = expanded.replace(&format!("{{params.{key}}}"), &string_val);
    }
    expanded = super::expand_to_fixed_point(&expanded, wildcard_values, render_wildcard_value);
    expanded
}

/// Expand every `{key}` placeholder in a path pattern with the instance's
/// wildcard values, using shell-friendly rendering for array config values
/// (same semantics as the trailing wildcard pass of `render_shell_command`).
fn expand_wildcards_in_pattern(pattern: &str, wildcard_values: &HashMap<String, String>) -> String {
    super::expand_to_fixed_point(pattern, wildcard_values, render_wildcard_value)
}

/// Render a path as absolute under `root` when `root` is given and the path
/// is relative; otherwise pass it through unchanged.
fn absolute_path(root: Option<&Path>, path: &str) -> String {
    match root {
        Some(root) if !Path::new(path).is_absolute() => root.join(path).display().to_string(),
        _ => path.to_string(),
    }
}

/// Normalize a wildcard value for shell interpolation.
///
/// Callers (CLI/web) stringify TOML config arrays as `["a", "b"]` literals;
/// shells need the space-joined form (`a b`), matching the multi-value
/// semantics of `{input}`. Scalar values pass through unchanged.
fn render_wildcard_value(value: &str) -> String {
    // Cheap guard: TOML array literals always start with '['.
    if value.starts_with('[') {
        let wrapped = format!("_x = {value}");
        if let Ok(table) = toml::from_str::<toml::Table>(&wrapped)
            && let Some(toml::Value::Array(items)) = table.get("_x")
        {
            let mut joined = String::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    joined.push(' ');
                }
                match item {
                    toml::Value::String(s) => joined.push_str(s),
                    other => joined.push_str(&other.to_string()),
                }
            }
            return joined;
        }
    }
    value.to_string()
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

impl ExecutionStats {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};

    #[test]
    fn mask_sensitive_redacts_matching_values_only() {
        // Values >= 4 chars are masked; short/empty values are skipped (the
        // same threshold GitHub Actions uses, to avoid mass-redacting common
        // short strings like thresholds). Overlapping values are masked
        // independently of order.
        let values = vec!["s3cr3t-token-42".to_string(), "ab".to_string()];
        let text = "token is s3cr3t-token-42 and again s3cr3t-token-42; ab stays";
        let masked = mask_sensitive(text, &values);
        assert_eq!(
            masked, "token is *** and again ***; ab stays",
            "long values mask, short values stay"
        );
        assert!(!masked.contains("s3cr3t-token-42"));

        // Empty input and empty value list are no-ops.
        assert_eq!(mask_sensitive("plain", &[]), "plain");
        assert_eq!(mask_sensitive("plain", &["".to_string()]), "plain");
    }

    fn executor_with(max_threads: u32, max_memory_mb: u64) -> LocalExecutor {
        LocalExecutor::new(ExecutorConfig {
            max_threads: Some(max_threads),
            max_memory_mb: Some(max_memory_mb),
            ..Default::default()
        })
    }

    fn docker_rule(memory: &str) -> Rule {
        Rule {
            name: "bigmem".to_string(),
            resources: Resources {
                memory: Some(memory.to_string()),
                ..Default::default()
            },
            environment: EnvironmentSpec {
                docker: Some("img:latest".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn resolve_command_clamps_container_memory_to_system_total() {
        let ex = executor_with(4, 2048);
        // 72G (an upstream HPC label) on a 2G box — the container cgroup
        // must reflect the machine, not the declaration.
        let cmd = ex.resolve_command("echo hi", &docker_rule("72G"), None);
        assert!(cmd.contains("--memory 2048M"), "cmd: {cmd}");
    }

    #[test]
    fn resolve_command_keeps_declared_memory_when_within_system_total() {
        let ex = executor_with(4, 2048);
        let cmd = ex.resolve_command("echo hi", &docker_rule("1G"), None);
        assert!(cmd.contains("--memory 1G"), "cmd: {cmd}");
    }
}
