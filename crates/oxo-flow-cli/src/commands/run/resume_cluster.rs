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
//! same [`oxo_flow_core::backend::ExecutorBackend`] the driver uses. Unit
//! tests inject a fake probe;
//! nothing here needs a live scheduler.

mod probe;
mod query;

use probe::{ClusterJobProbe, SchedulerRoute, recorded_probe};
use query::{RecordedQuery, Verdict, reattach_required_error, recorded_queries, resolve_verdict};

use super::config_placeholder_values;
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::backend::{BackendJobStatus, TerminalRecord};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::executor::checkpoint::{
    BenchmarkRecord, CheckpointState, ClusterJobRecord, rule_outputs_exist,
};
use oxo_flow_core::executor::{JobRecord, JobStatus};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;

/// Poll cadence while waiting for a recorded job to settle. Mirrors the
/// driver's [`DriverConfig::poll_interval`](oxo_flow_core::backend::driver::DriverConfig)
/// default; a profile's own `poll_interval` does not apply here because the
/// reconciliation deliberately knows only what the checkpoint recorded.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Rounds between "still waiting" lines while a recorded job is live
/// (5 s × 12 ≈ one line per minute).
const WAIT_HEARTBEAT_ROUNDS: u32 = 12;

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

    // A recorded job whose rule is already complete is settled by
    // definition: a later invocation (typically a plain local `run`)
    // completed the rule, and keeping the stale id would wedge resume on a
    // scheduler record that may since have been purged. Drop these FIRST,
    // before the fail-closed gate, so a fully-completed checkpoint never
    // demands a profile for a job nobody needs.
    let stale_completed: Vec<(String, ClusterJobRecord)> = checkpoint
        .unsettled_cluster_jobs()
        .filter(|(rule, _)| checkpoint.completed_rules.contains(*rule))
        .map(|(rule, job)| (rule.to_string(), job.clone()))
        .collect();
    for (rule, job) in &stale_completed {
        eprintln!(
            "  {} {}: already complete — dropping its stale recorded {} job {} instead of reconciling it",
            "Note:".yellow(),
            rule,
            job.backend,
            job.job_id
        );
        checkpoint.clear_cluster_job(rule);
    }

    if checkpoint.cluster_jobs.is_empty() {
        // Persist a prune even when nothing else is reconciled: the stale id
        // is exactly the permanent-wedge evidence this cleanup removes.
        if !stale_completed.is_empty() {
            checkpoint
                .save_to_file_async(checkpoint_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to persist the removal of stale recorded cluster jobs from '{}'",
                        checkpoint_path.display()
                    )
                })?;
        }
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

    // Reconciliation always talks to the backend that actually owns each job
    // id — recorded per job at submit time — never to whatever a (possibly
    // edited) profile or the checkpoint's latest cluster run resolves to
    // today.
    let Some(cluster_run) = checkpoint.cluster_run.clone() else {
        anyhow::bail!(
            "checkpoint '{}' records {} unsettled scheduler job(s) but no cluster identity.\n  \
             Refusing to cancel or re-run jobs whose backend is unknown. Pass --profile <NAME> \
             and verify the jobs with 'oxo-flow cluster status' first.",
            checkpoint_path.display(),
            checkpoint.cluster_jobs.len()
        );
    };
    let config = load_expanded_config(workflow, run_profile.as_deref())?;

    // Group the remaining jobs by the scheduler that owns them.
    // `cluster_run.backend` is the LATEST cluster invocation's route and may
    // differ from an older recorded job's (a crash on SLURM followed by a
    // `run --profile pbs`). Polling or cancelling an id through the wrong
    // scheduler could touch an unrelated job, so every group gets a probe
    // built from its own recorded backend.
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for (_, job) in checkpoint.unsettled_cluster_jobs() {
        *groups.entry(job.backend.clone()).or_default() += 1;
    }
    if groups.len() > 1
        || groups
            .keys()
            .next()
            .is_some_and(|b| b != &cluster_run.backend)
    {
        let routes: Vec<String> = groups
            .iter()
            .map(|(backend, count)| format!("{backend} ({count})"))
            .collect();
        eprintln!(
            "  {} the checkpoint's recorded cluster run is '{}', but its unsettled jobs span {} — each group is reconciled through the scheduler that owns it",
            "Warning:".bold().yellow(),
            cluster_run.backend,
            routes.join(", ")
        );
    }
    if abandon_jobs && run_profile.is_none() {
        eprintln!(
            "  {} no usable profile — after cancelling, the remaining rules run LOCALLY",
            "Note:".yellow()
        );
    }

    eprintln!(
        "{} re-attaching to {} recorded scheduler job(s) (recorded profile '{}')",
        "Cluster:".bold().cyan(),
        oxo_flow_core::plural(checkpoint.cluster_jobs.len(), "job", "jobs"),
        cluster_run.profile
    );
    if abandon_jobs {
        eprintln!(
            "  {} --abandon-jobs: live recorded jobs will be cancelled, not waited for",
            "Note:".yellow()
        );
    }

    let mut outcome = ClusterReconcileOutcome::default();
    for (backend_name, job_count) in &groups {
        // The queue/account only provably belong to a group submitted through
        // the same backend as the checkpoint's recorded run; passing another
        // backend's route would be worse than none.
        let same_route = *backend_name == cluster_run.backend;
        let route = SchedulerRoute::new(
            backend_name.clone(),
            same_route.then(|| cluster_run.queue.clone()).flatten(),
        );
        let account = same_route.then(|| cluster_run.account.clone()).flatten();
        let probe = recorded_probe(backend_name, route.queue.as_deref(), account.as_deref())?;
        let queries = recorded_queries(checkpoint, backend_name, probe.polls_elements_directly());
        eprintln!(
            "  {} {} job(s) through {}",
            "→".dimmed(),
            job_count,
            backend_name
        );
        let group = reconcile_recorded_jobs(
            &probe,
            checkpoint,
            queries,
            &route,
            &config,
            workdir,
            abandon_jobs,
            DEFAULT_POLL_INTERVAL,
        )
        .await?;
        outcome.adopted.extend(group.adopted);
        outcome.failed.extend(group.failed);
        outcome.cancelled.extend(group.cancelled);
    }

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
        // Silent: the delegated `run` merges the same profile and narrates it
        // once; a second "Profile: Applied…" line here is noise.
        super::super::run_preview::merge_profile_silent(&mut config, name, workflow_dir)?;
    }
    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;
    Ok(config)
}

