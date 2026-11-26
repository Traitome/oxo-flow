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
use oxo_flow_core::executor::checkpoint::{
    BenchmarkRecord, CheckpointState, ClusterJobRecord, ClusterRunRecord,
};
use oxo_flow_core::executor::{JobRecord, JobStatus};
use oxo_flow_core::rule::Rule;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// `--provenance` bookkeeping for the cluster path (issue #1093 F1).
///
/// The local executor records output checksums as each rule completes; a
/// cluster checkpoint used to record none because the flag never reached
/// this path. [`fold_record`] now mirrors that recording at its settle
/// point, and this tracker carries what the fold needs to know about the
/// run: whether checksums were requested, how many were recorded (drives
/// the zero-result warning), and which rules were already hashed — the
/// same record is folded twice on the normal path (settle hook, then the
/// post-run loop) and outputs can be hundreds of GB.
///
/// Hashing itself runs OFF the driver's poll loop: the settle hook is
/// awaited inline between polls, so hashing a large output there would
/// stall scheduling, signal handling, and the submission of dependents.
/// Each eligible rule schedules one background task; [`Self::join_pending`]
/// settles them before the run reports its result.
pub(crate) struct ClusterProvenance {
    pub requested: bool,
    recorded: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    rules_hashed: std::sync::Mutex<HashSet<String>>,
    pending: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl ClusterProvenance {
    pub fn new(requested: bool) -> Self {
        Self {
            requested,
            recorded: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            rules_hashed: std::sync::Mutex::new(HashSet::new()),
            pending: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Checksums recorded so far. Only meaningful after [`Self::join_pending`].
    pub fn recorded(&self) -> usize {
        self.recorded.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Await every scheduled hashing task. Called before the run reports
    /// its result so the zero-checksum warning sees the final count.
    pub async fn join_pending(&self) {
        let handles: Vec<tokio::task::JoinHandle<()>> =
            std::mem::take(&mut *self.pending.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

/// Everything the cluster path needs from `run_command`'s scope.
pub(crate) struct ClusterRunArgs<'a> {
    pub config: &'a WorkflowConfig,
    pub dag: &'a WorkflowDag,
    pub order: &'a [String],
    pub checkpoint: &'a Arc<Mutex<CheckpointState>>,
    pub checkpoint_path: &'a Path,
    pub workdir: &'a Path,
    pub wildcard_values: &'a HashMap<String, String>,
    /// Profile name this cluster run was launched with (`--profile`, or the
    /// profile `resume` resolved from the checkpoint). Recorded in the
    /// checkpoint (issue #1427) so a later `resume` can re-resolve the same
    /// profile without asking the operator to retype it.
    pub profile_name: &'a str,
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
    /// `-k/--keep-going` (issue #1093 F2b): a required rule's failure is
    /// recorded and still fails the run, but must not cancel jobs already
    /// in flight — the driver finishes them and keeps dispatching ready
    /// independent instances until the queue drains, while dependents of
    /// the failed rule stay blocked. Interrupts and driver errors still
    /// cancel everything.
    pub keep_going: bool,
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
    /// `--provenance` / per-rule `checksum = "…"`: recorded by
    /// [`fold_record`] for every completed rule, mirroring the local
    /// executor (issue #1093 F1).
    pub provenance: &'a ClusterProvenance,
    /// Resolved run scratch directory (`[engine] tmp_dir` /
    /// `OXO_FLOW_TMPDIR`, issues #907 + #1013 C2): exported as
    /// `TMPDIR`/`TMP`/`TEMP` into every cluster job shell, matching the
    /// local executor's process-env injection.
    pub tmp_dir: Option<&'a Path>,
    /// Issue #1426: `resume`'s strict output verification (`false` for
    /// plain `run` and `--trust-mtime`). A rule the plan would skip is
    /// submitted instead when its recorded output manifest does not verify,
    /// so a resumed cluster checkpoint cannot reuse a damaged artifact
    /// merely because it exists.
    pub strict_output_verification: bool,
}

/// Outcome of a cluster run, mirroring what the local loop reports.
pub(crate) struct ClusterRunSummary {
    pub succeeded: usize,
    pub failed: usize,
    /// Failures of `required = false` rules: recorded and reported, but
    /// exempt from failing the run (issue #99 B2, cluster path).
    pub non_required_failed: usize,
    pub skipped: usize,
    /// `128 + signum` of the signal that interrupted the run (SIGINT→130,
    /// SIGTERM→143, SIGHUP→129), or `None` for an ordinary terminal
    /// outcome. Carried out of [`run_on_cluster`] so the caller ends the
    /// process the way the local executor does: an interrupt is never a
    /// successful run, even when every interrupted rule was
    /// `required = false`.
    pub interrupt_exit_code: Option<i32>,
}

impl ClusterRunSummary {
    pub fn is_success(&self) -> bool {
        self.failed == 0 && self.interrupt_exit_code.is_none()
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
            "unknown cluster backend '{backend_name}' — expected {}",
            oxo_flow_core::cluster::SUPPORTED_BACKENDS_HINT
        )
    })?;
    let volume = cluster.volume.clone();
    if let Some(volume) = &volume {
        volume.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
        if backend != ClusterBackend::Kubernetes {
            anyhow::bail!(
                "[cluster.volume] is only rendered by the kubernetes backend — \
                 backend '{backend_name}' would silently ignore it"
            );
        }
    }
    Ok((
        backend,
        ClusterJobConfig {
            backend,
            queue: cluster.partition.clone(),
            account: cluster.account.clone(),
            walltime: cluster.walltime.clone(),
            extra_args: cluster.extra_args.clone(),
            name_scope: None,
            volume,
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

/// `scratch = true` promises each instance a private scratch directory with
/// its outputs moved back on success. The scratch machinery (per-instance
/// dir selection via the storage-tier ladder, cwd swap, absolute input
/// rendering, output move-back) lives in the local executor only — the
/// cluster path would submit the job into the shared workdir, where tools
/// writing fixed filenames collide across concurrent instances
/// (issue #1013 C3). Reject at plan time, naming every offender, and point
/// at the path that does honor the directive.
fn reject_scratch_rules(plan: &ScheduledPlan, to_run: &HashSet<String>) -> Result<()> {
    let mut offenders: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue; // not schedulable — skipped later, nothing to submit
        };
        if sr.rule.scratch {
            offenders.push(name.clone());
        }
    }
    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "`scratch = true` cannot be honored on the cluster path — {} would \
         run directly in the shared workdir, where fixed-filename outputs \
         collide across concurrent instances; remove `scratch = true` or run \
         the workflow on the local path (`oxo-flow run`) instead:\n  {}",
        oxo_flow_core::plural(offenders.len(), "rule", "rules"),
        offenders.join("\n  ")
    ))
}

/// Issue #1078 F2: the local executor gates a rule's rendered command before
/// the shell spawns. The cluster path renders the job script itself, so the
/// same gate runs here over every command about to leave for the scheduler —
/// otherwise an unexpanded per-sample rule would ship a literal
/// `{meta.bam_genomic}` to a remote node and fail there.
fn reject_unbound_placeholders(
    plan: &ScheduledPlan,
    config: &WorkflowConfig,
    to_run: &HashSet<String>,
) -> Result<()> {
    let enabled = oxo_flow_core::config::resolve_placeholder_gate(
        config
            .engine
            .as_ref()
            .and_then(|engine| engine.strict_placeholders),
    );
    let mut offenders: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue;
        };
        let scanned = oxo_flow_core::wildcard::strip_shell_comments(&sr.shell_cmd);
        let outcome =
            oxo_flow_core::executor::placeholder_gate::evaluate([scanned.as_str()], enabled);
        if !outcome.fatal.is_empty() {
            offenders.push(format!("{name}: {}", outcome.tokens(&outcome.fatal)));
        }
    }
    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "unresolved placeholder(s) reached the rendered command of {} — the \
         engine bound no value for them, so the remote job would receive the \
         literal brace token. Fix the value source (`sample_pattern`, \
         `[[sample_groups]]`, `[[pairs]]`, `[[values]]`, or the `{{config…}}`/\
         `{{params…}}` declaration); a one-row metadata table is the constant \
         `{{meta.*}}` read (`singleton` does not soften this gate). The local \
         executor rejects the same commands; `OXO_FLOW_STRICT_PLACEHOLDERS=0` \
         downgrades the gate:\n  {}",
        oxo_flow_core::plural(offenders.len(), "rule", "rules"),
        offenders.join("\n  ")
    ))
}

