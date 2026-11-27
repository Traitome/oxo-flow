//! `--profile` cluster dispatch (issue #74 phase 2): the two-sided opt-in
//! gate plus the terminal summary parity the local executor path already
//! provides (#83 P1-14 report snapshot, #142 H6 `--json` contract).

use super::*;
use crate::commands::run_cluster::{ClusterRunArgs, run_on_cluster};
use oxo_flow_core::config::ClusterProfile;

/// Resolve the cluster route only when BOTH halves are present: a workflow
/// carrying `[cluster]` keeps running locally until the user opts in with
/// `--profile` (no existing workflow starts submitting jobs silently), and
/// `--profile` without `[cluster]` has no scheduler definition to submit
/// to. Returns the profile to dispatch when both are present.
pub(crate) fn routed_cluster_profile<'a>(
    profile: Option<&str>,
    cluster: Option<&'a ClusterProfile>,
) -> Option<&'a ClusterProfile> {
    match (profile, cluster) {
        (Some(_), Some(c)) => Some(c),
        _ => None,
    }
}

/// Run through the scheduler backend and close with the same terminal
/// contract as the local path: the `--json` summary on both outcomes
/// (issue #142 H6), the workflow webhook, a non-zero exit on failure, and
/// the `.oxo-flow/reports/` snapshot the local path leaves behind
/// (issue #83 P1-14).
pub(crate) async fn dispatch_cluster_run(
    cluster_profile: &ClusterProfile,
    args: ClusterRunArgs<'_>,
    json_summary: &mut RunJsonSummary,
    workflow: &Path,
    no_report_snapshot: bool,
) -> Result<()> {
    // Copy the references the terminal contract needs before `args` moves
    // into the driver call (all three are shared references, so the borrow
    // outlives the struct).
    let config: &WorkflowConfig = args.config;
    let checkpoint = args.checkpoint;
    let workdir = args.workdir;
    let summary = run_on_cluster(cluster_profile, args).await?;
    // #1400: a run the cluster cost gate HALTED reports its own observable
    // outcome (`cost_budget_exceeded`) with the structured detail, instead of
    // folding into the generic `failed` status. Review F2/F3: a run that
    // merely ENDED over its cap (every record terminal, `cost_halt` false)
    // keeps its own verdict — `completed`/`failed` by the records — with the
    // `cost_budget` detail attached below, mirroring the local completion path.
    let status = if summary.cost_halt {
        "cost_budget_exceeded"
    } else if summary.is_success() {
        "completed"
    } else {
        "failed"
    };
    if let Some(detail) = &summary.cost_budget {
        json_summary.set_cost_budget(detail.clone());
    }
    json_summary.emit(
        status,
        &RunCounts {
            succeeded: summary.succeeded,
            skipped: summary.skipped,
            failed: summary.failed,
            non_required_failed: summary.non_required_failed,
            blocked: 0,
        },
        // Cluster per-rule resources come from sacct accounting at
        // report time, not the live checkpoint — empty here.
        vec![],
    );
    notify_webhook(
        config,
        if summary.is_success() {
            oxo_flow_core::webhook::WebhookEvent::WorkflowCompleted
        } else {
            oxo_flow_core::webhook::WebhookEvent::WorkflowFailed
        },
        Some((summary.succeeded, summary.failed, summary.skipped)),
    )
    .await;
    // An interrupt ends the process with the signal's exit code (128 +
    // signum), matching the local executor. By now the interrupted records
    // are persisted, the `--json` summary was emitted as failed, and the
    // failure webhook was sent — an interrupt is never reported as a
    // completed run, even when every interrupted rule was
    // `required = false` (which is counted as non-required failed).
    if let Some(code) = summary.interrupt_exit_code {
        crate::background::remove_pid_file_from_env();
        std::process::exit(code);
    }
    if !summary.is_success() {
        return Err(anyhow::anyhow!("workflow execution failed"));
    }
    if !no_report_snapshot {
        let ck = checkpoint.lock().await;
        if let Err(e) = crate::commands::output::snapshot_report(workflow, workdir, &ck) {
            eprintln!("  {} Report snapshot failed: {e}", "⚠".yellow());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ClusterProfile {
        serde_json::from_str(r#"{"backend":"slurm"}"#).expect("minimal profile")
    }

    #[test]
    fn cluster_route_requires_both_profile_and_block() {
        let p = profile();
        assert!(
            routed_cluster_profile(Some("cluster"), Some(&p))
                .is_some_and(|got| std::ptr::eq(got, &p))
        );
        assert!(routed_cluster_profile(Some("cluster"), None).is_none());
        assert!(routed_cluster_profile(None, Some(&p)).is_none());
        assert!(routed_cluster_profile(None, None).is_none());
    }
}
