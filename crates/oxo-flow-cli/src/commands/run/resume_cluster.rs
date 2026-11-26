//! `resume`'s cluster re-attach path (issue #1427).
//!
//! A cluster run records the scheduler id of every accepted job in the
//! checkpoint and removes it only when the job settles. A driver that died
//! without settling anything therefore leaves exact job ids behind — and a
//! `resume` that re-runs those rules locally (or resubmits them) while the
//! scheduler still owns them duplicates computation and races two writers on
//! the same outputs.
//!
//! This module reconciles a checkpoint's recorded jobs with the scheduler
//! **before** the run loop is entered: live jobs are waited for, terminal
//! success is adopted only when the rule's declared outputs exist, terminal
//! failure is recorded for a normal re-run, and an unreachable/unknown
//! scheduler fails closed. `--abandon-jobs` cancels the recorded jobs through
//! the backend that owns them instead of waiting.
//!
//! The scheduler seam is [`ClusterJobProbe`] — the three operations the
//! reconciliation needs, with the production implementation delegating to the
//! same [`ExecutorBackend`] the driver uses. Unit tests inject a fake probe;
//! nothing here needs a live scheduler.

use super::config_placeholder_values;
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::backend::cluster::ClusterExecutor;
use oxo_flow_core::backend::{BackendJobStatus, ExecutorBackend, TerminalRecord};
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::executor::checkpoint::{
    BenchmarkRecord, CheckpointState, ClusterJobRecord, ClusterRunRecord, rule_outputs_exist,
};
use oxo_flow_core::executor::{JobRecord, JobStatus};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// Poll cadence while waiting for a recorded job to settle. Mirrors the
/// driver's [`DriverConfig::poll_interval`](oxo_flow_core::backend::driver::DriverConfig)
/// default; a profile's own `poll_interval` does not apply here because the
/// reconciliation deliberately knows only what the checkpoint recorded.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Rounds between "still waiting" lines while a recorded job is live
/// (5 s × 12 ≈ one line per minute).
const WAIT_HEARTBEAT_ROUNDS: u32 = 12;

/// A future the reconciliation can await inside a `&dyn` probe — the same
/// boxed-future shape the driver's hooks use, so the CLI needs no
/// `async_trait` dependency for its test seam.
type ProbeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The scheduler operations re-attach needs, and nothing else.
trait ClusterJobProbe: Send + Sync {
    /// Live/terminal statuses for the given ids; ids the scheduler does not
    /// list are simply absent (read as [`BackendJobStatus::Unknown`]).
    fn poll_jobs<'a>(
        &'a self,
        ids: &'a [String],
    ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>>;

    /// Terminal record from the accounting store, or `None` when the store
    /// has nothing for the id.
    fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>>;

    /// Cancel one scheduler job (an array element cancels through its base).
    fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>>;

    /// Whether the backend can answer for individual array elements; when
    /// false, polling targets the array base id.
    fn polls_elements_directly(&self) -> bool;
}

/// [`ClusterJobProbe`] over the production [`ExecutorBackend`] — the same
/// implementation the driver polls and cancels through, so the two surfaces
/// cannot drift on scheduler state mapping.
struct BackendProbe {
    backend: Arc<dyn ExecutorBackend>,
}

impl BackendProbe {
    fn new(backend: Arc<dyn ExecutorBackend>) -> Self {
        Self { backend }
    }
}

