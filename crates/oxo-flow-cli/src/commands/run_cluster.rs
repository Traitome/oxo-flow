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
use oxo_flow_core::backend::driver::{BackendDriver, CancelSignal, DriverConfig, DriverOptions};
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
    /// `--cache-dir`: env cache location (falls back to the same
    /// `workdir/.oxo-flow/env-cache` default the local executor uses, so
    /// the two execution paths share env state — issue #136 tier-2 audit).
    pub cache_dir: Option<PathBuf>,
    /// `--skip-env-setup`: honored by the local executor. The cluster path
    /// never auto-creates environments, so the flag is a no-op there — it
    /// is announced at submit time, never silently accepted.
    pub skip_env_setup: bool,
    /// `--ai-recover`: supported by the local loop only; the cluster path
    /// has no AI recovery. A request is announced at submit time, never
    /// silently accepted.
    pub ai_recover: bool,
    /// Resolved run scratch directory (`[engine] tmp_dir` /
    /// `OXO_FLOW_TMPDIR`, issues #907 + #1013 C2): exported as
    /// `TMPDIR`/`TMP`/`TEMP` into every cluster job shell, matching the
    /// local executor's process-env injection.
    pub tmp_dir: Option<&'a Path>,
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
            "unknown cluster backend '{backend_name}' — expected slurm, pbs, sge, lsf, \
             kubernetes (k8s), aws-batch, or gcp-batch"
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

/// Fail before any submission when a rule declares more than the workflow's
/// own `[resource_budget]` allows.
///
/// Only an explicit cap gates — auto-detected machine limits are advice, not
/// a threshold, the same contract the local path applies to
/// `--max-threads`/`--max-memory`. A job that alone exceeds the budget its
/// workflow declares can never be scheduled inside it; on a scheduler the
/// queue rejects it (or runs it into swap) long after the submit, so saying
/// so here — with every offender named — beats discovering it mid-run.
fn check_budget(
    plan: &ScheduledPlan,
    to_run: &HashSet<String>,
    budget: &oxo_flow_core::config::ResourceBudget,
) -> Result<()> {
    // A cap that cannot be read is not "no cap": gating on `u64::MAX` would
    // silently wave every rule through.
    let memory_cap = budget
        .max_memory
        .as_deref()
        .map(|m| {
            oxo_flow_core::scheduler::parse_memory_mb(m)
                .map(|mb| (m, mb))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "[resource_budget] max_memory = '{m}' is not a memory figure — use e.g. 64G"
                    )
                })
        })
        .transpose()?;
    let mut breaches: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue; // not schedulable — skipped later, nothing to submit
        };
        let rule = &sr.rule;
        if let Some((cap, cap_mb)) = memory_cap
            && let Some(requested) = rule.effective_memory()
            && oxo_flow_core::scheduler::parse_memory_mb(requested).is_some_and(|mb| mb > cap_mb)
        {
            breaches.push(format!(
                "rule '{name}' requests {requested} but [resource_budget] max_memory = {cap}"
            ));
        }
        if let Some(cap) = budget.max_threads
            && rule.effective_threads() > cap
        {
            breaches.push(format!(
                "rule '{name}' requests {} threads but [resource_budget] max_threads = {cap}",
                rule.effective_threads()
            ));
        }
    }
    if breaches.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "[resource_budget] exceeded by {} — nothing was submitted:\n  {}",
        oxo_flow_core::plural(breaches.len(), "rule", "rules"),
        breaches.join("\n  ")
    ))
}

/// Checkpoint re-entry (`checkpoint = true` rules that expand into new
/// instances mid-run) is wired on the local executor only. The driver
/// accepts the hooks, but the cluster path passes `None` — the manifest
/// would be silently ignored and the dynamically discovered instances
/// would never execute. Reject at plan time, naming every offender,
/// instead of under-executing.
fn reject_checkpoint_reentry(plan: &ScheduledPlan, to_run: &HashSet<String>) -> Result<()> {
    let mut offenders: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue; // not schedulable — skipped later, nothing to submit
        };
        if sr.rule.checkpoint {
            offenders.push(name.clone());
        }
    }
    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "checkpoint re-entry is not supported on the cluster path yet — {} would \
         silently skip the dynamically discovered instances; remove `checkpoint = true` \
         or run these on the local path (`oxo-flow run`) instead:\n  {}",
        oxo_flow_core::plural(offenders.len(), "rule", "rules"),
        offenders.join("\n  ")
    ))
}

