//! `resume` command execution: loads checkpoint state and re-enters the run loop.

use super::*;
use crate::commands::print_banner;
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use serde::Serialize;
use std::path::PathBuf;

/// The `resume --json` document (issue #1334): one machine-readable
/// description of what a `resume` invocation was asked to do and how it
/// ended. Field order is fixed by the struct declaration, so repeated runs
/// over the same checkpoint emit key-ordered JSON (AGENTS.md
/// order-independence invariant).
///
/// The counts are the checkpoint's own state exactly as the human banner
/// reports them (`completed`/`failed`/`remaining`); no field is fabricated.
/// `remaining` is what this resume will actually execute (the nodes it
/// "resumes"), `completed` is what the checkpoint already had — the two
/// halves the "resumed vs already-complete" question asks for.
#[derive(Serialize)]
struct ResumeJson {
    command: &'static str,
    workflow: String,
    checkpoint: String,
    /// `nothing_to_resume` | `completed` | `failed`.
    status: &'static str,
    completed: usize,
    failed: usize,
    remaining: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_by_when: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    running: Option<usize>,
    /// Recorded scheduler jobs this resume adopted as completed (issue
    /// #1427) — present only when a cluster reconciliation ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    adopted: Option<usize>,
    /// Recorded scheduler jobs `--abandon-jobs` cancelled before re-running
    /// (issue #1427) — present only when a cluster reconciliation ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    cancelled_jobs: Option<usize>,
}

impl ResumeJson {
    fn emit(&self) {
        println!(
            "{}",
            serde_json::to_string_pretty(self).expect("resume JSON is serializable")
        );
    }
}

