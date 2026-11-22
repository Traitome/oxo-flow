//! Verified AI-repair loop (issue #1164): after `apply_fix` succeeds, the
//! engine attempts ONE bounded verification rerun of the failed rules using
//! the UPDATED workflow, records the outcome in the checkpoint
//! (`repair_record`), and narrates stable machine-readable `REPAIR_*`
//! prefixes. The prefixes are a cross-agent contract consumed verbatim by
//! the eval harness — never reword them:
//!
//! - `REPAIR_APPLIED <session-id>` — the AI fix was written to the workflow
//! - `REPAIR_RERUN_SUCCEEDED <detail>` — the verification rerun exited 0
//! - `REPAIR_RERUN_FAILED <detail>` — the rerun exited non-zero
//! - `REPAIR_RERUN_SKIPPED <reason>` — no rerun was attempted

use super::*;

/// Environment variable marking a process as a verification-rerun child
/// (issue #1164 loop guard): a rerun never spawns another rerun, and (review
/// B1) it skips the exclusive workdir lock its parent already holds. Set on
/// the child process by [`verification_rerun`]; both the failure handler and
/// the run command consult [`loop_guard_active`] before acting.
pub(crate) const REPAIR_RERUN_ENV: &str = "OXO_FLOW_REPAIR_RERUN";

/// Parent run context forwarded into the verification rerun's argv (review
/// R3, issue #1164): the child must execute the SAME plan the parent ran,
/// not a freshly re-derived one. Every field is conditional — a knob the
/// child would resolve identically on its own is not forwarded, keeping the
/// spawned argv minimal.
///
/// Plan-shaping flags (review of the v0.32.0 cycle) are forwarded too: the
/// child is a fresh `run` that replans from the workflow file, so a parent
/// invocation restricted with `--samples` / `--target` / `--from-rule` /
/// `--module` / `--profile` would otherwise verify against the WHOLE DAG —
/// executing hours of rules the user never requested and blaming the AI fix
/// for unrelated failures. Execution-semantics flags (`--timeout`,
/// `--retry`, `--max-threads`, `--max-memory`) ride along so the
/// verification runs under the parent's limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RerunContext {
    /// The parent's effective workdir when it differs from the child's
    /// default (the workflow's directory): an explicit `--workdir`, a
    /// repository run's cwd default, or a resume's recorded workdir. Without
    /// this the child would execute rules in the wrong cwd while loading the
    /// parent's checkpoint.
    pub workdir: Option<PathBuf>,
    /// Config override tokens (the merged `KEY=VALUE` / `--KEY=VALUE` /
    /// `--KEY VALUE` positionals) forwarded verbatim, so the child re-derives
    /// the same config — and therefore the same rule fingerprints — instead
    /// of invalidating every config-referencing rule.
    pub config_args: Vec<String>,
    /// Forward `--provenance` when the parent tracked output checksums, so
    /// the rerun's re-executed rules record them too.
    pub provenance: bool,
    /// Forward `--jobs <n>` when the parent ran with explicit concurrency
    /// other than the default 1.
    pub jobs: Option<usize>,
    /// Forward `--keep-going` when the parent tolerated failures, so a
    /// non-required failure in the rerun does not abort it early.
    pub keep_going: bool,
    /// The parent's `--samples` entries (plan-shaping: they select the
    /// fan-out domain).
    pub samples: Vec<String>,
    /// The parent's `--target` rules (plan-shaping: they select the
    /// execution closure).
    pub targets: Vec<String>,
    /// The parent's `--module` selections.
    pub modules: Vec<String>,
    /// The parent's `--from-rule` (downstream-closure selection).
    pub from_rule: Option<String>,
    /// The parent's `--profile` (fills config keys before planning).
    pub profile: Option<String>,
    /// The parent's per-job `--timeout` when non-default.
    pub timeout: Option<String>,
    /// The parent's `--retry` count when non-default.
    pub retry: Option<u32>,
    /// The parent's `--max-threads` cap when non-default.
    pub max_threads: Option<u32>,
    /// The parent's `--max-memory` cap when non-default.
    pub max_memory: Option<u64>,
}

/// Outcome of one verification-rerun decision (issue #1164).
pub(crate) struct VerificationRerun {
    /// Whether the rerun was actually attempted (guards and launch failures
    /// leave this false).
    pub attempted: bool,
    /// Whether the child completed the workflow (exit 0). Meaningful only
    /// when `attempted`.
    pub succeeded: bool,
    /// Human-readable detail: skip reason or exit code.
    pub detail: String,
}

/// Whether the current process is itself a verification rerun (loop guard).
pub(crate) fn loop_guard_active() -> bool {
    std::env::var_os(REPAIR_RERUN_ENV).is_some()
}

/// Whether this run invocation must acquire the exclusive workdir lock.
///
/// Review-B1 exception (issue #1164): the verification rerun spawned by the
/// repair loop carries the [`REPAIR_RERUN_ENV`] guard while its parent holds
/// the workdir lock (flock on `.oxo-flow/lock`) for the whole run and is
/// blocked awaiting the child — no other writer can exist for the
/// checkpoint's lifetime, and a flock cannot be shared across processes, so
/// skipping the acquisition is the only way the child can run at all. The
/// parent never releases its own lock mid-run.
pub(crate) fn workdir_lock_required() -> bool {
    workdir_lock_required_checked(loop_guard_active())
}

/// [`workdir_lock_required`] with the guard injected — the explicit form
/// lets tests exercise both branches without mutating the process
/// environment (edition-2024 `set_var` is unsafe and races parallel tests).
pub(crate) fn workdir_lock_required_checked(rerun_guard: bool) -> bool {
    !rerun_guard
}

