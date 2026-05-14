#![allow(deprecated)]
//! Cluster execution backends for HPC job submission.
//!
//! Supports SLURM, PBS/Torque, SGE, and LSF job schedulers.

use crate::environment::EnvironmentResolver;
use crate::rule::Rule;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Supported HPC cluster backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterBackend {
    /// SLURM Workload Manager.
    Slurm,
    /// PBS/Torque scheduler.
    Pbs,
    /// Sun Grid Engine (SGE).
    Sge,
    /// IBM Spectrum LSF.
    Lsf,
}

impl fmt::Display for ClusterBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slurm => write!(f, "slurm"),
            Self::Pbs => write!(f, "pbs"),
            Self::Sge => write!(f, "sge"),
            Self::Lsf => write!(f, "lsf"),
        }
    }
}

impl FromStr for ClusterBackend {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "slurm" => Ok(Self::Slurm),
            "pbs" => Ok(Self::Pbs),
            "sge" => Ok(Self::Sge),
            "lsf" => Ok(Self::Lsf),
            other => Err(format!("unknown cluster backend: {other}")),
        }
    }
}

/// Configuration for submitting a job to a cluster scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterJobConfig {
    /// Which cluster backend to target.
    pub backend: ClusterBackend,
    /// Job queue or partition name.
    pub queue: Option<String>,
    /// Billing / accounting account.
    pub account: Option<String>,
    /// Wall-time limit (e.g. "24:00:00").
    pub walltime: Option<String>,
    /// Additional scheduler-specific arguments.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Status of a job submitted to a cluster scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterJobStatus {
    /// Job is queued and waiting to start.
    Pending,
    /// Job is currently executing.
    Running,
    /// Job finished successfully.
    Completed,
    /// Job finished with an error.
    Failed,
    /// Status could not be determined.
    Unknown,
}

impl fmt::Display for ClusterJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// A job that has been submitted to a cluster scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterJob {
    /// Scheduler-assigned job identifier.
    pub job_id: String,
    /// Name of the rule this job executes.
    pub rule_name: String,
    /// Current status of the job.
    pub status: ClusterJobStatus,
    /// Time the job was submitted.
    pub submit_time: Option<DateTime<Utc>>,
}

/// Generate a scheduler submit script for the given backend.
///
/// The returned string is a complete shell script including the `#!/bin/bash`
/// shebang and all scheduler directives derived from the rule's resource
/// requirements and the cluster configuration.
pub fn generate_submit_script(
    backend: &ClusterBackend,
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> String {
    match backend {
        ClusterBackend::Slurm => generate_slurm_script(rule, shell_cmd, cluster_config),
        ClusterBackend::Pbs => generate_pbs_script(rule, shell_cmd, cluster_config),
        ClusterBackend::Sge => generate_sge_script(rule, shell_cmd, cluster_config),
        ClusterBackend::Lsf => generate_lsf_script(rule, shell_cmd, cluster_config),
    }
}

/// Generate a scheduler submit script with environment wrapping.
///
/// This is the recommended way to generate cluster scripts as it properly
/// wraps the shell command through the environment resolver (conda, docker,
/// singularity, pixi, venv) before generating the submit script.
pub fn generate_submit_script_with_env(
    backend: &ClusterBackend,
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
    env_resolver: &EnvironmentResolver,
) -> Result<String, String> {
    // Wrap command through environment resolver
    let wrapped_cmd = env_resolver
        .wrap_command(shell_cmd, &rule.environment, Some(&rule.resources))
        .map_err(|e| e.to_string())?;

    Ok(generate_submit_script(
        backend,
        rule,
        &wrapped_cmd,
        cluster_config,
    ))
}

/// Returns the shell command used to submit a job to the given backend.
pub fn submit_command(backend: &ClusterBackend) -> &'static str {
    match backend {
        ClusterBackend::Slurm => "sbatch",
        ClusterBackend::Pbs => "qsub",
        ClusterBackend::Sge => "qsub",
        ClusterBackend::Lsf => "bsub",
    }
}

