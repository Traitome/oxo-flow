//! Cluster execution backends for HPC job submission.
// Test code uses deprecated `Rule::threads` / `Rule::memory` shorthand fields.
#![allow(deprecated)]
//!
//! The four HPC schedulers — SLURM, PBS/Torque, SGE, and LSF — generate
//! submit scripts here; the cloud backends (Kubernetes, AWS Batch, GCP
//! Batch) delegate to `backend/`.

use crate::error::Result;
use crate::rule::Rule;
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
    /// Kubernetes batch Jobs.
    Kubernetes,
    /// AWS Batch managed compute.
    AwsBatch,
    TencentBatch,
    /// Google Cloud Batch.
    GcpBatch,
}

impl fmt::Display for ClusterBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slurm => write!(f, "slurm"),
            Self::Pbs => write!(f, "pbs"),
            Self::Sge => write!(f, "sge"),
            Self::Lsf => write!(f, "lsf"),
            Self::Kubernetes => write!(f, "kubernetes"),
            Self::AwsBatch => write!(f, "aws-batch"),
            Self::TencentBatch => write!(f, "tencent-batch"),
            Self::GcpBatch => write!(f, "gcp-batch"),
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
            "kubernetes" | "k8s" => Ok(Self::Kubernetes),
            "aws-batch" | "awsbatch" => Ok(Self::AwsBatch),
            "tencent-batch" | "tencentbatch" => Ok(Self::TencentBatch),
            "gcp-batch" | "gcpbatch" => Ok(Self::GcpBatch),
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
    /// Optional run/submission scope folded into Kubernetes Job names so two
    /// runs of the same workflow never collide on `AlreadyExists` (issue
    /// #1158 live campaign). Other backends ignore it.
    #[serde(default)]
    pub name_scope: Option<String>,
    /// Shared-filesystem declaration (`[cluster.volume]`): the Kubernetes
    /// renderer mounts it at `mount_root` and rewrites the command's folded
    /// workdir path accordingly (multi-node clusters, issue #1387). Other
    /// backends ignore it.
    #[serde(default)]
    pub volume: Option<crate::config::ClusterVolume>,
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
) -> Result<String> {
    generate_submit_script_with_secrets(
        backend,
        rule,
        shell_cmd,
        &std::collections::BTreeMap::new(),
        cluster_config,
    )
}

/// [`generate_submit_script`] with the operator's secret-reference mapping
/// (issue #1120 / ledger RB-12).
///
/// Only GCP Batch consumes the map here: it is the one cloud backend whose
/// submission payload carries a secret-reference field
/// (`taskSpec.environment.secretVariables`). Kubernetes builds its
/// `secretKeyRef` block from the routed names themselves, and AWS Batch's
/// `SubmitJob` has no secrets field at all (the job definition owns it — see
/// [`crate::backend::aws_batch`]).
pub fn generate_submit_script_with_secrets(
    backend: &ClusterBackend,
    rule: &Rule,
    shell_cmd: &str,
    secret_refs: &std::collections::BTreeMap<String, String>,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    match backend {
        ClusterBackend::Slurm => Ok(generate_slurm_script(rule, shell_cmd, cluster_config)),
        ClusterBackend::Pbs => Ok(generate_pbs_script(rule, shell_cmd, cluster_config)),
        ClusterBackend::Sge => Ok(generate_sge_script(rule, shell_cmd, cluster_config)),
        ClusterBackend::Lsf => Ok(generate_lsf_script(rule, shell_cmd, cluster_config)),
        ClusterBackend::Kubernetes => {
            crate::backend::k8s::generate_k8s_job_manifest(rule, shell_cmd, cluster_config)
        }
        ClusterBackend::AwsBatch => {
            crate::backend::aws_batch::generate_aws_batch_job_spec(rule, shell_cmd, cluster_config)
        }
        ClusterBackend::TencentBatch => {
            crate::backend::tencent_batch::generate_tencent_batch_job_spec(
                rule,
                shell_cmd,
                cluster_config,
            )
        }
        ClusterBackend::GcpBatch => {
            crate::backend::gcp_batch::generate_gcp_batch_job_spec_with_secrets(
                rule,
                shell_cmd,
                secret_refs,
                cluster_config,
            )
        }
    }
}

/// Generate an ARRAY submit script for a group of same-rule instances
/// (issue #74 phase 3).
///
/// The script carries the backend's array-range directive and dispatches on
/// the backend's task-index variable to per-index command files the driver
/// writes next to the script — `bash '<cmd_dir>/cmd.${INDEX}.sh'`. Per-index
/// files (rather than an inline case table) keep arbitrary multi-line
/// commands (shell preludes, environment-wrapped commands) valid.
/// Extra render inputs for a cloud array body: the routed sensitive
/// references over the chunk's command files, the operator's
/// secret-reference map (issue #1120 / RB-12), and the run workdir the
/// per-index command files fold `cd '<workdir>' &&` to (Kubernetes mounts it
/// beside the command directory; other backends ignore it).
pub struct ArrayBodyExtras<'a> {
    pub sensitive_refs: &'a [String],
    pub secret_refs: &'a std::collections::BTreeMap<String, String>,
    pub workdir: Option<&'a str>,
}

pub fn generate_array_submit_script(
    backend: &ClusterBackend,
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    // Union of routed sensitive references over the chunk's command files —
    // the dispatch line in a cloud array body carries none of them itself
    // (issue #1120 / RB-12).
    sensitive_refs: &[String],
    // Run directory the per-index command files fold `cd '<workdir>' &&` to.
    // Kubernetes mounts it beside the command directory (see
    // `generate_k8s_array_job_manifest`); other backends ignore it.
    workdir: Option<&str>,
    config: &ClusterJobConfig,
) -> Result<String> {
    generate_array_submit_script_with_secrets(
        backend,
        rule,
        cmd_dir,
        count,
        &ArrayBodyExtras {
            sensitive_refs,
            secret_refs: &std::collections::BTreeMap::new(),
            workdir,
        },
        config,
    )
}