/// The exact argv of the verification rerun child: `run <workflow>
/// --resume-failed --no-ai-recover --checkpoint <p>` plus the forwarded
/// parent context ([`RerunContext`]). Pure so tests can pin the contract.
///
/// `--resume-failed` clears the failed rules so they re-execute while
/// succeeded rules stay skipped (resume semantics). `--no-ai-recover` both
/// disables AI recovery inside the child (no recursion through the recovery
/// path) and, together with the [`REPAIR_RERUN_ENV`] guard, makes a second
/// verification rerun impossible. Override tokens come LAST — run's
/// overrides positional is `trailing_var_arg`, so everything after the first
/// `KEY=VALUE` token is override text.
pub(crate) fn build_rerun_args(
    workflow: &Path,
    checkpoint_path: &Path,
    extra: &RerunContext,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        workflow.display().to_string(),
        "--resume-failed".to_string(),
        "--no-ai-recover".to_string(),
        "--checkpoint".to_string(),
        checkpoint_path.display().to_string(),
    ];
    if let Some(wd) = &extra.workdir {
        args.push("--workdir".to_string());
        args.push(wd.display().to_string());
    }
    if extra.provenance {
        args.push("--provenance".to_string());
    }
    if let Some(jobs) = extra.jobs {
        args.push("--jobs".to_string());
        args.push(jobs.to_string());
    }
    if extra.keep_going {
        args.push("--keep-going".to_string());
    }
    for sample in &extra.samples {
        args.push("--samples".to_string());
        args.push(sample.clone());
    }
    for target in &extra.targets {
        args.push("--target".to_string());
        args.push(target.clone());
    }
    for module in &extra.modules {
        args.push("--module".to_string());
        args.push(module.clone());
    }
    if let Some(rule) = &extra.from_rule {
        args.push("--from-rule".to_string());
        args.push(rule.clone());
    }
    if let Some(profile) = &extra.profile {
        args.push("--profile".to_string());
        args.push(profile.clone());
    }
    if let Some(timeout) = &extra.timeout {
        args.push("--timeout".to_string());
        args.push(timeout.clone());
    }
    if let Some(retry) = extra.retry {
        args.push("--retry".to_string());
        args.push(retry.to_string());
    }
    if let Some(threads) = extra.max_threads {
        args.push("--max-threads".to_string());
        args.push(threads.to_string());
    }
    if let Some(memory) = extra.max_memory {
        args.push("--max-memory".to_string());
        args.push(memory.to_string());
    }
    args.extend(extra.config_args.iter().cloned());
    args
}

/// Attempt the ONE bounded verification rerun (issue #1164 R1): resolve the
/// current binary, spawn the child run against the SAME checkpoint with the
/// parent's context forwarded ([`RerunContext`]), and wait for it with
/// inherited stdio so the verification run's logs interleave with the
/// parent's narration. The child carries [`REPAIR_RERUN_ENV`] so it can
/// never start a second rerun.
pub(crate) async fn verification_rerun(
    workflow: &Path,
    checkpoint_path: &Path,
    extra: &RerunContext,
) -> VerificationRerun {
    verification_rerun_checked(workflow, checkpoint_path, loop_guard_active(), extra).await
}

/// [`verification_rerun`] with the loop guard injected — the explicit-guard
/// form exists so tests can exercise the skip branch without mutating the
/// process environment (edition-2024 `set_var` is unsafe and races parallel
/// tests).
pub(crate) async fn verification_rerun_checked(
    workflow: &Path,
    checkpoint_path: &Path,
    guard_active: bool,
    extra: &RerunContext,
) -> VerificationRerun {
    if guard_active {
        return VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: "already a verification rerun (loop guard)".into(),
        };
    }
    // Binary resolution: an explicit `OXO_FLOW_BIN` pin wins (the same knob
    // the web server/desktop use to spawn runs — reuse, never retype), then
    // the running executable itself.
    let exe = match std::env::var_os("OXO_FLOW_BIN") {
        Some(bin) if !bin.is_empty() => PathBuf::from(bin),
        _ => match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                return VerificationRerun {
                    attempted: false,
                    succeeded: false,
                    detail: format!("cannot resolve the current executable: {e}"),
                };
            }
        },
    };
    // The workflow path is absolutized against the inherited cwd so the
    // child resolves the same file even if a future caller changes directory
    // before the spawn.
    let workflow = if workflow.is_absolute() {
        workflow.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(workflow))
            .unwrap_or_else(|_| workflow.to_path_buf())
    };
    let status = tokio::process::Command::new(&exe)
        .args(build_rerun_args(&workflow, checkpoint_path, extra))
        .env(REPAIR_RERUN_ENV, "1")
        .status()
        .await;
    match status {
        Ok(status) if status.success() => VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        },
        Ok(status) => VerificationRerun {
            attempted: true,
            succeeded: false,
            detail: format!(
                "verification rerun exited {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "by signal".into())
            ),
        },
        Err(e) => VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: format!("verification rerun could not be launched: {e}"),
        },
    }
}

/// The `REPAIR_APPLIED` narration line (issue #1164 contract): the AI fix
/// landed, carrying the diagnosis session id that links the archived session
/// and the before-fix workflow snapshot.
pub(crate) fn applied_narration(session_id: &str) -> String {
    format!("REPAIR_APPLIED session_id={session_id}")
}

/// The `REPAIR_RERUN_*` narration line for a rerun outcome (issue #1164
/// contract) — exactly one of SUCCEEDED / FAILED / SKIPPED.
pub(crate) fn rerun_narration(rerun: &VerificationRerun) -> String {
    if !rerun.attempted {
        format!("REPAIR_RERUN_SKIPPED {}", rerun.detail)
    } else if rerun.succeeded {
        format!("REPAIR_RERUN_SUCCEEDED {}", rerun.detail)
    } else {
        format!("REPAIR_RERUN_FAILED {}", rerun.detail)
    }
}

/// Emit one machine-readable repair-contract line: plain text on stderr
/// (never colored, never quiet-suppressed — the eval harness parses it) plus
/// a plain-text copy in the archived run log.
pub(crate) fn narrate_repair_line(line: &str, run_log: Option<&SharedRunLog>) {
    narrate_console_line(format_args!("{line}"));
    if let Some(log) = run_log {
        write_run_log_line(log, line);
    }
}