/// Lifecycle hooks (rule-level `pre_exec` / `on_success` / `on_failure`,
/// workflow-level `on_complete` / `on_error`) are run by the local executor
/// only. The cluster script generators take just the bare shell command, so
/// a workflow that fires hooks locally would silently no-op them on a
/// scheduler with no diagnostic (issue #1340). Reject at plan time, naming
/// every offending rule and hook field, and point at the local path.
///
/// The hook fields are enumerated by `Rule::lifecycle_hooks` /
/// `WorkflowMeta::lifecycle_hooks` in core — the same definitions the local
/// executor's placeholder gate uses — so the set cannot drift (reuse, never
/// retype).
fn reject_lifecycle_hooks(
    plan: &ScheduledPlan,
    config: &WorkflowConfig,
    to_run: &HashSet<String>,
) -> Result<()> {
    let mut offenders: Vec<String> = Vec::new();
    for name in to_run {
        let Some(sr) = plan.rules.get(name) else {
            continue; // not schedulable — skipped later, nothing to submit
        };
        for (field, hook) in sr.rule.lifecycle_hooks() {
            if hook.is_some() {
                offenders.push(format!("{name}: `{field}`"));
            }
        }
    }
    for (field, hook) in config.workflow.lifecycle_hooks() {
        if hook.is_some() {
            offenders.push(format!("(workflow): `{field}`"));
        }
    }
    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "lifecycle hooks are not supported on the cluster path yet — the job \
         script generator renders only the command, so these hooks would be \
         silently dropped on the scheduler; remove them or run the workflow on \
         the local path (`oxo-flow run`) instead:\n  {}",
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

/// Schedule output-record collection for this rule instance: the always-on
/// declared-output manifest (issue #1426) plus, when requested, the
/// `--provenance` / per-rule `checksum = "…"` digests (issue #1093 F1).
///
/// The collection routines are the same ones the local executor calls
/// ([`oxo_flow_core::executor::output_checksums::collect_output_manifest`] /
/// [`oxo_flow_core::executor::output_checksums::collect_output_checksums`]),
/// so a cluster checkpoint carries identical entries (keys, algorithms) for
/// the same workflow.
///
/// It runs as a background task rather than inline at the settle point:
/// the driver awaits its settle hook between polls, and hashing a
/// multi-GB output there would stall scheduling, Ctrl-C handling, and the
/// submission of dependents — while the completion verdict is already
/// persisted by the time this is scheduled. Recording is deduplicated per
/// rule within one run (the record is folded twice), and the tasks are
/// joined by [`ClusterProvenance::join_pending`] before the run reports.
async fn schedule_output_records(args: &ClusterRunArgs<'_>, rule: &Rule, rule_name: &str) {
    {
        let mut hashed = match args.provenance.rules_hashed.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // Nothing in this block panics, but a poisoned lock must
                // never crash a settle path that is only de-duplicating
                // hashing work: recover the set and keep recording.
                tracing::warn!("provenance hash set lock was poisoned — recovering");
                poisoned.into_inner()
            }
        };
        if !hashed.insert(rule_name.to_string()) {
            return;
        }
    }
    let mut wildcard_scope: HashMap<String, String> = (*args.wildcard_values).clone();
    for (k, v) in args.config.instance_bindings(rule_name) {
        wildcard_scope.insert(k, v);
    }
    let wants_checksums = args.provenance.requested || rule.checksum.is_some();
    let rule = rule.clone();
    let workdir = args.workdir.to_path_buf();
    let checkpoint = args.checkpoint.clone();
    let checkpoint_path = args.checkpoint_path.to_path_buf();
    let recorded = args.provenance.recorded.clone();
    let count = args.provenance.requested;
    let rule_name = rule_name.to_string();
    let handle = tokio::spawn(async move {
        // File I/O off the async workers: outputs are routinely tens of GB.
        let (manifest, entries) = match tokio::task::spawn_blocking(move || {
            let manifest = oxo_flow_core::executor::output_checksums::collect_output_manifest(
                &rule,
                &workdir,
                &wildcard_scope,
                oxo_flow_core::executor::checkpoint::output_manifest_hash_max_bytes(),
            );
            let entries = if wants_checksums {
                oxo_flow_core::executor::output_checksums::collect_output_checksums(
                    &rule,
                    &workdir,
                    &wildcard_scope,
                )
            } else {
                Vec::new()
            };
            (manifest, entries)
        })
        .await
        {
            Ok(records) => records,
            Err(e) => {
                // A panicked collector must not disappear into a silent
                // default: the operator needs to know which rule lost its
                // provenance/manifest evidence. The completion verdict is
                // already saved and collection is best-effort, so the run
                // continues (strict `resume` will re-run this rule).
                tracing::warn!(
                    rule = %rule_name,
                    error = %e,
                    "output record collection failed — manifest/checksums for this rule were not recorded"
                );
                (Vec::new(), Vec::new())
            }
        };
        let recorded_here = entries.len();
        {
            let mut ck = checkpoint.lock().await;
            ck.record_output_manifest(&rule_name, manifest);
            for (path, checksum) in entries {
                ck.record_checksum(&path, checksum);
            }
            // Always save: the output manifest is recorded regardless of
            // whether provenance checksums were requested (issue #1426).
            if let Err(e) = ck.save_to_file_async(&checkpoint_path).await {
                tracing::warn!("Failed to save output records (manifest/checksums): {e}");
            }
        }
        if count {
            recorded.fetch_add(recorded_here, std::sync::atomic::Ordering::Relaxed);
        }
    });
    args.provenance.pending.lock().await.push(handle);
}