/// [`generate_array_submit_script`] with the operator's secret-reference
/// mapping (issue #1120 / ledger RB-12); see
/// [`generate_submit_script_with_secrets`] for which backend consumes it.
pub fn generate_array_submit_script_with_secrets(
    backend: &ClusterBackend,
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    extras: &ArrayBodyExtras,
    config: &ClusterJobConfig,
) -> Result<String> {
    match backend {
        ClusterBackend::Kubernetes => {
            return crate::backend::k8s::generate_k8s_array_job_manifest(
                rule,
                cmd_dir,
                count,
                extras.sensitive_refs,
                extras.workdir,
                config,
            );
        }
        ClusterBackend::AwsBatch => {
            return crate::backend::aws_batch::generate_aws_batch_array_spec(
                rule, cmd_dir, count, config,
            );
        }
        ClusterBackend::GcpBatch => {
            return crate::backend::gcp_batch::generate_gcp_batch_array_spec_with_secrets(
                rule,
                cmd_dir,
                count,
                extras.sensitive_refs,
                extras.secret_refs,
                config,
            );
        }
        ClusterBackend::TencentBatch => {
            return crate::backend::tencent_batch::generate_tencent_batch_array_spec(
                rule, cmd_dir, count, config,
            );
        }
        _ => {}
    }
    let base = generate_submit_script(backend, rule, "", config)?;
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();

    let (range_directive, task_var) = match backend {
        ClusterBackend::Slurm => ("#SBATCH --array=1-{count}", "SLURM_ARRAY_TASK_ID"),
        ClusterBackend::Pbs => ("#PBS -J 1-{count}", "PBS_ARRAY_INDEX"),
        ClusterBackend::Sge => ("#$ -t 1-{count}", "SGE_TASK_ID"),
        ClusterBackend::Lsf => ("#BSUB -J \"{name}[1-{count}]\"", "LSB_JOBINDEX"),
        _ => unreachable!(),
    };
    let range = range_directive
        .replace("{count}", &count.to_string())
        .replace("{name}", &rule.name);

    // Insert the range after the shebang; for LSF replace the base -J line
    // so the array name wins instead of duplicating the flag.
    match backend {
        ClusterBackend::Lsf => {
            if let Some(idx) = lines.iter().position(|l| l.starts_with("#BSUB -J ")) {
                lines[idx] = range;
            } else {
                lines.insert(1, range);
            }
        }
        _ => lines.insert(1, range),
    }

    // Replace the trailing (empty) command line with the dispatch body.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.push("set -e".to_string());
    lines.push("mkdir -p logs".to_string());
    lines.push(format!("INDEX=\"${task_var}\""));
    // Double quotes so ${INDEX} expands (single quotes would pass the
    // literal `${INDEX}` to sh). The run dir is engine-generated and never
    // contains shell metacharacters. Isolated like the command line so array
    // elements do not inherit the submitting shell's environment either.
    lines.push(isolate_dispatch_line(
        &format!("bash \"{cmd_dir}/cmd.${{INDEX}}.sh\""),
        rule.environment.is_container_runtime(),
    ));

    // Per-element logs (issue #142 M5): a shared output/error path would
    // interleave every element's output into one file. SLURM interpolates
    // %A (job id) + %a (array index) in the directive; LSF uses %I. PBS and
    // SGE append per-subjob suffixes themselves, so their directives stay
    // unchanged.
    match backend {
        ClusterBackend::Slurm => {
            for line in lines.iter_mut() {
                if line.starts_with("#SBATCH --output=") {
                    *line = format!("#SBATCH --output=logs/{}.%A_%a.out", rule.name);
                } else if line.starts_with("#SBATCH --error=") {
                    *line = format!("#SBATCH --error=logs/{}.%A_%a.err", rule.name);
                }
            }
        }
        ClusterBackend::Lsf => {
            for line in lines.iter_mut() {
                if line.starts_with("#BSUB -o ") {
                    *line = format!("#BSUB -o logs/{}.%I.out", rule.name);
                } else if line.starts_with("#BSUB -e ") {
                    *line = format!("#BSUB -e logs/{}.%I.err", rule.name);
                }
            }
        }
        _ => {}
    }

    Ok(lines.join("\n"))
}

/// Returns the shell command used to submit a job to the given backend.
pub fn submit_command(backend: &ClusterBackend) -> &'static str {
    match backend {
        ClusterBackend::Slurm => "sbatch",
        ClusterBackend::Pbs => "qsub",
        ClusterBackend::Sge => "qsub",
        ClusterBackend::Lsf => "bsub",
        ClusterBackend::Kubernetes => "kubectl",
        ClusterBackend::AwsBatch => "aws",
        ClusterBackend::GcpBatch => "gcloud",
        ClusterBackend::TencentBatch => "tccli",
    }
}

/// The accepted backend names, for user-facing error messages. One list so
/// the CLI's `unknown cluster backend` messages can never omit a backend
/// the parser accepts (gcp-batch was missing from all three copies).
pub const SUPPORTED_BACKENDS_HINT: &str = "slurm, pbs, sge, lsf, kubernetes (k8s), aws-batch, \
                                           gcp-batch, or tencent-batch (Tencent Cloud \
                                           BatchCompute)";