/// Persist the repair record after the fix is written (issue #1164 R2):
/// `applied = true`, rerun outcome still pending.
pub(crate) async fn record_repair_applied(
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    session_id: &str,
) {
    let record = oxo_flow_core::executor::checkpoint::RepairRecord {
        applied: true,
        session_id: session_id.to_string(),
        rerun_attempted: false,
        rerun_succeeded: false,
        detail: "AI fix applied; verification rerun pending".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    persist_repair_record(checkpoint, checkpoint_path, record).await;
}

/// Persist the rerun outcome into the repair record (issue #1164 R2).
/// Overwrites the pending record written by [`record_repair_applied`]; the
/// session id is passed through so the record stays linked even if the
/// apply-time save was lost.
pub(crate) async fn record_repair_rerun(
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    session_id: &str,
    rerun: &VerificationRerun,
) {
    let record = oxo_flow_core::executor::checkpoint::RepairRecord {
        applied: true,
        session_id: session_id.to_string(),
        rerun_attempted: rerun.attempted,
        rerun_succeeded: rerun.succeeded,
        detail: rerun.detail.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    persist_repair_record(checkpoint, checkpoint_path, record).await;
}

/// Write the repair record onto the checkpoint FILE (review B2, issue
/// #1164): the persisted state is authoritative — after the verification
/// rerun the child has rewritten the checkpoint with the FINAL rule records
/// (completed/failed, benchmarks, tails) while the parent's in-memory copy
/// still describes the failed parent run. Serializing the in-memory state
/// would clobber the child's outcome, so only the loaded file's state gets
/// the `repair_record` set and saved; the parent's in-memory copy is updated
/// in place for in-process readers but is never written after the child ran.
///
/// Record-write semantics when a checkpoint already exists at
/// `checkpoint_path` (issue #1352 F4): the file is loaded and the record is
/// appended ADDITIVELY — every prior field (completion records, benchmarks,
/// provenance) is preserved. The checkpoint at the run's checkpoint path is
/// the workdir's run ledger, and a repair that patched the workflow is a real
/// ledger event even when the gate aborted before any rule ran (the
/// admission-abort path). An EXISTING but unreadable file is never clobbered:
/// the write is skipped (record kept in the in-memory copy only) so a torn or
/// corrupt checkpoint survives for the next run's refuse-to-start gate to
/// surface, instead of being replaced by a synthetic state.
async fn persist_repair_record(
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    record: oxo_flow_core::executor::checkpoint::RepairRecord,
) {
    // Load the authoritative file. When it cannot be read:
    //   * absent file → the caller's baseline is the fresh ledger;
    //   * existing-but-unreadable file → never clobber evidence; keep the
    //     record in the in-memory copy only.
    let mut authoritative = match CheckpointState::load_from_file(checkpoint_path) {
        Ok(ck) => ck,
        Err(_) if !checkpoint_path.exists() => checkpoint.lock().await.clone(),
        Err(load_err) => {
            tracing::warn!(
                error = %load_err,
                checkpoint = %checkpoint_path.display(),
                "checkpoint exists but cannot be loaded — repair_record kept out of the \
                 file to avoid clobbering unreadable evidence"
            );
            checkpoint.lock().await.record_repair(record);
            return;
        }
    };
    authoritative.record_repair(record.clone());
    if let Err(e) = authoritative.save_to_file_async(checkpoint_path).await {
        tracing::warn!(error = %e, "failed to persist repair_record into the checkpoint");
    }
    checkpoint.lock().await.record_repair(record);
}

/// A stable session id for a reviewed-patch apply (issue #1302): the
/// file-only path has no AI session, but the REPAIR_* contract and the
/// checkpoint `repair_record` both carry a `session_id`, so the caller-side
/// reviewed patch uses this fixed label.
pub(crate) const REVIEWED_PATCH_SESSION_ID: &str = "reviewed-patch";

/// Outcome of the shared apply+verify path (issues #1164 / #1302).
pub(crate) enum RepairApplyOutcome {
    /// `apply_fix` rejected or failed; the workflow is left unchanged.
    Failed(String),
    /// The fix landed but the verification rerun did not complete the
    /// workflow (skipped, or exited non-zero) — the caller keeps its
    /// failure path.
    AppliedNotVerified,
    /// The fix landed AND the verification rerun completed the workflow.
    Verified,
}

/// Apply a repair document through the SAME verified-repair path the
/// AI-decided fix uses, reused verbatim by `--apply-patch` (issue #1302) so
/// the two surfaces can never drift: `apply_fix` (validate -> archive ->
/// atomic write), the `REPAIR_APPLIED` line, ONE bounded verification rerun,
/// the `REPAIR_RERUN_*` line, and the persisted `repair_record` — nothing is
/// retyped. On a verified completion the caller's `json_summary` receives the
/// `completed` + `repair` document.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_and_verify_repair(
    workflow: &Path,
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    modified_toml: &str,
    session_id: &str,
    run_log: &SharedRunLog,
    rerun_ctx: &RerunContext,
    json_summary: &mut RunJsonSummary,
    skipped_count: &std::sync::atomic::AtomicUsize,
) -> RepairApplyOutcome {
    use colored::Colorize as _;
    if let Err(e) = crate::commands::ai_recover::apply_fix(workflow, modified_toml, session_id) {
        return RepairApplyOutcome::Failed(e.to_string());
    }
    narrate_repair_line(&applied_narration(session_id), Some(run_log));
    record_repair_applied(checkpoint, checkpoint_path, session_id).await;
    let rerun = verification_rerun(workflow, checkpoint_path, rerun_ctx).await;
    narrate_repair_line(&rerun_narration(&rerun), Some(run_log));
    record_repair_rerun(checkpoint, checkpoint_path, session_id, &rerun).await;
    if rerun.attempted && rerun.succeeded {
        progress_narrate(
            format_args!("{} repair verified — rerun succeeded", "✓".green()),
            run_log,
        );
        let final_ck = match CheckpointState::load_from_file(checkpoint_path) {
            Ok(ck) => ck,
            Err(_) => checkpoint.lock().await.clone(),
        };
        json_summary.set_repair(RunRepairSummary {
            applied: true,
            session_id: session_id.to_string(),
            rerun_attempted: true,
            rerun_succeeded: true,
        });
        json_summary.emit(
            "completed",
            &RunCounts {
                succeeded: final_ck.completed_rules.len(),
                skipped: skipped_count.load(std::sync::atomic::Ordering::Relaxed),
                failed: 0,
                non_required_failed: final_ck.failed_rules.len(),
                blocked: 0,
            },
            rule_resource_rows(&final_ck),
        );
        RepairApplyOutcome::Verified
    } else {
        RepairApplyOutcome::AppliedNotVerified
    }
}

/// Outcome of routing the BioType type-admission bail through the repair
/// loop (issue #1265).
pub(crate) enum AdmissionRepair {
    /// The error is not the type-admission bail, no AI provider resolved, or
    /// no patch was written — the caller keeps the historical fail-fast path
    /// byte-for-byte.
    NotApplicable,
    /// A patch was written but could not be verified (diagnosis /
    /// authorization / validation failed, or the patched workflow still fails
    /// the gate). The caller fails the run with this error.
    NotCleared(anyhow::Error),
    /// A patch was written and the patched workflow clears the type gate; the
    /// caller continues the run with the re-derived plan.
    Cleared {
        config: Box<WorkflowConfig>,
        dag: Box<WorkflowDag>,
        target: Vec<String>,
    },
}

/// The ONE recovery-authorization predicate (issue #1352 F2): recovery runs
/// when the caller asked for it (`--ai-recover`) or the workflow's `[ai]
/// enabled` auto-detect turns it on, and the `--no-ai-recover` loop guard is
/// off. This exact expression used to live twice — `failure.rs`'s
/// `should_recover` and [`admission_repair_enabled`]'s tail — so a future
/// change to recovery authorization could update one site and not the other
/// ("don't mirror semantics — share them"). `workflow_auto_enabled` is the
/// precomputed `should_use_ai(Some(workflow), false)` so the predicate stays
/// pure and unit-testable.
pub(crate) fn ai_recovery_enabled(
    ai_recover: bool,
    no_ai_recover: bool,
    workflow_auto_enabled: bool,
) -> bool {
    !no_ai_recover && (ai_recover || workflow_auto_enabled)
}

/// Pure eligibility gate for [admission repair] (issue #1265): the error must
/// be the type-admission bail AND AI recovery must be enabled — the shared
/// [`ai_recovery_enabled`] predicate, not a private re-expression of it.
/// Extracted so the "recovery off → unchanged" contract is unit-testable
/// without a live provider.
pub(crate) fn admission_repair_enabled(
    error_message: &str,
    ai_recover: bool,
    no_ai_recover: bool,
    workflow_ai_enabled: bool,
) -> bool {
    error_message.contains(TYPE_ADMISSION_MARKER)
        && ai_recovery_enabled(ai_recover, no_ai_recover, workflow_ai_enabled)
}

/// Route a BioType type-admission abort (issue #1265) through the SAME
/// verified-repair machinery the rule-failure path uses (`failure.rs`): the
/// gate aborts in preflight BEFORE any rule runs, so without this the
/// `admission_abort` residual is structurally unrecoverable. Only the
/// `TYPE_ADMISSION_MARKER` bail is eligible.
///
/// Unlike the rule-failure path there is no `RerunContext` and no run log yet
/// (both materialize after this seam), so verification is IN-PROCESS: after a
/// written patch the workflow is re-parsed and re-entered through
/// [`build_dag_and_targets`] AT MOST ONCE, and the patch only "clears" when the
/// patched workflow passes `validate_workflow_types`. Nothing is spawned.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn repair_admission_abort(
    workflow: &Path,
    error: &anyhow::Error,
    target: Vec<String>,
    module: Vec<String>,
    config_args: Vec<String>,
    workdir_flag: Option<&Path>,
    discovery_fallback: Option<&Path>,
    profile: Option<&str>,
    checkpoint_path: &Path,
    samples_filter: &[String],
    samples_base_dir: &Path,
    ai_recover: bool,
    no_ai_recover: bool,
) -> AdmissionRepair {
    // Eligibility: only the type-admission bail, and only when AI recovery is
    // on. With recovery off the caller's fail-fast stays byte-for-byte.
    let message = error.to_string();
    if !admission_repair_enabled(
        &message,
        ai_recover,
        no_ai_recover,
        crate::commands::ai_template::should_use_ai(Some(workflow), false),
    ) {
        return AdmissionRepair::NotApplicable;
    }
    // No `RerunContext` exists this early, so narrate without a run log. The
    // in-memory baseline is the checkpoint FILE where one already exists —
    // the gate aborted in preflight, no rule ran, and the baseline must never
    // pretend otherwise or orphan a previous run's records. Semantics (issue
    // #1352 F4): the checkpoint at `checkpoint_path` is the workdir's run
    // ledger; the repair record is appended additively to whatever state is
    // already there (exactly like the rule-failure path — see
    // [`persist_repair_record`]), and a fresh ledger is started only when no
    // checkpoint exists yet. An existing-but-unreadable file is never
    // clobbered with this baseline.
    let checkpoint: Mutex<CheckpointState> = Mutex::new(
        CheckpointState::load_from_file(checkpoint_path).unwrap_or_else(|_| CheckpointState::new()),
    );
    let run_log: Option<&SharedRunLog> = None;

    let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(workflow), true) else {
        return AdmissionRepair::NotApplicable;
    };
    // The gate aborts before any rule is named; a stable synthetic sentinel
    // identifies the abort, and the full diagnostic text is the stderr signal
    // the diagnosis model reasons over.
    let rule = "type-admission-gate";
    let diag =
        match crate::commands::ai_recover::diagnose_failure(workflow, rule, 1, &message, &provider)
            .await
        {
            Ok(d) => d,
            Err(e) => return AdmissionRepair::NotCleared(e),
        };
    let Some(toml) = diag.modified_toml.as_deref() else {
        narrate_console_line(format_args!(
            "  AI fix not auto-applied: the proposal carried no corrected workflow. Review the suggestion manually."
        ));
        return AdmissionRepair::NotApplicable;
    };
    // Authorization — same layered decision as the rule-failure path.
    let mode = crate::commands::ai_recover::resolve_auto_fix_mode(workflow);
    let decision = crate::commands::ai_recover::auto_fix_decision(mode, diag.safe_to_auto_apply);
    let apply_ready = match decision {
        crate::commands::ai_recover::AutoFixDecision::ShowOnly(reason) => {
            narrate_console_line(format_args!(
                "  AI fix not auto-applied ({reason}): review the suggestion manually."
            ));
            false
        }
        crate::commands::ai_recover::AutoFixDecision::Prompt => {
            let interactive = {
                use std::io::IsTerminal as _;
                std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
            };
            if !interactive {
                narrate_console_line(format_args!(
                    "  AI fix not auto-applied: auto_fix = \"ask\" needs confirmation, but this session is not interactive. Review the suggestion manually or set [ai] auto_fix = \"always\" in the workflow file."
                ));
                false
            } else {
                crate::commands::ai_recover::confirm_auto_fix_blocking()
            }
        }
        crate::commands::ai_recover::AutoFixDecision::Apply => true,
    };
    if !apply_ready {
        return AdmissionRepair::NotApplicable;
    }
    // Validation gate: the generated TOML must parse before it is written.
    if let Err(e) = oxo_flow_core::WorkflowConfig::parse(toml) {
        narrate_console_line(format_args!(
            "  AI fix not auto-applied: generated TOML failed validation ({e}). Review the suggestion manually."
        ));
        return AdmissionRepair::NotApplicable;
    }
    if let Err(e) = crate::commands::ai_recover::apply_fix(workflow, toml, &diag.session_id) {
        narrate_console_line(format_args!("  Failed to apply AI fix: {e}"));
        return AdmissionRepair::NotApplicable;
    }
    // A patch is now written: emit the repair contract and persist it.
    narrate_repair_line(&applied_narration(&diag.session_id), run_log);
    record_repair_applied(&checkpoint, checkpoint_path, &diag.session_id).await;

    // Re-enter AT MOST ONCE, in-process: re-parse the patched workflow and
    // rebuild the plan. `build_dag_and_targets` re-runs the type gate itself,
    // so clearing it here IS the verification that `validate_workflow_types`
    // passes on the patched workflow.
    let rerun = match rebuild_and_clear(
        workflow,
        target,
        module,
        config_args,
        workdir_flag,
        discovery_fallback,
        profile,
        samples_filter,
        samples_base_dir,
    ) {
        Ok((config, dag, target)) => {
            let rerun = VerificationRerun {
                attempted: true,
                succeeded: true,
                detail: "patched workflow clears the type-admission gate".into(),
            };
            narrate_repair_line(&rerun_narration(&rerun), run_log);
            record_repair_rerun(&checkpoint, checkpoint_path, &diag.session_id, &rerun).await;
            narrate_console_line(format_args!(
                "{} repair verified — patched workflow clears the type gate",
                "✓".green()
            ));
            return AdmissionRepair::Cleared {
                config: Box::new(config),
                dag: Box::new(dag),
                target,
            };
        }
        Err(e) => VerificationRerun {
            attempted: true,
            succeeded: false,
            detail: format!("patched workflow still fails the gate: {e}"),
        },
    };
    narrate_repair_line(&rerun_narration(&rerun), run_log);
    record_repair_rerun(&checkpoint, checkpoint_path, &diag.session_id, &rerun).await;
    AdmissionRepair::NotCleared(anyhow::anyhow!(
        "AI repair wrote a patch but the workflow still fails the type-admission gate:\n  {}",
        rerun.detail
    ))
}

