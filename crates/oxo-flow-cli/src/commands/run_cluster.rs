//! `run --profile <NAME>` cluster path (issue #74 phase 2).
//!
//! Submission, tracking, and checkpoint bookkeeping for a workflow executed
//! through a scheduler instead of the local executor. The engine's decisions
//! are not re-derived here: the will-run set comes from `preview_run_plan` —
//! the same producer `dry-run` uses, and the set `BackendDriver`'s #78
//! acceptance test already pins submissions to — and job execution belongs to
//! `BackendDriver`. This module is the glue between them.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::backend::ScheduledPlan;
use oxo_flow_core::backend::cluster::ClusterExecutor;
use oxo_flow_core::backend::driver::{BackendDriver, DriverConfig, DriverOptions};
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use oxo_flow_core::config::{ClusterProfile, WorkflowConfig};
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::environment::EnvironmentResolver;
use oxo_flow_core::executor::checkpoint::{BenchmarkRecord, CheckpointState};
use oxo_flow_core::executor::{JobRecord, JobStatus};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Everything the cluster path needs from `run_command`'s scope.
pub(crate) struct ClusterRunArgs<'a> {
    pub config: &'a WorkflowConfig,
    pub dag: &'a WorkflowDag,
    pub order: &'a [String],
    pub checkpoint: &'a Arc<Mutex<CheckpointState>>,
    pub checkpoint_path: &'a Path,
    pub workdir: &'a Path,
    pub wildcard_values: &'a HashMap<String, String>,
    pub sensitive_keys: &'a HashSet<String>,
    /// Sensitive config values masked out of recorded job commands
    /// (issue #99 B1, cluster path).
    pub sensitive_values: &'a [String],
    /// Rules the run's invalidation analysis already forced. The local
    /// executor receives this set to bypass its freshness gate; the cluster
    /// path unions it into the submission set for the same reason.
    pub force_rules: &'a HashSet<String>,
    /// `--max-submitted`: a one-off override of the profile's queue cap.
    pub max_submitted: Option<usize>,
    pub rerun: bool,
    pub resume_failed: bool,
}

/// Outcome of a cluster run, mirroring what the local loop reports.
pub(crate) struct ClusterRunSummary {
    pub succeeded: usize,
    pub failed: usize,
    /// Failures of `required = false` rules: recorded and reported, but
    /// exempt from failing the run (issue #99 B2, cluster path).
    pub non_required_failed: usize,
    pub skipped: usize,
}

impl ClusterRunSummary {
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

/// Translate a `[cluster]` profile into the job config the renderer uses.
///
/// `backend` is the only required key: without it there is no scheduler to
/// submit to, and guessing one would submit real jobs to the wrong place.
fn job_config(cluster: &ClusterProfile) -> Result<(ClusterBackend, ClusterJobConfig)> {
    let backend_name = cluster.backend.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "profile has a [cluster] block without `backend` — set backend = \"slurm\" \
             (or pbs/sge/lsf)"
        )
    })?;
    let backend = ClusterBackend::from_str(backend_name).map_err(|_| {
        anyhow::anyhow!(
            "unknown cluster backend '{backend_name}' — expected slurm, pbs, sge, or lsf"
        )
    })?;
    Ok((
        backend,
        ClusterJobConfig {
            backend,
            queue: cluster.partition.clone(),
            account: cluster.account.clone(),
            walltime: cluster.walltime.clone(),
            extra_args: cluster.extra_args.clone(),
        },
    ))
}

/// Create `.oxo-flow/runs/<timestamp>/` and repoint the `latest` symlink.
///
/// The timestamp is the directory name so runs sort chronologically with
/// `ls`; `latest` is a convenience for `cd`, not something the code reads.
fn create_run_dir(workdir: &Path) -> Result<PathBuf> {
    let runs_root = workdir.join(".oxo-flow").join("runs");
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let run_dir = runs_root.join(&stamp);
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create run directory {}", run_dir.display()))?;

    #[cfg(unix)]
    {
        let latest = runs_root.join("latest");
        // Replace rather than fail: `latest` points at the newest run, and a
        // stale symlink from a previous run is expected, not an error.
        let _ = std::fs::remove_file(&latest);
        if let Err(e) = std::os::unix::fs::symlink(&stamp, &latest) {
            tracing::debug!(error = %e, "could not update the runs/latest symlink");
        }
    }
    Ok(run_dir)
}