/// Returns the shell command used to query job status on the given backend.
pub fn status_command(backend: &ClusterBackend) -> &'static str {
    match backend {
        ClusterBackend::Slurm => "squeue -j",
        ClusterBackend::Pbs => "qstat",
        ClusterBackend::Sge => "qstat",
        ClusterBackend::Lsf => "bjobs",
    }
}

// ---------------------------------------------------------------------------
// Private helpers for each backend
// ---------------------------------------------------------------------------

/// Convert duration string ("24h", "30m", "2d") to scheduler format ("DD-HH:MM:SS" or "HH:MM:SS")
fn format_walltime_for_scheduler(time_str: &str) -> String {
    let time_str = time_str.trim();
    // If already in scheduler format, return as-is
    if time_str.contains(':') {
        return time_str.to_string();
    }

    // Parse duration like "24h", "30m", "2d"
    let total_secs = crate::rule::parse_duration_secs(time_str).unwrap_or(3600);
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if days > 0 {
        format!("{}-{:02}:{:02}:{:02}", days, hours, mins, secs)
    } else {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }
}

fn generate_slurm_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#SBATCH --job-name={}", rule.name));
    lines.push(format!(
        "#SBATCH --cpus-per-task={}",
        rule.effective_threads()
    ));

    if let Some(mem) = rule.effective_memory() {
        lines.push(format!("#SBATCH --mem={mem}"));
    }

    // GPU handling - translate from resources.gpu or gpu_spec
    if let Some(gpu_count) = rule.resources.gpu {
        lines.push(format!("#SBATCH --gres=gpu:{}", gpu_count));
    }
    if let Some(ref spec) = rule.resources.gpu_spec {
        let gpu_str = match &spec.model {
            Some(model) => format!("gpu:{}:{}", model, spec.count),
            None => format!("gpu:{}", spec.count),
        };
        lines.push(format!("#SBATCH --gres={}", gpu_str));
    }

    // Per-rule walltime (override config walltime)
    let walltime = rule
        .resources
        .time_limit
        .as_ref()
        .or(config.walltime.as_ref());
    if let Some(wt) = walltime {
        let formatted = format_walltime_for_scheduler(wt);
        lines.push(format!("#SBATCH --time={formatted}"));
    }

    if let Some(ref queue) = config.queue {
        lines.push(format!("#SBATCH --partition={queue}"));
    }
    if let Some(ref account) = config.account {
        lines.push(format!("#SBATCH --account={account}"));
    }
    lines.push(format!("#SBATCH --output=logs/{}.out", rule.name));
    lines.push(format!("#SBATCH --error=logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#SBATCH {arg}"));
    }

    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Module loading for HPC environments
    if !rule.environment.modules.is_empty() {
        lines.push("module purge".to_string());
        for module in &rule.environment.modules {
            lines.push(format!("module load {}", module));
        }
    }

    lines.push(shell_cmd.to_string());
    lines.join("\n")
}

fn generate_pbs_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#PBS -N {}", rule.name));

    let threads = rule.effective_threads();
    let mut resource_parts = vec![format!("nodes=1:ppn={threads}")];
    if let Some(mem) = rule.effective_memory() {
        resource_parts.push(format!("mem={mem}"));
    }

    // GPU for PBS
    if let Some(gpu_count) = rule.resources.gpu {
        resource_parts.push(format!("gpu={}", gpu_count));
    }

    // Per-rule walltime (override config walltime)
    let walltime = rule
        .resources
        .time_limit
        .as_ref()
        .or(config.walltime.as_ref());
    if let Some(wt) = walltime {
        let formatted = format_walltime_for_scheduler(wt);
        resource_parts.push(format!("walltime={formatted}"));
    }
    lines.push(format!("#PBS -l {}", resource_parts.join(",")));

    if let Some(ref queue) = config.queue {
        lines.push(format!("#PBS -q {queue}"));
    }
    if let Some(ref account) = config.account {
        lines.push(format!("#PBS -A {account}"));
    }
    lines.push(format!("#PBS -o logs/{}.out", rule.name));
    lines.push(format!("#PBS -e logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#PBS {arg}"));
    }

    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Module loading for HPC environments
    if !rule.environment.modules.is_empty() {
        lines.push("module purge".to_string());
        for module in &rule.environment.modules {
            lines.push(format!("module load {}", module));
        }
    }

    lines.push(shell_cmd.to_string());
    lines.join("\n")
}