/// Re-derive the plan from a just-patched workflow (issue #1265), mirroring
/// the pre-seam prep block in `mod.rs` (the parse → CLI overrides → samples
/// filter → profile → defaults → runtime_workdir → expansion → DAG sequence).
/// Every STEP calls the shared helper `mod.rs` calls — the helpers are
/// borrowed, never retyped; the ORDER is the one deliberate second copy, and
/// this doc comment is its divergence ledger (issue #1352 F3):
///
/// - **`warn_host_resource_fit` is omitted**: it is advisory narration about
///   the SUBMIT host's capacity, already emitted for the original plan
///   earlier in the same `run` invocation — re-emitting it after an in-process
///   patch would double-warn the same facts, and a cluster-routed profile
///   suppresses it there too.
/// - **The `[resource_budget] max_jobs` `-j` clamp is omitted**: it mutates
///   the caller's `jobs` variable, which happened BEFORE the type gate could
///   abort (the clamp precedes `build_dag_and_targets` in `mod.rs`), so the
///   repaired plan runs under the already-clamped concurrency; the seam does
///   not own `-j` and cannot re-clamp it.
/// - The samples-filter readiness section is also not re-printed: the plan
///   rebuild is internal to the repair, and the section belongs to the
///   user-facing planning output that already ran.
///
/// Returns the rebuilt config, DAG, and resolved target list, or the gate
/// error when the patch did not clear it.
#[allow(clippy::too_many_arguments)]
fn rebuild_and_clear(
    workflow: &Path,
    target: Vec<String>,
    module: Vec<String>,
    config_args: Vec<String>,
    workdir_flag: Option<&Path>,
    discovery_fallback: Option<&Path>,
    profile: Option<&str>,
    samples_filter: &[String],
    samples_base_dir: &Path,
) -> Result<(WorkflowConfig, WorkflowDag, Vec<String>)> {
    let mut config =
        WorkflowConfig::from_file_with_discovery_fallback(workflow, discovery_fallback)
            .with_context(|| format!("failed to parse patched {}", workflow.display()))?;
    apply_cli_override_phase(&mut config, workflow, config_args, "run")?;
    // Mirror mod.rs: apply the `--samples` selection BEFORE expansion so the
    // rebuilt plan fans out the SAME subset the user asked for. Dropping this
    // would silently run the full cohort after a repair (issue #1265 review P2,
    // mirroring the pre-seam block in mod.rs). The shared helper is called, not
    // retyped, so execution and preview can never drift (reuse invariant).
    if !samples_filter.is_empty() {
        let _ = crate::commands::samples::apply_samples_filter(
            &mut config,
            samples_filter,
            true,
            samples_base_dir,
        )?;
    }
    if let Some(profile_name) = profile {
        let workflow_dir = normalize_dots(absolutize(oxo_flow_core::parent_dir(workflow))?);
        crate::commands::run_preview::merge_profile(&mut config, profile_name, &workflow_dir)?;
    }
    config.apply_defaults();
    // Mirror mod.rs: leaf shells run with cwd = workdir, so an explicit
    // --workdir makes expansion emit workflow-absolute input paths.
    if let Some(effective_workdir) = workdir_flag {
        config.runtime_workdir = Some(normalize_dots(absolutize(effective_workdir)?));
    }
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;
    let (dag, target) = build_dag_and_targets(&config, target, &module)?;
    Ok((config, dag, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The narration prefixes are a cross-agent contract (eval harness);
    /// pin them verbatim.
    #[test]
    fn narration_prefixes_are_stable() {
        assert_eq!(
            applied_narration("sess-1"),
            "REPAIR_APPLIED session_id=sess-1"
        );
        let ok = VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        };
        assert!(rerun_narration(&ok).starts_with("REPAIR_RERUN_SUCCEEDED "));
        let failed = VerificationRerun {
            attempted: true,
            succeeded: false,
            detail: "verification rerun exited 3".into(),
        };
        assert_eq!(
            rerun_narration(&failed),
            "REPAIR_RERUN_FAILED verification rerun exited 3"
        );
        let skipped = VerificationRerun {
            attempted: false,
            succeeded: false,
            detail: "already a verification rerun (loop guard)".into(),
        };
        assert_eq!(
            rerun_narration(&skipped),
            "REPAIR_RERUN_SKIPPED already a verification rerun (loop guard)"
        );
    }

    /// The rerun argv contract (review R3): fixed flags first, forwarded
    /// parent context conditionally, override tokens LAST (the overrides
    /// positional is trailing_var_arg — nothing may follow them). The
    /// plan-shaping flags (`--samples`/`--target`/`--module`/`--from-rule`/
    /// `--profile`) and the execution-semantics caps ride along so the
    /// verification rerun replans the SAME plan the parent ran — a
    /// restricted parent must never "verify" by executing the whole DAG.
    #[test]
    fn rerun_argv_pins_forwarded_parent_context() {
        let ctx = RerunContext {
            workdir: Some(PathBuf::from("/data/run")),
            config_args: vec!["threads=8".into(), "--api-token".into(), "secret".into()],
            provenance: true,
            jobs: Some(8),
            keep_going: true,
            samples: vec!["first:10".into(), "tumor_1".into()],
            targets: vec!["align".into()],
            modules: vec!["qc".into()],
            from_rule: Some("trim".into()),
            profile: Some("small".into()),
            timeout: Some("3600".into()),
            retry: Some(2),
            max_threads: Some(16),
            max_memory: Some(32768),
        };
        let args = build_rerun_args(
            Path::new("/w/wf.oxoflow"),
            Path::new("/w/.oxo-flow/checkpoint.json"),
            &ctx,
        );
        assert_eq!(
            args,
            vec![
                "run",
                "/w/wf.oxoflow",
                "--resume-failed",
                "--no-ai-recover",
                "--checkpoint",
                "/w/.oxo-flow/checkpoint.json",
                "--workdir",
                "/data/run",
                "--provenance",
                "--jobs",
                "8",
                "--keep-going",
                "--samples",
                "first:10",
                "--samples",
                "tumor_1",
                "--target",
                "align",
                "--module",
                "qc",
                "--from-rule",
                "trim",
                "--profile",
                "small",
                "--timeout",
                "3600",
                "--retry",
                "2",
                "--max-threads",
                "16",
                "--max-memory",
                "32768",
                "threads=8",
                "--api-token",
                "secret",
            ]
        );
    }

    /// A parent that used nothing non-default spawns the minimal argv: no
    /// `--workdir`, `--provenance`, `--jobs`, `--keep-going`, or overrides.
    #[test]
    fn rerun_argv_is_minimal_for_a_default_parent() {
        let args = build_rerun_args(
            Path::new("wf.oxoflow"),
            Path::new(".oxo-flow/checkpoint.json"),
            &RerunContext::default(),
        );
        assert_eq!(
            args,
            vec![
                "run",
                "wf.oxoflow",
                "--resume-failed",
                "--no-ai-recover",
                "--checkpoint",
                ".oxo-flow/checkpoint.json",
            ]
        );
    }

    /// The loop guard must short-circuit before any spawn: with the guard
    /// active, the rerun is skipped and no child process is started.
    #[tokio::test]
    async fn loop_guard_blocks_a_second_rerun() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = tmp.path().join("wf.oxoflow");
        std::fs::write(&wf, "[workflow]\nname = \"t\"\n").unwrap();
        let ck_path = tmp.path().join("checkpoint.json");

        let rerun = verification_rerun_checked(&wf, &ck_path, true, &RerunContext::default()).await;
        assert!(!rerun.attempted, "guarded rerun must not spawn a child");
        assert!(!rerun.succeeded);
        assert!(rerun.detail.contains("loop guard"));
        // The skip must be narratable as the contract's SKIPPED line.
        assert!(rerun_narration(&rerun).starts_with("REPAIR_RERUN_SKIPPED "));
    }

    /// Review B1: the guard that lets a verification rerun skip the
    /// workdir lock is exactly [`REPAIR_RERUN_ENV`] — the child must not
    /// deadlock against its parent's held flock.
    #[test]
    fn workdir_lock_is_skipped_under_the_rerun_guard() {
        assert!(
            !workdir_lock_required_checked(true),
            "a verification rerun must skip the workdir lock (its parent holds it)"
        );
        assert!(
            workdir_lock_required_checked(false),
            "a normal run must still acquire the workdir lock"
        );
        // The runtime gate composes the two exactly as the exception
        // requires: env guard present → skip, absent → acquire.
        assert_eq!(workdir_lock_required(), !loop_guard_active());
    }

    /// Decision → apply → rerun plumbing without a live provider (issue
    /// #1164 R5a): the `auto_fix = "always"` + safe verdict decision
    /// authorizes the write, `apply_fix` rewrites the poisoned workflow with
    /// the fixed TOML, and the rerun layer is then reached with the UPDATED
    /// workflow. The rerun itself fails to launch here (the test process is
    /// not the CLI binary), which the record must carry honestly as an
    /// attempted-but-failed rerun — the failed-workflow outcome semantics.
    #[test]
    fn decision_apply_then_rerun_reaches_the_updated_workflow() {
        use crate::commands::ai_recover::{apply_fix, auto_fix_decision};
        use oxo_flow_ai::config::AutoFixMode;

        let _cwd = crate::commands::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::Builder::new()
            .prefix("oxo-repair-loop-")
            .tempdir()
            .unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let _restore = crate::commands::RestoreCwd(prev);

        let poisoned =
            "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"boom\"\nshell = \"exit 1\"\n";
        let fixed =
            "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"boom\"\nshell = \"echo fixed\"\n";
        std::fs::write("wf.oxoflow", poisoned).unwrap();

        // Decision plumbing: `always` + a safe verdict = Apply (never
        // ShowOnly) — the only branch that reaches the writer.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Always, true),
            crate::commands::ai_recover::AutoFixDecision::Apply
        );
        apply_fix(Path::new("wf.oxoflow"), fixed, "sess-1164").unwrap();
        assert_eq!(
            std::fs::read_to_string("wf.oxoflow").unwrap(),
            fixed,
            "the fix must be written before the rerun is attempted"
        );

        // The rerun layer receives the UPDATED workflow. The guard is off,
        // so the spawn IS attempted; the child here is the test harness
        // binary, which rejects the `run` argv — asserted as an attempted,
        // failed rerun (exit code in the detail), not a skip.
        let rerun = futures_block_on(verification_rerun_checked(
            Path::new("wf.oxoflow"),
            &tmp.path().join("checkpoint.json"),
            false,
            &RerunContext::default(),
        ));
        assert!(rerun.attempted, "unguarded rerun must be attempted");
        assert!(!rerun.succeeded);
        assert!(rerun.detail.contains("exited"), "detail: {}", rerun.detail);
        assert!(rerun_narration(&rerun).starts_with("REPAIR_RERUN_FAILED "));
    }

    /// Block on an async call from a sync test without a full runtime
    /// dependency: the CLI tests already link tokio.
    fn futures_block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// The repair record round-trips through the shared checkpoint
    /// persistence: apply-then-rerun overwrites the pending record and the
    /// file on disk carries the final outcome.
    #[tokio::test]
    async fn repair_record_persists_apply_then_rerun() {
        let tmp = tempfile::tempdir().unwrap();
        let ck_path = tmp.path().join("checkpoint.json");
        let checkpoint = Mutex::new(CheckpointState::new());

        record_repair_applied(&checkpoint, &ck_path, "sess-persist").await;
        {
            let ck = checkpoint.lock().await;
            let record = ck.repair_record.as_ref().expect("applied record set");
            assert!(record.applied);
            assert!(!record.rerun_attempted);
            // The apply-time save landed on disk.
            let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
            assert!(loaded.repair_record.as_ref().expect("persisted").applied);
        }

        let rerun = VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        };
        record_repair_rerun(&checkpoint, &ck_path, "sess-persist", &rerun).await;
        let ck = checkpoint.lock().await;
        let record = ck.repair_record.as_ref().expect("rerun record set");
        assert_eq!(record.session_id, "sess-persist");
        assert!(record.rerun_attempted);
        assert!(record.rerun_succeeded);
        assert_eq!(record.detail, "verification rerun completed the workflow");
        drop(ck);
        let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
        let record = loaded.repair_record.expect("rerun outcome persisted");
        assert!(record.rerun_attempted && record.rerun_succeeded);
    }

    /// Review B2 regression: the repair-record write is a FILE merge — the
    /// parent's stale in-memory state (failed rule, poisoned command, no
    /// child benchmarks) must never be serialized over the child's final
    /// checkpoint; only `repair_record` changes.
    #[tokio::test]
    async fn repair_record_write_is_file_merge_not_in_memory_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let ck_path = tmp.path().join("checkpoint.json");

        // The child's final state on disk: `boom` completed with the FIXED
        // command and a benchmark.
        let mut child_final = CheckpointState::new();
        child_final.completed_rules.insert("boom".to_string());
        child_final.mark_completed(
            "boom",
            oxo_flow_core::executor::BenchmarkRecord {
                rule: "boom".into(),
                wall_time_secs: 0.25,
                max_memory_mb: Some(64),
                memory_limit_mb: None,
                cpu_seconds: Some(0.1),
                retries: 0,
                recorded_as: None,
            },
        );
        child_final.rule_runs.insert(
            "boom".to_string(),
            oxo_flow_core::executor::checkpoint::RuleRunRecord {
                exit_code: Some(0),
                started_at: None,
                finished_at: None,
                command: Some("echo fixed".into()),
                stderr_tail: None,
                stdout_tail: None,
                caption: None,
                status: None,
                skip_reason: None,
                signal: None,
            },
        );
        child_final.save_to_file_async(&ck_path).await.unwrap();

        // The parent's stale in-memory copy: `boom` still Failed.
        let mut stale = CheckpointState::new();
        stale.failed_rules.insert("boom".to_string());
        let checkpoint = Mutex::new(stale);

        let rerun = VerificationRerun {
            attempted: true,
            succeeded: true,
            detail: "verification rerun completed the workflow".into(),
        };
        record_repair_rerun(&checkpoint, &ck_path, "sess-b2", &rerun).await;

        // The FILE keeps the child's final rule state; only repair_record
        // was added.
        let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
        assert!(
            loaded.completed_rules.contains("boom") && loaded.failed_rules.is_empty(),
            "the child's completed state must survive the parent's record write"
        );
        assert!(
            loaded.benchmarks.contains_key("boom"),
            "the child's benchmarks must survive the parent's record write"
        );
        let record = loaded.repair_record.expect("repair record persisted");
        assert_eq!(record.session_id, "sess-b2");
        assert!(record.rerun_succeeded);
    }

    /// Issue #1352 F4 regression: the admission-abort seam's record write is
    /// a load-preserve APPEND onto an existing checkpoint — a reused workdir's
    /// previous-run ledger (completed rules, benchmarks) must survive the
    /// admission repair record exactly as the rule-failure path preserves the
    /// verification rerun's records.
    #[tokio::test]
    async fn admission_repair_record_preserves_a_preexisting_checkpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let ck_path = tmp.path().join("checkpoint.json");

        // A previous run's ledger on disk: `done` completed with a benchmark.
        let mut prev = CheckpointState::new();
        prev.completed_rules.insert("done".to_string());
        prev.mark_completed(
            "done",
            oxo_flow_core::executor::BenchmarkRecord {
                rule: "done".into(),
                wall_time_secs: 1.0,
                max_memory_mb: Some(32),
                memory_limit_mb: None,
                cpu_seconds: Some(0.5),
                retries: 0,
                recorded_as: None,
            },
        );
        prev.save_to_file_async(&ck_path).await.unwrap();

        // The admission seam's baseline (issue #1352 F4): loaded from the
        // file, never a fresh default that would orphan the previous run's
        // records.
        let checkpoint = Mutex::new(
            CheckpointState::load_from_file(&ck_path).unwrap_or_else(|_| CheckpointState::new()),
        );
        record_repair_applied(&checkpoint, &ck_path, "sess-admission").await;

        let loaded = CheckpointState::load_from_file(&ck_path).unwrap();
        assert!(
            loaded.completed_rules.contains("done"),
            "the previous run's completion records must survive the admission repair record"
        );
        assert!(
            loaded.benchmarks.contains_key("done"),
            "the previous run's benchmarks must survive the admission repair record"
        );
        let record = loaded
            .repair_record
            .expect("admission repair record appended");
        assert_eq!(record.session_id, "sess-admission");
        assert!(record.applied);
    }

    /// Issue #1352 F4 regression: an EXISTING but unreadable checkpoint is
    /// never clobbered by the record write — the unreadable evidence survives
    /// untouched for the next run's refuse-to-start gate to surface, and the
    /// record is kept in the in-memory copy only.
    #[tokio::test]
    async fn repair_record_never_clobbers_an_unreadable_checkpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let ck_path = tmp.path().join("checkpoint.json");
        let torn = "{\"completed_rules\": [\"done\""; // truncated mid-file
        std::fs::write(&ck_path, torn).unwrap();

        let checkpoint = Mutex::new(CheckpointState::new());
        record_repair_applied(&checkpoint, &ck_path, "sess-torn").await;

        let on_disk = std::fs::read_to_string(&ck_path).unwrap();
        assert_eq!(
            on_disk, torn,
            "an unreadable checkpoint must never be overwritten by the record write"
        );
        let ck = checkpoint.lock().await;
        let record = ck.repair_record.as_ref().expect("record kept in memory");
        assert_eq!(record.session_id, "sess-torn");
    }

    /// Issue #1265: the type-admission bail is the ONLY abort routed through
    /// the repair loop, and only when AI recovery is enabled. With recovery
    /// off the fail-fast is byte-for-byte unchanged.
    #[test]
    fn admission_repair_eligibility_matches_the_gate_and_the_recovery_flags() {
        let gate = format!("{TYPE_ADMISSION_MARKER} — fix before running:\n  TYPE-001 …");
        // Reachable: type-gate bail + --ai-recover.
        assert!(admission_repair_enabled(&gate, true, false, false));
        // Reachable: type-gate bail + `[ai] enabled = true`.
        assert!(admission_repair_enabled(&gate, false, false, true));
        // Recovery off (no flag, no [ai]) → unchanged fail-fast.
        assert!(!admission_repair_enabled(&gate, false, false, false));
        // --no-ai-recover always wins, even with recovery requested.
        assert!(!admission_repair_enabled(&gate, true, true, true));
        // A non-type bail (E005 undefined config var, unknown module, DAG
        // build) is never routed through repair.
        let other = "workflow references undefined config variables — fix before running:\n  x";
        assert!(!admission_repair_enabled(other, true, false, true));
    }

    /// Falsifiable plan-parity test for the issue #1265 review P2: the rebuilt
    /// plan MUST honor the same `--samples` selection the parent run applied
    /// before its plan was built. If `rebuild_and_clear` drops the filter,
    /// `expand_wildcards` fans out the FULL cohort and this test fails with a
    /// count of 3 (or a `sample_groups` length of 3).
    #[test]
    fn rebuild_and_clear_preserves_the_samples_filter() {
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("cohort.oxoflow");
        std::fs::write(
            &wf,
            r#"
            [workflow]
            name = "cohort"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2", "S3"]

            [[rules]]
            name = "align"
            input = ["raw/{sample}.fq"]
            output = ["aln/{sample}.bam"]
            shell = "touch {output}"
            "#,
        )
        .unwrap();

        // Unfiltered: the full cohort fans out to one instance per sample.
        let (full_cfg, full_dag, _) = rebuild_and_clear(
            &wf,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            &[],
            dir.path(),
        )
        .unwrap();
        assert_eq!(full_cfg.sample_groups[0].samples.len(), 3);
        assert_eq!(full_dag.node_count(), 3);

        // Filtered: `--samples S1` must survive the rebuild — one sample, one
        // instance. This assertion is what the P2 fix arms (it fails if the
        // filter is dropped, which left it at 3/3).
        let filter = vec!["S1".to_string()];
        let (cfg, dag, _) = rebuild_and_clear(
            &wf,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            &filter,
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            cfg.sample_groups[0].samples,
            vec!["S1".to_string()],
            "the --samples filter must survive rebuild_and_clear"
        );
        assert_eq!(
            dag.node_count(),
            1,
            "the rebuilt plan must fan out exactly the requested subset"
        );
    }
}