impl ClusterJobProbe for BackendProbe {
    fn poll_jobs<'a>(
        &'a self,
        ids: &'a [String],
    ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>> {
        Box::pin(async move {
            self.backend
                .poll(ids)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>> {
        Box::pin(async move { self.backend.terminal_status(id).await })
    }

    fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>> {
        Box::pin(async move {
            self.backend
                .cancel(id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    fn polls_elements_directly(&self) -> bool {
        self.backend.polls_elements_directly()
    }
}

/// What one reconciliation did — the counts `resume --json` reports and the
/// operator sees narrated.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClusterReconcileOutcome {
    /// Rules whose job finished successfully and whose declared outputs
    /// exist; marked completed without re-running.
    pub adopted: Vec<String>,
    /// Rules whose recorded job ended without a usable success (terminal
    /// failure, cancellation, missing outputs) — recorded so the normal
    /// resume re-runs them.
    pub failed: Vec<String>,
    /// Rules whose live jobs `--abandon-jobs` cancelled before the re-run.
    pub cancelled: Vec<String>,
}

/// Everything `resume` needs from the cluster side before delegating to the
/// run loop.
#[derive(Debug, Default)]
pub(crate) struct ResumeClusterPlan {
    /// Profile the delegated `run` must use. `None` keeps the local path
    /// (only reachable when no job is unsettled).
    pub run_profile: Option<String>,
    /// Reconciliation outcome; `None` when the checkpoint recorded no
    /// unsettled job.
    pub outcome: Option<ClusterReconcileOutcome>,
}

/// Resolve the profile a resumed cluster run should use — the explicit
/// `--profile` first, then the profile the checkpoint recorded — if its file
/// still exists next to the workflow.
fn resolve_resume_profile(
    checkpoint: &CheckpointState,
    workflow_dir: &Path,
    explicit_profile: Option<&str>,
) -> Result<Option<String>> {
    if let Some(name) = explicit_profile {
        if super::super::run_preview::profile_file(workflow_dir, name).is_none() {
            // Same diagnostic `run --profile` gives; fail before any job is
            // touched, because the rerun would fail on this profile later.
            return Err(super::super::run_preview::profile_not_found(
                workflow_dir,
                name,
            ));
        }
        return Ok(Some(name.to_string()));
    }
    Ok(checkpoint
        .cluster_run
        .as_ref()
        .map(|record| record.profile.as_str())
        .filter(|name| super::super::run_preview::profile_file(workflow_dir, name).is_some())
        .map(str::to_string))
}

/// Reconcile the checkpoint's recorded scheduler jobs with the scheduler, or
/// fail closed when that cannot be done safely.
///
/// Called by `resume` before it delegates to the run loop (which is what
/// clears stale `running` entries and re-plans execution). On success the
/// checkpoint is persisted inside this function, before any rule can be
/// re-submitted.
pub(crate) async fn prepare_cluster_resume(
    checkpoint: &mut CheckpointState,
    checkpoint_path: &Path,
    workflow: &Path,
    explicit_profile: Option<&str>,
    abandon_jobs: bool,
    workdir: &Path,
) -> Result<ResumeClusterPlan> {
    let workflow_dir = oxo_flow_core::parent_dir(workflow);
    let recorded_profile = checkpoint
        .cluster_run
        .as_ref()
        .map(|record| record.profile.clone());
    let run_profile = resolve_resume_profile(checkpoint, workflow_dir, explicit_profile)?;

    if checkpoint.cluster_jobs.is_empty() {
        // No unsettled job: re-attach is unnecessary, but a cluster
        // checkpoint still resumes through its recorded profile so the
        // remaining rules keep running on the scheduler instead of silently
        // falling back to the local executor (the issue #1427 half that does
        // not involve live jobs).
        if abandon_jobs && checkpoint.cluster_run.is_some() {
            eprintln!(
                "  {} --abandon-jobs: the checkpoint records no unsettled scheduler job ids — nothing to cancel",
                "Note:".yellow()
            );
        }
        if explicit_profile.is_none()
            && let Some(recorded) = &recorded_profile
        {
            match &run_profile {
                Some(_) => eprintln!(
                    "{} Resuming through the recorded cluster profile '{}'",
                    "Cluster:".bold().cyan(),
                    recorded
                ),
                None => eprintln!(
                    "{} the checkpoint records cluster profile '{}', but its profile file is gone — resuming locally. Re-run with --profile <NAME> to use a scheduler",
                    "Warning:".bold().yellow(),
                    recorded
                ),
            }
        }
        return Ok(ResumeClusterPlan {
            run_profile,
            outcome: None,
        });
    }

    if run_profile.is_none() && !abandon_jobs {
        return Err(reattach_required_error(
            checkpoint,
            checkpoint_path,
            recorded_profile.as_deref(),
        ));
    }

    // Reconciliation always talks to the backend that actually owns the job
    // ids — recorded at submit time — never to whatever a (possibly edited)
    // profile resolves to today.
    let Some(cluster_run) = checkpoint.cluster_run.clone() else {
        anyhow::bail!(
            "checkpoint '{}' records {} unsettled scheduler job(s) but no cluster identity.\n  \
             Refusing to cancel or re-run jobs whose backend is unknown. Pass --profile <NAME> \
             and verify the jobs with 'oxo-flow cluster status' first.",
            checkpoint_path.display(),
            checkpoint.cluster_jobs.len()
        );
    };
    let probe = recorded_probe(&cluster_run)?;
    let config = load_expanded_config(workflow, run_profile.as_deref())?;

    eprintln!(
        "{} re-attaching to {} recorded scheduler job(s) through {} (profile '{}')",
        "Cluster:".bold().cyan(),
        oxo_flow_core::plural(checkpoint.cluster_jobs.len(), "job", "jobs"),
        cluster_run.backend,
        cluster_run.profile
    );
    if abandon_jobs {
        eprintln!(
            "  {} --abandon-jobs: live recorded jobs will be cancelled, not waited for",
            "Note:".yellow()
        );
    }

    let outcome = reconcile_recorded_jobs(
        &probe,
        checkpoint,
        &config,
        workdir,
        abandon_jobs,
        DEFAULT_POLL_INTERVAL,
    )
    .await?;

    // Persist BEFORE the delegated run loads the checkpoint again: an
    // unpersisted adoption would be re-planned as never-completed and the
    // job re-submitted — the exact duplication this path exists to prevent.
    checkpoint
        .save_to_file_async(checkpoint_path)
        .await
        .with_context(|| {
            format!(
                "reconciled the recorded cluster jobs but could not persist '{}'; refusing to \
                 continue because the next load would re-submit them",
                checkpoint_path.display()
            )
        })?;

    Ok(ResumeClusterPlan {
        run_profile,
        outcome: Some(outcome),
    })
}

/// Build the production probe from the checkpoint's recorded cluster run.
fn recorded_probe(record: &ClusterRunRecord) -> Result<BackendProbe> {
    let backend = ClusterBackend::from_str(&record.backend).map_err(|_| {
        anyhow::anyhow!(
            "checkpoint records cluster backend '{}', which this build does not know — expected {}",
            record.backend,
            oxo_flow_core::cluster::SUPPORTED_BACKENDS_HINT
        )
    })?;
    let executor = ClusterExecutor::new(
        backend,
        ClusterJobConfig {
            backend,
            queue: record.queue.clone(),
            account: record.account.clone(),
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        },
    );
    Ok(BackendProbe::new(Arc::new(executor)))
}

/// Load the workflow with the resume's profile applied and wildcards
/// expanded — the form `rule_outputs_exist` needs to check a rule instance's
/// declared outputs. Mirrors `run`'s preprocessing minus CLI overrides
/// (`resume` carries none).
fn load_expanded_config(workflow: &Path, profile: Option<&str>) -> Result<WorkflowConfig> {
    let workflow_dir = oxo_flow_core::parent_dir(workflow);
    let mut config = WorkflowConfig::from_file(workflow).with_context(|| {
        format!(
            "failed to load workflow '{}' while reconciling recorded cluster jobs",
            workflow.display()
        )
    })?;
    if let Some(name) = profile {
        super::super::run_preview::merge_profile(&mut config, name, workflow_dir)?;
    }
    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;
    Ok(config)
}

/// One recorded job plus the ids this backend must be queried with.
struct RecordedQuery {
    rule: String,
    job: ClusterJobRecord,
    /// Id `poll` accepts: the element id on element-aware backends, the
    /// array base on base-only backends (PBS/SGE/LSF).
    poll_id: String,
    /// Id the accounting store resolves (`sacct`/`qacct` accept the array
    /// base for every element).
    accounting_id: String,
    /// Whether a terminal record describes THIS element rather than a whole
    /// array allocation.
    per_element: bool,
}

impl RecordedQuery {
    fn cancel_target(&self) -> String {
        self.job
            .array_base
            .clone()
            .unwrap_or_else(|| self.job.job_id.clone())
    }
}

fn recorded_queries(checkpoint: &CheckpointState, direct: bool) -> Vec<RecordedQuery> {
    checkpoint
        .unsettled_cluster_jobs()
        .map(|(rule, job)| {
            let poll_id = if direct {
                job.job_id.clone()
            } else {
                job.array_base.clone().unwrap_or_else(|| job.job_id.clone())
            };
            let accounting_id = job.array_base.clone().unwrap_or_else(|| job.job_id.clone());
            RecordedQuery {
                rule: rule.to_string(),
                job: job.clone(),
                poll_id,
                accounting_id,
                per_element: direct || job.array_base.is_none(),
            }
        })
        .collect()
}

/// Fail-closed diagnostic when jobs are recorded but no usable profile is.
fn reattach_required_error(
    checkpoint: &CheckpointState,
    checkpoint_path: &Path,
    recorded_profile: Option<&str>,
) -> anyhow::Error {
    let jobs: Vec<String> = checkpoint
        .unsettled_cluster_jobs()
        .map(|(rule, job)| format!("  {rule}: {} job {}", job.backend, job.job_id))
        .collect();
    let profile_note = match recorded_profile {
        Some(name) => format!(
            "the checkpoint records profile '{name}', but profiles/{name}.toml|.oxoflow is not present next to the workflow"
        ),
        None => "the checkpoint records no cluster profile".to_string(),
    };
    anyhow::anyhow!(
        "checkpoint '{}' records {} scheduler job(s) from the interrupted cluster run that were never settled:\n{}\n  ({profile_note})\n\
         Refusing to clear them and re-run: they may still be queued or running, and a re-run would duplicate computation and race on the same output files.\n  \
         Re-attach and wait for them:  oxo-flow resume {} --profile <NAME>\n  \
         Cancel them and re-run:       oxo-flow resume {} --abandon-jobs\n  \
         (the recorded profile name is the one the original run used, e.g. 'slurm')",
        checkpoint_path.display(),
        checkpoint.unsettled_cluster_jobs().count(),
        jobs.join("\n"),
        checkpoint_path.display(),
        checkpoint_path.display()
    )
}

enum Verdict {
    /// Queued or running — still owned by the scheduler.
    Live,
    /// The scheduler reported a terminal state (plus accounting, when the
    /// store had a record).
    Terminal(BackendJobStatus, Option<Box<TerminalRecord>>),
    /// Absent from the live queue AND unknown to the accounting store.
    Unknown,
}

async fn resolve_verdict(
    probe: &dyn ClusterJobProbe,
    statuses: &HashMap<String, BackendJobStatus>,
    query: &RecordedQuery,
) -> Verdict {
    let status = statuses
        .get(&query.poll_id)
        .copied()
        .unwrap_or(BackendJobStatus::Unknown);
    match status {
        BackendJobStatus::Pending | BackendJobStatus::Running => Verdict::Live,
        BackendJobStatus::Completed | BackendJobStatus::Failed | BackendJobStatus::Cancelled => {
            // One accounting lookup at the terminal transition, like the
            // driver: the store supplies exit code and measurements the live
            // queue never reports.
            let record = probe.terminal_record(&query.accounting_id).await;
            Verdict::Terminal(status, record.map(Box::new))
        }
        BackendJobStatus::Unknown => match probe.terminal_record(&query.accounting_id).await {
            Some(record)
                if matches!(
                    record.status,
                    BackendJobStatus::Completed
                        | BackendJobStatus::Failed
                        | BackendJobStatus::Cancelled
                ) =>
            {
                Verdict::Terminal(record.status, Some(Box::new(record)))
            }
            _ => Verdict::Unknown,
        },
    }
}

/// Reconcile every recorded job, mutating `checkpoint` with the verdicts.
///
/// Live jobs are waited for (or cancelled under `abandon`); terminal jobs are
/// classified immediately. The checkpoint is NOT persisted here — the caller
/// persists once, on success.
async fn reconcile_recorded_jobs(
    probe: &dyn ClusterJobProbe,
    checkpoint: &mut CheckpointState,
    config: &WorkflowConfig,
    workdir: &Path,
    abandon: bool,
    poll_interval: Duration,
) -> Result<ClusterReconcileOutcome> {
    let direct = probe.polls_elements_directly();
    let mut pending = recorded_queries(checkpoint, direct);
    let total = pending.len();
    let mut outcome = ClusterReconcileOutcome::default();
    let wildcard_values = config_placeholder_values(&config.config);
    let tail_bytes = oxo_flow_core::executor::output_tail_bytes_from_env();
    let mut cancelled_targets: BTreeSet<String> = BTreeSet::new();
    let mut round: u32 = 0;

    while !pending.is_empty() {
        let mut ids: Vec<String> = pending.iter().map(|q| q.poll_id.clone()).collect();
        ids.sort();
        ids.dedup();
        let statuses = probe.poll_jobs(&ids).await.map_err(|error| {
            let jobs: Vec<String> = pending
                .iter()
                .map(|q| format!("{} ({} job {})", q.rule, q.job.backend, q.job.job_id))
                .collect();
            anyhow::anyhow!(
                "cannot reach the scheduler while reconciling {} recorded cluster job(s): {error}\n  jobs: {}\n  \
                 Refusing to re-run rules whose jobs may still be running. Retry when the scheduler is reachable, or pass --abandon-jobs to cancel them.",
                pending.len(),
                jobs.join(", ")
            )
        })?;

        let mut still_live: Vec<RecordedQuery> = Vec::new();
        for query in pending.drain(..) {
            match resolve_verdict(probe, &statuses, &query).await {
                Verdict::Live => {
                    if abandon {
                        cancel_one(probe, &query, &mut cancelled_targets).await?;
                        record_abandoned(checkpoint, &query, tail_bytes, &mut outcome);
                    } else {
                        still_live.push(query);
                    }
                }
                Verdict::Terminal(status, record) => match status {
                    BackendJobStatus::Completed => adopt_completed(
                        checkpoint,
                        config,
                        workdir,
                        &query,
                        record.as_deref(),
                        &wildcard_values,
                        tail_bytes,
                        &mut outcome,
                    ),
                    // A cancelled job is not a success and not a self-caused
                    // failure — but resume must re-run it either way.
                    BackendJobStatus::Failed | BackendJobStatus::Cancelled => {
                        record_failed(checkpoint, &query, status, record.as_deref(), tail_bytes);
                        outcome.failed.push(query.rule.clone());
                    }
                    _ => unreachable!("resolve_verdict only returns terminal states here"),
                },
                Verdict::Unknown => {
                    if abandon {
                        // Cancelling is the operator's explicit choice; the
                        // scheduler either confirms it or the reconciliation
                        // fails closed naming the job.
                        cancel_one(probe, &query, &mut cancelled_targets).await?;
                        record_abandoned(checkpoint, &query, tail_bytes, &mut outcome);
                    } else {
                        return Err(unknown_job_error(&query));
                    }
                }
            }
        }
        pending = still_live;
        if pending.is_empty() {
            break;
        }
        round += 1;
        if round == 1 || round.is_multiple_of(WAIT_HEARTBEAT_ROUNDS) {
            let jobs: Vec<String> = pending.iter().map(|q| q.job.job_id.clone()).collect();
            eprintln!(
                "  {} waiting for {} of {} recorded scheduler job(s) to settle: {}",
                "...".dimmed(),
                pending.len(),
                total,
                jobs.join(", ")
            );
        }
        tokio::time::sleep(poll_interval).await;
    }
    Ok(outcome)
}

fn unknown_job_error(query: &RecordedQuery) -> anyhow::Error {
    anyhow::anyhow!(
        "scheduler cannot confirm the state of recorded {} job {} (rule '{}'): it is not in the live queue and the accounting store reports no terminal record (the scheduler may be unreachable, or the job left the queue before the store saw it).\n  \
         Refusing to re-run a rule whose job may still be running. Check it with 'oxo-flow cluster status {}', wait for the scheduler's accounting store, or pass --abandon-jobs to cancel it.",
        query.job.backend,
        query.job.job_id,
        query.rule,
        query.job.job_id
    )
}

async fn cancel_one(
    probe: &dyn ClusterJobProbe,
    query: &RecordedQuery,
    cancelled_targets: &mut BTreeSet<String>,
) -> Result<()> {
    let target = query.cancel_target();
    if !cancelled_targets.insert(target.clone()) {
        // One array base covers many elements; the scheduler is asked once.
        return Ok(());
    }
    probe.cancel_job(&target).await.map_err(|error| {
        anyhow::anyhow!(
            "failed to cancel recorded {} job {} (rule '{}'): {error}\n  \
             The job may still be running; refusing to clear it and re-run its rule. Cancel it with 'oxo-flow cluster cancel {}' and retry.",
            query.job.backend,
            target,
            query.rule,
            target
        )
    })
}

/// Mark an abandoned rule for a normal re-run: recorded as failed so the
/// resume banner counts it and the run loop re-executes it (a rule left
/// unrecorded would be treated as `nothing_to_resume`).
fn record_abandoned(
    checkpoint: &mut CheckpointState,
    query: &RecordedQuery,
    tail_bytes: usize,
    outcome: &mut ClusterReconcileOutcome,
) {
    eprintln!(
        "  {} {}: cancelled {} job {} (--abandon-jobs) — it will re-run",
        "⊝".yellow(),
        query.rule,
        query.job.backend,
        query.job.job_id
    );
    checkpoint.record_run(
        &terminal_job_record(
            query,
            JobStatus::Cancelled,
            None,
            Some(format!(
                "cancelled by `resume --abandon-jobs` ({} job {})",
                query.job.backend, query.job.job_id
            )),
        ),
        tail_bytes,
    );
    checkpoint.mark_failed(&query.rule);
    checkpoint.clear_cluster_job(&query.rule);
    outcome.cancelled.push(query.rule.clone());
}

fn record_failed(
    checkpoint: &mut CheckpointState,
    query: &RecordedQuery,
    status: BackendJobStatus,
    record: Option<&TerminalRecord>,
    tail_bytes: usize,
) {
    let job_status = match status {
        BackendJobStatus::Cancelled => JobStatus::Cancelled,
        _ => JobStatus::Failed,
    };
    eprintln!(
        "  {} {}: {} job {} ended {} — it will re-run",
        "↻".yellow(),
        query.rule,
        query.job.backend,
        query.job.job_id,
        status.label()
    );
    checkpoint.record_run(
        &terminal_job_record(
            query,
            job_status,
            usable_exit_code(record, query),
            Some(format!(
                "cluster job {} ended {}",
                query.job.job_id,
                status.label()
            )),
        ),
        tail_bytes,
    );
    checkpoint.mark_failed(&query.rule);
    checkpoint.clear_cluster_job(&query.rule);
}

#[allow(clippy::too_many_arguments)]
fn adopt_completed(
    checkpoint: &mut CheckpointState,
    config: &WorkflowConfig,
    workdir: &Path,
    query: &RecordedQuery,
    record: Option<&TerminalRecord>,
    wildcard_values: &HashMap<String, String>,
    tail_bytes: usize,
    outcome: &mut ClusterReconcileOutcome,
) {
    let rule = query.rule.as_str();
    let Some(rule_def) = config.get_rule(rule) else {
        eprintln!(
            "  {} {}: {} job {} finished successfully but the rule is not in the current workflow — leaving it for a normal re-run",
            "Warning:".bold().yellow(),
            rule,
            query.job.backend,
            query.job.job_id
        );
        record_unadoptable(
            checkpoint,
            query,
            format!(
                "cluster job {} succeeded but rule '{rule}' is not in the current workflow",
                query.job.job_id
            ),
            tail_bytes,
        );
        outcome.failed.push(rule.to_string());
        return;
    };
    if !rule_outputs_exist(rule_def, workdir, wildcard_values) {
        eprintln!(
            "  {} {}: {} job {} finished successfully but its declared outputs are missing — leaving it for a normal re-run",
            "Warning:".bold().yellow(),
            rule,
            query.job.backend,
            query.job.job_id
        );
        record_unadoptable(
            checkpoint,
            query,
            format!(
                "cluster job {} succeeded but the rule's declared outputs are missing — re-running",
                query.job.job_id
            ),
            tail_bytes,
        );
        outcome.failed.push(rule.to_string());
        return;
    }

    // A base-only array's accounting row is the whole allocation's: drop the
    // per-process numbers, keep the shared duration (same rule the driver
    // applies when it settles array elements, issue #1132/RB-06).
    let per_element = query.per_element.then_some(record).flatten();
    checkpoint.record_run(
        &JobRecord {
            resources: per_element.and_then(|r| r.resources.clone()),
            signal: None,
            rule: rule.to_string(),
            status: JobStatus::Success,
            enqueued_at: None,
            started_at: None,
            finished_at: Some(chrono::Utc::now()),
            exit_code: per_element.and_then(|r| r.exit_code),
            stdout: None,
            stderr: None,
            command: None,
            retries: 0,
            skip_reason: None,
            max_rss_mb: per_element.and_then(|r| r.max_rss_mb),
            cpu_seconds: per_element.and_then(|r| r.cpu_seconds).map(|s| s as f64),
            caption: None,
        },
        tail_bytes,
    );
    if checkpoint.benchmarks.contains_key(rule) {
        // Never overwrite a measured benchmark with this synthetic one.
        checkpoint.mark_completed_quiet(rule);
    } else {
        checkpoint.mark_completed(
            rule,
            BenchmarkRecord {
                rule: rule.to_string(),
                wall_time_secs: record
                    .and_then(|r| r.elapsed_secs)
                    .map_or(0.0, |s| s as f64),
                max_memory_mb: per_element.and_then(|r| r.max_rss_mb),
                memory_limit_mb: rule_def
                    .effective_memory()
                    .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                cpu_seconds: per_element.and_then(|r| r.cpu_seconds).map(|s| s as f64),
                retries: 0,
                recorded_as: Some(format!(
                    "adopted from {} job {}",
                    query.job.backend, query.job.job_id
                )),
            },
        );
    }
    // Same input manifest a normal cluster settle records (fold_record), so a
    // later run's input-change detection treats the adopted completion
    // exactly like an observed one.
    if let Ok(Some(manifest)) = oxo_flow_core::executor::checkpoint::snapshot_input_manifest(
        rule_def,
        workdir,
        wildcard_values,
        &crate::commands::run_preview::storage_resolver(),
    ) {
        checkpoint.record_input_manifest(rule, manifest);
    }
    checkpoint.clear_cluster_job(rule);
    eprintln!(
        "  {} {}: {} job {} finished successfully — adopted from the scheduler",
        "✓".green(),
        rule,
        query.job.backend,
        query.job.job_id
    );
    outcome.adopted.push(rule.to_string());
}

/// A terminal success whose outputs cannot be verified is NOT a completion:
/// record it as a failure-to-adopt so resume re-runs the rule (and the
/// banner counts it), naming the reason.
fn record_unadoptable(
    checkpoint: &mut CheckpointState,
    query: &RecordedQuery,
    reason: String,
    tail_bytes: usize,
) {
    checkpoint.record_run(
        &terminal_job_record(query, JobStatus::Failed, None, Some(reason)),
        tail_bytes,
    );
    checkpoint.mark_failed(&query.rule);
    checkpoint.clear_cluster_job(&query.rule);
}

/// The terminal record an accounting store reports for THIS element, if the
/// store can speak per-element; a base-only array's row is the allocation's.
fn usable_exit_code(record: Option<&TerminalRecord>, query: &RecordedQuery) -> Option<i32> {
    query
        .per_element
        .then_some(record)
        .flatten()
        .and_then(|r| r.exit_code)
}

fn terminal_job_record(
    query: &RecordedQuery,
    status: JobStatus,
    exit_code: Option<i32>,
    skip_reason: Option<String>,
) -> JobRecord {
    JobRecord {
        resources: None,
        signal: None,
        rule: query.rule.clone(),
        status,
        enqueued_at: None,
        started_at: None,
        finished_at: Some(chrono::Utc::now()),
        exit_code,
        stdout: None,
        stderr: None,
        command: None,
        retries: 0,
        skip_reason,
        max_rss_mb: None,
        cpu_seconds: None,
        caption: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::executor::checkpoint::ClusterRunRecord;
    use std::sync::Mutex;

    fn submitted_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn checkpoint_with_jobs(jobs: &[(&str, &str, Option<&str>)]) -> CheckpointState {
        let mut ck = CheckpointState::new();
        ck.set_cluster_run(Some(ClusterRunRecord {
            profile: "slurm".to_string(),
            backend: "slurm".to_string(),
            queue: None,
            account: None,
            run_dir: None,
        }));
        for (rule, job_id, base) in jobs {
            ck.mark_running(rule);
            ck.record_cluster_job(
                rule,
                ClusterJobRecord {
                    job_id: (*job_id).to_string(),
                    array_base: base.map(str::to_string),
                    backend: "slurm".to_string(),
                    submitted_at: submitted_at(),
                },
            );
        }
        ck
    }

    fn config_with_outputs(workdir: &Path, outputs: &[&str]) -> WorkflowConfig {
        let mut toml = String::from("[workflow]\nname = \"reattach\"\n");
        for (i, output) in outputs.iter().enumerate() {
            toml.push_str(&format!(
                "[[rules]]\nname = \"r{i}\"\nshell = \"true\"\noutput = [\"{output}\"]\n"
            ));
        }
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        config
    }

    /// Scripted fake scheduler: poll verdicts in order (the last repeats),
    /// accounting records by id, and a log of every call.
    #[derive(Default)]
    struct FakeProbe {
        verdicts: Mutex<Vec<BackendJobStatus>>,
        accounting: Mutex<HashMap<String, TerminalRecord>>,
        poll_error: Mutex<bool>,
        polls: Mutex<Vec<Vec<String>>>,
        cancelled: Mutex<Vec<String>>,
        cancel_error: Mutex<bool>,
        direct: bool,
    }

    impl FakeProbe {
        fn new(direct: bool, verdicts: Vec<BackendJobStatus>) -> Self {
            Self {
                verdicts: Mutex::new(verdicts),
                direct,
                ..Self::default()
            }
        }
        fn with_accounting(self, id: &str, status: BackendJobStatus) -> Self {
            self.accounting
                .lock()
                .unwrap()
                .insert(id.to_string(), TerminalRecord::status_only(status));
            self
        }
        fn fail_poll(&self) {
            *self.poll_error.lock().unwrap() = true;
        }
        fn fail_cancel(&self) {
            *self.cancel_error.lock().unwrap() = true;
        }
        fn next_verdict(&self) -> BackendJobStatus {
            let mut verdicts = self.verdicts.lock().unwrap();
            if verdicts.len() > 1 {
                verdicts.remove(0)
            } else {
                verdicts[0]
            }
        }
        fn cancelled(&self) -> Vec<String> {
            self.cancelled.lock().unwrap().clone()
        }
        fn poll_count(&self) -> usize {
            self.polls.lock().unwrap().len()
        }
    }

    impl ClusterJobProbe for FakeProbe {
        fn poll_jobs<'a>(
            &'a self,
            ids: &'a [String],
        ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>> {
            self.polls.lock().unwrap().push(ids.to_vec());
            if *self.poll_error.lock().unwrap() {
                return Box::pin(async { Err(anyhow::anyhow!("injected scheduler failure")) });
            }
            let verdict = self.next_verdict();
            let statuses = ids.iter().map(|id| (id.clone(), verdict)).collect();
            Box::pin(async move { Ok(statuses) })
        }

        fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>> {
            let rec = self.accounting.lock().unwrap().get(id).cloned();
            Box::pin(async move { rec })
        }

        fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>> {
            self.cancelled.lock().unwrap().push(id.to_string());
            if *self.cancel_error.lock().unwrap() {
                return Box::pin(async { Err(anyhow::anyhow!("injected cancel failure")) });
            }
            Box::pin(async { Ok(()) })
        }

        fn polls_elements_directly(&self) -> bool {
            self.direct
        }
    }

    fn zero_poll() -> Duration {
        Duration::from_millis(1)
    }

    #[tokio::test]
    async fn terminal_success_with_outputs_is_adopted_without_resubmission() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed])
            .with_accounting("101", BackendJobStatus::Completed);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap();

        assert_eq!(outcome.adopted, vec!["r0".to_string()]);
        assert!(ck.completed_rules.contains("r0"));
        assert!(!ck.running_rules().contains("r0"));
        assert!(ck.cluster_jobs.is_empty(), "the settled job id is cleared");
        assert_eq!(probe.poll_count(), 1);
        assert!(probe.cancelled().is_empty());
        // The synthetic benchmark is transparent, not a fabricated measure.
        assert_eq!(
            ck.benchmarks["r0"].recorded_as.as_deref(),
            Some("adopted from slurm job 101")
        );
        assert_eq!(ck.rule_runs["r0"].status.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn terminal_success_without_outputs_is_left_for_a_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed]);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap();

        assert!(outcome.adopted.is_empty());
        assert_eq!(outcome.failed, vec!["r0".to_string()]);
        assert!(!ck.completed_rules.contains("r0"));
        assert!(ck.failed_rules.contains("r0"), "the rerun is guaranteed");
        assert!(ck.cluster_jobs.is_empty());
        assert_eq!(
            ck.rule_runs["r0"].skip_reason.as_deref(),
            Some(
                "cluster job 101 succeeded but the rule's declared outputs are missing — re-running"
            )
        );
    }

    #[tokio::test]
    async fn terminal_failure_is_recorded_for_the_normal_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        // A partial output does NOT make a failed job adoptable.
        std::fs::write(dir.path().join("out/result.txt"), "partial").unwrap();
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Failed]);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap();

        assert_eq!(outcome.failed, vec!["r0".to_string()]);
        assert!(!ck.completed_rules.contains("r0"));
        assert!(ck.failed_rules.contains("r0"));
        assert!(ck.cluster_jobs.is_empty());
    }