/// Reconcile one backend's recorded jobs, mutating `checkpoint` with the
/// verdicts.
///
/// Live jobs are waited for (or cancelled under `abandon`); terminal jobs are
/// classified immediately. The checkpoint is NOT persisted here — the caller
/// persists once, after every backend group has been reconciled.
#[allow(clippy::too_many_arguments)]
async fn reconcile_recorded_jobs(
    probe: &dyn ClusterJobProbe,
    checkpoint: &mut CheckpointState,
    queries: Vec<RecordedQuery>,
    route: &SchedulerRoute,
    config: &WorkflowConfig,
    workdir: &Path,
    abandon: bool,
    poll_interval: Duration,
) -> Result<ClusterReconcileOutcome> {
    let mut pending = queries;
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
                        cancel_one(probe, &query, route, &mut cancelled_targets).await?;
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
                        cancel_one(probe, &query, route, &mut cancelled_targets).await?;
                        record_abandoned(checkpoint, &query, tail_bytes, &mut outcome);
                    } else {
                        return Err(unknown_job_error(&query, route));
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

fn unknown_job_error(query: &RecordedQuery, route: &SchedulerRoute) -> anyhow::Error {
    anyhow::anyhow!(
        "scheduler cannot confirm the state of recorded {} job {} (rule '{}'): it is not in the live queue and the accounting store reports no terminal record (the scheduler may be unreachable, or the job left the queue before the store saw it).\n  \
         Refusing to re-run a rule whose job may still be running. Check it with '{}', wait for the scheduler's accounting store, or pass --abandon-jobs to cancel it.",
        query.job.backend,
        query.job.job_id,
        query.rule,
        route.status_command(&query.job.job_id)
    )
}

async fn cancel_one(
    probe: &dyn ClusterJobProbe,
    query: &RecordedQuery,
    route: &SchedulerRoute,
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
             The job may still be running; refusing to clear it and re-run its rule. Cancel it with '{}' and retry.",
            query.job.backend,
            target,
            query.rule,
            route.cancel_command(&target)
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
    // Wall time is only known when the accounting store reported it. A
    // status-only record must not become a synthetic 0.0 s measurement:
    // complete the rule without a benchmark in that case, so "unknown" stays
    // unknown (the same contract synthetic `recorded_as` entries obey).
    match record.and_then(|r| r.elapsed_secs) {
        Some(elapsed) if !checkpoint.benchmarks.contains_key(rule) => {
            checkpoint.mark_completed(
                rule,
                BenchmarkRecord {
                    rule: rule.to_string(),
                    wall_time_secs: elapsed as f64,
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
        // A measured benchmark is never overwritten by an adopted one, and an
        // unknown wall time records no benchmark at all.
        _ => checkpoint.mark_completed_quiet(rule),
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
mod tests;