/// Fold one driver-returned record into the checkpoint, mirroring the local
/// loop's bookkeeping so `status`, `resume`, and re-runs cannot tell the two
/// execution paths apart.
async fn record_outcome(
    args: &ClusterRunArgs<'_>,
    record: &JobRecord,
    summary: &mut ClusterRunSummary,
) {
    let duration = record
        .finished_at
        .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
        .map(|d| d.num_milliseconds() as f64 / 1000.0)
        .unwrap_or(0.0);
    let mut ck = args.checkpoint.lock().await;
    match record.status {
        JobStatus::Success => {
            summary.succeeded += 1;
            let rule = args.config.get_rule(&record.rule);
            let benchmark = BenchmarkRecord {
                rule: record.rule.clone(),
                wall_time_secs: duration,
                // Real usage needs `sacct` polling — phase 4. Reporting a
                // fabricated number here would be worse than reporting none.
                max_memory_mb: None,
                memory_limit_mb: rule
                    .and_then(|r| r.effective_memory())
                    .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                cpu_seconds: None,
                retries: record.retries,
            };
            ck.record_run(record);
            ck.mark_completed(&record.rule, benchmark);
            if let Some(rule) = rule
                && let Ok(Some(manifest)) =
                    oxo_flow_core::executor::checkpoint::snapshot_input_manifest(
                        rule,
                        args.workdir,
                        args.wildcard_values,
                        &crate::commands::run_preview::storage_resolver(),
                    )
            {
                ck.record_input_manifest(&record.rule, manifest);
            }
        }
        JobStatus::Failed => {
            // A failed `required = false` rule is recorded and reported but
            // does not fail the run (issue #99 B2, cluster path) — the same
            // contract as the local loop.
            let is_required = args
                .config
                .get_rule(&record.rule)
                .is_none_or(|r| r.required);
            if is_required {
                summary.failed += 1;
            } else {
                summary.non_required_failed += 1;
            }
            ck.record_run(record);
            ck.mark_failed(&record.rule);
        }
        _ => {
            summary.skipped += 1;
        }
    }
    if let Err(e) = ck.save_to_file(args.checkpoint_path) {
        tracing::warn!("Failed to save checkpoint: {e}");
    }
}