    #[tokio::test]
    async fn live_job_is_waited_for_until_it_settles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(
            true,
            vec![BackendJobStatus::Running, BackendJobStatus::Completed],
        );

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap();

        assert_eq!(outcome.adopted, vec!["r0".to_string()]);
        assert!(
            probe.poll_count() >= 2,
            "the live job must be polled, not resubmitted"
        );
        assert!(probe.cancelled().is_empty());
    }

    #[tokio::test]
    async fn abandon_cancels_live_jobs_then_leaves_them_for_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Running]);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), true, zero_poll())
                .await
                .unwrap();

        assert_eq!(outcome.cancelled, vec!["r0".to_string()]);
        assert_eq!(probe.cancelled(), vec!["101".to_string()]);
        assert!(ck.failed_rules.contains("r0"), "abandoned rules re-run");
        assert!(ck.cluster_jobs.is_empty());
        assert_eq!(probe.poll_count(), 1, "abandon does not wait");
    }

    #[tokio::test]
    async fn array_elements_cancel_their_base_once() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["a.txt", "b.txt"]);
        let mut ck =
            checkpoint_with_jobs(&[("r0", "44_1", Some("44")), ("r1", "44_2", Some("44"))]);
        // Base-only backend: polling targets the base.
        let probe = FakeProbe::new(false, vec![BackendJobStatus::Running]);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), true, zero_poll())
                .await
                .unwrap();

        assert_eq!(
            probe.cancelled(),
            vec!["44".to_string()],
            "one cancel per array base"
        );
        assert_eq!(outcome.cancelled, vec!["r0".to_string(), "r1".to_string()]);
        assert_eq!(
            probe.polls.lock().unwrap()[0],
            vec!["44".to_string()],
            "a base-only backend is polled by base id"
        );
    }

    #[tokio::test]
    async fn scheduler_poll_error_fails_closed_without_clearing_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed]);
        probe.fail_poll();

        let error =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap_err()
                .to_string();

        assert!(
            error.contains("cannot reach the scheduler"),
            "the scheduler error must surface: {error}"
        );
        assert!(error.contains("101"), "the job id must be named: {error}");
        // Nothing was cleared: a retry re-attaches to the same evidence.
        assert!(ck.cluster_jobs.contains_key("r0"));
    }

    #[tokio::test]
    async fn unknown_job_state_fails_closed_unless_abandoned() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        // Absent from the live queue and unknown to accounting.
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown]);

        let error =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap_err()
                .to_string();
        assert!(error.contains("cannot confirm the state"), "{error}");
        assert!(error.contains("--abandon-jobs"), "{error}");
        assert!(ck.cluster_jobs.contains_key("r0"));

        // abandon turns the unknown into a confirmed cancel + rerun.
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown]);
        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), true, zero_poll())
                .await
                .unwrap();
        assert_eq!(outcome.cancelled, vec!["r0".to_string()]);
        assert!(ck.cluster_jobs.is_empty());
    }

    #[tokio::test]
    async fn cancel_failure_fails_closed_and_never_reruns() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Running]);
        probe.fail_cancel();

        let error =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), true, zero_poll())
                .await
                .unwrap_err()
                .to_string();
        assert!(error.contains("failed to cancel"), "{error}");
        assert!(ck.cluster_jobs.contains_key("r0"));
        assert!(!ck.failed_rules.contains("r0"));
    }

    #[tokio::test]
    async fn accounting_settles_a_job_missing_from_the_live_queue() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
        let config = config_with_outputs(dir.path(), &["out/result.txt"]);
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown])
            .with_accounting("101", BackendJobStatus::Completed);

        let outcome =
            reconcile_recorded_jobs(&probe, &mut ck, &config, dir.path(), false, zero_poll())
                .await
                .unwrap();

        assert_eq!(outcome.adopted, vec!["r0".to_string()]);
        assert!(ck.cluster_jobs.is_empty());
    }

    #[test]
    fn resolve_profile_prefers_explicit_then_recorded_then_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        std::fs::write(
            dir.path().join("profiles/slurm.toml"),
            "[cluster]\nbackend = \"slurm\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("profiles/pbs.toml"),
            "[cluster]\nbackend = \"pbs\"\n",
        )
        .unwrap();
        let mut ck = CheckpointState::new();
        ck.set_cluster_run(Some(ClusterRunRecord {
            profile: "slurm".to_string(),
            backend: "slurm".to_string(),
            queue: None,
            account: None,
            run_dir: None,
        }));

        assert_eq!(
            resolve_resume_profile(&ck, dir.path(), None).unwrap(),
            Some("slurm".to_string())
        );
        assert_eq!(
            resolve_resume_profile(&ck, dir.path(), Some("pbs")).unwrap(),
            Some("pbs".to_string())
        );
        // A recorded profile whose file is gone is not usable — the caller
        // decides between fail-closed (live jobs) and local (no live jobs).
        ck.cluster_run.as_mut().unwrap().profile = "gone".to_string();
        assert_eq!(resolve_resume_profile(&ck, dir.path(), None).unwrap(), None);
        // An EXPLICIT missing profile is always an error (same message `run`
        // gives), never silently ignored.
        let error = resolve_resume_profile(&ck, dir.path(), Some("gone"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("profile 'gone' not found"), "{error}");
        // No recorded identity at all.
        let bare = CheckpointState::new();
        assert_eq!(
            resolve_resume_profile(&bare, dir.path(), None).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn no_recorded_jobs_never_touches_the_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        std::fs::write(
            dir.path().join("profiles/slurm.toml"),
            "[cluster]\nbackend = \"slurm\"\n",
        )
        .unwrap();
        let workflow = dir.path().join("wf.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"x\"\n[[rules]]\nname = \"a\"\nshell = \"true\"\n",
        )
        .unwrap();
        let checkpoint_path = dir.path().join("cp.json");
        let mut ck = CheckpointState::new();
        ck.set_cluster_run(Some(ClusterRunRecord {
            profile: "slurm".to_string(),
            backend: "slurm".to_string(),
            queue: None,
            account: None,
            run_dir: None,
        }));
        ck.save_to_file(&checkpoint_path).unwrap();

        let plan = prepare_cluster_resume(
            &mut ck,
            &checkpoint_path,
            &workflow,
            None,
            false,
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            plan.run_profile,
            Some("slurm".to_string()),
            "a cluster checkpoint resumes through its recorded profile"
        );
        assert!(plan.outcome.is_none());
    }

    #[tokio::test]
    async fn recorded_jobs_without_a_profile_fail_closed_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = dir.path().join("wf.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"x\"\n[[rules]]\nname = \"r0\"\nshell = \"true\"\noutput = [\"a.txt\"]\n",
        )
        .unwrap();
        let checkpoint_path = dir.path().join("cp.json");
        let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
        ck.save_to_file(&checkpoint_path).unwrap();
        let before = std::fs::read_to_string(&checkpoint_path).unwrap();

        let error = prepare_cluster_resume(
            &mut ck,
            &checkpoint_path,
            &workflow,
            None,
            false,
            dir.path(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("101"), "the message names the job: {error}");
        assert!(error.contains("--profile"), "{error}");
        assert!(error.contains("--abandon-jobs"), "{error}");
        // Fail-closed happens before anything is cleared or persisted.
        assert_eq!(
            std::fs::read_to_string(&checkpoint_path).unwrap(),
            before,
            "the checkpoint must be untouched"
        );
        assert!(ck.is_running("r0"));
    }
}
