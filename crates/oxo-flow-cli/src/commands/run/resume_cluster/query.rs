//! The pure query/verdict half of cluster re-attach: which scheduler ids a
//! recorded job must be polled/accounted through, the verdict classifier, and
//! the fail-closed diagnostic. Kept out of the driver-facing module so the
//! reconciliation flow stays readable.

use super::probe::ClusterJobProbe;
use oxo_flow_core::backend::{BackendJobStatus, TerminalRecord};
use oxo_flow_core::executor::checkpoint::{CheckpointState, ClusterJobRecord};
use std::collections::HashMap;
use std::path::Path;

/// One recorded job plus the ids this backend must be queried with.
pub(crate) struct RecordedQuery {
    pub(crate) rule: String,
    pub(crate) job: ClusterJobRecord,
    /// Id `poll` accepts: the element id on element-aware backends, the
    /// array base on base-only backends (PBS/SGE/LSF).
    pub(crate) poll_id: String,
    /// Id the accounting store resolves (`sacct`/`qacct` accept the array
    /// base for every element).
    pub(crate) accounting_id: String,
    /// Whether a terminal record describes THIS element rather than a whole
    /// array allocation.
    pub(crate) per_element: bool,
}

impl RecordedQuery {
    pub(crate) fn cancel_target(&self) -> String {
        self.job
            .array_base
            .clone()
            .unwrap_or_else(|| self.job.job_id.clone())
    }
}

/// Drop recorded jobs whose rule a later invocation already completed, and
/// whose execution is no longer marked in flight.
///
/// The `running` mark is the discriminator (issue #1427 re-review): a
/// `run --rerun` re-submits a completed rule, and the submit hook records the
/// NEW job id while the rule intentionally stays in `completed_rules`. That
/// job is live — pruning it would let the next resume skip it and run
/// dependents against outputs it is still rewriting. A genuine later
/// completion clears the running mark, at which point the id is superseded.
///
/// Returns the removed `(rule, job)` pairs so the caller can narrate and
/// persist the cleanup.
pub(crate) fn prune_stale_completed_jobs(
    checkpoint: &mut CheckpointState,
) -> Vec<(String, ClusterJobRecord)> {
    let stale: Vec<(String, ClusterJobRecord)> = checkpoint
        .unsettled_cluster_jobs()
        .filter(|(rule, _)| {
            checkpoint.completed_rules.contains(*rule) && !checkpoint.is_running(rule)
        })
        .map(|(rule, job)| (rule.to_string(), job.clone()))
        .collect();
    for (rule, _) in &stale {
        checkpoint.clear_cluster_job(rule);
    }
    stale
}

pub(crate) fn recorded_queries(
    checkpoint: &CheckpointState,
    backend: &str,
    direct: bool,
) -> Vec<RecordedQuery> {
    checkpoint
        .unsettled_cluster_jobs()
        // Only the jobs this backend owns: a recorded id means nothing (and
        // can be a different job) on another scheduler.
        .filter(|(_, job)| job.backend == backend)
        .map(|(rule, job)| {
            let poll_id = if direct {
                job.job_id.clone()
            } else {
                job.array_base.clone().unwrap_or_else(|| job.job_id.clone())
            };
            // The driver probes accounting by ELEMENT id on element-aware
            // backends and by array base on base-only ones (driver.rs). A
            // finished SLURM element `44_1` must be looked up as `44_1`, not
            // as its parent `44`.
            let accounting_id = if direct {
                job.job_id.clone()
            } else {
                job.array_base.clone().unwrap_or_else(|| job.job_id.clone())
            };
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
pub(crate) fn reattach_required_error(
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

pub(crate) enum Verdict {
    /// Queued or running — still owned by the scheduler.
    Live,
    /// The scheduler reported a terminal state (plus accounting, when the
    /// store had a record).
    Terminal(BackendJobStatus, Option<Box<TerminalRecord>>),
    /// Absent from the live queue AND unknown to the accounting store.
    Unknown,
}

pub(crate) async fn resolve_verdict(
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