/// `resume[-- <args>]` entry point. Wraps [`resume_command_inner`] so the
/// `--json` error envelope is emitted for the early hard-fail paths (a
/// malformed checkpoint, a workflow that no longer exists) that bail before
/// the success document is built — a JSON consumer must never receive empty
/// stdout (same contract as `status`, issue #859).
#[allow(clippy::too_many_arguments)]
pub async fn resume_command(
    checkpoint: PathBuf,
    jobs: usize,
    ai_recover: bool,
    ai_max_retries: Option<u32>,
    keep_going: bool,
    timeout: String,
    workdir: Option<PathBuf>,
    no_report_snapshot: bool,
    profile: Option<String>,
    abandon_jobs: bool,
    trust_mtime: bool,
    json: bool,
) -> Result<()> {
    let checkpoint_for_error = checkpoint.clone();
    let mut json_written = false;
    let result = resume_command_inner(
        checkpoint,
        jobs,
        ai_recover,
        ai_max_retries,
        keep_going,
        timeout,
        workdir,
        no_report_snapshot,
        profile,
        abandon_jobs,
        trust_mtime,
        json,
        &mut json_written,
    )
    .await;
    if json
        && !json_written
        && let Err(error) = &result
    {
        crate::commands::json_error::emit(
            "resume",
            crate::commands::json_error::code_for(error, "CHECKPOINT"),
            &format!("{error:#}"),
            serde_json::json!({"checkpoint": checkpoint_for_error.display().to_string()}),
        );
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn resume_command_inner(
    checkpoint: PathBuf,
    jobs: usize,
    ai_recover: bool,
    ai_max_retries: Option<u32>,
    keep_going: bool,
    timeout: String,
    workdir: Option<PathBuf>,
    no_report_snapshot: bool,
    profile: Option<String>,
    abandon_jobs: bool,
    trust_mtime: bool,
    json: bool,
    json_written: &mut bool,
) -> Result<()> {
    print_banner();

    // Load checkpoint state
    let state = CheckpointState::load_from_file(&checkpoint)
        .with_context(|| {
            format!(
                "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).",
                checkpoint.display()
            )
        })
        .map_err(|e| {
            // Same friendly hint `status` gives for the identical mistake
            // (audit #276 P4-4): passing a workflow file where a checkpoint
            // belongs should name the confusion, not just the JSON parse error.
            if checkpoint.extension().is_some_and(|ext| ext == "oxoflow") {
                anyhow::anyhow!(
                    "{e}\n  '{}' appears to be a workflow file, not a checkpoint.\n  \
                 The 'resume' command expects a checkpoint file (e.g., .oxo-flow/checkpoint.json).",
                    checkpoint.display()
                )
            } else {
                e
            }
        })?;

    // Fail-safe format gate (issue #1309): the standalone `resume` command
    // refuses a checkpoint written by a NEWER build at its OWN load, before
    // narrating a banner or a `--json` document from the untrusted completion
    // counts. The delegated `run_command` below re-applies the same gate in
    // Execute mode; gating here keeps the resume banner and its early
    // `nothing_to_resume` return honest too — no surface reads a newer
    // format's completion state without refusing first.
    let mut state = super::gate_checkpoint_format(state, &checkpoint, CheckpointLoadMode::Execute)?;

    // Get workflow path from checkpoint
    let workflow_path = match &state.workflow_path {
        Some(p) => PathBuf::from(p),
        None => {
            anyhow::bail!(
                "Checkpoint does not contain a workflow reference.\n  \
                 The checkpoint at '{}' was generated by an older version of oxo-flow.\n  \
                 To resume manually, run: oxo-flow run <workflow.oxoflow>\n  \
                 (oxo-flow run automatically resumes from the checkpoint)",
                checkpoint.display()
            );
        }
    };

    if !workflow_path.exists() {
        anyhow::bail!(
            "Workflow file '{}' referenced by checkpoint no longer exists.\n  \
             The workflow may have been moved or deleted.",
            workflow_path.display()
        );
    }

    // Names captured before the paths move into the RunRequest below.
    let workflow_display = workflow_path.display().to_string();
    let checkpoint_display = checkpoint.display().to_string();

    eprintln!(
        "{} Resuming workflow '{}'",
        "Resume:".bold().cyan(),
        workflow_path.display()
    );
    eprintln!("  Checkpoint: {}", checkpoint.display());

    // Workdir precedence (issue #68): explicit --workdir > the directory the
    // original run recorded in the checkpoint > the workflow's directory.
    // Resolved here because the cluster re-attach's output checks need the
    // same directory the delegated run will use.
    let effective_workdir = workdir
        .clone()
        .or_else(|| state.workdir.clone().map(PathBuf::from));
    let execution_workdir = effective_workdir
        .clone()
        .unwrap_or_else(|| oxo_flow_core::parent_dir(&workflow_path).to_path_buf());

    // ── Cluster re-attach (issue #1427) ─────────────────────────────────
    // MUST run before the delegated run loop: that loop clears the persisted
    // `running` set and re-plans execution, which is exactly the destructive
    // step that used to re-submit (or locally re-run) jobs the scheduler
    // still owns. Reconciliation also resolves the profile name the run must
    // carry — `profile: None` routed a resumed cluster checkpoint to the
    // local executor. On a fail-closed error nothing has been cleared or
    // persisted here.
    let cluster_plan = super::resume_cluster::prepare_cluster_resume(
        &mut state,
        &checkpoint,
        &workflow_path,
        profile.as_deref(),
        abandon_jobs,
        &execution_workdir,
    )
    .await?;
    let adopted = cluster_plan.outcome.as_ref().map(|o| o.adopted.len());
    let cancelled_jobs = cluster_plan.outcome.as_ref().map(|o| o.cancelled.len());

    let completed = state.completed_rules.len();
    // Issue #690: don't count stale when-gated entries as failures in the
    // resume banner either — they are skips, and the executor will re-judge
    // their gate on this resume anyway.
    let (failed, skipped_when_false) =
        reconcile_failed_rules(&state.failed_rules, &state.when_verdicts);
    let failed = failed.len();
    let skipped_count = skipped_when_false.len();
    // Issue #685: entries persisting in the running set mean the prior run
    // died mid-flight; run_command clears them (with a warning) on start.
    // Cluster jobs the re-attach just reconciled are already out of this
    // set, so what remains describes a local (or unreconcilable) process.
    let running_count = state.running_rules().len();

    // Remaining = the rules this resume will actually execute: everything
    // the checkpoint has not completed (failed rules are retried). The
    // checkpoint records only what ran, so the workflow supplies the total
    // (its template count — wildcards expand later); if it cannot be read
    // here, `run_command` reports that error with full context a moment
    // later and the failed count is the fallback. The old arithmetic
    // reduced to `failed` (audit finding).
    let remaining = WorkflowConfig::from_file(&workflow_path)
        .map(|cfg| cfg.rules.len().saturating_sub(completed))
        .unwrap_or(failed);
    eprintln!("  State: {completed} completed, {failed} failed, {remaining} remaining");
    if !trust_mtime {
        eprintln!(
            "  Note: verifying recorded output manifests for completed rules — a missing, \
             resized, content-changed, or unhashed output re-runs (pass --trust-mtime to keep \
             the size+mtime verdict)."
        );
    }
    if !skipped_when_false.is_empty() {
        eprintln!(
            "  Note: {} {} skipped by their `when` condition (recorded false); the gate is re-judged on this resume.",
            skipped_when_false.len(),
            if skipped_when_false.len() == 1 {
                "rule was"
            } else {
                "rules were"
            }
        );
    }
    if running_count > 0 {
        eprintln!(
            "  Note: {} {} still running when the previous run died; they will re-run.",
            running_count,
            if running_count == 1 {
                "rule was"
            } else {
                "rules were"
            }
        );
    }

    if completed == 0 && failed == 0 {
        eprintln!(
            "  {} No rules have been executed yet. Use 'oxo-flow run' instead.",
            "Note:".yellow()
        );
        if json {
            ResumeJson {
                command: "resume",
                workflow: workflow_display,
                checkpoint: checkpoint_display,
                status: "nothing_to_resume",
                completed,
                failed,
                remaining,
                skipped_by_when: (skipped_count > 0).then_some(skipped_count),
                running: (running_count > 0).then_some(running_count),
                adopted,
                cancelled_jobs,
            }
            .emit();
            *json_written = true;
        }
        return Ok(());
    }

    // Re-run the workflow — the checkpoint with completed rules will cause
    // already-finished rules to be skipped automatically
    eprintln!();
    eprintln!(
        "{} Launching executor with {}...",
        "Info:".bold().cyan(),
        oxo_flow_core::plural(jobs, "parallel job", "parallel jobs")
    );

    eprintln!(
        "  Workdir: {}",
        effective_workdir
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(workflow directory)".to_string())
    );

    let outcome = run_command(RunRequest {
        workflow: Some(workflow_path),
        jobs,
        keep_going,
        // recorded by the original run
        workdir: effective_workdir,
        log_file: None,
        target: Vec::new(),
        module: Vec::new(),
        retry: 0,
        timeout,
        // user can re-run with --resume-failed in 'run'
        resume_failed: false,
        // Issue #1427: the explicit --profile, or the profile the checkpoint
        // recorded. Passing None here was the defect — it forced every
        // resumed cluster checkpoint onto the local executor.
        profile: cluster_plan.run_profile,
        max_threads: 0,
        max_memory: 0,
        skip_env_setup: false,
        // resume: refs already built
        skip_ref_build: true,
        cache_dir: None,
        // Issue #1426: the checkpoint's output manifests are verified for
        // every completed rule (size + content hash), not just recorded
        // provenance checksums — `--provenance` was opt-in and never covered
        // large outputs, so a plain run → interrupt → resume sequence could
        // silently reuse a truncated artifact.
        provenance: false,
        // `--trust-mtime` restores the historical size+mtime verdict.
        verify_outputs: true,
        trust_mtime,
        // resume delegates to the run loop, which narrates to stderr; the
        // `resume` document is the one emitted below, so the run summary
        // would be a competing `{"command":"run"}` document.
        json: false,
        // resume reuses checkpoint state
        cli_args: Vec::new(),
        ai_recover,
        // resume honors the user's AI recovery choice (no loop guard here —
        // a verification rerun goes through `run`, never `resume`)
        no_ai_recover: false,
        ai_max_retries,
        // resume restores checkpoint state as-is
        samples_filter: Vec::new(),
        // resume skips completed rules by design
        rerun: false,
        no_report_snapshot,
        // cluster queue cap — resume keeps the profile's
        max_submitted: None,
        // #538: the user's checkpoint path wins over the workdir-derived
        // default — the banner's "Resuming… N completed" must be the truth.
        checkpoint_override: Some(checkpoint),
        // resume honors the protection gate
        allow_protected_drift: false,
        // resume never re-derives a from-rule closure: it continues the
        // recorded checkpoint's pending set exactly as-is.
        from_rule: None,
        // resume re-solves degraded environments like a fresh run would
        strict_envs: false,
        // resume carries no reviewed repair file: `--apply-patch` is a `run`
        // flag (issue #1302), not a `resume` one.
        apply_patch: None,
        // `resume` has no --locked flag; a workflow's [engine] locked key
        // still enforces the lock through the run path (issue #1399), so a
        // locked workflow cannot resume across drift.
        locked: false,
    })
    .await;

    if json {
        ResumeJson {
            command: "resume",
            workflow: workflow_display,
            checkpoint: checkpoint_display,
            status: if outcome.is_ok() {
                "completed"
            } else {
                "failed"
            },
            completed,
            failed,
            remaining,
            skipped_by_when: (skipped_count > 0).then_some(skipped_count),
            running: (running_count > 0).then_some(running_count),
            adopted,
            cancelled_jobs,
        }
        .emit();
        *json_written = true;
    }

    outcome
}