/// Fold one driver-returned record into the checkpoint, mirroring the local
/// loop's bookkeeping so `status`, `resume`, and re-runs cannot tell the two
/// execution paths apart.
///
/// Invoked twice per record on the normal path: once by the driver's settle
/// hook as the job settles (issue #1083 — a crash from here on must not lose
/// the verdict), and once by the post-run loop that also counts the summary.
/// Both writes are idempotent.
async fn fold_record(args: &ClusterRunArgs<'_>, record: &JobRecord) {
    // Skip-only records carry no completion credit. Return before taking
    // the lock: a blocked downstream wave can produce thousands of these,
    // and each one would otherwise pay a full checkpoint serialization.
    if !matches!(
        record.status,
        JobStatus::Success | JobStatus::Failed | JobStatus::Cancelled
    ) {
        return;
    }
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
            // Output records (issue #1093 F1 + #1426): the always-on output
            // manifest and, when requested, `--provenance` / per-rule
            // `checksum = "…"` digests are recorded from the same settle
            // point that persists completion. Scheduled off this path — the
            // verdict above is saved before any hashing starts, and the
            // driver's poll loop keeps running while outputs are hashed.
            if let Some(rule) = rule {
                schedule_output_records(args, rule, &record.rule).await;
            }
        }
        JobStatus::Failed => {
            ck.record_run(record, output_tail_bytes);
            ck.mark_failed(&record.rule);
        }
        // A signal-interrupted job arrives as Failed (handled above) — the
        // driver only ever produces Cancelled records from the scheduler
        // itself reporting a cancelled job, which counts as neither success
        // nor failure but must still persist to the checkpoint.
        JobStatus::Cancelled => {
            ck.record_run(record, output_tail_bytes);
            ck.mark_failed(&record.rule);
        }
        // Skipped records carry no completion credit — nothing to persist.
        _ => {}
    }
    // The job's fate is now recorded, so its scheduler id is no longer
    // evidence of a live submission (issue #1427). Persisted in the same
    // save as the terminal record: a resume can never see a charted job
    // whose verdict this checkpoint already holds.
    ck.clear_cluster_job(&record.rule);
    // M-7: the local scheduler loop saves through the async API at every
    // site; the cluster path must not serialize submissions behind fsync
    // latency either.
    if let Err(e) = ck.save_to_file_async(args.checkpoint_path).await {
        tracing::warn!("Failed to save checkpoint: {e}");
    }
}

/// Count one driver-returned record into the run summary.
///
/// Kept separate from [`fold_record`]: the settle hook persists state during
/// the run but must not increment counters that the post-run loop owns (the
/// same record arrives twice).
fn count_outcome(config: &WorkflowConfig, record: &JobRecord, summary: &mut ClusterRunSummary) {
    match record.status {
        JobStatus::Success => summary.succeeded += 1,
        JobStatus::Failed => {
            // A failed `required = false` rule is recorded and reported but
            // does not fail the run (issue #99 B2, cluster path) — the same
            // contract as the local loop.
            let is_required = config.get_rule(&record.rule).is_none_or(|r| r.required);
            if is_required {
                summary.failed += 1;
            } else {
                summary.non_required_failed += 1;
            }
        }
        // A signal-interrupted job arrives as Failed (handled above) — the
        // driver only ever produces Cancelled records from the scheduler
        // itself reporting a cancelled job, which counts as neither success
        // nor failure but must still persist to the checkpoint.
        JobStatus::Cancelled => summary.skipped += 1,
        _ => summary.skipped += 1,
    }
}

/// Fold one driver-returned record into the checkpoint and the summary.
async fn record_outcome(
    args: &ClusterRunArgs<'_>,
    record: &JobRecord,
    summary: &mut ClusterRunSummary,
) {
    count_outcome(args.config, record, summary);
    fold_record(args, record).await;
}

/// What this cluster run will submit, derived from the same preview
/// `dry-run` reports.
struct SubmissionPlan {
    to_run: HashSet<String>,
    /// Rules skipped because their declared outputs exist but the
    /// checkpoint holds no completion record. A killed job can leave
    /// exactly that shape (issue #1083) — the run narrates them instead of
    /// counting them silently as done.
    skipped_fresh: Vec<String>,
}

/// The will-run set: the preview's non-skip rules, unioned with the run's
/// own forced rules.
///
/// `force_rules` must be unioned here because by the time the cluster path
/// runs, `run_command` has already applied and persisted its invalidation
/// analysis — the cleared completion records and refreshed manifests the
/// preview would need to re-derive a cascade are gone, so a cascaded rule
/// whose outputs still look fresh classifies as a skip. `force_rules` is
/// that analysis's own output, and the local executor gets it for exactly
/// this purpose (see `ExecutorConfig.force_rules`), so consuming both here
/// is what keeps the two paths deciding identically.
#[allow(clippy::too_many_arguments)]
async fn plan_submissions(
    ck: &CheckpointState,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    sensitive_keys: &HashSet<String>,
    interpreter_map: &HashMap<String, String>,
    checkpoint_path: &Path,
    rerun: bool,
    resume_failed: bool,
    force_rules: &HashSet<String>,
    strict_output_verification: bool,
) -> SubmissionPlan {
    let preview = crate::commands::run_preview::preview_run_plan(
        ck,
        config,
        dag,
        order,
        workdir,
        wildcard_values,
        sensitive_keys,
        interpreter_map,
        checkpoint_path,
        rerun,
        resume_failed,
    );
    let mut to_run: HashSet<String> = preview
        .plan
        .iter()
        .filter(|p| !p.status.is_skip())
        .map(|p| p.name.clone())
        .collect();
    // Forced rules are still constrained to this run's execution set — a
    // `--target` subset must not pull in rules the user excluded.
    to_run.extend(
        order
            .iter()
            .filter(|name| force_rules.contains(*name))
            .cloned(),
    );
    // Strict resume (issue #1426): the read-only preview classifies skips
    // from existence, mtime, and recorded checksums — it does not verify the
    // output manifest. Re-check every rule the plan would skip and submit it
    // when its manifest does not verify, so a resumed cluster checkpoint
    // cannot reuse a damaged artifact.
    //
    // Verification hashes on the blocking pool (LOW-5), and a failure also
    // cascades to the rule's completed downstream: otherwise a dependent
    // pre-skipped on its own intact output would keep results derived from
    // the damaged producer (MEDIUM-3). `when`-false and tombstoned rules are
    // stop points: neither will regenerate its outputs for this run, so its
    // own downstream remains consistent with them.
    if strict_output_verification {
        let mut failed: Vec<String> = Vec::new();
        let mut candidates = Vec::new();
        for entry in &preview.plan {
            if !entry.status.is_skip()
                || matches!(
                    entry.status,
                    crate::commands::run_preview::RuleStatus::SkippedByWhen
                )
                || ck.tombstones.contains_key(&entry.name)
            {
                continue;
            }
            let Some(rule) = config.get_rule(&entry.name) else {
                continue;
            };
            // Same merged expansion scope the executor's gate uses for this
            // instance.
            let mut scope = wildcard_values.clone();
            for (key, value) in config.instance_bindings(&entry.name) {
                scope.insert(key, value);
            }
            candidates.push((
                entry.name.clone(),
                rule.clone(),
                scope,
                ck.output_manifests.get(&entry.name).cloned(),
            ));
        }
        for (name, rule, scope, manifest) in candidates {
            if let Err(failure) = oxo_flow_core::executor::checkpoint::verify_output_manifest_async(
                rule,
                workdir.to_path_buf(),
                scope,
                manifest,
            )
            .await
            {
                tracing::warn!(
                    rule = %name,
                    path = failure.path().unwrap_or("<rule>"),
                    reason = %failure.describe(),
                    "strict output verification failed — submitting the rule instead of \
                     reusing its recorded outputs (resume default; pass --trust-mtime to \
                     accept mtime evidence instead)"
                );
                failed.push(name);
            }
        }
        if !failed.is_empty() {
            let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
            let mut closure: HashSet<String> = HashSet::new();
            let mut frontier = failed.clone();
            while let Some(name) = frontier.pop() {
                let Ok(dependents) = dag.dependents(&name) else {
                    continue;
                };
                for dep in dependents {
                    if !order_set.contains(dep.as_str()) || closure.contains(&dep) {
                        continue;
                    }
                    let Some(dep_rule) = config.get_rule(&dep) else {
                        continue;
                    };
                    if crate::commands::run_preview::when_condition_false(
                        dep_rule,
                        config,
                        wildcard_values,
                    ) || ck.tombstones.contains_key(&dep)
                    {
                        // Not regenerated this run — stop the cascade here.
                        continue;
                    }
                    closure.insert(dep.clone());
                    frontier.push(dep);
                }
            }
            for name in failed.iter().chain(closure.iter()) {
                to_run.insert(name.clone());
            }
            if !closure.is_empty() {
                let mut cascaded: Vec<&String> = closure.iter().collect();
                cascaded.sort();
                tracing::warn!(
                    rules = %cascaded
                        .iter()
                        .map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    "strict output verification re-runs a damaged rule — its completed downstream is resubmitted too"
                );
            }
        }
    }
    let skipped_fresh = preview
        .plan
        .iter()
        .filter(|p| {
            p.status == crate::commands::run_preview::RuleStatus::SkippedFresh
                && !to_run.contains(&p.name)
        })
        .map(|p| p.name.clone())
        .collect();
    SubmissionPlan {
        to_run,
        skipped_fresh,
    }
}