fn generate_sge_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#$ -N {}", rule.name));
    lines.push(format!("#$ -pe smp {}", rule.effective_threads()));

    let mut resource_parts = vec![];
    if let Some(mem) = rule.effective_memory() {
        resource_parts.push(format!("h_vmem={mem}"));
    }

    // GPU handling for SGE
    if let Some(gpu_count) = rule.resources.gpu {
        resource_parts.push(format!("gpu={}", gpu_count));
    }

    // Per-rule walltime (override config walltime)
    let walltime = rule
        .resources
        .time_limit
        .as_ref()
        .or(config.walltime.as_ref());
    if let Some(wt) = walltime {
        let formatted = format_walltime_for_scheduler(wt);
        resource_parts.push(format!("h_rt={formatted}"));
    }

    if !resource_parts.is_empty() {
        for part in resource_parts {
            lines.push(format!("#$ -l {}", part));
        }
    }

    if let Some(ref queue) = config.queue {
        lines.push(format!("#$ -q {queue}"));
    }
    lines.push(format!("#$ -o logs/{}.out", rule.name));
    lines.push(format!("#$ -e logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#$ {arg}"));
    }

    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Module loading for HPC environments
    if !rule.environment.modules.is_empty() {
        lines.push("module purge".to_string());
        for module in &rule.environment.modules {
            lines.push(format!("module load {}", module));
        }
    }

    lines.push(shell_cmd.to_string());
    lines.join("\n")
}