/// `localrule = true` is a promise that a rule never leaves this machine.
/// The cluster path has no local executor to honor it with, and skipping
/// the rule instead would unblock its dependents while its outputs never
/// materialize (the driver treats anything outside the run set as
/// satisfied) — under-execution. Reject at plan time, naming every
/// offender, and point at the path that does honor the directive (issue
/// #1013 E2).
fn reject_localrules(plan: &ScheduledPlan, to_run: &HashSet<String>) -> Result<()> {
    let mut offenders: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue; // not schedulable — skipped later, nothing to submit
        };
        if sr.rule.localrule {
            offenders.push(name.clone());
        }
    }
    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "`localrule = true` cannot be honored on the cluster path — {} would \
         be submitted to the scheduler anyway; remove `localrule = true` or run \
         the workflow on the local path (`oxo-flow run`) instead:\n  {}",
        oxo_flow_core::plural(offenders.len(), "rule", "rules"),
        offenders.join("\n  ")
    ))
}

/// Create `.oxo-flow/runs/<timestamp>/` and repoint the `latest` symlink.
///
/// The timestamp is the directory name so runs sort chronologically with
/// `ls`; `latest` is a convenience for `cd`, not something the code reads.
fn create_run_dir(workdir: &Path) -> Result<PathBuf> {
    let runs_root = workdir.join(".oxo-flow").join("runs");
    // Sub-second suffix: two cluster runs inside the same second used to
    // create_dir_all into ONE directory and share checkpoints/logs.
    let now = chrono::Utc::now();
    let stamp = format!(
        "{}-{:03}",
        now.format("%Y-%m-%dT%H-%M-%S"),
        now.timestamp_subsec_millis()
    );
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

/// The env cache dir for a cluster run: `--cache-dir` when given, else the
/// same `workdir/.oxo-flow/env-cache` default the local executor uses, so
/// the two execution paths share env state (issue #136 tier-2 audit).
fn cluster_env_cache_dir(cache_dir: Option<&Path>, workdir: &Path) -> PathBuf {
    cache_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workdir.join(".oxo-flow").join("env-cache"))
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
    // No ExecutorConfig on the cluster path — the tail size comes from the
    // same env override the signal handler reads (issue #691).
    let output_tail_bytes = oxo_flow_core::executor::output_tail_bytes_from_env();
    let mut ck = args.checkpoint.lock().await;
    match record.status {
        JobStatus::Success => {
            summary.succeeded += 1;
            let rule = args.config.get_rule(&record.rule);
            let benchmark = BenchmarkRecord {
                rule: record.rule.clone(),
                wall_time_secs: duration,
                // Read from the scheduler's accounting store as the job
                // settled, so this is the store's own peak rather than the
                // sampled one the local executor reports. Still `None` when
                // the store did not report it (LSF, and any job whose
                // accounting row never appeared) — a fabricated number
                // would be worse than none.
                max_memory_mb: record.max_rss_mb,
                memory_limit_mb: rule
                    .and_then(|r| r.effective_memory())
                    .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                cpu_seconds: record.cpu_seconds,
                retries: record.retries,
                recorded_as: None,
            };
            ck.record_run(record, output_tail_bytes);
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
            ck.record_run(record, output_tail_bytes);
            ck.mark_failed(&record.rule);
        }
        // A signal-interrupted job arrives as Failed (handled above) — the
        // driver only ever produces Cancelled records from the scheduler
        // itself reporting a cancelled job, which counts as neither success
        // nor failure but must still persist to the checkpoint.
        JobStatus::Cancelled => {
            summary.skipped += 1;
            ck.record_run(record, output_tail_bytes);
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

    // The local path's cache default is `workdir/.oxo-flow/env-cache` (or
    // --cache-dir); the cluster path must share it so env state is not
    // split across two caches (issue #136 tier-2 audit — `EnvironmentResolver::new()`
    // would silently use the process-global default instead).
    let env_resolver = EnvironmentResolver::with_cache_dir(&cluster_env_cache_dir(
        args.cache_dir.as_deref(),
        args.workdir,
    ));
    // Flags the cluster path cannot honor are announced at submit time —
    // never silently accepted (issue #136 tier-2 audit).
    if args.skip_env_setup {
        eprintln!(
            "  {} --skip-env-setup has no effect on the cluster path — environments are \
             never auto-created there (accepted for command-line parity)",
            "Warning:".bold().yellow()
        );
    }
    if args.ai_recover {
        eprintln!(
            "  {} --ai-recover is not supported on the cluster path — failed jobs are \
             recorded for resume/--resume-failed instead of AI-repaired",
            "Warning:".bold().yellow()
        );
    }
    // issue #1013 C2: honor the run's scratch directory on the cluster
    // path — export it into every job shell (build_with_scratch), since
    // jobs start from the scheduler's clean environment. `merge` is None
    // on this path, so nothing re-expands the prefixed commands.
    let mut plan = ScheduledPlan::build_with_scratch(
        args.config,
        args.dag,
        args.workdir,
        &env_resolver,
        args.wildcard_values,
        args.tmp_dir,
    )
    .map_err(|e| anyhow::anyhow!("failed to build the execution plan: {e}"))?;

    let run_dir = create_run_dir(args.workdir)?;

    // A rule that alone exceeds the workflow's own declared budget would be
    // rejected by the queue (or run into swap) after the fact — say so before
    // anything leaves for the scheduler.
    if let Some(budget) = args.config.resource_budget.as_ref() {
        check_budget(&plan, &to_run, budget)?;
    }
    reject_checkpoint_reentry(&plan, &to_run)?;
    reject_localrules(&plan, &to_run)?;

    let driver_defaults = DriverConfig::default();
    let mut max_submitted = args
        .max_submitted
        .or(cluster.max_submitted)
        .unwrap_or(driver_defaults.max_submitted);
    // `[resource_budget] max_jobs` caps the total concurrently running jobs;
    // on the cluster path the in-flight cap IS max_submitted, so the budget
    // clamps the resolved cap the same way the local path clamps `-j`
    // (issue #1013 E1). max_memory/max_threads stay hard per-rule errors in
    // check_budget above.
    if let Some(max_jobs) = args
        .config
        .resource_budget
        .as_ref()
        .and_then(|b| b.max_jobs)
        && max_submitted > max_jobs
    {
        eprintln!(
            "  {} [resource_budget] max_jobs = {} caps the requested submission concurrency — submitting at most {} jobs in flight",
            "Warning:".bold().yellow(),
            max_jobs,
            max_jobs
        );
        max_submitted = max_jobs;
    }
    let mut driver_config = DriverConfig {
        // CLI flag beats the profile, which beats the driver default.
        max_submitted,
        max_array_size: cluster
            .max_array_size
            .unwrap_or(driver_defaults.max_array_size),
        no_arrays: false,
        poll_interval: cluster
            .poll_interval_secs()
            .map(std::time::Duration::from_secs)
            .unwrap_or(driver_defaults.poll_interval),
        // No wall-clock budget: cluster jobs legitimately sit in the queue
        // for hours. Cancellation is the user's signal call (below).
        poll_timeout: None,
        unknown_settle_grace: std::time::Duration::from_secs(90),
        // Wired below, after the watch channel exists.
        cancel: None,
    };
    // Wire OS signals into the driver: SIGINT/SIGTERM/SIGHUP cancel every
    // in-flight scheduler job and surface as a Cancelled error carrying the
    // interrupted records (issue #1013 C1).
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<CancelSignal>);
    let signal_task = tokio::spawn(async move {
        if let Some(signal) = crate::commands::run::signals::wait_for_termination_signal().await {
            let _ = cancel_tx.send(Some(CancelSignal {
                signal_name: signal.name,
                signum: signal.signum,
            }));
        }
    });
    driver_config.cancel = Some(cancel_rx);

    eprintln!(
        "{} submitting {} to {} (max {} in flight)",
        "Cluster:".bold().cyan(),
        oxo_flow_core::plural(to_run.len(), "job", "jobs"),
        backend,
        driver_config.max_submitted
    );
    eprintln!("  run directory: {}", run_dir.display());

    let executor =
        ClusterExecutor::new(backend, cluster_config).with_workdir(args.workdir.to_path_buf());
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
        let output_tail_bytes = oxo_flow_core::executor::output_tail_bytes_from_env();
        Box::pin(async move {
            let mut ck = ck.lock().await;
            ck.record_run(
                &oxo_flow_core::executor::JobRecord {
                    signal: None,
                    rule: rule.clone(),
                    status: oxo_flow_core::executor::JobStatus::Running,
                    enqueued_at: Some(chrono::Utc::now()),
                    started_at: Some(chrono::Utc::now()),
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    skip_reason: None,
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: None,
                },
                output_tail_bytes,
            );
            ck.save_to_file(&path)
        })
    };
    let run_result = driver
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
        .await;
    signal_task.abort();
    // A cancellation must persist its interrupted records like any terminal
    // state — dropping them through map_err would lose the only truthful
    // account of what the signal stopped (and skip the failed-count
    // summary the webhook + exit contract below rely on).
    let records = match run_result {
        Ok(records) => records,
        Err(oxo_flow_core::error::OxoFlowError::Cancelled {
            records: interrupted,
            ..
        }) => interrupted,
        Err(e) => return Err(anyhow::anyhow!("cluster execution failed: {e}")),
    };

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

#[cfg(test)]
mod tests {
    use super::check_budget;
    use super::cluster_env_cache_dir;
    use super::reject_checkpoint_reentry;
    use super::reject_localrules;
    use oxo_flow_core::backend::ScheduledPlan;
    use oxo_flow_core::config::{ResourceBudget, WorkflowConfig};
    use oxo_flow_core::dag::WorkflowDag;
    use oxo_flow_core::environment::EnvironmentResolver;
    use std::collections::HashMap;

    #[test]
    fn env_cache_defaults_to_workdir_env_cache() {
        // The cluster path must share the local executor's default cache
        // dir (`workdir/.oxo-flow/env-cache`), not EnvironmentResolver's
        // process-global default (issue #136 tier-2 audit).
        let dir = cluster_env_cache_dir(None, std::path::Path::new("/wf"));
        assert_eq!(dir, std::path::PathBuf::from("/wf/.oxo-flow/env-cache"));
    }

    #[test]
    fn env_cache_flag_wins_over_default() {
        let dir = cluster_env_cache_dir(
            Some(std::path::Path::new("/custom/cache")),
            std::path::Path::new("/wf"),
        );
        assert_eq!(dir, std::path::PathBuf::from("/custom/cache"));
    }

    /// A plan with one rule declaring `memory` and `threads`.
    fn hungry_plan(workdir: &std::path::Path, memory: &str, threads: u32) -> ScheduledPlan {
        let toml = format!(
            "[workflow]\nname = \"budget\"\n[[rules]]\nname = \"align\"\n\
             memory = \"{memory}\"\nthreads = {threads}\n\
             shell = \"true\"\noutput = [\"out.txt\"]\n"
        );
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn budget_rejects_a_rule_that_cannot_fit() {
        // A rule asking for 100000G against a declared budget of 64G used to
        // be handed to the scheduler verbatim.
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "100000G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let budget = ResourceBudget {
            max_threads: None,
            max_memory: Some("64G".to_string()),
            max_jobs: None,
        };
        let err = check_budget(&plan, &to_run, &budget).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("align") && msg.contains("100000G") && msg.contains("64G"),
            "the breach must name the rule, the request, and the cap: {msg}"
        );
        assert!(
            msg.contains("nothing was submitted"),
            "the run must be able to stop before any submission: {msg}"
        );
    }

    #[test]
    fn budget_rejects_an_impossible_thread_request() {
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 64);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let budget = ResourceBudget {
            max_threads: Some(16),
            max_memory: None,
            max_jobs: None,
        };
        let msg = check_budget(&plan, &to_run, &budget)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("requests 64 threads") && msg.contains("max_threads = 16"),
            "unexpected breach message: {msg}"
        );
    }

    #[test]
    fn budget_passes_a_request_that_fits() {
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let budget = ResourceBudget {
            max_threads: Some(16),
            max_memory: Some("64G".to_string()),
            max_jobs: None,
        };
        assert!(check_budget(&plan, &to_run, &budget).is_ok());
    }

    #[test]
    fn max_jobs_caps_the_resolved_max_submitted() {
        // E1: `[resource_budget] max_jobs` caps the cluster path's in-flight
        // submission cap. The precedence chain (CLI flag > profile > driver
        // default) resolves first; the budget then clamps the result — the
        // same clamp semantics the local path applies to `-j`.
        let budget = |max_jobs: Option<usize>| ResourceBudget {
            max_threads: None,
            max_memory: None,
            max_jobs,
        };
        let resolved = 50_usize; // driver default, as if no flag/profile set

        // Budget below the resolved cap: clamped down.
        let clamped = resolved.min(budget(Some(4)).max_jobs.unwrap_or(resolved));
        assert_eq!(clamped, 4);

        // Budget above the resolved cap (or absent): no change.
        assert_eq!(
            resolved.min(budget(Some(100)).max_jobs.unwrap_or(resolved)),
            50
        );
        assert_eq!(resolved.min(budget(None).max_jobs.unwrap_or(resolved)), 50);
    }

    #[test]
    fn budget_ignores_rules_outside_this_run() {
        // Only what this run submits is gated: a stale rule kept out of
        // `to_run` by the invalidation analysis must not block the run.
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "100000G", 64);
        let budget = ResourceBudget {
            max_threads: Some(1),
            max_memory: Some("1G".to_string()),
            max_jobs: None,
        };
        assert!(check_budget(&plan, &std::collections::HashSet::new(), &budget).is_ok());
    }

    /// A plan whose single rule declares checkpoint re-entry.
    fn checkpoint_plan(workdir: &std::path::Path) -> ScheduledPlan {
        let toml = "[workflow]\nname = \"checkpoint\"\n[[rules]]\nname = \"discover\"\n\
                    checkpoint = true\ncheckpoint_manifest = \"manifest.toml\"\n\
                    shell = \"true\"\noutput = [\"out.txt\"]\n";
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn checkpoint_reentry_is_rejected_on_the_cluster_plan() {
        // The driver accepts the hooks but the cluster path passes None —
        // without this gate the manifest would be silently ignored.
        let dir = tempfile::tempdir().unwrap();
        let plan = checkpoint_plan(dir.path());
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let msg = reject_checkpoint_reentry(&plan, &to_run)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("discover") && msg.contains("oxo-flow run"),
            "the rejection must name the rule and the workaround: {msg}"
        );
    }

    #[test]
    fn checkpoint_reentry_gate_passes_plain_rules() {
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(reject_checkpoint_reentry(&plan, &to_run).is_ok());
    }

    #[test]
    fn checkpoint_reentry_gate_ignores_rules_outside_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let plan = checkpoint_plan(dir.path());
        assert!(reject_checkpoint_reentry(&plan, &std::collections::HashSet::new()).is_ok());
    }

    /// A plan whose single rule pins itself to local execution.
    fn localrule_plan(workdir: &std::path::Path) -> ScheduledPlan {
        let toml = "[workflow]\nname = \"localrule\"\n[[rules]]\nname = \"stage_refs\"\n\
                    localrule = true\nshell = \"true\"\noutput = [\"out.txt\"]\n";
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn localrule_rules_are_rejected_on_the_cluster_plan() {
        // E2: `localrule = true` used to be parsed and ignored — the rule
        // went to the scheduler anyway. The directive must be honored by
        // refusing the submission, naming the rule, before anything leaves.
        let dir = tempfile::tempdir().unwrap();
        let plan = localrule_plan(dir.path());
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let msg = reject_localrules(&plan, &to_run).unwrap_err().to_string();
        assert!(
            msg.contains("stage_refs") && msg.contains("oxo-flow run"),
            "the rejection must name the rule and the local path: {msg}"
        );
        assert!(
            msg.contains("localrule"),
            "the rejection must name the directive being dishonored: {msg}"
        );
    }

    #[test]
    fn localrule_gate_passes_plain_rules() {
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(reject_localrules(&plan, &to_run).is_ok());
    }

    #[test]
    fn localrule_gate_ignores_rules_outside_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let plan = localrule_plan(dir.path());
        assert!(reject_localrules(&plan, &std::collections::HashSet::new()).is_ok());
    }

    #[test]
    fn budget_rejects_an_unreadable_cap_instead_of_ignoring_it() {
        // An unparseable cap must not silently become "no cap at all".
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let budget = ResourceBudget {
            max_threads: None,
            max_memory: Some("lots".to_string()),
            max_jobs: None,
        };
        let msg = check_budget(&plan, &to_run, &budget)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("max_memory = 'lots'"),
            "the message must name the bad value: {msg}"
        );
    }
}