/// Record the per-rule container-image provenance for a Kubernetes submission
/// (issue #1388) into `ck`.
///
/// Extracted from [`run_on_cluster`] so the wiring — which instances, which
/// map, merged into the checkpoint — is unit-tested without a live scheduler
/// (review M7). Persistence stays with the caller, which writes the checkpoint
/// before submitting so a crashed driver still leaves the record. The record
/// describes what the run PLANNED to submit; an abort after this point leaves
/// an identity for a job that may never have left.
fn record_planned_cluster_images(
    ck: &mut CheckpointState,
    plan: &ScheduledPlan,
    to_run: &HashSet<String>,
) {
    let mut planned: Vec<String> = to_run.iter().cloned().collect();
    planned.sort();
    ck.record_cluster_images(oxo_flow_core::backend::cluster_image_records(
        plan, &planned,
    ));
}

/// Submit and track this workflow through the scheduler named by `cluster`.
pub(crate) async fn run_on_cluster(
    cluster: &ClusterProfile,
    args: ClusterRunArgs<'_>,
) -> Result<ClusterRunSummary> {
    let (backend, cluster_config) = job_config(cluster)?;

    // The will-run set: the same computation `dry-run` reports (see
    // `plan_submissions` for why `force_rules` is unioned in).
    // Snapshot under the checkpoint lock, then plan WITHOUT holding it:
    // strict resume hashes output manifests (on the blocking pool), and an
    // async lock must never span that work (LOW-5). The preview clones the
    // checkpoint internally anyway, so the snapshot adds no new class of cost.
    let ck_snapshot = args.checkpoint.lock().await.clone();
    let submission = plan_submissions(
        &ck_snapshot,
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
        args.force_rules,
        args.strict_output_verification,
    )
    .await;
    let to_run = submission.to_run;
    // Issue #1371 review N1: mark preserved checkpoint work as NOT executed
    // in this invocation before any submission — a cluster re-run with
    // nothing to submit, or a subset submission, must not look like it
    // performed the earlier run's work. Rules that settle through
    // `fold_record` clear their marker on the terminal record; a crashed
    // invocation keeps it (the rule did not complete there).
    {
        let mut ck = args.checkpoint.lock().await;
        ck.begin_execution_pass();
        if let Err(e) = ck.save_to_file_async(args.checkpoint_path).await {
            tracing::warn!("Failed to save checkpoint at run start: {e}");
        }
    }
    if !submission.skipped_fresh.is_empty() {
        // Issue #1083 (suggestion 4): output existence is not completion.
        // Say when a rule is skipped on that heuristic alone, so a lost
        // sample cannot hide behind a green run.
        let shown: Vec<String> = submission.skipped_fresh.iter().take(5).cloned().collect();
        let more = submission.skipped_fresh.len().saturating_sub(shown.len());
        eprintln!(
            "  {} {} skipped because declared outputs exist but the checkpoint \
             has no completion record — a killed job can leave partial outputs; \
             pass --rerun to force re-execution: {}{}",
            "Warning:".bold().yellow(),
            oxo_flow_core::plural(submission.skipped_fresh.len(), "rule was", "rules were"),
            shown.join(", "),
            if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            }
        );
    }

    let mut summary = ClusterRunSummary {
        succeeded: 0,
        failed: 0,
        non_required_failed: 0,
        skipped: 0,
        interrupt_exit_code: None,
    };

    if to_run.is_empty() {
        eprintln!(
            "{} everything is up to date — nothing to submit",
            "Cluster:".bold().cyan()
        );
        // A run that executed nothing records nothing — and must say so,
        // exactly like the local path does when every rule was already
        // up to date (issue #276): the silent-empty case is what made
        // `provenance verify` claim "--provenance was never passed" to
        // users who had passed it (issue #1093).
        warn_if_nothing_recorded(&args).await;
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
    //
    // Sensitive values (issue #1120 / ledger RB-12) are routed BEFORE the
    // environment wrapper is built, so docker/singularity forward them by
    // name instead of re-expanding the literal on the compute node. Every
    // backend then gets the value through its own channel: an HPC env file
    // the driver writes and removes, or the cloud body's secret reference.
    let mut plan = ScheduledPlan::build_with_scratch_and_sensitive(
        args.config,
        args.dag,
        args.workdir,
        &env_resolver,
        args.wildcard_values,
        args.tmp_dir,
        args.sensitive_values,
    )
    .map_err(|e| anyhow::anyhow!("failed to build the execution plan: {e}"))?;

    let run_dir = create_run_dir(args.workdir)?;

    // Record the cluster identity before anything is submitted (issue
    // #1427): a later `resume` re-resolves the profile/backend/queue from the
    // checkpoint instead of guessing. An up-to-date run returns before this
    // point (it creates no run directory either), keeping the predecessor's
    // identity.
    {
        let mut ck = args.checkpoint.lock().await;
        ck.set_cluster_run(Some(ClusterRunRecord {
            profile: args.profile_name.to_string(),
            backend: backend.to_string(),
            queue: cluster_config.queue.clone(),
            account: cluster_config.account.clone(),
            run_dir: Some(run_dir.to_string_lossy().into_owned()),
        }));
        if let Err(e) = ck.save_to_file_async(args.checkpoint_path).await {
            tracing::warn!("Failed to save checkpoint cluster identity: {e}");
        }
    }

    // A rule that alone exceeds the workflow's own declared budget would be
    // rejected by the queue (or run into swap) after the fact — say so before
    // anything leaves for the scheduler.
    if let Some(budget) = args.config.resource_budget.as_ref() {
        check_budget(&plan, &to_run, budget)?;
    }
    reject_checkpoint_reentry(&plan, &to_run)?;
    reject_localrules(&plan, &to_run)?;
    reject_scratch_rules(&plan, &to_run)?;
    reject_unbound_placeholders(&plan, args.config, &to_run)?;
    reject_lifecycle_hooks(&plan, args.config, &to_run)?;

    // Container-image provenance (issue #1388). Kubernetes renders a rule's
    // container environment as the pod's own image; record which image each
    // planned rule is to run as — or that it fell back to the folded local
    // wrapper, or runs a system environment — so `provenance verify` can
    // report it after the fact. The digest is the submit host's offline
    // `docker image inspect` answer, or absent; it never claims what a node
    // pulled. Recorded (and persisted) before submission so a crashed driver
    // still leaves the record.
    if backend == ClusterBackend::Kubernetes {
        let mut ck = args.checkpoint.lock().await;
        record_planned_cluster_images(&mut ck, &plan, &to_run);
        if let Err(e) = ck.save_to_file_async(args.checkpoint_path).await {
            tracing::warn!("Failed to save checkpoint: {e}");
        }
    }

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
        // Issue #1093 F2b: `-k` reaches the driver — the required-failure
        // cancel decision is the one thing it changes (interrupts below
        // always cancel).
        keep_going: args.keep_going,
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

    // Operator-supplied secret references (OXO_FLOW_SECRET_REFS): only GCP
    // Batch has a submission-time field (`secretVariables`) for them; the
    // other backends resolve routed names through their own channel
    // (issue #1120 / ledger RB-12).
    let executor = ClusterExecutor::new(backend, cluster_config)
        .with_workdir(args.workdir.to_path_buf())
        // K8s Job names carry this scope so a re-run never collides with the
        // previous run's completed Jobs (issue #1158 live campaign).
        .with_name_scope(run_dir.to_string_lossy().into_owned())
        .with_secret_refs(oxo_flow_core::backend::secret_refs_from_env());
    let driver = BackendDriver::new(Arc::new(executor), driver_config);
    // Submit-time checkpoint recording (issue #136 H6): a crashed driver
    // leaves truthful RUNNING entries for accepted jobs, so `resume` /
    // `--resume-failed` re-queue them instead of losing them. The scheduler
    // job id (and an array element's base id) is recorded alongside
    // (issue #1427), which is what lets `resume` re-attach to a live job
    // instead of blindly resubmitting it.
    let submit_ck = args.checkpoint.clone();
    let submit_path = args.checkpoint_path.to_path_buf();
    let backend_name = backend.to_string();
    let on_submit = move |rule: String,
                          job: String,
                          array_base: Option<String>|
          -> std::pin::Pin<
        Box<dyn std::future::Future<Output = oxo_flow_core::error::Result<()>> + Send>,
    > {
        let ck = submit_ck.clone();
        let path = submit_path.clone();
        let backend_name = backend_name.clone();
        let output_tail_bytes = oxo_flow_core::executor::output_tail_bytes_from_env();
        Box::pin(async move {
            let mut ck = ck.lock().await;
            // The in-flight set is what the local scheduler consults
            // (issue #685); on this path it is also the truthful answer to
            // "was this rule submitted and never settled" when the driver
            // dies (issue #1083).
            ck.mark_running(&rule);
            ck.record_cluster_job(
                &rule,
                ClusterJobRecord {
                    job_id: job,
                    array_base,
                    backend: backend_name,
                    submitted_at: chrono::Utc::now(),
                },
            );
            ck.record_run(
                &oxo_flow_core::executor::JobRecord {
                    resources: None,
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
            // M-7: this hook is awaited inline inside the driver's submit
            // loop — the blocking save would park the executor thread on
            // disk I/O per submission.
            ck.save_to_file_async(&path).await
        })
    };
    // Incremental completion folding (issue #1083): the driver hands every
    // terminal record to this hook as it settles, and the checkpoint is
    // saved immediately. A crash from here on leaves authoritative
    // completed/failed sets instead of an empty `completed_rules`, so the
    // restart never has to guess from output existence.
    let settle_args = &args;
    let on_settle: oxo_flow_core::backend::driver::SettleHook<'_> =
        Box::new(move |record: JobRecord| {
            Box::pin(async move {
                fold_record(settle_args, &record).await;
                Ok(())
            })
        });
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
                on_settle: Some(on_settle),
            },
        )
        .await;
    signal_task.abort();
    // A cancellation must persist its interrupted records like any terminal
    // state — dropping them through map_err would lose the only truthful
    // account of what the signal stopped (and skip the failed-count
    // summary the webhook + exit contract below rely on). The signal's
    // `128 + signum` exit code rides along: grading the run by its failure
    // counts alone made a `required = false` interrupt look like success.
    let records = match run_result {
        Ok(records) => records,
        Err(oxo_flow_core::error::OxoFlowError::Cancelled {
            records: interrupted,
            exit_code,
            message,
            ..
        }) => {
            // Surface the cancellation detail (including "may still be
            // running: <ids>" when a cancel itself failed) at ERROR level:
            // the per-target WARNs are suppressed by --quiet / RUST_LOG=error,
            // and that detail must never be silent (review of #1188, D2).
            if !message.trim().is_empty() {
                eprintln!("{} {}", "Cancelled:".red().bold(), message);
            }
            summary.interrupt_exit_code = Some(exit_code);
            interrupted
        }
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
    // Settle every scheduled hashing task before reporting, then speak up
    // if nothing was recorded.
    warn_if_nothing_recorded(&args).await;
    Ok(summary)
}

/// The local completion path's zero-checksum warning (issue #276), shared
/// by the cluster run's early-return and normal-exit paths.
///
/// The historical cluster failure mode was silent AND empty — the flag
/// was dropped before this path, so the checkpoint held zero checksums
/// with no warning at all (issue #1093 F1).
async fn warn_if_nothing_recorded(args: &ClusterRunArgs<'_>) {
    args.provenance.join_pending().await;
    if args.provenance.requested && args.provenance.recorded() == 0 {
        eprintln!(
            "  {} --provenance recorded 0 checksums — no literal outputs completed \
             (templated or wildcard outputs are tracked per instance only after fan-out); \
             'provenance verify' will find nothing to check",
            "⚠".yellow()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::check_budget;
    use super::cluster_env_cache_dir;
    use super::plan_submissions;
    use super::record_planned_cluster_images;
    use super::reject_checkpoint_reentry;
    use super::reject_lifecycle_hooks;
    use super::reject_localrules;
    use super::reject_scratch_rules;
    use super::reject_unbound_placeholders;
    use oxo_flow_core::backend::ScheduledPlan;
    use oxo_flow_core::config::{ResourceBudget, WorkflowConfig};
    use oxo_flow_core::dag::WorkflowDag;
    use oxo_flow_core::environment::EnvironmentResolver;
    use oxo_flow_core::executor::checkpoint::{CheckpointState, RuleRunRecord};
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

    /// A plan spanning the three render decisions: an image-rendered docker
    /// rule, a conda fallback, and a system rule (issue #1388 review M7).
    fn image_plan(workdir: &std::path::Path) -> ScheduledPlan {
        let toml = "[workflow]\nname = \"images\"\n\
                    [[rules]]\nname = \"aligned\"\nshell = \"true\"\noutput = [\"a.out\"]\n\
                    environment = { docker = \"busybox:1.36\" }\n\
                    [[rules]]\nname = \"qc\"\nshell = \"true\"\noutput = [\"q.out\"]\n\
                    environment = { conda = \"envs/qc.yaml\" }\n\
                    [[rules]]\nname = \"report\"\nshell = \"true\"\noutput = [\"r.out\"]\n";
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

    /// Issue #1388 review M7: the run-cluster recording wiring stores the
    /// planned instances' render decisions (image / wrapper-fallback /
    /// system) in the checkpoint and persists them.
    #[test]
    fn planned_cluster_images_are_recorded_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let plan = image_plan(dir.path());
        let to_run: std::collections::HashSet<String> =
            ["aligned".to_string(), "report".to_string()]
                .into_iter()
                .collect();
        let mut ck = CheckpointState::new();

        record_planned_cluster_images(&mut ck, &plan, &to_run);

        assert_eq!(
            ck.cluster_images.len(),
            2,
            "only planned rules are recorded"
        );
        assert!(
            !ck.cluster_images.contains_key("qc"),
            "a rule outside to_run must not be recorded"
        );
        assert_eq!(
            ck.cluster_images["aligned"].render,
            oxo_flow_core::executor::checkpoint::ClusterImageRecord::IMAGE
        );
        assert_eq!(ck.cluster_images["aligned"].spec, "busybox:1.36");
        assert!(
            !ck.cluster_images["aligned"].image.contains("docker run"),
            "the record names an image, not a command"
        );
        assert_eq!(
            ck.cluster_images["report"].render,
            oxo_flow_core::executor::checkpoint::ClusterImageRecord::SYSTEM
        );

        // Persisted through the shared checkpoint writer (the run uses the
        // async variant of the same save path).
        let path = dir.path().join("checkpoint.json");
        ck.save_to_file(&path).unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(
            json.contains("\"cluster_images\"") && json.contains("busybox:1.36"),
            "the record must survive serialization: {json}"
        );
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

    /// A one-rule crash-recovery fixture: `emit` writes `out/S1/done.txt`
    /// incrementally (start marker first, `done` last) — the shape of a
    /// streaming tool and of the issue #1083 reproduction.
    fn interrupted_fixture(
        dir: &std::path::Path,
    ) -> (
        WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
        std::path::PathBuf,
    ) {
        let wf = dir.join("wf.oxoflow");
        std::fs::write(
            &wf,
            r#"
[workflow]
name = "crash"

[[rules]]
name = "emit"
output = ["out/S1/done.txt"]
shell = "mkdir -p out/S1 && echo started > out/S1/done.txt && sleep 45 && echo done >> out/S1/done.txt"
"#,
        )
        .unwrap();
        // The killed job's partial output: the file exists, the rule never
        // finished.
        std::fs::create_dir_all(dir.join("out/S1")).unwrap();
        std::fs::write(dir.join("out/S1/done.txt"), "started\n").unwrap();

        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        (
            config,
            dag,
            order,
            HashMap::new(),
            dir.join(".oxo-flow/checkpoint.json"),
        )
    }

    fn running_record() -> RuleRunRecord {
        RuleRunRecord {
            exit_code: None,
            started_at: None,
            finished_at: None,
            command: None,
            stderr_tail: None,
            stdout_tail: None,
            caption: None,
            status: Some("running".to_string()),
            skip_reason: None,
            signal: None,
        }
    }

    #[tokio::test]
    async fn killed_job_with_partial_output_is_requeued_on_restart() {
        // Issue #1083 regression: the driver was killed while this Slurm
        // job was running. Its declared output exists (partially written),
        // and the checkpoint carries the submit-time `running` record.
        // Nothing proves the rule finished — the restart must submit it.
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, values, checkpoint_path) = interrupted_fixture(dir.path());
        let mut ck = CheckpointState::new();
        ck.rule_runs.insert("emit".to_string(), running_record());

        let plan = plan_submissions(
            &ck,
            &config,
            &dag,
            &order,
            dir.path(),
            &values,
            &std::collections::HashSet::new(),
            &config.workflow.interpreter_map,
            &checkpoint_path,
            false,
            false,
            &std::collections::HashSet::new(),
            false,
        )
        .await;
        assert!(
            plan.to_run.contains("emit"),
            "an interrupted submission must be re-queued, not skipped as 'outputs up-to-date'"
        );
        assert!(
            plan.skipped_fresh.is_empty(),
            "the interrupted rule must not be reported as a fresh skip"
        );

        // Control: without the execution record the same on-disk state is
        // the historical crash-leftover skip — and it is now narrated
        // instead of passing silently.
        let plan = plan_submissions(
            &CheckpointState::new(),
            &config,
            &dag,
            &order,
            dir.path(),
            &values,
            &std::collections::HashSet::new(),
            &config.workflow.interpreter_map,
            &checkpoint_path,
            false,
            false,
            &std::collections::HashSet::new(),
            false,
        )
        .await;
        assert!(!plan.to_run.contains("emit"));
        assert_eq!(plan.skipped_fresh, vec!["emit".to_string()]);
    }

    /// Issue #1426: a rule the preview would skip is submitted when strict
    /// resume's recorded output manifest does not verify — existence/mtime
    /// evidence alone is not enough. A completed DEPENDENT of the damaged
    /// producer is submitted too (MEDIUM-3): its own intact output was built
    /// from the artifact the producer is about to regenerate.
    #[tokio::test]
    async fn strict_verification_submits_a_rule_whose_manifest_fails() {
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(
            &wf,
            "[workflow]\nname = \"strict\"\n[[rules]]\nname = \"emit\"\n\
             output = [\"out.txt\"]\nshell = \"true\"\n\n\
             [[rules]]\nname = \"consume\"\ninput = [\"out.txt\"]\n\
             output = [\"final.txt\"]\nshell = \"true\"\n",
        )
        .unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let values = config.config_placeholder_values();
        let checkpoint_path = dir.path().join("checkpoint.json");
        std::fs::write(dir.path().join("out.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("final.txt"), "done").unwrap();

        let mut ck = CheckpointState::new();
        ck.mark_completed_quiet("emit");
        ck.record_output_manifest(
            "emit",
            vec![oxo_flow_core::executor::checkpoint::OutputManifestEntry {
                path: "out.txt".to_string(),
                size: 99,
                sha256: None,
            }],
        );
        // The dependent's own output is intact, so its manifest verifies; it
        // is still stale with respect to the regenerated producer output.
        ck.mark_completed_quiet("consume");
        ck.record_output_manifest(
            "consume",
            vec![oxo_flow_core::executor::checkpoint::OutputManifestEntry {
                path: "final.txt".to_string(),
                size: 4,
                sha256: oxo_flow_core::executor::checkpoint::compute_file_checksum(
                    &dir.path().join("final.txt"),
                )
                .ok(),
            }],
        );

        let historical = plan_submissions(
            &ck,
            &config,
            &dag,
            &order,
            dir.path(),
            &values,
            &std::collections::HashSet::new(),
            &config.workflow.interpreter_map,
            &checkpoint_path,
            false,
            false,
            &std::collections::HashSet::new(),
            false,
        )
        .await;
        assert!(
            !historical.to_run.contains("emit") && !historical.to_run.contains("consume"),
            "without strict verification the existing skips stand"
        );

        let strict = plan_submissions(
            &ck,
            &config,
            &dag,
            &order,
            dir.path(),
            &values,
            &std::collections::HashSet::new(),
            &config.workflow.interpreter_map,
            &checkpoint_path,
            false,
            false,
            &std::collections::HashSet::new(),
            true,
        )
        .await;
        assert!(
            strict.to_run.contains("emit"),
            "a failing output manifest must force a submission"
        );
        assert!(
            strict.to_run.contains("consume"),
            "the damaged producer's completed downstream must be resubmitted too: {:?}",
            strict.to_run
        );
        assert!(
            strict.skipped_fresh.is_empty(),
            "the forced rules must not be reported as fresh skips"
        );
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

    /// A plan whose single rule declares `scratch = true`.
    fn scratch_plan(workdir: &std::path::Path) -> ScheduledPlan {
        let toml = "[workflow]\nname = \"scratch\"\n[[rules]]\nname = \"sort_bam\"\n\
                    scratch = true\nshell = \"true\"\noutput = [\"out.txt\"]\n";
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
    fn scratch_rules_are_rejected_on_the_cluster_plan() {
        // C3: `scratch = true` exists to run each instance in a private
        // scratch directory whose outputs are moved back. The cluster path
        // never consulted it — the job ran directly in the shared workdir,
        // where fixed-filename tools collide across concurrent instances.
        // The directive must be honored by refusing the submission, naming
        // the rule, before anything leaves (issue #1013 C3).
        let dir = tempfile::tempdir().unwrap();
        let plan = scratch_plan(dir.path());
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let msg = reject_scratch_rules(&plan, &to_run)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("sort_bam") && msg.contains("oxo-flow run"),
            "the rejection must name the rule and the local path: {msg}"
        );
        assert!(
            msg.contains("scratch"),
            "the rejection must name the directive being dishonored: {msg}"
        );
    }

    #[test]
    fn scratch_gate_passes_plain_rules() {
        let dir = tempfile::tempdir().unwrap();
        let plan = hungry_plan(dir.path(), "8G", 4);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(reject_scratch_rules(&plan, &to_run).is_ok());
    }

    #[test]
    fn scratch_gate_ignores_rules_outside_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let plan = scratch_plan(dir.path());
        assert!(reject_scratch_rules(&plan, &std::collections::HashSet::new()).is_ok());
    }

    /// A plan with one rule whose shell is supplied by the test.
    fn placeholder_plan(
        workdir: &std::path::Path,
        shell: &str,
        strict: Option<bool>,
    ) -> (WorkflowConfig, ScheduledPlan) {
        let toml = format!(
            "[workflow]\nname = \"ph\"\n[[rules]]\nname = \"bad\"\nshell = \"{shell}\"\n\
             output = [\"out.txt\"]\n"
        );
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        if let Some(strict) = strict {
            let mut engine = config.engine.clone().unwrap_or_default();
            engine.strict_placeholders = Some(strict);
            config.engine = Some(engine);
        }
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap();
        (config, plan)
    }

    #[test]
    fn placeholder_gate_rejects_unbound_cluster_commands() {
        // Issue #1078 F2: an unexpanded per-sample rule must never ship a
        // literal `{sample}` to a remote node.
        let dir = tempfile::tempdir().unwrap();
        let (config, plan) = placeholder_plan(dir.path(), "echo {sample} > out.txt", None);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        let msg = reject_unbound_placeholders(&plan, &config, &to_run)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("bad") && msg.contains("{sample}"),
            "the rejection must name the rule and the token: {msg}"
        );
    }

    #[test]
    fn placeholder_gate_passes_clean_cluster_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (config, plan) = placeholder_plan(dir.path(), "echo hi > out.txt", None);
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(reject_unbound_placeholders(&plan, &config, &to_run).is_ok());
    }

    #[test]
    fn placeholder_gate_honors_the_workflow_escape_hatch() {
        let dir = tempfile::tempdir().unwrap();
        let (config, plan) = placeholder_plan(dir.path(), "echo {sample} > out.txt", Some(false));
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(
            reject_unbound_placeholders(&plan, &config, &to_run).is_ok(),
            "[engine] strict_placeholders = false must restore warn-only mode"
        );
    }

    /// A plan whose single rule declares a rule-level lifecycle hook.
    fn rule_hook_plan(
        workdir: &std::path::Path,
        hook_field: &str,
    ) -> (WorkflowConfig, ScheduledPlan) {
        let toml = format!(
            "[workflow]\nname = \"hooks\"\n[[rules]]\nname = \"align\"\n\
             {hook_field} = \"echo hooked\"\nshell = \"true\"\noutput = [\"out.txt\"]\n"
        );
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap();
        (config, plan)
    }

    /// A plan whose `[workflow]` declares a terminal lifecycle hook.
    fn workflow_hook_plan(
        workdir: &std::path::Path,
        hook_field: &str,
    ) -> (WorkflowConfig, ScheduledPlan) {
        let toml = format!(
            "[workflow]\nname = \"hooks\"\n{hook_field} = \"echo done\"\n\
             [[rules]]\nname = \"align\"\nshell = \"true\"\noutput = [\"out.txt\"]\n"
        );
        let path = workdir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap();
        (config, plan)
    }

    #[test]
    fn rule_lifecycle_hooks_are_rejected_on_the_cluster_plan() {
        // Issue #1340: rule-level hooks run under the local executor only —
        // the cluster script generator renders just the command, so a hook
        // would be silently dropped. The gate must reject, naming the rule
        // and the hook field, before anything leaves for the scheduler.
        for field in ["pre_exec", "on_success", "on_failure"] {
            let dir = tempfile::tempdir().unwrap();
            let (config, plan) = rule_hook_plan(dir.path(), field);
            let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
            let msg = reject_lifecycle_hooks(&plan, &config, &to_run)
                .unwrap_err()
                .to_string();
            assert!(
                msg.contains("align") && msg.contains(field) && msg.contains("oxo-flow run"),
                "the rejection must name the rule, the `{field}` hook, and the local path: {msg}"
            );
        }
    }

    #[test]
    fn workflow_lifecycle_hooks_are_rejected_on_the_cluster_plan() {
        // Issue #1340: workflow-level terminal hooks are owned by the CLI run
        // loop, which the cluster path does not enter.
        for field in ["on_complete", "on_error"] {
            let dir = tempfile::tempdir().unwrap();
            let (config, plan) = workflow_hook_plan(dir.path(), field);
            let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
            let msg = reject_lifecycle_hooks(&plan, &config, &to_run)
                .unwrap_err()
                .to_string();
            assert!(
                msg.contains(field) && msg.contains("oxo-flow run"),
                "the rejection must name the `{field}` hook and the local path: {msg}"
            );
        }
    }

    #[test]
    fn lifecycle_hook_gate_passes_plain_workflows() {
        // No hooks declared => cluster submit behavior is byte-identical.
        let dir = tempfile::tempdir().unwrap();
        let toml = "[workflow]\nname = \"clean\"\n[[rules]]\nname = \"align\"\n\
                    shell = \"true\"\noutput = [\"out.txt\"]\n";
        let path = dir.path().join("clean.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let plan = ScheduledPlan::build(
            &config,
            &dag,
            dir.path(),
            &EnvironmentResolver::new(),
            &HashMap::new(),
        )
        .unwrap();
        let to_run: std::collections::HashSet<String> = plan.order.iter().cloned().collect();
        assert!(reject_lifecycle_hooks(&plan, &config, &to_run).is_ok());
    }

    #[test]
    fn lifecycle_hook_gate_ignores_rules_outside_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let (config, plan) = rule_hook_plan(dir.path(), "pre_exec");
        assert!(reject_lifecycle_hooks(&plan, &config, &std::collections::HashSet::new()).is_ok());
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

    // ── issue #1093 F1: cluster-path provenance recording ──────────────

    use super::ClusterProvenance;
    use super::ClusterRunArgs;
    use super::fold_record;
    use oxo_flow_core::executor::{JobRecord, JobStatus};

    /// A workflow directory with the given TOML, its parsed/expanded
    /// config, DAG, and a fresh checkpoint — everything [`fold_record`]
    /// needs except the provenance tracker, which each test drives.
    struct FoldFixture {
        _dir: tempfile::TempDir,
        workdir: std::path::PathBuf,
        config: WorkflowConfig,
        dag: WorkflowDag,
        order: Vec<String>,
        checkpoint: std::sync::Arc<tokio::sync::Mutex<CheckpointState>>,
        checkpoint_path: std::path::PathBuf,
        wildcard_values: HashMap<String, String>,
    }

    impl FoldFixture {
        fn new(toml: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let workdir = dir.path().to_path_buf();
            let path = workdir.join("wf.oxoflow");
            std::fs::write(&path, toml).unwrap();
            let mut config = WorkflowConfig::from_file(&path).unwrap();
            config.apply_defaults();
            config.expand_wildcards().unwrap();
            let dag = WorkflowDag::from_rules(&config.rules).unwrap();
            let order = dag.execution_order().unwrap();
            let checkpoint_path = workdir.join(".oxo-flow").join("checkpoint.json");
            std::fs::create_dir_all(checkpoint_path.parent().unwrap()).unwrap();
            Self {
                _dir: dir,
                workdir,
                config,
                dag,
                order,
                checkpoint: std::sync::Arc::new(tokio::sync::Mutex::new(CheckpointState::new())),
                checkpoint_path,
                wildcard_values: HashMap::new(),
            }
        }

        /// Fold one Success record for `rule` through the real
        /// [`fold_record`] path.
        async fn fold_success(&self, provenance: &ClusterProvenance, rule: &str) {
            let now = chrono::Utc::now();
            let record = JobRecord {
                resources: None,
                rule: rule.to_string(),
                status: JobStatus::Success,
                enqueued_at: Some(now),
                started_at: Some(now),
                finished_at: Some(now),
                exit_code: Some(0),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
                caption: None,
                signal: None,
            };
            let sensitive_keys = std::collections::HashSet::new();
            let force_rules = std::collections::HashSet::new();
            let args = ClusterRunArgs {
                config: &self.config,
                dag: &self.dag,
                order: &self.order,
                checkpoint: &self.checkpoint,
                checkpoint_path: &self.checkpoint_path,
                workdir: &self.workdir,
                wildcard_values: &self.wildcard_values,
                profile_name: "test-profile",
                sensitive_keys: &sensitive_keys,
                sensitive_values: &[],
                force_rules: &force_rules,
                max_submitted: None,
                rerun: false,
                resume_failed: false,
                keep_going: false,
                cache_dir: None,
                skip_env_setup: false,
                ai_recover: false,
                provenance,
                tmp_dir: None,
                strict_output_verification: false,
            };
            fold_record(&args, &record).await;
            // Checksum collection is scheduled off the settle path; settle
            // it before the test asserts on the checkpoint.
            provenance.join_pending().await;
        }

        async fn checksums(&self) -> std::collections::BTreeMap<String, String> {
            self.checkpoint.lock().await.checksums.clone()
        }

        async fn manifests(
            &self,
        ) -> std::collections::BTreeMap<String, oxo_flow_core::executor::checkpoint::OutputManifest>
        {
            self.checkpoint.lock().await.output_manifests.clone()
        }
    }

    const ONE_RULE_WORKFLOW: &str = "[workflow]\nname = \"prov\"\n[[rules]]\nname = \"emit\"\n\
                                     output = [\"out.txt\"]\nshell = \"true\"\n";

    #[tokio::test]
    async fn cluster_fold_records_the_output_manifest_without_provenance() {
        // Issue #1426: the always-on output manifest is recorded by the
        // cluster settle hook too, even when `--provenance` is absent — the
        // local executor and cluster paths must not diverge on reuse
        // evidence.
        let f = FoldFixture::new(ONE_RULE_WORKFLOW);
        std::fs::write(f.workdir.join("out.txt"), "hello").unwrap();
        let prov = ClusterProvenance::new(false);
        f.fold_success(&prov, "emit").await;
        let manifests = f.manifests().await;
        let entry = manifests
            .get("emit")
            .and_then(|m| m.first())
            .expect("the output manifest must be recorded");
        assert_eq!(entry.path, "out.txt");
        assert_eq!(entry.size, 5);
        assert!(
            entry
                .sha256
                .as_deref()
                .is_some_and(|hash| hash.starts_with("sha256:")),
            "the manifest must carry the sha256: {entry:?}"
        );
        assert!(
            f.checksums().await.is_empty(),
            "no provenance checksums are recorded without --provenance"
        );
    }

    #[tokio::test]
    async fn cluster_provenance_records_literal_output_checksums() {
        // Issue #1093 F1: `--provenance` used to be dropped on the cluster
        // path, leaving the checkpoint with zero checksums where the local
        // executor records one per literal output.
        let f = FoldFixture::new(ONE_RULE_WORKFLOW);
        std::fs::write(f.workdir.join("out.txt"), "hello").unwrap();
        let prov = ClusterProvenance::new(true);
        f.fold_success(&prov, "emit").await;
        let checksums = f.checksums().await;
        let recorded = checksums.get("out.txt").expect("out.txt must be recorded");
        assert!(
            recorded.starts_with("sha256:"),
            "the default algorithm must be sha256: {recorded}"
        );
        assert_eq!(
            prov.recorded.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the zero-warning counter must count this run's checksums"
        );
    }

    #[tokio::test]
    async fn cluster_provenance_hashes_each_rule_once_despite_double_fold() {
        // The same record is folded twice on the normal path (settle hook,
        // then the post-run loop); the second fold must not re-hash —
        // outputs are routinely tens of GB.
        let f = FoldFixture::new(ONE_RULE_WORKFLOW);
        std::fs::write(f.workdir.join("out.txt"), "hello").unwrap();
        let prov = ClusterProvenance::new(true);
        f.fold_success(&prov, "emit").await;
        f.fold_success(&prov, "emit").await;
        assert_eq!(
            prov.recorded.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the second fold must be a no-op, not a second hash"
        );
        assert_eq!(f.checksums().await.len(), 1);
    }

    #[tokio::test]
    async fn cluster_rule_checksum_records_without_the_flag() {
        // Per-rule `checksum = "md5"` opts a rule into recorded integrity
        // even without --provenance — the same contract the local executor
        // applies (issue #1013 E6), recomputed with the declared algorithm.
        let toml = "[workflow]\nname = \"prov\"\n[[rules]]\nname = \"emit\"\n\
                    output = [\"out.txt\"]\nchecksum = \"md5\"\nshell = \"true\"\n";
        let f = FoldFixture::new(toml);
        std::fs::write(f.workdir.join("out.txt"), "hello").unwrap();
        let prov = ClusterProvenance::new(false);
        f.fold_success(&prov, "emit").await;
        let checksums = f.checksums().await;
        assert!(
            checksums["out.txt"].starts_with("md5:"),
            "the rule's declared algorithm must be used: {checksums:?}"
        );
        assert_eq!(
            prov.recorded.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "without --provenance the zero-warning counter stays untouched"
        );
    }

    #[tokio::test]
    async fn cluster_provenance_skips_absent_outputs() {
        // No file on disk → nothing recorded, and the zero-warning counter
        // stays 0 so the completion path can speak up.
        let f = FoldFixture::new(ONE_RULE_WORKFLOW);
        let prov = ClusterProvenance::new(true);
        f.fold_success(&prov, "emit").await;
        assert!(f.checksums().await.is_empty());
        assert_eq!(prov.recorded.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn cluster_provenance_records_fanned_out_instance_outputs() {
        // Per-instance outputs are keyed by their expanded path, exactly
        // like the local executor records them — never by the template.
        let toml = "[workflow]\nname = \"prov\"\n[[sample_groups]]\nname = \"batch\"\n\
                    samples = [\"S1\", \"S2\"]\n[[rules]]\nname = \"align\"\n\
                    output = [\"aln/{sample}.bam\"]\nshell = \"true\"\n";
        let f = FoldFixture::new(toml);
        std::fs::create_dir_all(f.workdir.join("aln")).unwrap();
        std::fs::write(f.workdir.join("aln/S1.bam"), "bam").unwrap();
        let prov = ClusterProvenance::new(true);
        f.fold_success(&prov, "align_batch_S1").await;
        let checksums = f.checksums().await;
        assert!(
            checksums.contains_key("aln/S1.bam"),
            "the instance's expanded path must be the key: {checksums:?}"
        );
        assert!(
            !checksums.keys().any(|k| k.contains('{')),
            "template keys must never be recorded: {checksums:?}"
        );
    }
}