/// The manual submit command for a directory of rendered scripts, in the
/// form each backend actually accepts. The standalone `cluster submit`
/// hints print this instead of hand-rolled strings that had drifted from
/// the invocations `ClusterExecutor::submit` really issues (the cloud
/// backends take manifest files, not script arguments).
///
/// `selector` is the profile's `queue` value where the backend's CLI needs
/// one to address the submission: the GCP location (a bare `--config` path
/// plus job id still needs `--location` unless gcloud defaults carry one)
/// and the Tencent placement zone (whose derived region scopes tccli).
pub fn submit_invocation_hint(
    backend: &ClusterBackend,
    dir: &str,
    selector: Option<&str>,
) -> String {
    match backend {
        ClusterBackend::Slurm => format!("sbatch --parsable {dir}/*.sh"),
        ClusterBackend::Pbs | ClusterBackend::Sge => format!("qsub {dir}/*.sh"),
        ClusterBackend::Lsf => format!("bsub {dir}/*.sh"),
        ClusterBackend::Kubernetes => format!("kubectl create -f {dir}/*.sh"),
        ClusterBackend::AwsBatch => format!(
            "for f in {dir}/*.sh; do aws batch submit-job --cli-input-json \"file://$f\"; done"
        ),
        ClusterBackend::TencentBatch => {
            let region = selector
                .map(|zone| {
                    format!(
                        " --region '{}'",
                        crate::environment::escape_for_sh_single_quote(
                            crate::backend::tencent_batch::region_from_zone(zone)
                        )
                    )
                })
                .unwrap_or_default();
            format!(
                "for f in {dir}/*.sh; do tccli batch SubmitJob --cli-input-json \"file://$f\" \
                 --output json{region}; done"
            )
        }
        // gcloud's `--config` takes a plain local path — the API's
        // `file://`-style URIs are not a gcloud convention and every
        // submit failed with "file not found" when the hint carried one.
        // The job id is minted here the same way the executor does
        // (sanitized basename clamped to 41 chars inside GCP's 63-char id
        // cap).
        ClusterBackend::GcpBatch => {
            let location = selector
                .map(|loc| {
                    format!(
                        " --location '{}'",
                        crate::environment::escape_for_sh_single_quote(loc)
                    )
                })
                .unwrap_or_default();
            format!(
                "for f in {dir}/*.sh; do n=$(basename \"$f\" .sh | tr '[:upper:]_' '[:lower:]-' | \
                 sed 's/[^a-z0-9-]//g' | cut -c1-41); gcloud batch jobs submit \
                 \"oxoflow-$n-$(date +%s)\" --config \"$f\"{location}; done"
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers for each backend
// ---------------------------------------------------------------------------

/// The extra placement clause for LSF's `-R`: oxo-flow's `threads` are
/// shared-memory threads, so multi-threaded jobs must stay on one host
/// instead of letting LSF spread `-n` slots across machines. Empty for
/// single-threaded jobs, where spanning is meaningless. Leading space
/// included so it can be appended straight into an existing `-R` string.
fn lsf_span_clause(rule: &Rule) -> String {
    if rule.effective_threads() > 1 {
        " span[hosts=1]".to_string()
    } else {
        String::new()
    }
}

/// Parse a memory figure into KB for LSF's `-M`/`rusage[mem=]` by sharing
/// the engine-wide parser `scheduler::parse_memory_mb`: a bare number is
/// megabytes (the convention users see in `dry-run`/`run` resource
/// output), `K`/`M`/`G`/`T` suffixes are single-letter, and the MB result
/// converts ×1024 to KB. The two parsers used to disagree on bare values,
/// so the same `memory = "4096"` scheduled 4 GB locally but 4 MB on LSF
/// (#716). `None` now means exactly "local scheduling cannot parse this
/// figure either" — the call site renders the raw string verbatim.
fn lsf_mem_kb(mem: &str) -> Option<u64> {
    crate::scheduler::parse_memory_mb(mem).and_then(|mb| mb.checked_mul(1024))
}

/// Convert duration string ("24h", "30m", "2d") to scheduler format ("DD-HH:MM:SS" or "HH:MM:SS")
fn format_walltime_for_scheduler(time_str: &str) -> String {
    let time_str = time_str.trim();
    // One shared parser for every backend: suffix forms plus the colon
    // forms the config docs promise (#737).
    let total_secs = match crate::rule::parse_walltime_secs(time_str) {
        Some(secs) => secs,
        None => {
            // Unparseable walltimes travel verbatim (the #715 LSF
            // precedent): sbatch/qsub/qsub then reject the script loudly
            // instead of silently running with a 1-hour limit. The
            // default-to-1h behavior also broke `MM:SS`/`H:MM` inputs that
            // the old colon pre-check let fall through to a parser that
            // rejects all colons.
            tracing::warn!(
                "Invalid walltime '{}', passing it through verbatim — the scheduler will \
                 reject the script loudly instead of running with a hidden 1-hour limit \
                 (accepted: 90s, 30m, 24h, 2d, 01:00:00, 1-06:00:00)",
                time_str
            );
            return time_str.to_string();
        }
    };
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

/// Generate SLURM GPU directive from GpuSpec.
fn slurm_gpu_directive(spec: &crate::rule::GpuSpec) -> String {
    match &spec.model {
        Some(model) => {
            // Format: gpu:model:count (e.g., gpu:a100:2)
            // Some sites use gpu:model:count:memory_gb
            if let Some(mem_gb) = spec.memory_gb {
                format!("gpu:{}:{}:{}g", model, spec.count, mem_gb)
            } else {
                format!("gpu:{}:{}", model, spec.count)
            }
        }
        None => {
            // Generic GPU request
            format!("gpu:{}", spec.count)
        }
    }
}

/// A zero GPU count means "no GPUs" — every scheduler rejects or
/// mis-accounts a request for 0 of a consumable (SLURM: `--gres=gpu:0`
/// fails with Invalid generic resource; PBS/SGE/LSF take it literally).
fn gpu_count_requested(count: u32) -> bool {
    count > 0
}

/// Build the `env -i NAME='VALUE' …` isolation prefix for a generated
/// cluster job script (issue #1108, RB-15).
///
/// A scheduler forwards the submitting shell's environment to every job by
/// default (SLURM `--export=ALL`, PBS/SGE `-V`, LSF `-env`), which would
/// hand `ANTHROPIC_AUTH_TOKEN`, `OXO_FLOW_MASTER_KEY` and every other
/// ambient credential to a shared or imported rule. The allowlist reuses
/// the exact same policy as the local spawn path —
/// [`crate::executor::process::RULE_ENV_ALLOWLIST`], every `LC_*`, the
/// operator's `OXO_FLOW_ENV_PASSTHROUGH` names, and
/// [`crate::executor::process::CONTAINER_DAEMON_ENV_VARS`] when the rule
/// runs through a local container CLI (`rule.environment.
/// is_container_runtime()`, issue #1173: the engine invokes docker/
/// apptainer from the job's shell, and a scheduler that forwarded
/// `DOCKER_HOST`/`APPTAINER_CACHEDIR` before the isolation change must not
/// silently drop them now) — so the local and HPC paths cannot drift
/// (invariant "Reuse, never retype").
///
/// Applied inside the script body rather than as a submit flag: SLURM
/// rejects `--export=NONE` combined with an explicit `--export=<list>`, the
/// other schedulers spell the equivalent differently (`-V`/`-v`/`-env`),
/// and one body-level mechanism covers all four HPC backends with a single
/// shared implementation.
fn env_isolation_prefix(container_daemon: bool) -> String {
    env_isolation_prefix_with(container_daemon, &|name| std::env::var_os(name))
}

/// [`env_isolation_prefix`] with the ambient lookup injected, mirroring the
/// local path's `apply_rule_env_allowlist_from` split: tests pass a
/// synthetic map because this crate forbids `unsafe` (and `std::env::set_var`
/// is unsafe in edition 2024), while production always reads the process
/// environment.
fn env_isolation_prefix_with(
    container_daemon: bool,
    ambient: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> String {
    let mut names: Vec<String> = crate::executor::process::RULE_ENV_ALLOWLIST
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    // Locale family, matched by prefix exactly as the local path does, and
    // sorted so the generated script is deterministic per host (invariant
    // "Persisted artifacts are order-independent"); the base allowlist and
    // the passthrough names keep their declaration/ambient order.
    let mut locale: Vec<String> = std::env::vars_os()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .filter(|key| key.starts_with("LC_"))
        .collect();
    locale.sort();
    locale.dedup();
    names.extend(locale);
    names.extend(crate::executor::process::env_passthrough_names());
    if container_daemon {
        names.extend(
            crate::executor::process::CONTAINER_DAEMON_ENV_VARS
                .iter()
                .map(|name| (*name).to_string()),
        );
    }

    let mut prefix = String::from("env -i");
    for name in names {
        if let Some(value) = ambient(&name) {
            let value = value.to_string_lossy();
            prefix.push_str(&format!(
                " {name}='{}'",
                crate::environment::escape_for_sh_single_quote(&value)
            ));
        }
    }
    prefix.push(' ');
    prefix
}

/// Append the isolated command line for a generated job script: the
/// [`env_isolation_prefix`] followed by the shell command run through
/// `bash -c` — bash-first, matching the local spawn path
/// (`spawn_rule_shell`, which falls back to `sh` only when the *submit*
/// host lacks bash) — or nothing when there is no command of its own (an
/// array base script leaves the command slot empty; its per-index files
/// carry the commands). `container_daemon` is
/// `rule.environment.is_container_runtime()`, exactly as the local spawn
/// path computes it.
///
/// bash, not sh: before the isolation wrapper the command ran as a bare
/// line under the script's `#!/bin/bash`, and upstream workflows rely on
/// bash features (process substitution `<(…)`, brace expansion, `[[ ]]`)
/// that POSIX sh — dash on Debian/Ubuntu compute nodes — rejects. The
/// script header already requires bash, so no runtime fallback is needed
/// here: a compute node without bash cannot run the script at all.
///
/// The command travels as one `bash -c` argument rather than as a bare
/// prefix because the prefix must wrap the whole command: `env -i … cd dir
/// && cmd` changes the working directory only inside the ephemeral `env`
/// child, so a bare prefix silently runs `cmd` in the submit directory.
/// Passing the command as a single quoted argument keeps the `cd` guard
/// (and any shell prelude) in effect inside the isolated shell.
///
/// Declared per-rule `envvars` are re-exported by the command itself
/// (`export K='V' && …`), so they survive the outer `env -i`.
fn push_isolated_command(lines: &mut Vec<String>, shell_cmd: &str, container_daemon: bool) {
    if shell_cmd.trim().is_empty() {
        return;
    }
    lines.push(format!(
        "{}bash -c '{}'",
        env_isolation_prefix(container_daemon),
        crate::environment::escape_for_sh_single_quote(shell_cmd)
    ));
}

/// Wrap a dispatch line (`bash "<cmd_dir>/cmd.${INDEX}.sh"`) in the same
/// allowlist isolation, so array elements — whose real commands live in the
/// per-index files — are isolated too. bash for the same reason as
/// [`push_isolated_command`]: the per-index commands used to run under the
/// script's bash and may use bashisms.
fn isolate_dispatch_line(line: &str, container_daemon: bool) -> String {
    format!("{}{line}", env_isolation_prefix(container_daemon))
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
    if let Some(ref spec) = rule.resources.gpu_spec {
        // Use detailed GPU spec (preferred)
        if gpu_count_requested(spec.count) {
            lines.push(format!("#SBATCH --gres={}", slurm_gpu_directive(spec)));
        }
        // Add GPU memory constraint if specified
        if let Some(mem_gb) = spec.memory_gb {
            // Some SLURM configs support --mem-per-gpu
            lines.push(format!("#SBATCH --mem-per-gpu={}G", mem_gb));
        }
    } else if let Some(gpu_count) = rule.resources.gpu {
        // Simple GPU count
        if gpu_count_requested(gpu_count) {
            lines.push(format!("#SBATCH --gres=gpu:{}", gpu_count));
        }
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

    // Partition: rule-level overrides CLI flag
    let partition = rule
        .resources
        .partition
        .as_deref()
        .or(config.queue.as_deref());
    if let Some(part) = partition {
        lines.push(format!("#SBATCH --partition={part}"));
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

    // Error handling: exit on error
    lines.push("set -e".to_string());
    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Note: Module loading is handled by EnvironmentResolver.wrap_command()
    // to avoid duplication with conda/docker/singularity environments
    push_isolated_command(
        &mut lines,
        shell_cmd,
        rule.environment.is_container_runtime(),
    );
    lines.join("\n")
}

/// Generate PBS GPU directive (site-specific, varies widely).
/// Most PBS sites use a resource string like "gpu=count" or "ngpus=count".
fn pbs_gpu_directive(spec: &crate::rule::GpuSpec) -> Vec<String> {
    let mut directives = Vec::new();

    // Standard PBS GPU resource (varies by site configuration)
    directives.push(format!("gpu={}", spec.count));

    // Some PBS sites support model selection via separate flag
    if let Some(ref model) = spec.model {
        // This is site-specific; document that users may need --extra_args
        directives.push(format!(
            "# Model requested: {} (use extra_args for site-specific syntax)",
            model
        ));
    }

    directives
}

/// Normalize a rule's memory figure for a PBS/SGE directive (issue #740,
/// the unfixed half of #716): `parse_memory_mb` is the shared grammar, but
/// PBS `mem=` and SGE `h_vmem=` carry their own default units (words /
/// bytes), so the figure travels with an explicit unit instead of
/// verbatim. `unit` is the suffix the backend documents (`mb` for PBS,
/// `M` for SGE). Falls back to the raw string when unparsable — validate
/// rejects those earlier anyway.
fn scheduler_mem_spec(mem: &str, unit: &str) -> String {
    match crate::scheduler::parse_memory_mb(mem) {
        Some(mb) => format!("{mb}{unit}"),
        None => mem.to_string(),
    }
}

fn generate_pbs_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#PBS -N {}", rule.name));

    let threads = rule.effective_threads();
    let mut resource_parts = vec![format!("nodes=1:ppn={threads}")];
    if let Some(mem) = rule.effective_memory() {
        resource_parts.push(format!("mem={}", scheduler_mem_spec(mem, "mb")));
    }

    // GPU for PBS (site-specific format)
    if let Some(ref spec) = rule.resources.gpu_spec {
        if gpu_count_requested(spec.count) {
            for directive in pbs_gpu_directive(spec) {
                if directive.starts_with('#') {
                    // Comments go directly into the script
                    lines.push(directive);
                } else {
                    resource_parts.push(directive);
                }
            }
        }
    } else if let Some(gpu_count) = rule.resources.gpu
        && gpu_count_requested(gpu_count)
    {
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

    // Queue: rule-level partition overrides CLI flag
    let queue = rule
        .resources
        .partition
        .as_deref()
        .or(config.queue.as_deref());
    if let Some(q) = queue {
        lines.push(format!("#PBS -q {q}"));
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

    // Error handling: exit on error
    lines.push("set -e".to_string());
    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Note: Module loading is handled by EnvironmentResolver.wrap_command()
    push_isolated_command(
        &mut lines,
        shell_cmd,
        rule.environment.is_container_runtime(),
    );
    lines.join("\n")
}

fn generate_sge_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#$ -N {}", rule.name));
    lines.push(format!("#$ -pe smp {}", rule.effective_threads()));

    let mut resource_parts = vec![];
    if let Some(mem) = rule.effective_memory() {
        resource_parts.push(format!("h_vmem={}", scheduler_mem_spec(mem, "M")));
    }

    // GPU handling for SGE (site-specific)
    if let Some(ref spec) = rule.resources.gpu_spec {
        if gpu_count_requested(spec.count) {
            resource_parts.push(format!("gpu={}", spec.count));
        }
        // Model selection requires site-specific complex resource definition
    } else if let Some(gpu_count) = rule.resources.gpu
        && gpu_count_requested(gpu_count)
    {
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

    // Queue: rule-level partition overrides CLI flag
    let queue = rule
        .resources
        .partition
        .as_deref()
        .or(config.queue.as_deref());
    if let Some(q) = queue {
        lines.push(format!("#$ -q {q}"));
    }
    lines.push(format!("#$ -o logs/{}.out", rule.name));
    lines.push(format!("#$ -e logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#$ {arg}"));
    }

    lines.push(String::new());

    // Error handling: exit on error
    lines.push("set -e".to_string());
    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Note: Module loading is handled by EnvironmentResolver.wrap_command()
    push_isolated_command(
        &mut lines,
        shell_cmd,
        rule.environment.is_container_runtime(),
    );
    lines.join("\n")
}

fn generate_lsf_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#BSUB -J {}", rule.name));
    lines.push(format!("#BSUB -n {}", rule.effective_threads()));

    let span = lsf_span_clause(rule);
    if let Some(mem) = rule.effective_memory() {
        // -M's documented unit set is KB/MB/GB with KB as the default; the
        // bare "4G" form oxo-flow stores internally is not a valid mem_spec,
        // so the memory travels as an explicit KB figure plus the matching
        // rusage (which is what places the job on a host with room). LSF
        // reads one -R string, so the span clause rides along in it.
        match lsf_mem_kb(mem) {
            Some(kb) => {
                lines.push(format!("#BSUB -R 'rusage[mem={kb}]{span}'"));
                lines.push(format!("#BSUB -M {kb}"));
            }
            None => {
                lines.push(format!("#BSUB -M {mem}"));
                if !span.is_empty() {
                    lines.push(format!("#BSUB -R '{}'", span.trim_start()));
                }
            }
        }
    } else if !span.is_empty() {
        lines.push(format!("#BSUB -R '{}'", span.trim_start()));
    }

    // Account maps to LSF's project (-P), matching -A on PBS and --account
    // on SLURM.
    if let Some(account) = &config.account {
        lines.push(format!("#BSUB -P {account}"));
    }

    // GPU handling for LSF: -gpu expects a quoted run-options string
    // (`num=<count>[:mode=…]`), not a bare count.
    if let Some(gpu_count) = rule.resources.gpu
        && gpu_count_requested(gpu_count)
    {
        lines.push(format!("#BSUB -gpu \"num={gpu_count}\""));
    }

    // Per-rule walltime (override config walltime)
    let walltime = rule
        .resources
        .time_limit
        .as_ref()
        .or(config.walltime.as_ref());
    if let Some(wt) = walltime {
        // LSF uses [HH:]MM format
        match crate::rule::parse_walltime_secs(wt) {
            Some(total_secs) => {
                let total_mins = total_secs.div_ceil(60); // Round up to nearest minute
                let hours = total_mins / 60;
                let mins = total_mins % 60;

                if hours > 0 {
                    lines.push(format!("#BSUB -W {:02}:{:02}", hours, mins));
                } else {
                    lines.push(format!("#BSUB -W {}", mins));
                }
            }
            None => {
                // A present-but-unparseable walltime used to silently become
                // a 1-hour limit, killing long jobs with no hint that the
                // rule's walltime was never honored (#715). Render it
                // verbatim like the memory fallback above: bsub then rejects
                // the script loudly instead of running with the wrong limit.
                tracing::warn!(
                    rule = %rule.name,
                    walltime = %wt,
                    "unparseable LSF walltime; rendering it verbatim in #BSUB -W \
                     (accepted formats: 90s, 30m, 24h, 2d)"
                );
                lines.push(format!("#BSUB -W {wt}"));
            }
        }
    }

    // Queue: rule-level partition overrides CLI flag
    let queue = rule
        .resources
        .partition
        .as_deref()
        .or(config.queue.as_deref());
    if let Some(q) = queue {
        lines.push(format!("#BSUB -q {q}"));
    }
    lines.push(format!("#BSUB -o logs/{}.out", rule.name));
    lines.push(format!("#BSUB -e logs/{}.err", rule.name));

    for arg in &config.extra_args {
        lines.push(format!("#BSUB {arg}"));
    }

    lines.push(String::new());

    // Error handling: exit on error
    lines.push("set -e".to_string());
    lines.push(String::new());

    // Create logs directory before execution
    lines.push("mkdir -p logs".to_string());

    // Note: Module loading is handled by EnvironmentResolver.wrap_command()
    push_isolated_command(
        &mut lines,
        shell_cmd,
        rule.environment.is_container_runtime(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{EnvironmentSpec, Resources};
    use std::collections::HashMap;

    #[test]
    fn array_script_carries_directive_and_dispatch() {
        // Issue #74 phase 3: the array variant keeps the base directives,
        // adds the backend's array range, and dispatches on the backend's
        // task-index variable to per-index command files (multi-line-safe).
        let rule = make_rule("align", 4, Some("8G"));
        let config = ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: Some("compute".to_string()),
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script = generate_array_submit_script(
            &ClusterBackend::Slurm,
            &rule,
            "/run/jobs/align",
            3,
            &[],
            None,
            &config,
        )
        .unwrap();
        assert!(script.contains("#SBATCH --array=1-3"), "{script}");
        assert!(
            script.contains("--cpus-per-task=4"),
            "directives must persist"
        );
        assert!(script.contains("SLURM_ARRAY_TASK_ID"), "{script}");
        assert!(script.contains("/run/jobs/align/cmd."), "{script}");
        // Per-element logs (issue #142 M5): shared output paths would
        // interleave every element's output into one file.
        assert!(
            script.contains("#SBATCH --output=logs/align.%A_%a.out"),
            "{script}"
        );
        assert!(
            script.contains("#SBATCH --error=logs/align.%A_%a.err"),
            "{script}"
        );

        // LSF interpolates %I for per-element logs (issue #142 M5).
        let lsf = generate_array_submit_script(
            &ClusterBackend::Lsf,
            &rule,
            "/run/jobs/align",
            3,
            &[],
            None,
            &config,
        )
        .unwrap();
        assert!(lsf.contains("#BSUB -o logs/align.%I.out"), "{lsf}");
        assert!(lsf.contains("#BSUB -e logs/align.%I.err"), "{lsf}");

        // Other backends use their own range syntax + task variable.
        for (backend, range, var) in [
            (ClusterBackend::Pbs, "#PBS -J 1-3", "PBS_ARRAY_INDEX"),
            (ClusterBackend::Sge, "#$ -t 1-3", "SGE_TASK_ID"),
            (ClusterBackend::Lsf, "#BSUB -J", "LSB_JOBINDEX"),
        ] {
            let s2 = generate_array_submit_script(
                &backend,
                &rule,
                "/run/jobs/align",
                3,
                &[],
                None,
                &config,
            )
            .unwrap();
            assert!(s2.contains(range), "{backend:?}: {s2}");
            assert!(s2.contains(var), "{backend:?}: {s2}");
        }
    }

    /// The admitted-variable names carried by the `env -i` prefix of a
    /// generated line: `env -i NAME='VALUE' NAME='VALUE' <cmd>`.
    fn admitted_names(line: &str) -> Vec<String> {
        let rest = line
            .strip_prefix("env -i ")
            .expect("line must open with the env -i isolation prefix");
        let mut names = Vec::new();
        for token in rest.split(' ') {
            match token.split_once('=') {
                Some((name, _)) => names.push(name.to_string()),
                None => break, // first non-assignment token is the command
            }
        }
        names
    }

    /// Issue #1108 (RB-15): a scheduler forwards the submitting shell's
    /// environment to every job by default, so the generated script must
    /// isolate the command behind the SAME allowlist the local spawn path
    /// enforces — an ambient credential on the submit host must not reach a
    /// rule running on the node. Asserted on the generated text only (no
    /// live scheduler needed).
    #[test]
    fn generated_scripts_isolate_the_ambient_environment() {
        let rule = make_rule("align", 4, Some("8G"));
        let config = make_config();

        // Secret-shaped ambient names must never be re-admitted. They are
        // matched as whole tokens (boundary-safe) so `PATH` does not match a
        // longer name and `LC_ALL` is still allowed.
        const SECRETS: &[&str] = &[
            "ANTHROPIC_AUTH_TOKEN",
            "OXO_FLOW_MASTER_KEY",
            "DEEPSEEK_API_KEY",
            "OPENAI_API_KEY",
            "OXO_FLOW_AI_API_KEY",
        ];
        // The policy bound, reused from the local path (not retyped): the
        // ambient allowlist constant plus every `LC_*` plus the operator's
        // `OXO_FLOW_ENV_PASSTHROUGH` names.
        let allowlisted = |name: &str| {
            crate::executor::process::RULE_ENV_ALLOWLIST.contains(&name)
                || name.starts_with("LC_")
                || crate::executor::process::env_passthrough_names()
                    .iter()
                    .any(|n| n == name)
        };

        for backend in [
            ClusterBackend::Slurm,
            ClusterBackend::Pbs,
            ClusterBackend::Sge,
            ClusterBackend::Lsf,
        ] {
            let script =
                generate_submit_script(&backend, &rule, "bwa mem ref.fa in.fq", &config).unwrap();

            // The command runs behind `env -i`, not bare.
            let cmd_line = script
                .lines()
                .find(|l| l.trim_start().starts_with("env -i "))
                .unwrap_or_else(|| {
                    panic!("{backend:?}: no env -i isolated command line: {script}")
                });
            assert!(
                cmd_line.ends_with("bash -c 'bwa mem ref.fa in.fq'"),
                "{backend:?}: command must be wrapped in a single bash -c argument after the prefix: {cmd_line}"
            );

            // Every name the prefix admits is proof-carrying: it comes from
            // the shared allowlist — nothing else can pass through, whatever
            // the submit host happens to export.
            for name in admitted_names(cmd_line) {
                assert!(
                    allowlisted(&name),
                    "{backend:?}: non-allowlisted variable {name:?} admitted: {cmd_line}"
                );
                assert!(
                    !SECRETS.contains(&name.as_str()),
                    "{backend:?}: secret {name:?} leaked into the job environment"
                );
            }
            for secret in SECRETS {
                assert!(
                    !cmd_line.contains(secret),
                    "{backend:?}: {secret} present in generated script"
                );
            }
        }
    }

    /// The array dispatch line is isolated too: elements run their real
    /// commands from per-index files, so without the prefix they would
    /// inherit the submit host's environment via the scheduler default.
    #[test]
    fn array_dispatch_line_is_isolated() {
        for backend in [
            ClusterBackend::Slurm,
            ClusterBackend::Pbs,
            ClusterBackend::Sge,
            ClusterBackend::Lsf,
        ] {
            let script = generate_array_submit_script(
                &backend,
                &make_rule("align", 4, Some("8G")),
                "/run/jobs/align",
                3,
                &[],
                None,
                &make_config(),
            )
            .unwrap();
            let dispatch = script
                .lines()
                .find(|l| l.contains("/run/jobs/align/cmd."))
                .unwrap_or_else(|| panic!("{backend:?}: no dispatch line: {script}"));
            assert!(
                dispatch.trim_start().starts_with("env -i "),
                "{backend:?}: dispatch line not isolated: {dispatch}"
            );
            assert!(
                dispatch.ends_with("bash \"/run/jobs/align/cmd.${INDEX}.sh\""),
                "{backend:?}: dispatch line body altered: {dispatch}"
            );
        }
    }

    /// A docker/singularity rule's job script re-admits the exact
    /// container-daemon names (#1173), the same way the local spawn path
    /// does — the scheduler used to forward `DOCKER_HOST` etc. via
    /// `--export=ALL`, and the isolation change must not silently drop them
    /// for remote-daemon setups. A bare rule's prefix never widens, and a
    /// secret that is not on either list stays out even when ambient.
    #[test]
    fn container_rules_admit_daemon_env_like_the_local_path() {
        let ambient: std::collections::HashMap<&str, &str> = [
            ("DOCKER_HOST", "tcp://10.0.0.5:2375"),
            ("ANTHROPIC_AUTH_TOKEN", "leaked-secret"),
        ]
        .into_iter()
        .collect();
        let lookup = |name: &str| {
            ambient
                .get(name)
                .map(|value| std::ffi::OsString::from(*value))
        };
        let mut rule = make_rule("align", 4, Some("8G"));
        rule.environment = EnvironmentSpec::default();
        // A docker environment (the shared `is_container_runtime` predicate
        // decides, exactly as the local spawn path computes it).
        rule.environment.docker = Some("python:3.12".into());
        assert!(rule.environment.is_container_runtime());

        let prefix = env_isolation_prefix_with(rule.environment.is_container_runtime(), &lookup);
        assert!(
            prefix.contains("DOCKER_HOST='tcp://10.0.0.5:2375'"),
            "docker rules must re-admit the daemon endpoint like the local path: {prefix}"
        );
        assert!(
            !prefix.contains("leaked-secret") && !prefix.contains("ANTHROPIC_AUTH_TOKEN"),
            "secrets are never admitted by the daemon branch: {prefix}"
        );

        // The bare path is unchanged: no daemon names even when ambient.
        rule.environment = EnvironmentSpec::default();
        assert!(!rule.environment.is_container_runtime());
        let prefix = env_isolation_prefix_with(rule.environment.is_container_runtime(), &lookup);
        assert!(
            !prefix.contains("DOCKER_HOST"),
            "a bare rule's allowlist must never widen: {prefix}"
        );
        assert!(
            !prefix.contains("leaked-secret"),
            "secrets stay out of the bare prefix: {prefix}"
        );
    }

    /// The isolated command must run under bash, not POSIX sh: before the
    /// RB-15 wrapper the command was a bare line under the script's
    /// `#!/bin/bash`, so workflows using bashisms (`[[ ]]`, brace
    /// expansion, `<(...)`) executed fine on the cluster. Wrapping in
    /// `sh -c` (dash on Debian/Ubuntu) would silently break them.
    #[test]
    fn isolated_commands_run_under_bash_not_sh() {
        for backend in [
            ClusterBackend::Slurm,
            ClusterBackend::Pbs,
            ClusterBackend::Sge,
            ClusterBackend::Lsf,
        ] {
            let script = generate_submit_script(
                &backend,
                &make_rule("align", 4, Some("8G")),
                "if [[ -d ref.fa ]]; then echo dir; fi",
                &make_config(),
            )
            .unwrap();
            let cmd_line = script
                .lines()
                .find(|l| l.contains("[[ -d ref.fa ]]"))
                .unwrap_or_else(|| panic!("{backend:?}: no isolated command line: {script}"));
            assert!(
                cmd_line.contains("bash -c '"),
                "{backend:?}: bashism must run under bash -c, not sh -c: {cmd_line}"
            );
            assert!(
                script.starts_with("#!/bin/bash"),
                "{backend:?}: script must keep the bash shebang the wrapper assumes"
            );
        }
    }

    fn make_rule(name: &str, threads: u32, memory: Option<&str>) -> Rule {
        Rule {
            name: name.to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
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

    // -- Submit / status commands -------------------------------------------

    #[test]
    fn submit_commands() {
        assert_eq!(submit_command(&ClusterBackend::Slurm), "sbatch");
        assert_eq!(submit_command(&ClusterBackend::Pbs), "qsub");
        assert_eq!(submit_command(&ClusterBackend::Sge), "qsub");
        assert_eq!(submit_command(&ClusterBackend::Lsf), "bsub");
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
        )
        .unwrap();

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#SBATCH --job-name=bwa_align"));
        assert!(script.contains("#SBATCH --cpus-per-task=16"));
        assert!(script.contains("#SBATCH --mem=32G"));
        // "24:00:00" parses to 24 h, rendered in SLURM's canonical
        // D-HH:MM:SS form (same limit, canonical shape — #737).
        assert!(script.contains("#SBATCH --time=1-00:00:00"));
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "fastqc input.fq", &config)
                .unwrap();

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#PBS -N fastqc"));
        assert!(script.contains("nodes=1:ppn=4"));
        // Explicit unit: PBS's mem= default unit is words, so the figure
        // travels as MB instead of the raw config string (#740).
        assert!(script.contains("mem=8192mb"));
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "gatk HaplotypeCaller", &config)
                .unwrap();

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#$ -N variant_call"));
        assert!(script.contains("#$ -pe smp 8"));
        // Explicit unit: SGE's h_vmem default is bytes, so the figure
        // travels as megabytes instead of the raw config string (#740).
        assert!(script.contains("#$ -l h_vmem=16384M"));
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
            account: Some("proj01".to_string()),
            walltime: Some("01:00".to_string()),
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script = generate_submit_script(
            &ClusterBackend::Lsf,
            &rule,
            "samtools sort in.bam -o out.bam",
            &config,
        )
        .unwrap();

        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("#BSUB -J samtools_sort"));
        assert!(script.contains("#BSUB -n 2"));
        // threads=2 is shared-memory SMP: the -R string must keep the job on
        // one host (span[hosts=1]) next to the memory reservation.
        assert!(
            script.contains("#BSUB -R 'rusage[mem=4194304] span[hosts=1]'"),
            "{script}"
        );
        // Account maps to LSF's project (-P), like -A on PBS and --account
        // on SLURM.
        assert!(script.contains("#BSUB -P proj01"), "{script}");
        // LSF -M takes a KB figure (documented units: KB/MB/GB, default KB);
        // a bare "4G" is not a valid mem_spec. Scheduling placement needs the
        // matching rusage (plus the single-host span), so both go out.
        assert!(
            script.contains("#BSUB -R 'rusage[mem=4194304] span[hosts=1]'"),
            "{script}"
        );
        assert!(script.contains("#BSUB -M 4194304"), "{script}");
        assert!(script.contains("#BSUB -W 01:00"));
        assert!(script.contains("#BSUB -q short"));
        assert!(script.contains("#BSUB -o logs/samtools_sort.out"));
        assert!(script.contains("#BSUB -e logs/samtools_sort.err"));
        assert!(script.contains("samtools sort in.bam -o out.bam"));
    }

    #[test]
    fn lsf_memory_falls_back_verbatim_when_unparseable() {
        let rule = make_rule("odd_job", 1, Some("lots"));
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "echo hi", &config).unwrap();
        assert!(script.contains("#BSUB -M lots"), "{script}");
        assert!(!script.contains("rusage"), "{script}");
    }

    #[test]
    fn lsf_walltime_renders_parsed_limit() {
        // 90m = 5400 s → rounds up to [HH:]MM as 01:30.
        let mut rule = make_rule("variant_call", 8, Some("16G"));
        rule.resources.time_limit = Some("90m".to_string());
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "gatk HaplotypeCaller", &config)
                .unwrap();
        assert!(script.contains("#BSUB -W 01:30"), "{script}");
    }

    #[test]
    fn lsf_walltime_unparseable_renders_verbatim_not_silent_default() {
        // A present-but-unparseable walltime must not silently become a
        // 1-hour limit (#715): it travels verbatim so bsub rejects the
        // script loudly instead of killing a long job at 1 h.
        let mut rule = make_rule("assembly", 16, Some("64G"));
        rule.resources.time_limit = Some("24 hours".to_string());
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "spades.py --isolate", &config)
                .unwrap();
        assert!(script.contains("#BSUB -W 24 hours"), "{script}");
        assert!(!script.contains("#BSUB -W 01:00"), "{script}");
        assert!(!script.contains("#BSUB -W 1:00"), "{script}");
    }

    #[test]
    fn slurm_walltime_unparseable_renders_verbatim_not_silent_default() {
        // #737: #715 fixed LSF only — SLURM/PBS/SGE still substituted a
        // silent 1-hour limit. Verbatim passthrough makes sbatch reject
        // the script loudly instead.
        let mut rule = make_rule("assembly", 16, Some("64G"));
        rule.resources.time_limit = Some("24 hours".to_string());
        let config = ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "spades.py", &config).unwrap();
        assert!(script.contains("#SBATCH --time=24 hours"), "{script}");
        assert!(!script.contains("#SBATCH --time=01:00:00"), "{script}");
    }

    #[test]
    fn lsf_walltime_colon_form_parses_like_other_backends() {
        // The documented HH:MM:SS form used to reach LSF verbatim, where
        // `-W 24:00:00` is invalid — a portability break vs the other
        // backends (#737). The shared parser renders it as [HH:]MM.
        let mut rule = make_rule("variant_call", 8, Some("16G"));
        rule.resources.time_limit = Some("24:00:00".to_string());
        let config = ClusterJobConfig {
            backend: ClusterBackend::Lsf,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "gatk HaplotypeCaller", &config)
                .unwrap();
        assert!(script.contains("#BSUB -W 24:00"), "{script}");
    }

    #[test]
    fn lsf_mem_kb_parses_every_documented_unit_form() {
        // lsf_mem_kb shares scheduler::parse_memory_mb (MB convention): a
        // bare figure is MB, every accepted form converts ×1024 to KB
        // (#716).
        assert_eq!(super::lsf_mem_kb("4096"), Some(4096 * 1024));
        assert_eq!(super::lsf_mem_kb("4G"), Some(4 * 1024 * 1024));
        assert_eq!(super::lsf_mem_kb("4g"), Some(4 * 1024 * 1024));
        assert_eq!(super::lsf_mem_kb("1500M"), Some(1500 * 1024));
        assert_eq!(super::lsf_mem_kb("1T"), Some(1024 * 1024 * 1024));
        // Unparsable figures — including multi-char suffixes and sub-MB K
        // values, which local scheduling also rejects — return None so the
        // script renders them verbatim instead of guessing a unit.
        assert_eq!(super::lsf_mem_kb("lots"), None);
        assert_eq!(super::lsf_mem_kb(""), None);
        assert_eq!(super::lsf_mem_kb("512kb"), None);
    }

    #[test]
    fn lsf_mem_kb_agrees_with_engine_memory_convention() {
        // Whatever parse_memory_mb reports in MB, LSF must receive in KB —
        // the same config must not schedule 4 GB locally and 4 MB on the
        // cluster (#716).
        for mem in ["4096", "8192", "4096M", "4G", "4g", "1T", "1t"] {
            let mb = crate::scheduler::parse_memory_mb(mem)
                .unwrap_or_else(|| panic!("parse_memory_mb({mem}) returned None"));
            assert_eq!(
                super::lsf_mem_kb(mem),
                Some(mb * 1024),
                "lsf_mem_kb({mem}) diverges from parse_memory_mb × 1024"
            );
        }
        // Grammar parity: a figure the engine cannot parse locally is
        // unparsable on LSF too.
        for mem in ["lots", "", "4096mb", "2GB", "512k"] {
            assert_eq!(crate::scheduler::parse_memory_mb(mem), None, "{mem}");
            assert_eq!(super::lsf_mem_kb(mem), None, "{mem}");
        }
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "echo done", &config).unwrap();

        assert!(script.contains("#SBATCH --gres=gpu:1"));
        assert!(script.contains("#SBATCH --exclusive"));
    }

    // -- GPU directive tests ------------------------------------------------

    #[test]
    fn slurm_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            generate_submit_script(&ClusterBackend::Slurm, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("#SBATCH --gres=gpu:2"));
    }

    #[test]
    fn slurm_gpu_spec_directive() {
        let rule = Rule {
            name: "gpu_v100".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            generate_submit_script(&ClusterBackend::Slurm, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("#SBATCH --gres=gpu:v100:4"));
    }

    #[test]
    fn pbs_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("gpu=2"));
    }

    #[test]
    fn sge_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("#$ -l gpu=2"));
    }

    #[test]
    fn lsf_gpu_directive() {
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "python train.py", &config)
                .unwrap();

        // -gpu takes a quoted run-options string (num=<n>), not a bare count.
        assert!(script.contains("#BSUB -gpu \"num=2\""), "{script}");
    }

    #[test]
    fn zero_gpu_requests_omit_the_directive_on_every_backend() {
        // gpu = 0 means "no GPUs" — emitting `--gres=gpu:0` / `gpu=0`
        // makes schedulers reject or mis-account the job (SLURM: Invalid
        // generic resource). The directive must simply disappear.
        let cases = [
            (ClusterBackend::Slurm, "#SBATCH --gres=gpu:0"),
            (ClusterBackend::Pbs, "gpu=0"),
            (ClusterBackend::Sge, "gpu=0"),
            (ClusterBackend::Lsf, "#BSUB -gpu 0"),
        ];
        for (backend, forbidden) in cases {
            let rule = Rule {
                name: "no_gpu".to_string(),
                input: vec![].into(),
                output: vec![].into(),
                shell: Some("echo hello".to_string()),
                script: None,
                threads: Some(1),
                memory: None,
                resources: Resources {
                    gpu: Some(0),
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
                backend,
                queue: None,
                account: None,
                walltime: None,
                extra_args: vec![],
                name_scope: None,
                volume: None,
            };
            let script = generate_submit_script(&backend, &rule, "echo hello", &config).unwrap();
            assert!(
                !script.contains(forbidden),
                "{backend:?} script must not contain '{forbidden}':\n{script}"
            );
        }
    }

    #[test]
    fn zero_count_gpu_spec_omits_the_directive_but_keeps_memory() {
        // A spec with count = 0 and a memory floor still constrains GPU
        // memory (mem-per-gpu applies to any GPU the site assigns).
        let rule = Rule {
            name: "mem_only".to_string(),
            input: vec![].into(),
            output: vec![].into(),
            shell: Some("echo hello".to_string()),
            script: None,
            threads: Some(1),
            memory: None,
            resources: Resources {
                gpu_spec: Some(crate::rule::GpuSpec {
                    count: 0,
                    model: Some("a100".to_string()),
                    memory_gb: Some(40),
                    ..Default::default()
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
            generate_submit_script(&ClusterBackend::Slurm, &rule, "echo hello", &config).unwrap();
        assert!(!script.contains("gres=gpu:0"), "\n{script}");
        assert!(script.contains("#SBATCH --mem-per-gpu=40G"), "\n{script}");
    }

    // -- Per-rule walltime tests --------------------------------------------

    #[test]
    fn per_rule_walltime_slurm() {
        let rule = Rule {
            name: "long_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "sleep 100", &config).unwrap();

        // Per-rule walltime should override config walltime (DD-HH:MM:SS format)
        assert!(script.contains("#SBATCH --time=2-00:00:00"));
        assert!(!script.contains("#SBATCH --time=24:00:00"));
    }

    #[test]
    fn per_rule_walltime_pbs() {
        let rule = Rule {
            name: "long_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "sleep 100", &config).unwrap();

        assert!(script.contains("walltime=3-00:00:00"));
    }

    #[test]
    fn per_rule_walltime_sge() {
        let rule = Rule {
            name: "medium_job".to_string(),
            input: vec![].into(),
            output: vec![].into(),
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "analysis.sh", &config).unwrap();

        assert!(script.contains("#$ -l h_rt=06:00:00"));
    }

    #[test]
    fn walltime_format_conversion() {
        // Test the format_walltime_for_scheduler helper
        // SLURM uses DD-HH:MM:SS format for durations >= 24 hours
        assert_eq!(format_walltime_for_scheduler("24h"), "1-00:00:00");
        assert_eq!(format_walltime_for_scheduler("30m"), "00:30:00");
        assert_eq!(format_walltime_for_scheduler("2d"), "2-00:00:00");
        assert_eq!(format_walltime_for_scheduler("48h"), "2-00:00:00");
        // Colon forms parse through the shared walltime parser and render
        // canonically zero-padded (#737).
        assert_eq!(format_walltime_for_scheduler("1:30:00"), "01:30:00");
        assert_eq!(format_walltime_for_scheduler("12h"), "12:00:00"); // Less than 24h
        // Unparseable → verbatim, never a silent 1-hour default (#715/#737).
        assert_eq!(format_walltime_for_scheduler("24 hours"), "24 hours");
    }

    // -- Logs directory tests ------------------------------------------------

    #[test]
    fn slurm_creates_logs_dir() {
        let rule = make_rule("test", 1, None);
        let config = make_config();
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "echo test", &config).unwrap();

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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "echo test", &config).unwrap();

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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "echo test", &config).unwrap();

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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Lsf, &rule, "echo test", &config).unwrap();

        assert!(script.contains("mkdir -p logs"));
    }

    // -- Enhanced GPU spec tests ------------------------------------------------

    #[test]
    fn slurm_gpu_directive_helper_with_model() {
        use crate::rule::GpuSpec;
        let spec = GpuSpec {
            count: 2,
            model: Some("a100".to_string()),
            memory_gb: Some(40),
            compute_capability: None,
        };
        let directive = super::slurm_gpu_directive(&spec);
        assert!(directive.contains("a100"));
        assert!(directive.contains("2"));
        assert!(directive.contains("40g"));
    }

    #[test]
    fn slurm_gpu_directive_helper_without_model() {
        use crate::rule::GpuSpec;
        let spec = GpuSpec {
            count: 4,
            model: None,
            memory_gb: None,
            compute_capability: None,
        };
        let directive = super::slurm_gpu_directive(&spec);
        assert_eq!(directive, "gpu:4");
    }

    #[test]
    fn slurm_script_with_detailed_gpu() {
        use crate::rule::{GpuSpec, Resources};
        let rule = Rule {
            name: "gpu_job".to_string(),
            input: vec!["data.h5".to_string()].into(),
            output: vec!["model.pt".to_string()].into(),
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: None,
            resources: Resources {
                gpu_spec: Some(GpuSpec {
                    count: 2,
                    model: Some("a100".to_string()),
                    memory_gb: Some(40),
                    compute_capability: None,
                }),
                ..Default::default()
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
            walltime: None,
            extra_args: vec![],
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Slurm, &rule, "python train.py", &config)
                .unwrap();
        assert!(script.contains("--gres=gpu:a100:2:40g"));
        assert!(script.contains("--mem-per-gpu=40G"));
    }

    #[test]
    fn pbs_gpu_spec_directive() {
        use crate::rule::{GpuSpec, Resources};
        let rule = Rule {
            name: "gpu_pbs".to_string(),
            input: vec![].into(),
            output: vec![].into(),
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: None,
                gpu_spec: Some(GpuSpec {
                    count: 2,
                    model: Some("a100".to_string()),
                    memory_gb: None,
                    compute_capability: None,
                }),
                ..Default::default()
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Pbs, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("gpu=2"));
        // Model request is commented, not a directive
        assert!(script.contains("# Model requested: a100"));
    }

    #[test]
    fn sge_gpu_spec_directive() {
        use crate::rule::{GpuSpec, Resources};
        let rule = Rule {
            name: "gpu_sge".to_string(),
            input: vec![].into(),
            output: vec![].into(),
            shell: Some("python train.py".to_string()),
            script: None,
            threads: Some(4),
            memory: Some("16G".to_string()),
            resources: Resources {
                gpu: None,
                gpu_spec: Some(GpuSpec {
                    count: 4,
                    model: Some("v100".to_string()),
                    memory_gb: None,
                    compute_capability: None,
                }),
                ..Default::default()
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
            name_scope: None,
            volume: None,
        };
        let script =
            generate_submit_script(&ClusterBackend::Sge, &rule, "python train.py", &config)
                .unwrap();

        assert!(script.contains("#$ -l gpu=4"));
    }
}