fn generate_lsf_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#BSUB -J {}", rule.name));
    lines.push(format!("#BSUB -n {}", rule.effective_threads()));

    if let Some(mem) = rule.effective_memory() {
        lines.push(format!("#BSUB -M {mem}"));
    }

    // GPU handling for LSF
    if let Some(gpu_count) = rule.resources.gpu {
        lines.push(format!("#BSUB -gpu {}", gpu_count));
    }

    // Per-rule walltime (override config walltime)
    let walltime = rule
        .resources
        .time_limit
        .as_ref()
        .or(config.walltime.as_ref());
    if let Some(wt) = walltime {
        let formatted = format_walltime_for_scheduler(wt);
        // LSF uses minutes format or HH:MM
        lines.push(format!("#BSUB -W {}", formatted));
    }

    if let Some(ref queue) = config.queue {
        lines.push(format!("#BSUB -q {queue}"));
    }
    lines.push(format!("#BSUB -o logs/{}.out", rule.name));
    lines.push(format!("#BSUB -e logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#BSUB {arg}"));
    }

    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Module loading for HPC environments
    if !rule.environment.modules.is_empty() {
        lines.push("module purge".to_string());
        for module in &rule.environment.modules {
            lines.push(format!("module load {}", module));
        }
    }

    lines.push(shell_cmd.to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};
    use std::collections::HashMap;

    fn make_rule(name: &str, threads: u32, memory: Option<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            input: vec![],
            output: vec![],
            shell: Some("echo hello".to_string()),
            script: None,
            threads: Some(threads),
            memory: memory.map(String::from),
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

    fn make_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: Some("compute".to_string()),
            account: Some("proj123".to_string()),
            walltime: Some("24:00:00".to_string()),
            extra_args: vec![],
        }
    }

    // -- Display / FromStr ---------------------------------------------------

    #[test]
    fn backend_display() {
        assert_eq!(ClusterBackend::Slurm.to_string(), "slurm");
        assert_eq!(ClusterBackend::Pbs.to_string(), "pbs");
        assert_eq!(ClusterBackend::Sge.to_string(), "sge");
        assert_eq!(ClusterBackend::Lsf.to_string(), "lsf");
    }

    #[test]
    fn backend_from_str() {
        assert_eq!(
            ClusterBackend::from_str("slurm").unwrap(),
            ClusterBackend::Slurm
        );
        assert_eq!(
            ClusterBackend::from_str("PBS").unwrap(),
            ClusterBackend::Pbs
        );
        assert_eq!(
            ClusterBackend::from_str("Sge").unwrap(),
            ClusterBackend::Sge
        );
        assert_eq!(
            ClusterBackend::from_str("LSF").unwrap(),
            ClusterBackend::Lsf
        );
        assert!(ClusterBackend::from_str("unknown").is_err());
    }

    #[test]
    fn job_status_display() {
        assert_eq!(ClusterJobStatus::Pending.to_string(), "pending");
        assert_eq!(ClusterJobStatus::Running.to_string(), "running");
        assert_eq!(ClusterJobStatus::Completed.to_string(), "completed");
        assert_eq!(ClusterJobStatus::Failed.to_string(), "failed");
        assert_eq!(ClusterJobStatus::Unknown.to_string(), "unknown");
    }

    // -- Submit / status commands -------------------------------------------

    #[test]
    fn submit_commands() {
        assert_eq!(submit_command(&ClusterBackend::Slurm), "sbatch");
        assert_eq!(submit_command(&ClusterBackend::Pbs), "qsub");
        assert_eq!(submit_command(&ClusterBackend::Sge), "qsub");
        assert_eq!(submit_command(&ClusterBackend::Lsf), "bsub");
    }

    #[test]
    fn status_commands() {
        assert_eq!(status_command(&ClusterBackend::Slurm), "squeue -j");
        assert_eq!(status_command(&ClusterBackend::Pbs), "qstat");
        assert_eq!(status_command(&ClusterBackend::Sge), "qstat");
        assert_eq!(status_command(&ClusterBackend::Lsf), "bjobs");
    }

    // -- Script generation --------------------------------------------------

    #[test]
    fn slurm_script_generation() {
        let rule = make_rule("bwa_align", 16, Some("32G"));
        let config = make_config();
        let script = generate_submit_script(
            &ClusterBackend::Slurm,
            &rule,
            "bwa mem ref.fa in.fq",
            &config,
        );

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#SBATCH --job-name=bwa_align"));
        assert!(script.contains("#SBATCH --cpus-per-task=16"));
        assert!(script.contains("#SBATCH --mem=32G"));
        assert!(script.contains("#SBATCH --time=24:00:00"));
        assert!(script.contains("#SBATCH --partition=compute"));
        assert!(script.contains("#SBATCH --account=proj123"));
        assert!(script.contains("#SBATCH --output=logs/bwa_align.out"));
        assert!(script.contains("#SBATCH --error=logs/bwa_align.err"));
        assert!(script.contains("bwa mem ref.fa in.fq"));
    }

    #[test]
    fn pbs_script_generation() {
        let rule = make_rule("fastqc", 4, Some("8G"));
        let config = ClusterJobConfig {
            backend: ClusterBackend::Pbs,
            queue: Some("batch".to_string()),
            account: Some("lab01".to_string()),
            walltime: Some("02:00:00".to_string()),
            extra_args: vec![],
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "fastqc input.fq", &config);

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#PBS -N fastqc"));
        assert!(script.contains("nodes=1:ppn=4"));
        assert!(script.contains("mem=8G"));
        assert!(script.contains("walltime=02:00:00"));
        assert!(script.contains("#PBS -q batch"));
        assert!(script.contains("#PBS -A lab01"));
        assert!(script.contains("#PBS -o logs/fastqc.out"));
        assert!(script.contains("#PBS -e logs/fastqc.err"));
        assert!(script.contains("fastqc input.fq"));
    }

    #[test]
    fn sge_script_generation() {
        let rule = make_rule("variant_call", 8, Some("16G"));
        let config = ClusterJobConfig {
            backend: ClusterBackend::Sge,
            queue: Some("all.q".to_string()),
            account: None,
            walltime: Some("12:00:00".to_string()),
            extra_args: vec![],
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "gatk HaplotypeCaller", &config);

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#$ -N variant_call"));
        assert!(script.contains("#$ -pe smp 8"));
        assert!(script.contains("#$ -l h_vmem=16G"));
        assert!(script.contains("#$ -l h_rt=12:00:00"));
        assert!(script.contains("#$ -q all.q"));
        assert!(script.contains("#$ -o logs/variant_call.out"));
        assert!(script.contains("#$ -e logs/variant_call.err"));
        assert!(script.contains("gatk HaplotypeCaller"));
    }

    #[test]
    fn lsf_script_generation() {
        let rule = make_rule("samtools_sort", 2, Some("4G"));
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: Some("short".to_string()),
            account: None,
            walltime: Some("01:00".to_string()),
            extra_args: vec![],
        };
        let script = generate_submit_script(
            &ClusterBackend::Lsf,
            &rule,
            "samtools sort in.bam -o out.bam",
            &config,
        );

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#BSUB -J samtools_sort"));
        assert!(script.contains("#BSUB -n 2"));
        assert!(script.contains("#BSUB -M 4G"));
        assert!(script.contains("#BSUB -W 01:00"));
        assert!(script.contains("#BSUB -q short"));
        assert!(script.contains("#BSUB -o logs/samtools_sort.out"));
        assert!(script.contains("#BSUB -e logs/samtools_sort.err"));
        assert!(script.contains("samtools sort in.bam -o out.bam"));
    }

    #[test]
    fn slurm_script_with_extra_args() {
        let rule = make_rule("test_rule", 1, None);
        let config = ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec!["--gres=gpu:1".to_string(), "--exclusive".to_string()],
        };
        let script = generate_submit_script(&ClusterBackend::Slurm, &rule, "echo done", &config);

        assert!(script.contains("#SBATCH --gres=gpu:1"));
        assert!(script.contains("#SBATCH --exclusive"));
    }

    #[test]
    fn cluster_job_construction() {
        let job = ClusterJob {
            job_id: "12345".to_string(),
            rule_name: "bwa_align".to_string(),
            status: ClusterJobStatus::Running,
            submit_time: Some(Utc::now()),
        };
        assert_eq!(job.job_id, "12345");
        assert_eq!(job.status, ClusterJobStatus::Running);
    }

    // -- GPU directive tests ------------------------------------------------

    #[test]
    fn slurm_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: Some(2),
                gpu_spec: None,
                ..Resources::default()
            },
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
        let config = make_config();
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "python train.py", &config);

        assert!(script.contains("#SBATCH --gres=gpu:2"));
    }

    #[test]
    fn slurm_gpu_spec_directive() {
        let rule = Rule {
            name: "gpu_v100".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("32G".to_string()),
            resources: Resources {
                gpu: None,
                gpu_spec: Some(crate::rule::GpuSpec {
                    count: 4,
                    model: Some("v100".to_string()),
                    memory_gb: None,
                    compute_capability: None,
                }),
                ..Resources::default()
            },
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
        let config = make_config();
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "python train.py", &config);

        assert!(script.contains("#SBATCH --gres=gpu:v100:4"));
    }

    #[test]
    fn pbs_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: Some(2),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Pbs,
            queue: Some("gpu".to_string()),
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "python train.py", &config);

        assert!(script.contains("gpu=2"));
    }

    #[test]
    fn sge_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: Some(2),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Sge,
            queue: Some("gpu.q".to_string()),
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "python train.py", &config);

        assert!(script.contains("#$ -l gpu=2"));
    }

    #[test]
    fn lsf_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: Some(2),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: Some("gpu".to_string()),
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "python train.py", &config);

        assert!(script.contains("#BSUB -gpu 2"));
    }

    // -- Per-rule walltime tests --------------------------------------------

    #[test]
    fn per_rule_walltime_slurm() {
        let rule = Rule {
            name: "long_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("sleep 100".to_string()),
            script: None,
            threads: Some(1),
            memory: None,
            resources: Resources {
                time_limit: Some("48h".to_string()),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: None,
            account: None,
            walltime: Some("24:00:00".to_string()),
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Slurm, &rule, "sleep 100", &config);

        // Per-rule walltime should override config walltime
        assert!(script.contains("#SBATCH --time=48:00:00"));
        assert!(!script.contains("#SBATCH --time=24:00:00"));
    }

    #[test]
    fn per_rule_walltime_pbs() {
        let rule = Rule {
            name: "long_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("sleep 100".to_string()),
            script: None,
            threads: Some(1),
            memory: None,
            resources: Resources {
                time_limit: Some("72h".to_string()),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Pbs,
            queue: None,
            account: None,
            walltime: Some("48:00:00".to_string()),
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Pbs, &rule, "sleep 100", &config);

        assert!(script.contains("walltime=72:00:00"));
    }

    #[test]
    fn per_rule_walltime_sge() {
        let rule = Rule {
            name: "medium_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("analysis.sh".to_string()),
            script: None,
            threads: Some(2),
            memory: Some("4G".to_string()),
            resources: Resources {
                time_limit: Some("6h".to_string()),
                ..Resources::default()
            },
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
        let config = ClusterJobConfig {
            backend: ClusterBackend::Sge,
            queue: None,
            account: None,
            walltime: Some("12:00:00".to_string()),
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Sge, &rule, "analysis.sh", &config);

        assert!(script.contains("#$ -l h_rt=06:00:00"));
    }

    #[test]
    fn walltime_format_conversion() {
        // Test the format_walltime_for_scheduler helper
        assert_eq!(format_walltime_for_scheduler("24h"), "24:00:00");
        assert_eq!(format_walltime_for_scheduler("30m"), "00:30:00");
        assert_eq!(format_walltime_for_scheduler("2d"), "2-00:00:00");
        assert_eq!(format_walltime_for_scheduler("48h"), "48:00:00");
        assert_eq!(format_walltime_for_scheduler("1:30:00"), "1:30:00"); // Already formatted
    }

    // -- Module loading tests ------------------------------------------------

    #[test]
    fn slurm_module_loading() {
        let rule = Rule {
            name: "gatk_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("gatk HaplotypeCaller".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("8G".to_string()),
            resources: Resources::default(),
            environment: EnvironmentSpec {
                modules: vec!["java/11".to_string(), "gatk/4.2".to_string()],
                ..EnvironmentSpec::default()
            },
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
            ..Default::default()
        };
        let config = make_config();
        let script = generate_submit_script(
            &ClusterBackend::Slurm,
            &rule,
            "gatk HaplotypeCaller",
            &config,
        );

        assert!(script.contains("module purge"));
        assert!(script.contains("module load java/11"));
        assert!(script.contains("module load gatk/4.2"));
    }

    #[test]
    fn pbs_module_loading() {
        let rule = Rule {
            name: "bwa_job".to_string(),
            input: vec![],
            output: vec![],
            shell: Some("bwa mem ref.fa reads.fq".to_string()),
            script: None,
            threads: Some(8),
            memory: Some("16G".to_string()),
            resources: Resources::default(),
            environment: EnvironmentSpec {
                modules: vec!["bwa/0.7.17".to_string()],
                ..EnvironmentSpec::default()
            },
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
            ..Default::default()
        };
        let config = ClusterJobConfig {
            backend: ClusterBackend::Pbs,
            queue: Some("batch".to_string()),
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script = generate_submit_script(
            &ClusterBackend::Pbs,
            &rule,
            "bwa mem ref.fa reads.fq",
            &config,
        );

        assert!(script.contains("module purge"));
        assert!(script.contains("module load bwa/0.7.17"));
    }

    // -- Logs directory tests ------------------------------------------------

    #[test]
    fn slurm_creates_logs_dir() {
        let rule = make_rule("test", 1, None);
        let config = make_config();
        let script = generate_submit_script(&ClusterBackend::Slurm, &rule, "echo test", &config);

        assert!(script.contains("mkdir -p logs"));
    }

    #[test]
    fn pbs_creates_logs_dir() {
        let rule = make_rule("test", 1, None);
        let config = ClusterJobConfig {
            backend: ClusterBackend::Pbs,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Pbs, &rule, "echo test", &config);

        assert!(script.contains("mkdir -p logs"));
    }

    #[test]
    fn sge_creates_logs_dir() {
        let rule = make_rule("test", 1, None);
        let config = ClusterJobConfig {
            backend: ClusterBackend::Sge,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Sge, &rule, "echo test", &config);

        assert!(script.contains("mkdir -p logs"));
    }

    #[test]
    fn lsf_creates_logs_dir() {
        let rule = make_rule("test", 1, None);
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
        };
        let script = generate_submit_script(&ClusterBackend::Lsf, &rule, "echo test", &config);

        assert!(script.contains("mkdir -p logs"));
    }
}