/// Submit and track this workflow through the scheduler named by `cluster`.
pub(crate) async fn run_on_cluster(
    cluster: &ClusterProfile,
    args: ClusterRunArgs<'_>,
) -> Result<ClusterRunSummary> {
    let (backend, cluster_config) = job_config(cluster)?;

    // The will-run set: the same computation `dry-run` reports.
    //
    // It is unioned with `force_rules` because by the time the cluster path
    // runs, `run_command` has already applied and persisted its invalidation
    // analysis — the cleared completion records and refreshed manifests the
    // preview would need to re-derive a cascade are gone, so a cascaded rule
    // whose outputs still look fresh classifies as a skip. `force_rules` is
    // that analysis's own output, and the local executor gets it for exactly
    // this purpose (see `ExecutorConfig.force_rules`), so consuming both here
    // is what keeps the two paths deciding identically.
    let mut to_run: HashSet<String> = {
        let ck = args.checkpoint.lock().await;
        let preview = crate::commands::run_preview::preview_run_plan(
            &ck,
            args.config,
            args.dag,
            args.order,
            args.workdir,
            args.wildcard_values,
            args.sensitive_keys,
            &args.config.workflow.interpreter_map,
            args.checkpoint_path,
            args.rerun,
            args.resume_failed,
        );
        preview
            .plan
            .iter()
            .filter(|p| !p.status.is_skip())
            .map(|p| p.name.clone())
            .collect()
    };
    // Forced rules are still constrained to this run's execution set — a
    // `--target` subset must not pull in rules the user excluded.
    to_run.extend(
        args.order
            .iter()
            .filter(|name| args.force_rules.contains(*name))
            .cloned(),
    );

    let mut summary = ClusterRunSummary {
        succeeded: 0,
        failed: 0,
        non_required_failed: 0,
        skipped: 0,
    };

    if to_run.is_empty() {
        eprintln!(
            "{} everything is up to date — nothing to submit",
            "Cluster:".bold().cyan()
        );
        return Ok(summary);
    }

    let env_resolver = EnvironmentResolver::new();
    let mut plan = ScheduledPlan::build(
        args.config,
        args.dag,
        args.workdir,
        &env_resolver,
        args.wildcard_values,
    )
    .map_err(|e| anyhow::anyhow!("failed to build the execution plan: {e}"))?;

    let run_dir = create_run_dir(args.workdir)?;

    let driver_defaults = DriverConfig::default();
    let driver_config = DriverConfig {
        // CLI flag beats the profile, which beats the driver default.
        max_submitted: args
            .max_submitted
            .or(cluster.max_submitted)
            .unwrap_or(driver_defaults.max_submitted),
        max_array_size: cluster
            .max_array_size
            .unwrap_or(driver_defaults.max_array_size),
        no_arrays: false,
        poll_interval: cluster
            .poll_interval_secs()
            .map(std::time::Duration::from_secs)
            .unwrap_or(driver_defaults.poll_interval),
        // No wall-clock budget: cluster jobs legitimately sit in the queue
        // for hours. Cancellation is the user's call (phase 2 follow-up).
        poll_timeout: None,
    };

    eprintln!(
        "{} submitting {} job(s) to {} (max {} in flight)",
        "Cluster:".bold().cyan(),
        to_run.len(),
        backend,
        driver_config.max_submitted
    );
    eprintln!("  run directory: {}", run_dir.display());

    let executor = ClusterExecutor::new(backend, cluster_config);
    let driver = BackendDriver::new(Arc::new(executor), driver_config);
    // Submit-time checkpoint recording (issue #136 H6): a crashed driver
    // leaves truthful RUNNING entries for accepted jobs, so `resume` /
    // `--resume-failed` re-queue them instead of losing them.
    let submit_ck = args.checkpoint.clone();
    let submit_path = args.checkpoint_path.to_path_buf();
    let on_submit = move |rule: String,
                          _job: String|
          -> std::pin::Pin<
        Box<dyn std::future::Future<Output = oxo_flow_core::error::Result<()>> + Send>,
    > {
        let ck = submit_ck.clone();
        let path = submit_path.clone();
        Box::pin(async move {
            let mut ck = ck.lock().await;
            ck.record_run(&oxo_flow_core::executor::JobRecord {
                rule: rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Running,
                started_at: Some(chrono::Utc::now()),
                finished_at: None,
                exit_code: None,
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
            });
            ck.save_to_file(&path)
        })
    };
    let records = driver
        .run(
            &mut plan,
            &to_run,
            DriverOptions {
                run_dir: &run_dir,
                on_checkpoint: None,
                merge: None,
                sensitive_values: args.sensitive_values,
                on_submit: Some(Box::new(on_submit)),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("cluster execution failed: {e}"))?;

    for record in &records {
        record_outcome(&args, record, &mut summary).await;
    }

    if summary.non_required_failed > 0 {
        eprintln!(
            "\n{} {} succeeded, {} failed, {} non-required failed, {} skipped",
            "Cluster:".bold(),
            summary.succeeded,
            summary.failed,
            summary.non_required_failed,
            summary.skipped
        );
    } else {
        eprintln!(
            "\n{} {} succeeded, {} failed, {} skipped",
            "Cluster:".bold(),
            summary.succeeded,
            summary.failed,
            summary.skipped
        );
    }
    Ok(summary)
}
