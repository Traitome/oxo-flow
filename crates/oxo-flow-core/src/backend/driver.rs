//! BackendDriver: executes a [`ScheduledPlan`] through an [`ExecutorBackend`].
//!
//! The driver never decides *what* to run: the caller computes `to_run` from
//! the shared invalidation predicates (run_preview). It submits waves of at
//! most `max_submitted` in-flight jobs, polls, and maps scheduler states to
//! [`JobRecord`]s — the same record type the local executor produces, so
//! checkpoint semantics are identical.

use super::{
    BackendJobStatus, ExecutorBackend, ScheduledPlan, ScheduledRule, TerminalRecord,
    source_sensitive_env_file, write_private, write_sensitive_env_file,
};
use crate::error::{OxoFlowError, Result};
use crate::executor::{JobRecord, JobStatus};
use chrono::{DateTime, Utc};
use std::collections::{
    BTreeMap, {HashMap, HashSet},
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Behaviour knobs for [`BackendDriver::run`].
#[derive(Debug, Clone)]
pub struct DriverConfig {
    /// Jobs in flight at once (pending + running). Submissions top up to
    /// this cap as slots free.
    pub max_submitted: usize,
    /// Maximum scheduler array size (SLURM MaxArraySize, commonly 1001):
    /// larger scatter groups are chunked into several arrays (issue #74
    /// phase 3).
    pub max_array_size: usize,
    /// Disable automatic array grouping — every instance submits as its
    /// own job (the pre-phase-3 behavior).
    pub no_arrays: bool,
    /// Delay between scheduler polls.
    pub poll_interval: Duration,
    /// Overall wall-clock budget; `None` = run until terminal states.
    pub poll_timeout: Option<Duration>,
    /// How long a job must be in flight before the blind-settlement guard
    /// starts counting (issue #244): accounting stores legitimately lag —
    /// slurmdbd can take tens of seconds to surface a record — so the
    /// guard must never race a slow-but-alive store. `UNKNOWN_SETTLE_
    /// ROUNDS` consecutive blind rounds AFTER this grace settle the job
    /// as Failed with an unknown exit code.
    pub unknown_settle_grace: Duration,
    /// Watch channel delivering an interrupt request (Ctrl-C / SIGTERM /
    /// SIGHUP). `Some(Some(signal))` cancels every in-flight job, records
    /// them as interrupted, and ends the run with
    /// [`OxoFlowError::Cancelled`]; `None` (or `Some(None)`) leaves the
    /// driver polling to terminal states. The receiver is cloned into the
    /// driver; the sender stays with the caller's signal watcher.
    pub cancel: Option<tokio::sync::watch::Receiver<Option<CancelSignal>>>,
    /// `-k/--keep-going` (issue #1093 F2b): a required rule's failure is
    /// recorded but does not cancel jobs already in flight. The driver
    /// finishes them, keeps dispatching every independent ready instance
    /// until the queue drains, and leaves dependents of the failed rule
    /// blocked — the caller still fails the run because a required rule
    /// failed. `false` keeps the local executor's
    /// abort-on-first-required-failure default.
    ///
    /// This gates ONLY the required-failure decision: interrupts
    /// ([`Self::cancel`]) and driver-internal errors always cancel every
    /// in-flight job.
    pub keep_going: bool,
}

/// An interrupt request delivered through [`DriverConfig::cancel`].
///
/// Core-side mirror of the CLI's `InterruptSignal` (the CLI module is
/// private and core must not depend on the CLI crate).
#[derive(Debug, Clone)]
pub struct CancelSignal {
    /// Canonical name, e.g. `"SIGINT"` — surfaces in skip_reason and events.
    pub signal_name: &'static str,
    /// Raw signal number, e.g. `2` — recorded on the job record.
    pub signum: i32,
}

impl CancelSignal {
    /// Process exit convention for signal termination: 128 + signum.
    pub fn exit_code(&self) -> i32 {
        128 + self.signum
    }
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            max_submitted: 50,
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: Duration::from_secs(5),
            poll_timeout: None,
            unknown_settle_grace: Duration::from_secs(90),
            cancel: None,
            keep_going: false,
        }
    }
}

/// Checkpoint completion hook: returns the names of newly created rule
/// instances, or an error that fails the checkpoint rule.
pub type CheckpointHook<'a> = Box<dyn FnMut(&str) -> Result<Vec<String>> + Send + 'a>;

/// Plan-merge hook: folds newly created instances into the plan (the
/// caller's re-expansion has already produced them).
pub type PlanMerge<'a> = Box<dyn FnMut(&mut ScheduledPlan, &[String]) -> Result<()> + Send + 'a>;

/// Submit hook: records a rule as RUNNING in the caller's checkpoint the
/// moment its job is accepted — a crashed driver then leaves truthful
/// pending state instead of nothing (issue #136 H6, terminal-state-only
/// recording). Errors are logged, not fatal.
pub type SubmitHook = Box<
    dyn FnMut(
            String,
            String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send,
>;

/// Settle hook: observes every terminal [`JobRecord`] the moment the driver
/// reconciles it.
///
/// The cluster path uses this to fold completions into the checkpoint
/// incrementally (issue #1083): folding only after `run` returns lost every
/// completion when the driver was killed, leaving the restart with no
/// trustworthy state. Errors are logged, not fatal — a checkpoint write
/// failure must not abort a run whose jobs are already executing.
pub type SettleHook<'a> = Box<
    dyn FnMut(
            JobRecord,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
        + Send
        + 'a,
>;

/// Per-run hooks: checkpoint re-entry (issue #78 P3).
pub struct DriverOptions<'a> {
    /// Run directory root: `events.jsonl` + `jobs/<rule>/` below it.
    pub run_dir: &'a Path,
    /// Invoked when a `checkpoint = true` rule completes.
    pub on_checkpoint: Option<CheckpointHook<'a>>,
    /// Merges newly created instances into the plan; called when
    /// `on_checkpoint` returns names.
    pub merge: Option<PlanMerge<'a>>,
    /// Sensitive config values masked out of recorded job commands
    /// (issue #99 B1, cluster path — mirrors the local executor's
    /// capture-boundary masking).
    pub sensitive_values: &'a [String],
    /// Invoked after each successful submission `(rule, job_id)`.
    pub on_submit: Option<SubmitHook>,
    /// Invoked as each job settles with its terminal record (issue #1083).
    pub on_settle: Option<SettleHook<'a>>,
}

/// Executes a static plan through a backend.
pub struct BackendDriver {
    backend: Arc<dyn ExecutorBackend>,
    config: DriverConfig,
}

#[derive(Clone)]
struct InFlight {
    rule: String,
    submitted_at: DateTime<Utc>,
    /// Base array job id for element submissions (`None` = a plain job).
    /// Polling/cancelling targets the base for schedulers that report
    /// arrays only by their base id (PBS/LSF/SGE — issue #136 H4).
    array_base: Option<String>,
    /// Private env file carrying this job's routed sensitive values
    /// (issue #1120 / RB-12). Removed as soon as the driver observes the job
    /// terminal — or by the run-end sweep if the driver itself exits first.
    env_file: Option<PathBuf>,
}

/// Scheduler-visible directive signature: two instances are array-eligible
/// only when every directive a scheduler could split on agrees (issue #74
/// phase 3). The environment is included — conda vs docker instances must
/// never share an array.
fn array_key(sr: &ScheduledRule) -> String {
    format!(
        "{}|{}|{}|{}|{}|{:?}|{}",
        sr.rule.effective_threads(),
        sr.rule.effective_memory().unwrap_or_default(),
        sr.rule.resources.time_limit.as_deref().unwrap_or_default(),
        sr.rule.resources.partition.as_deref().unwrap_or_default(),
        sr.rule
            .resources
            .gpu_spec
            .as_ref()
            .map(|g| format!("{:?}", g))
            .unwrap_or_else(|| sr
                .rule
                .resources
                .gpu
                .map(|n| n.to_string())
                .unwrap_or_default()),
        sr.rule.environment,
        sr.workdir.display(),
    )
}

/// Do two instances depend on the same set of rules?
///
/// Order-insensitive on purpose: the dependency list is assembled from DAG
/// edges and `depends_on`, so two instances of one template can carry the
/// same producers in a different order. Comparing the vectors directly (as
/// this used to) meant a second-level fan-out — whose deps arrive in
/// discovery order — silently never grouped into an array.
fn same_dependencies(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|d| b.contains(d))
}

/// How many polls a job may sit in PENDING before the driver says so again.
/// With the default 5s poll interval this is one line per minute — enough
/// to tell a full partition from a hung driver without drowning the log.
const PENDING_HEARTBEAT_ROUNDS: u32 = 12;

/// How many consecutive blind rounds (invisible to the live queue AND the
/// accounting/terminal probes) a job may sit in before the driver settles
/// it as Failed with an unknown exit code (issue #244). At the default 5s
/// poll interval this is 3 minutes — long enough for a slow accounting
/// store to catch up, short enough that a slurmdbd-less cluster cannot
/// poll forever.
const UNKNOWN_SETTLE_ROUNDS: u32 = 36;

impl BackendDriver {
    pub fn new(backend: Arc<dyn ExecutorBackend>, config: DriverConfig) -> Self {
        Self { backend, config }
    }

    /// Fair-dispatch ordering (issue #134): the cluster counterpart of the
    /// local priority aging — ready rules are sorted by effective priority
    /// (declared + rounds waited), so fresh high-priority arrivals cannot
    /// starve a producer parked at the submission cap. Effective-priority
    /// ties follow the plan's [`DispatchKeys`](crate::scheduler::DispatchKeys)
    /// — name order by default, sample-aware ordering under `[engine]
    /// dispatch_order = "sample"` (issue #1089). The caller ages the tail
    /// that is NOT submitted this round.
    fn aged_ready_order(
        ready: Vec<String>,
        plan: &ScheduledPlan,
        waited: &HashMap<String, i32>,
    ) -> Vec<String> {
        let mut ready = ready;
        ready.sort_by(|a, b| {
            let pa = plan.rules.get(a).map(|sr| sr.rule.priority).unwrap_or(0)
                + waited.get(a).copied().unwrap_or(0);
            let pb = plan.rules.get(b).map(|sr| sr.rule.priority).unwrap_or(0)
                + waited.get(b).copied().unwrap_or(0);
            pb.cmp(&pa).then_with(|| plan.dispatch.cmp_names(a, b))
        });
        ready
    }

    /// The scheduler ids a cancellation would target: an element cancels via
    /// its array BASE id (qdel/bkill do not understand `{base}_{index}` —
    /// issue #136 H4), and each target appears once.
    fn cancel_targets(jobs: &[(String, String, Option<String>)]) -> Vec<String> {
        let mut targets: Vec<String> = Vec::new();
        for (id, _rule, base) in jobs {
            let target = base.clone().unwrap_or_else(|| id.clone());
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        targets
    }

    /// Cancel every job in `jobs` (`(job_id, rule_name, array_base)` triples),
    /// attempting every target even when an earlier one fails. Returns an
    /// error naming exactly the jobs the scheduler did NOT confirm cancelled:
    /// those may still be running, and a caller must never report them as
    /// cancelled (issue #1156, ledger CL-01).
    pub async fn cancel_inflight(&self, jobs: &[(String, String, Option<String>)]) -> Result<()> {
        // (id, scheduler-reported error) — the first error rides in the
        // returned message so the operator sees WHY the cancel failed, not
        // only which job is unresolved.
        let mut unresolved: Vec<(String, String)> = Vec::new();
        for id in Self::cancel_targets(jobs) {
            if let Err(error) = self.backend.cancel(&id).await {
                tracing::warn!(
                    job = %id,
                    error = %error,
                    "scheduler cancel failed — the job may still be running"
                );
                unresolved.push((id, error.to_string()));
            }
        }
        if unresolved.is_empty() {
            Ok(())
        } else {
            let named = unresolved
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(OxoFlowError::Config {
                message: format!(
                    "failed to cancel {} scheduler job(s) — they may still be running: {} (scheduler error: {})",
                    unresolved.len(),
                    named,
                    unresolved[0].1
                ),
            })
        }
    }

    /// Cancel every accepted job before returning a terminal driver error.
    /// Errors after submission must never leave billable scheduler jobs
    /// running after `run` has already returned to its caller.
    ///
    /// The returned error names the accepted scheduler job ids and their
    /// cancel outcome: confirmed cancellations say so, and a failed cleanup
    /// is appended as "may still be running: <ids>" instead of being
    /// silently swallowed (issue #1156, ledger CL-01). The original failure
    /// text is always preserved.
    async fn fail_with_cancel<T>(
        &self,
        inflight: &HashMap<String, InFlight>,
        error: OxoFlowError,
    ) -> Result<T> {
        let jobs: Vec<(String, String, Option<String>)> = inflight
            .iter()
            .map(|(id, f)| (id.clone(), f.rule.clone(), f.array_base.clone()))
            .collect();
        let targets = Self::cancel_targets(&jobs);
        if targets.is_empty() {
            return Err(error);
        }
        match self.cancel_inflight(&jobs).await {
            Ok(()) => Err(append_error_context(
                error,
                &format!(
                    "accepted scheduler job(s) cancelled: {}",
                    targets.join(", ")
                ),
            )),
            Err(cancel_error) => Err(append_error_context(error, &cancel_error.to_string())),
        }
    }

    /// Execute exactly `to_run` on `plan`, returning one [`JobRecord`] per
    /// rule. Errors are terminal: everything in flight is cancelled and the
    /// error propagates (partial state lives in the run directory). The one
    /// exception is [`OxoFlowError::Cancelled`] — an interrupt delivered via
    /// [`DriverConfig::cancel`] — which carries the interrupted job records
    /// itself so the caller can persist them before exiting.
    ///
    /// Sensitive env files this run wrote are swept on EVERY exit path —
    /// success, terminal error, interrupt (issue #1120 / ledger RB-12), so an
    /// inspected or restarted run directory never leaves a recoverable value.
    pub async fn run(
        &self,
        plan: &mut ScheduledPlan,
        to_run: &HashSet<String>,
        opts: DriverOptions<'_>,
    ) -> Result<Vec<JobRecord>> {
        let run_dir = opts.run_dir.to_path_buf();
        // A resume (or a caller reusing a directory after a crash) may find
        // env files from a previous process that never reached its sweep.
        sweep_sensitive_env_files(&run_dir);
        let result = self.run_inner(plan, to_run, opts).await;
        sweep_sensitive_env_files(&run_dir);
        result
    }

    async fn run_inner(
        &self,
        plan: &mut ScheduledPlan,
        to_run: &HashSet<String>,
        mut opts: DriverOptions<'_>,
    ) -> Result<Vec<JobRecord>> {
        std::fs::create_dir_all(opts.run_dir).map_err(|e| OxoFlowError::Config {
            message: format!("cannot create run dir {}: {e}", opts.run_dir.display()),
        })?;
        let events_path = opts.run_dir.join("events.jsonl");
        let mut events = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .map_err(|e| OxoFlowError::Config {
                message: format!("cannot open {}: {e}", events_path.display()),
            })?;

        let started = Instant::now();
        let deadline = self.config.poll_timeout.map(|t| started + t);
        let mut records: Vec<JobRecord> = Vec::new();
        let mut done: HashMap<String, JobStatus> = HashMap::new();
        let mut inflight: HashMap<String, InFlight> = HashMap::new();
        let mut to_run_set: HashSet<String> = to_run.clone();
        // The run's stable `VAR -> value` table for routed `sensitive = true`
        // values. The plan carries only the `${OXO_FLOW_SENSITIVE_<n>}`
        // references; an HPC backend needs the values to write each job's
        // private env file (issue #1120 / RB-12). Same helper, same ordering
        // the plan routing used, so the names cannot disagree.
        let sensitive_bindings =
            crate::executor::process::sensitive_bindings(opts.sensitive_values);
        // Priority aging (issue #134): each round a ready rule spends
        // beyond the cap adds +1 to its effective priority.
        let mut waited_rounds: HashMap<String, i32> = HashMap::new();
        // Array index: base job id → element instance names, accumulated in
        // memory and persisted after each change (issue #136 H3 — the
        // read-modify-write pattern could lose earlier chunks).
        // BTreeMap: index.json is a user-visible, diff-able run artifact —
        // HashMap iteration is process-random and the key order shuffles
        // across restarts (issue #471).
        let mut array_index: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let index_path = opts.run_dir.join("index.json");
        // Rules skipped because an upstream failed. Tracked separately from
        // `done`'s Skipped status so the dependency gate can tell "blocked"
        // (must not release downstream rules) from "skipped as complete"
        // (up-to-date / non-schedulable — may release). Transitive: a rule
        // blocked this round blocks its own dependents next round.
        let mut blocked: HashSet<String> = HashSet::new();
        // Consecutive polls each job has been seen waiting (PENDING, or not
        // yet reported by the scheduler) — drives the queue-wait heartbeat.
        let mut pending_rounds: HashMap<String, u32> = HashMap::new();
        // Consecutive polls a job has been invisible to BOTH the live
        // queue and the accounting/terminal probes (issue #244 guard).
        let mut unknown_rounds: HashMap<String, u32> = HashMap::new();

        macro_rules! cancel_on_error {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(error) => return self.fail_with_cancel(&inflight, error).await,
                }
            };
        }

        loop {
            // 1. Failure propagation: pending rules blocked by a failed dep.
            let failed: HashSet<String> = done
                .iter()
                .filter(|(_, s)| **s == JobStatus::Failed)
                .map(|(r, _)| r.clone())
                .collect();
            let pending_names: Vec<String> = plan
                .order
                .iter()
                .filter(|n| to_run_set.contains(*n) && !done.contains_key(*n))
                .cloned()
                .collect();
            for name in pending_names {
                let is_blocked = plan.rules.get(&name).is_some_and(|sr| {
                    sr.dependencies
                        .iter()
                        .any(|d| failed.contains(d) || blocked.contains(d))
                });
                if is_blocked {
                    blocked.insert(name.clone());
                    records.push(JobRecord {
                        signal: None,
                        rule: name.clone(),
                        status: JobStatus::Skipped,
                        enqueued_at: None,
                        started_at: None,
                        finished_at: Some(Utc::now()),
                        exit_code: None,
                        stdout: None,
                        stderr: None,
                        command: None,
                        retries: 0,
                        skip_reason: Some("blocked by failed upstream dependency".into()),
                        max_rss_mb: None,
                        cpu_seconds: None,
                        caption: plan.rules.get(&name).and_then(|sr| {
                            crate::executor::process::rule_report_caption(&sr.rule, &sr.workdir)
                        }),
                    });
                    done.insert(name.clone(), JobStatus::Skipped);
                    emit(
                        &mut events,
                        "SKIPPED",
                        &name,
                        None,
                        Some("blocked by failed upstream"),
                    );
                }
            }

            // 2. Submit a wave of ready rules up to the in-flight cap,
            // ordered by effective priority (aging, issue #134); the tail
            // passed over this round ages +1.
            let mut submitted_this_round = 0usize;
            if inflight.len() < self.config.max_submitted {
                let ready_now: Vec<String> = plan
                    .order
                    .iter()
                    .filter(|n| {
                        to_run_set.contains(*n)
                            && !done.contains_key(*n)
                            && !inflight.values().any(|f| f.rule == **n)
                            && deps_ok(n, &done, &to_run_set, plan, &blocked)
                    })
                    .cloned()
                    .collect();
                let ordered = Self::aged_ready_order(ready_now, plan, &waited_rounds);
                let available = self.config.max_submitted.saturating_sub(inflight.len());
                let to_submit: Vec<String> = ordered.iter().take(available).cloned().collect();
                for name in ordered.iter().skip(available) {
                    *waited_rounds.entry(name.clone()).or_insert(0) += 1;
                }
                for name in to_submit {
                    if inflight.len() >= self.config.max_submitted {
                        break;
                    }
                    if !to_run_set.contains(&name)
                        || done.contains_key(&name)
                        || inflight.values().any(|f| f.rule == name)
                        || !deps_ok(&name, &done, &to_run_set, plan, &blocked)
                    {
                        continue;
                    }
                    if !plan.rules.contains_key(&name) {
                        // Not schedulable (no shell/script) — mirror the local
                        // executor's skip. The rule has no plan entry to read
                        // a declared caption from, so the record's caption is
                        // always None here.
                        records.push(JobRecord {
                            signal: None,
                            rule: name.clone(),
                            status: JobStatus::Skipped,
                            enqueued_at: None,
                            started_at: None,
                            finished_at: Some(Utc::now()),
                            exit_code: None,
                            stdout: None,
                            stderr: None,
                            command: None,
                            retries: 0,
                            skip_reason: Some("no shell or script defined".into()),
                            max_rss_mb: None,
                            cpu_seconds: None,
                            caption: None,
                        });
                        done.insert(name.clone(), JobStatus::Skipped);
                        emit(
                            &mut events,
                            "SKIPPED",
                            &name,
                            None,
                            Some("no shell or script"),
                        );
                        continue;
                    }
                    let sr = plan.rules[&name].clone();
                    // Array grouping (issue #74 phase 3): batch every READY
                    // sibling instance of the same template with an identical
                    // directive signature into one scheduler array, chunked
                    // at max_array_size. Instance-level records, dependency
                    // gates, and resume semantics are unchanged — arrays are
                    // transport-level (one submission, element-wise records).
                    let siblings: Vec<(String, ScheduledRule)> = if self.config.no_arrays {
                        vec![]
                    } else {
                        plan.order
                            .iter()
                            .filter(|o| {
                                **o != name
                                    && !done.contains_key(*o)
                                    && to_run_set.contains(*o)
                                    && !inflight.values().any(|f| f.rule == **o)
                                    && deps_ok(o, &done, &to_run_set, plan, &blocked)
                                    && plan.rules.get(*o).is_some_and(|o_sr| {
                                        o_sr.template == sr.template
                                            && array_key(o_sr) == array_key(&sr)
                                            && same_dependencies(
                                                &o_sr.dependencies,
                                                &sr.dependencies,
                                            )
                                    })
                            })
                            .map(|o| (o.clone(), plan.rules[o].clone()))
                            .collect()
                    };
                    let mut batch: Vec<(String, ScheduledRule)> = vec![(name.clone(), sr.clone())];
                    batch.extend(siblings);
                    // Chunking walks the batch by hand rather than via
                    // `slice::chunks`: every array's elements count against
                    // the in-flight cap, so the LAST chunk of a batch is cut
                    // to the slots that actually remain (issue #136 H5 fixed
                    // the per-rule overshoot; a 900-element array under a cap
                    // of 50 overshot it just the same).
                    let mut offset = 0usize;
                    let mut chunk_k = 0usize;
                    while offset < batch.len() {
                        if inflight.len() >= self.config.max_submitted {
                            // The cap binds per SUBMISSION, not per rule —
                            // a multi-chunk batch must not overshoot it
                            // (issue #136 H5).
                            break;
                        }
                        let quota = self
                            .config
                            .max_submitted
                            .saturating_sub(inflight.len())
                            .max(1);
                        let size = self.config.max_array_size.max(1).min(quota);
                        let end = (offset + size).min(batch.len());
                        let chunk = &batch[offset..end];
                        offset = end;
                        chunk_k += 1;
                        if chunk.len() > 1 {
                            // Array submission: one script + per-index
                            // command files under a PER-CHUNK job dir —
                            // sibling chunks must never overwrite each
                            // other's cmd files / job.sh while the earlier
                            // array is still pending (issue #136 H3).
                            let job_dir = opts
                                .run_dir
                                .join("jobs")
                                .join(sanitize(&sr.template))
                                .join(format!("chunk-{}", chunk_k));
                            cancel_on_error!(std::fs::create_dir_all(&job_dir).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot create {}: {e}", job_dir.display()),
                                }
                            }));
                            // Routed sensitive references live in the
                            // per-index command files, not the dispatch
                            // line; cloud array bodies still need the union
                            // so their secret channel can provide every
                            // name (issue #1120 / RB-12).
                            let sensitive_refs: Vec<String> = chunk
                                .iter()
                                .flat_map(|(_, sr)| {
                                    crate::executor::process::sensitive_env_refs(&sr.shell_cmd)
                                })
                                .collect::<std::collections::BTreeSet<_>>()
                                .into_iter()
                                .collect();
                            // HPC backends: one private env file per chunk,
                            // sourced by every per-index command file. The
                            // scheduler's copy of `job.sh` therefore carries
                            // only references (issue #1120 / RB-12).
                            let env_file = if self.backend.uses_secret_env_file()
                                && !sensitive_refs.is_empty()
                            {
                                let path = job_dir.join(super::SENSITIVE_ENV_FILE_NAME);
                                cancel_on_error!(write_sensitive_env_file(
                                    &path,
                                    &sensitive_bindings,
                                    &sensitive_refs,
                                    chunk[0].1.rule.environment.kind(),
                                ));
                                Some(path)
                            } else {
                                None
                            };
                            let env_prefix = env_file
                                .as_deref()
                                .map(|path| format!("{} && ", source_sensitive_env_file(path)));
                            for (i, (_, c_sr)) in chunk.iter().enumerate() {
                                let cmd_path = job_dir.join(format!("cmd.{}.sh", i + 1));
                                let contents = match env_prefix {
                                    Some(ref prefix) => format!("{prefix}{}", c_sr.shell_cmd),
                                    None => c_sr.shell_cmd.clone(),
                                };
                                cancel_on_error!(
                                    write_private(&cmd_path, contents.as_bytes()).map_err(|e| {
                                        OxoFlowError::Config {
                                            message: format!(
                                                "cannot write {}: {e}",
                                                cmd_path.display()
                                            ),
                                        }
                                    })
                                );
                            }
                            // The array presents itself under the TEMPLATE
                            // name: naming it after `chunk[0]`'s instance
                            // made squeue/sacct report a job named after one
                            // arbitrary sample. The per-index log paths
                            // follow, so every element shares the template's
                            // log prefix.
                            let mut array_rule = chunk[0].1.rule.clone();
                            array_rule.name = chunk[0].1.template.clone();
                            let script = cancel_on_error!(self.backend.render_array_script(
                                &array_rule,
                                &job_dir.to_string_lossy(),
                                chunk.len(),
                                &sensitive_refs,
                            ));
                            let script_path = job_dir.join("job.sh");
                            cancel_on_error!(
                                write_private(&script_path, script.as_bytes()).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot write {}: {e}",
                                            script_path.display()
                                        ),
                                    }
                                })
                            );
                            let base_id = cancel_on_error!(self.backend.submit(&script_path).await);
                            let submitted_at = Utc::now();
                            // Register accepted elements before any bookkeeping
                            // write. If job.id, index.json, or status.json
                            // fails, fail_with_cancel must still see and
                            // reclaim the scheduler allocation.
                            for (i, (c_name, _)) in chunk.iter().enumerate() {
                                let element_id = self.backend.array_element_id(&base_id, i + 1);
                                inflight.insert(
                                    element_id,
                                    InFlight {
                                        rule: c_name.clone(),
                                        submitted_at,
                                        array_base: Some(base_id.clone()),
                                        env_file: env_file.clone(),
                                    },
                                );
                            }
                            cancel_on_error!(
                                write_private(job_dir.join("job.id").as_path(), base_id.as_bytes())
                                    .map_err(|e| {
                                        OxoFlowError::Config {
                                            message: format!(
                                                "cannot write {}: {e}",
                                                job_dir.join("job.id").display()
                                            ),
                                        }
                                    })
                            );
                            // index.json: array index → instance name, so the
                            // array stays an implementation detail (issue #74
                            // phase 3 — "an array index is meaningless on its
                            // own"). Accumulated in memory; the file is
                            // rewritten from the full map (issue #136 H3).
                            array_index.insert(
                                base_id.clone(),
                                chunk.iter().map(|(n, _)| n.clone()).collect(),
                            );
                            // The array→instance map is what status/resume
                            // read back — a lost write silently misleads
                            // them, so it fails the run like the sibling
                            // run-dir writes above (issue #136 tier-2
                            // audit; the old `let _ =` swallowed it).
                            if let Some(parent) = index_path.parent() {
                                cancel_on_error!(std::fs::create_dir_all(parent).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!("cannot create {}: {e}", parent.display()),
                                    }
                                }));
                            }
                            let index_json = cancel_on_error!(
                                serde_json::to_string_pretty(&array_index).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot serialize {}: {e}",
                                            index_path.display()
                                        ),
                                    }
                                })
                            );
                            cancel_on_error!(std::fs::write(&index_path, index_json).map_err(
                                |e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot write {}: {e}",
                                            index_path.display()
                                        ),
                                    }
                                }
                            ));
                            for (i, (c_name, c_sr)) in chunk.iter().enumerate() {
                                let element_id = self.backend.array_element_id(&base_id, i + 1);
                                // Per-INSTANCE dirs keep the run directory
                                // greppable (jobs/<instance>/job.sh + job.id)
                                // — the array stays an implementation detail
                                // (issue #74 phase 3). job.sh is a copy of
                                // the array script: it is exactly what ran
                                // for this element.
                                let inst_dir = opts.run_dir.join("jobs").join(sanitize(c_name));
                                cancel_on_error!(std::fs::create_dir_all(&inst_dir).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot create {}: {e}",
                                            inst_dir.display()
                                        ),
                                    }
                                }));
                                cancel_on_error!(
                                    std::fs::copy(&script_path, inst_dir.join("job.sh")).map_err(
                                        |e| OxoFlowError::Config {
                                            message: format!(
                                                "cannot copy {}: {e}",
                                                inst_dir.join("job.sh").display()
                                            ),
                                        },
                                    )
                                );
                                cancel_on_error!(
                                    write_private(
                                        inst_dir.join("job.id").as_path(),
                                        element_id.as_bytes()
                                    )
                                    .map_err(|e| {
                                        OxoFlowError::Config {
                                            message: format!(
                                                "cannot write {}: {e}",
                                                inst_dir.join("job.id").display()
                                            ),
                                        }
                                    },)
                                );
                                cancel_on_error!(write_status(
                                    &inst_dir,
                                    &element_id,
                                    &crate::executor::process::mask_sensitive(
                                        &c_sr.shell_cmd,
                                        opts.sensitive_values,
                                    ),
                                    "PENDING",
                                ));
                                emit(&mut events, "SUBMITTED", c_name, Some(&element_id), None);
                                if let Some(ref mut hook) = opts.on_submit {
                                    let fut = hook(c_name.clone(), element_id.clone());
                                    if let Err(e) = fut.await {
                                        tracing::warn!(rule = %c_name, error = %e, "checkpoint submit hook failed");
                                    }
                                }
                            }
                            submitted_this_round += chunk.len();
                        } else {
                            let (c_name, c_sr) = &chunk[0];
                            let job_dir = opts.run_dir.join("jobs").join(sanitize(c_name));
                            cancel_on_error!(std::fs::create_dir_all(&job_dir).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot create {}: {e}", job_dir.display()),
                                }
                            }));
                            // HPC backends: the literal never enters the
                            // scheduler-visible script. A `0o600` env file
                            // beside it carries the routed values and the
                            // command sources it inside the `env -i` shell
                            // (issue #1120 / RB-12).
                            let env_file = {
                                let refs =
                                    crate::executor::process::sensitive_env_refs(&c_sr.shell_cmd);
                                if self.backend.uses_secret_env_file() && !refs.is_empty() {
                                    let path = job_dir.join(super::SENSITIVE_ENV_FILE_NAME);
                                    cancel_on_error!(write_sensitive_env_file(
                                        &path,
                                        &sensitive_bindings,
                                        &refs,
                                        c_sr.rule.environment.kind(),
                                    ));
                                    Some(path)
                                } else {
                                    None
                                }
                            };
                            let mut rendered = c_sr.clone();
                            if let Some(ref path) = env_file {
                                rendered.shell_cmd = format!(
                                    "{} && {}",
                                    source_sensitive_env_file(path),
                                    rendered.shell_cmd
                                );
                            }
                            let script = cancel_on_error!(self.backend.render_script(&rendered));
                            let script_path = job_dir.join("job.sh");
                            cancel_on_error!(
                                write_private(&script_path, script.as_bytes()).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot write {}: {e}",
                                            script_path.display()
                                        ),
                                    }
                                })
                            );
                            let job_id = cancel_on_error!(self.backend.submit(&script_path).await);
                            inflight.insert(
                                job_id.clone(),
                                InFlight {
                                    rule: c_name.clone(),
                                    submitted_at: Utc::now(),
                                    array_base: None,
                                    env_file,
                                },
                            );
                            cancel_on_error!(
                                write_private(job_dir.join("job.id").as_path(), job_id.as_bytes())
                                    .map_err(|e| {
                                        OxoFlowError::Config {
                                            message: format!(
                                                "cannot write {}: {e}",
                                                job_dir.display()
                                            ),
                                        }
                                    })
                            );
                            cancel_on_error!(write_status(
                                &job_dir,
                                &job_id,
                                &crate::executor::process::mask_sensitive(
                                    &c_sr.shell_cmd,
                                    opts.sensitive_values,
                                ),
                                "PENDING",
                            ));
                            submitted_this_round += 1;
                            emit(&mut events, "SUBMITTED", c_name, Some(&job_id), None);
                            if let Some(ref mut hook) = opts.on_submit {
                                let fut = hook(c_name.clone(), job_id.clone());
                                if let Err(e) = fut.await {
                                    tracing::warn!(rule = %c_name, error = %e, "checkpoint submit hook failed");
                                }
                            }
                        }
                    }
                }
            }

            // 3. Poll and settle terminal jobs.
            let mut settled_this_round = 0usize;
            if !inflight.is_empty() {
                // Schedulers that report arrays only by their base id
                // (PBS/LSF/SGE) are polled by BASE id; the verdict then
                // expands to every element of that array (issue #136 H4).
                // SLURM polls element ids directly and keeps per-element
                // fidelity.
                let direct = self.backend.polls_elements_directly();
                let ids: Vec<String> = if direct {
                    inflight.keys().cloned().collect()
                } else {
                    let mut targets: Vec<String> = inflight
                        .values()
                        .filter_map(|f| f.array_base.clone())
                        .collect();
                    targets.extend(
                        inflight
                            .iter()
                            .filter(|(_, f)| f.array_base.is_none())
                            .map(|(id, _)| id.clone()),
                    );
                    targets.sort();
                    targets.dedup();
                    targets
                };
                let polled = cancel_on_error!(self.backend.poll(&ids).await);
                let mut statuses = HashMap::new();
                if direct {
                    statuses = polled;
                } else {
                    for (id, f) in &inflight {
                        let target = f.array_base.as_deref().unwrap_or(id);
                        if let Some(st) = polled.get(target).copied() {
                            statuses.insert(id.clone(), st);
                        }
                    }
                }
                // Short jobs vanish from the live queue the instant they
                // finish (squeue/qstat only list active jobs), so a missing
                // id is NOT "still running" — settle it from the accounting
                // store once it is old enough (a fresh submission can
                // legitimately be absent for a moment; the accounting
                // record itself takes a few seconds to appear). Array
                // elements probe the accounting store by BASE id on
                // non-element backends (their element ids never exist
                // there); SLURM probes per element.
                const ACCOUNT_GRACE_SECS: i64 = 5;
                let now = Utc::now();
                let mut probes: Vec<String> = inflight
                    .iter()
                    .filter(|(id, _)| !statuses.contains_key(*id))
                    .filter(|(_, f)| (now - f.submitted_at).num_seconds() >= ACCOUNT_GRACE_SECS)
                    .map(|(id, f)| {
                        if direct {
                            id.clone()
                        } else {
                            f.array_base.clone().unwrap_or_else(|| id.clone())
                        }
                    })
                    .collect();
                probes.sort();
                probes.dedup();
                let mut accounting: HashMap<String, TerminalRecord> = HashMap::new();
                for probe in probes {
                    if let Some(rec) = self.backend.terminal_status(&probe).await
                        && rec.status != BackendJobStatus::Unknown
                    {
                        if direct {
                            statuses.insert(probe.clone(), rec.status);
                            accounting.insert(probe, rec);
                        } else {
                            // Base verdict expands to every element of the
                            // array (per-element accounting fidelity stays
                            // SLURM-only).
                            for (id, f) in &inflight {
                                let target = f.array_base.as_deref().unwrap_or(id);
                                if target == probe {
                                    statuses.insert(id.clone(), rec.status);
                                    accounting.insert(id.clone(), rec);
                                }
                            }
                        }
                    }
                }
                // Queue-wait heartbeat: a job parked in PENDING used to be
                // completely silent, so a driver facing a full partition was
                // indistinguishable from a hung one. Every
                // PENDING_HEARTBEAT_ROUNDS polls each waiting job gets one
                // line naming the rule, the job id, and how long it has been
                // waiting. A job that left the queue drops out of the count.
                for id in inflight.keys() {
                    let is_waiting = statuses
                        .get(id)
                        .is_none_or(|s| *s == BackendJobStatus::Pending);
                    if is_waiting {
                        *pending_rounds.entry(id.clone()).or_insert(0) += 1;
                    } else {
                        pending_rounds.remove(id);
                    }
                }
                let mut waiting: Vec<(&String, &InFlight, u32)> = inflight
                    .iter()
                    .filter(|(id, _)| {
                        pending_rounds
                            .get(*id)
                            .is_some_and(|r| r % PENDING_HEARTBEAT_ROUNDS == 0)
                    })
                    .map(|(id, f)| (id, f, pending_rounds[id]))
                    .collect();
                waiting.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
                for (id, f, rounds) in waiting {
                    let waited = (now - f.submitted_at).num_seconds();
                    tracing::info!(
                        rule = %f.rule,
                        job = %id,
                        pending_polls = rounds,
                        waited_secs = waited,
                        "job is still waiting in the queue"
                    );
                    // The event stream keeps the wait on record after the run
                    // ends — `queue_wait_secs` only exists once the job
                    // settles, and a job killed mid-queue never does.
                    emit(
                        &mut events,
                        "WAITING",
                        &f.rule,
                        Some(id),
                        Some(&format!("pending for {waited}s")),
                    );
                }
                // Terminal-unknown settlement guard (issue #244): a job
                // gone from the live queue whose accounting store (and the
                // scontrol fallback) yields NOTHING is otherwise polled
                // forever — on a slurmdbd-less cluster the engine hung
                // indefinitely (queue empty, sacct empty, zero progress).
                // After UNKNOWN_SETTLE_ROUNDS consecutive blind rounds the
                // job settles as Failed with exit code unavailable and a
                // loud warning telling the operator to verify via output
                // files. Never an infinite poll.
                for (id, f) in inflight.iter() {
                    let age = (now - f.submitted_at).to_std().unwrap_or_default();
                    match statuses.get(id) {
                        None | Some(BackendJobStatus::Unknown)
                            if age >= self.config.unknown_settle_grace =>
                        {
                            *unknown_rounds.entry(id.clone()).or_insert(0) += 1;
                        }
                        _ => {
                            unknown_rounds.remove(id);
                        }
                    }
                }
                let mut blind: Vec<String> = unknown_rounds
                    .iter()
                    .filter(|(_, r)| **r >= UNKNOWN_SETTLE_ROUNDS)
                    .map(|(id, _)| id.clone())
                    .collect();
                blind.sort();
                for id in &blind {
                    if let Some(f) = inflight.get(id) {
                        tracing::warn!(
                            rule = %f.rule,
                            job = %id,
                            blind_rounds = unknown_rounds.get(id).copied().unwrap_or(0),
                            "job left the queue but no terminal state is available from the scheduler or accounting store — settling as FAILED with exit code unknown; verify via the rule's output files"
                        );
                        emit(
                            &mut events,
                            "FAILED",
                            &f.rule,
                            Some(id),
                            Some(
                                "terminal state unavailable (no accounting store?) — verify via output files",
                            ),
                        );
                        statuses.insert(id.clone(), BackendJobStatus::Failed);
                    }
                    unknown_rounds.remove(id);
                }
                let settled: Vec<(String, InFlight, BackendJobStatus)> = inflight
                    .iter()
                    .filter_map(|(id, f)| statuses.get(id).map(|s| (id.clone(), f.clone(), *s)))
                    .filter(|(_, _, s)| {
                        *s != BackendJobStatus::Pending
                            && *s != BackendJobStatus::Running
                            && *s != BackendJobStatus::Unknown
                    })
                    .collect();
                settled_this_round = settled.len();
                // Jobs the live poller settled have no accounting yet: the
                // poller reports the state, the store reports what the job
                // cost. One lookup per job as it finishes, not per poll.
                for (job_id, f, _) in &settled {
                    if accounting.contains_key(job_id) {
                        continue;
                    }
                    // On backends whose accounting store never knew the
                    // element ids, the array base is the only key that
                    // resolves.
                    let probe = if direct {
                        job_id.as_str()
                    } else {
                        f.array_base.as_deref().unwrap_or(job_id)
                    };
                    if let Some(rec) = self.backend.terminal_status(probe).await {
                        accounting.insert(job_id.clone(), rec);
                    }
                }
                // When the driver NOTICED the job was terminal, which is not
                // when the job ended: a poll interval, plus the accounting
                // grace period for jobs that vanished from the live queue,
                // can sit in between. One instant for the whole batch keeps
                // `started_at`, `finished_at`, and `queue_wait_secs`
                // consistent with each other instead of drifting apart by
                // however long the writes took.
                let observed_at = Utc::now();
                // Pre-remove EVERY settled id before any bookkeeping write.
                // Removing one at a time left same-round siblings in
                // `inflight`; a write failure then made `fail_with_cancel`
                // try to cancel jobs the scheduler had just reported
                // terminal, and a scheduler that rejects cancelling a
                // finished job would produce a false "may still be running"
                // message (review of #1188, D1).
                for (job_id, _, _) in &settled {
                    inflight.remove(job_id);
                }
                // Issue #1120 / RB-12: once no in-flight job still needs a
                // secret env file (a chunk's array elements share one), it is
                // removed — the value must not outlive the job it belongs to.
                for (_, f, _) in &settled {
                    let Some(path) = f.env_file.as_ref() else {
                        continue;
                    };
                    let still_needed = inflight
                        .values()
                        .any(|other| other.env_file.as_deref() == Some(path.as_path()));
                    if !still_needed {
                        let _ = std::fs::remove_file(path);
                    }
                }
                for (job_id, f, status) in settled {
                    // Array elements on a base-only backend (PBS/LSF/SGE)
                    // carry an aggregate verdict, not an independently
                    // observed one: this adapter's poll invocations see only
                    // the array base id (issue #136 H4, live-verified). The
                    // record therefore drops the aggregate's process-level
                    // numbers (exit code, memory, CPU — they are not the
                    // element's numbers) and names its provenance in
                    // `skip_reason` below, so a checkpoint never presents
                    // the aggregate as a per-element measurement (issue
                    // #1132 / ledger RB-06). The aggregate DURATION is kept:
                    // it describes the shared array allocation, which is
                    // how long this element ran. Cloud backends poll
                    // elements directly and never set this.
                    let base_only_array = (!direct && f.array_base.is_some()).then(|| {
                        format!(
                            "array base {} verdict {}: per-element outcome unavailable from {}",
                            f.array_base.as_deref().unwrap_or_default(),
                            status.label(),
                            self.backend.name()
                        )
                    });
                    // On a base-only backend the accounting record read for
                    // an ARRAY ELEMENT is the whole array's aggregate. Its
                    // DURATION genuinely describes this element — an array
                    // runs its elements as one scheduler allocation, so the
                    // aggregate `Elapsed` IS how long each element spent
                    // running — but its exit code / memory / CPU belong to
                    // whichever element the scheduler happened to report, so
                    // they are dropped and `skip_reason` names the
                    // provenance. Keeping the duration while withholding
                    // every per-process number is exactly what the non-array
                    // path does: `started_at` (hence
                    // `benchmarks.wall_time_secs` and `queue_wait_secs`)
                    // comes from the store's run duration instead of
                    // submit-to-settle, so wall time stops absorbing queue
                    // wait (issue #1132 F2). With no duration the record is
                    // `None` and submit-to-settle stays the fallback.
                    let acct = accounting.get(&job_id).copied().map(|a| {
                        if base_only_array.is_some() {
                            TerminalRecord {
                                status: a.status,
                                exit_code: None,
                                elapsed_secs: a.elapsed_secs,
                                max_rss_mb: None,
                                cpu_seconds: None,
                            }
                        } else {
                            a
                        }
                    });
                    // The driver only ever sees submit-to-settle. When the
                    // store reports how long the job RAN, the start time is
                    // recomputed from it so `benchmarks.wall_time_secs`
                    // stops silently including queue wait.
                    let started_at = acct
                        .and_then(|a| a.elapsed_secs)
                        .and_then(|s| i64::try_from(s).ok())
                        .and_then(|s| observed_at.checked_sub_signed(chrono::Duration::seconds(s)))
                        .unwrap_or(f.submitted_at);
                    let record = match status {
                        BackendJobStatus::Completed => {
                            // Checkpoint re-entry (P3): new instances merge
                            // and are added to the work set.
                            if plan.rules[&f.rule].rule.checkpoint
                                && let Some(ref mut hook) = opts.on_checkpoint
                            {
                                match hook(&f.rule) {
                                    Err(e) => {
                                        cancel_on_error!(write_status(
                                            &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                            &job_id,
                                            "",
                                            "FAILED",
                                        ));
                                        let record =
                                            return_record_failure(&mut records, &mut done, &f, &e);
                                        // Issue #1083 F2: this failure record
                                        // never reaches the main settle site
                                        // below, so fire the settle hook here
                                        // too — a crash between this settle
                                        // and run() returning must not lose
                                        // the verdict either.
                                        if let Some(ref mut hook) = opts.on_settle
                                            && let Err(e) = hook(record.clone()).await
                                        {
                                            tracing::warn!(
                                                rule = %record.rule,
                                                error = %e,
                                                "checkpoint settle hook failed"
                                            );
                                        }
                                        emit(
                                            &mut events,
                                            "FAILED",
                                            &f.rule,
                                            Some(&job_id),
                                            Some(&e.to_string()),
                                        );
                                        continue;
                                    }
                                    Ok(new_names) => {
                                        if !new_names.is_empty() {
                                            match &mut opts.merge {
                                                Some(merge) => {
                                                    cancel_on_error!(merge(plan, &new_names));
                                                    for n in &new_names {
                                                        to_run_set.insert(n.clone());
                                                    }
                                                }
                                                None => {
                                                    let error = OxoFlowError::Config {
                                                        message: "checkpoint re-entry produced new instances but no merge hook is configured".into(),
                                                    };
                                                    return self
                                                        .fail_with_cancel(&inflight, error)
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            cancel_on_error!(write_status_full(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                &crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                ),
                                "COMPLETED",
                                Some((f.submitted_at, observed_at)),
                                acct,
                            ));
                            JobRecord {
                                signal: None,
                                rule: f.rule.clone(),
                                status: JobStatus::Success,
                                enqueued_at: Some(f.submitted_at),
                                started_at: Some(started_at),
                                finished_at: Some(observed_at),
                                // A Completed verdict proves the scheduler
                                // saw the job finish; only the accounting
                                // store can say with WHICH code. Without an
                                // accounting record the code is unknown, not
                                // 0 — the same rule the Failed branch applies
                                // below (issue #1132 / ledger RB-06).
                                exit_code: acct.and_then(|a| a.exit_code),
                                stdout: None,
                                stderr: None,
                                command: Some(crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                )),
                                retries: 0,
                                skip_reason: base_only_array.clone(),
                                max_rss_mb: acct.and_then(|a| a.max_rss_mb),
                                cpu_seconds: acct.and_then(|a| a.cpu_seconds).map(|s| s as f64),
                                caption: crate::executor::process::rule_report_caption(
                                    &plan.rules[&f.rule].rule,
                                    &plan.rules[&f.rule].workdir,
                                ),
                            }
                        }
                        BackendJobStatus::Failed => {
                            cancel_on_error!(write_status_full(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                "",
                                "FAILED",
                                Some((f.submitted_at, observed_at)),
                                acct,
                            ));
                            // Fail fast, mirroring the local executor's
                            // abort-on-first-failure default (run.rs): the
                            // remaining inflight entries are exactly this
                            // failure's live siblings, and a run that can no
                            // longer succeed must not keep billing for them.
                            // Non-required failures leave the run salvageable
                            // (record_outcome counts them separately), so
                            // only a required rule's failure cancels.
                            //
                            // `-k/--keep-going` opts out of that decision
                            // (issue #1093 F2b): the failure is still
                            // recorded and still fails the run, but
                            // independent jobs already in flight finish and
                            // the driver keeps dispatching ready independent
                            // instances until the queue drains. Dependents
                            // of this rule are blocked by the propagation
                            // step at the top of each round.
                            if plan.rules[&f.rule].rule.required && !self.config.keep_going {
                                let siblings: Vec<(String, String, Option<String>)> = inflight
                                    .iter()
                                    .map(|(id, f)| {
                                        (id.clone(), f.rule.clone(), f.array_base.clone())
                                    })
                                    .collect();
                                if !siblings.is_empty() {
                                    tracing::warn!(
                                        rule = %f.rule,
                                        job = %job_id,
                                        cancelling = siblings.len(),
                                        "required rule failed — cancelling remaining in-flight jobs"
                                    );
                                    // The run already fails on the required
                                    // rule's record; a cleanup failure is
                                    // reported loudly here but must not
                                    // replace that verdict or drop the
                                    // records gathered so far (issue #1156).
                                    if let Err(cancel_error) = self.cancel_inflight(&siblings).await
                                    {
                                        tracing::error!(
                                            rule = %f.rule,
                                            error = %cancel_error,
                                            "required rule failed and some in-flight jobs could not be cancelled"
                                        );
                                    }
                                }
                            } else if plan.rules[&f.rule].rule.required {
                                tracing::warn!(
                                    rule = %f.rule,
                                    job = %job_id,
                                    "required rule failed — --keep-going: recording the failure and continuing with independent jobs (dependents stay skipped)"
                                );
                            }
                            JobRecord {
                                signal: None,
                                rule: f.rule.clone(),
                                status: JobStatus::Failed,
                                enqueued_at: Some(f.submitted_at),
                                started_at: Some(started_at),
                                finished_at: Some(observed_at),
                                // Without accounting the code is unknown, not
                                // 1: reporting a rule that exited 7 as 1 is a
                                // wrong answer where none was available.
                                exit_code: acct.and_then(|a| a.exit_code),
                                stdout: None,
                                stderr: None,
                                command: Some(crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                )),
                                retries: 0,
                                skip_reason: base_only_array.clone(),
                                max_rss_mb: acct.and_then(|a| a.max_rss_mb),
                                cpu_seconds: acct.and_then(|a| a.cpu_seconds).map(|s| s as f64),
                                caption: crate::executor::process::rule_report_caption(
                                    &plan.rules[&f.rule].rule,
                                    &plan.rules[&f.rule].workdir,
                                ),
                            }
                        }
                        BackendJobStatus::Cancelled => JobRecord {
                            signal: None,
                            rule: f.rule.clone(),
                            status: JobStatus::Cancelled,
                            enqueued_at: Some(f.submitted_at),
                            started_at: Some(started_at),
                            finished_at: Some(observed_at),
                            exit_code: acct.and_then(|a| a.exit_code),
                            stdout: None,
                            stderr: None,
                            command: Some(crate::executor::process::mask_sensitive(
                                &plan.rules[&f.rule].shell_cmd,
                                opts.sensitive_values,
                            )),
                            retries: 0,
                            skip_reason: Some(
                                base_only_array
                                    .clone()
                                    .unwrap_or_else(|| "cancelled".into()),
                            ),
                            max_rss_mb: acct.and_then(|a| a.max_rss_mb),
                            cpu_seconds: acct.and_then(|a| a.cpu_seconds).map(|s| s as f64),
                            caption: crate::executor::process::rule_report_caption(
                                &plan.rules[&f.rule].rule,
                                &plan.rules[&f.rule].workdir,
                            ),
                        },
                        _ => unreachable!("non-terminal states filtered above"),
                    };
                    let event_t = record_t(&record.status);
                    emit(
                        &mut events,
                        event_t,
                        &f.rule,
                        Some(&job_id),
                        record.skip_reason.as_deref(),
                    );
                    done.insert(f.rule.clone(), record.status);
                    // Issue #1083: persist the terminal verdict before the
                    // next polling round — a driver killed from here on
                    // leaves a checkpoint that already knows what finished.
                    if let Some(ref mut hook) = opts.on_settle
                        && let Err(e) = hook(record.clone()).await
                    {
                        tracing::warn!(
                            rule = %record.rule,
                            error = %e,
                            "checkpoint settle hook failed"
                        );
                    }
                    records.push(record);
                }
            }

            // 4. Exit, stall, or deadline.
            let all_done = plan
                .order
                .iter()
                .all(|n| !to_run_set.contains(n) || done.contains_key(n));
            if all_done && inflight.is_empty() {
                break;
            }
            // A stall is "nothing in flight, nothing submitted AND nothing
            // settled this round": the wave above already tried every ready
            // rule, so the remaining pending rules are blocked on deps that
            // can never finish (plan/dependency mismatch). Settling counts —
            // a completion can unblock dependents that the NEXT round's wave
            // will pick up.
            if !all_done
                && inflight.is_empty()
                && submitted_this_round == 0
                && settled_this_round == 0
            {
                return Err(OxoFlowError::Config {
                    message: "driver stall: no jobs in flight but rules remain pending (plan/dependency mismatch)".into(),
                });
            }
            if let Some(d) = deadline
                && Instant::now() > d
            {
                let jobs: Vec<(String, String, Option<String>)> = inflight
                    .iter()
                    .map(|(id, f)| (id.clone(), f.rule.clone(), f.array_base.clone()))
                    .collect();
                let timeout = OxoFlowError::Config {
                    message: "poll timeout exceeded".into(),
                };
                return Err(match self.cancel_inflight(&jobs).await {
                    Ok(()) => timeout,
                    // Never drop the unresolved job ids from the terminal
                    // error: the run is ending, and jobs the scheduler did
                    // not confirm cancelled may still be running (#1156).
                    Err(cancel_error) => append_error_context(timeout, &cancel_error.to_string()),
                });
            }
            // Interrupt requested (Ctrl-C / SIGTERM / SIGHUP): cancel every
            // in-flight job, record each as interrupted, and end the run
            // with a Cancelled error carrying those records — the caller
            // persists them like any terminal state.
            if let Some(cancel) = &self.config.cancel
                && let Some(sig) = cancel.borrow().as_ref()
            {
                let signal_name = sig.signal_name;
                let exit_code = sig.exit_code();
                let jobs: Vec<(String, String, Option<String>)> = inflight
                    .iter()
                    .map(|(id, f)| (id.clone(), f.rule.clone(), f.array_base.clone()))
                    .collect();
                // Keep going even when a cancel is not confirmed: the
                // interrupted records are the only truthful account of what
                // the signal stopped and must never be dropped. The outcome
                // rides in the terminal message instead — an unconfirmed
                // cancel is never reported as "cancelled" (issue #1156).
                let cancel_outcome = self.cancel_inflight(&jobs).await;
                let mut interrupted: Vec<JobRecord> = Vec::new();
                for (job_id, f) in inflight.iter() {
                    let reason = format!("interrupted by {signal_name}");
                    let job_dir = opts.run_dir.join("jobs").join(sanitize(&f.rule));
                    let status_id = f.array_base.as_deref().unwrap_or(job_id);
                    cancel_on_error!(write_status_full(
                        &job_dir,
                        status_id,
                        "",
                        "FAILED",
                        Some((f.submitted_at, Utc::now())),
                        None,
                    ));
                    emit(
                        &mut events,
                        "FAILED",
                        &f.rule,
                        Some(status_id),
                        Some(&reason),
                    );
                    interrupted.push(JobRecord {
                        signal: Some(sig.signum),
                        rule: f.rule.clone(),
                        // Failed, not a fabricated exit: the job was killed
                        // by the signal, so the exit code is unknown (never
                        // fabricate — issue #525).
                        status: JobStatus::Failed,
                        enqueued_at: Some(f.submitted_at),
                        started_at: Some(f.submitted_at),
                        finished_at: Some(Utc::now()),
                        exit_code: None,
                        stdout: None,
                        stderr: None,
                        command: Some(crate::executor::process::mask_sensitive(
                            &plan.rules[&f.rule].shell_cmd,
                            opts.sensitive_values,
                        )),
                        retries: 0,
                        skip_reason: Some(reason),
                        max_rss_mb: None,
                        cpu_seconds: None,
                        caption: crate::executor::process::rule_report_caption(
                            &plan.rules[&f.rule].rule,
                            &plan.rules[&f.rule].workdir,
                        ),
                    });
                }
                tracing::warn!(
                    signal = %signal_name,
                    cancelled = interrupted.len(),
                    "interrupt received — cancelling in-flight jobs"
                );
                let message = match cancel_outcome {
                    Ok(()) => format!("interrupted by {signal_name}; in-flight jobs cancelled"),
                    Err(cancel_error) => format!("interrupted by {signal_name}; {cancel_error}"),
                };
                return Err(OxoFlowError::Cancelled {
                    message,
                    records: interrupted,
                    signal_name: signal_name.to_string(),
                    exit_code,
                });
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
        Ok(records)
    }
}

/// Append `context` to an error's message, preserving the original failure
/// text so the cause stays first and greppable (e.g. the exact run-dir file
/// that could not be written). Used to attach the scheduler cancel outcome
/// to a terminal driver error — a failed cleanup must never be reported as
/// a successful cancellation (issue #1156, ledger CL-01).
fn append_error_context(error: OxoFlowError, context: &str) -> OxoFlowError {
    match error {
        OxoFlowError::Config { message } => OxoFlowError::Config {
            message: format!("{message}; {context}"),
        },
        other => OxoFlowError::Config {
            message: format!("{other}; {context}"),
        },
    }
}

/// Is every dependency of `name` satisfied?
///
/// A dependency outside `to_run` is satisfied by definition: the caller's
/// invalidation analysis decided it is already complete, so it is never
/// submitted and never lands in `done`. Requiring it there would wedge every
/// partial re-run — resume after a failure, an edit to a downstream rule,
/// `--target` on a leaf — into the stall branch below with nothing submitted.
///
/// A dependency in `blocked` is NOT satisfied even though `done` records it
/// as Skipped: that Skipped means "an upstream failed", so releasing
/// downstream rules would run them against stale or missing inputs and
/// record success (live audit finding — a gather rule completed after its
/// whole prep array failed). Only genuinely up-to-date skips — which never
/// enter `to_run` — may satisfy a dependency.
fn deps_ok(
    name: &str,
    done: &HashMap<String, JobStatus>,
    to_run: &HashSet<String>,
    plan: &ScheduledPlan,
    blocked: &HashSet<String>,
) -> bool {
    plan.rules.get(name).is_none_or(|sr| {
        sr.dependencies.iter().all(|d| {
            !to_run.contains(d)
                || (!blocked.contains(d)
                    && matches!(done.get(d), Some(JobStatus::Success | JobStatus::Skipped)))
        })
    })
}

fn record_t(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Success => "COMPLETED",
        JobStatus::Failed => "FAILED",
        JobStatus::Skipped => "SKIPPED",
        JobStatus::Cancelled => "CANCELLED",
        _ => "UNKNOWN",
    }
}

/// Append one event to `events.jsonl`.
///
/// Every line carries an RFC 3339 `ts`. Ordering alone cannot answer the
/// questions the log exists for — how long a job waited in the queue, where
/// a run's wall-clock actually went, what was in flight when the driver
/// died — and the timestamp has to be written at emit time because nothing
/// downstream can reconstruct it afterwards.
fn emit(events: &mut std::fs::File, t: &str, rule: &str, job: Option<&str>, reason: Option<&str>) {
    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    // serde_json, not Rust's {finding} Debug (#536): Debug escapes control
    // bytes as \u{1} (invalid JSON) — commands embed arbitrary user shell
    // text and reason embeds scheduler stderr.
    let mut obj = serde_json::json!({
        "ts": ts,
        "t": t,
        "rule": rule,
    });
    if let Some(j) = job {
        obj["job"] = serde_json::Value::String(j.to_string());
    }
    if let Some(r) = reason {
        obj["reason"] = serde_json::Value::String(r.to_string());
    }
    if let Err(e) = writeln!(events, "{obj}") {
        // The events stream is the cluster run's audit trail — silently
        // swallowing write failures (full disk, closed handle) truncated
        // the trail with no warning anywhere, defeating the post-mortem
        // analysis this file exists for (audit #655). Warn once; the run
        // itself continues unaffected.
        use std::sync::atomic::{AtomicBool, Ordering};
        static EMIT_WARNED: AtomicBool = AtomicBool::new(false);
        if !EMIT_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "cluster events.jsonl write failed — the event trail from here on is incomplete: {e}"
            );
        }
    }
}

fn write_status(job_dir: &Path, job_id: &str, command: &str, state: &str) -> Result<()> {
    write_status_full(job_dir, job_id, command, state, None, None)
}

/// Remove every private secret env file below `run_dir/jobs` (issue #1120 /
/// ledger RB-12).
///
/// Runs at the start of a driver run (a crashed predecessor may have left
/// files behind, and a resume reuses the directory) and on every exit path.
/// The per-job removal in the settle loop is the normal case; this is the
/// backstop that also covers cancellation, driver errors, and interrupts.
fn sweep_sensitive_env_files(run_dir: &Path) {
    fn walk(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path);
            } else if path
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new(super::SENSITIVE_ENV_FILE_NAME))
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    walk(&run_dir.join("jobs"));
}

/// Write `jobs/<instance>/status.json`.
///
/// Terminal writes carry the accounting record plus the submit and
/// observation timestamps, so the file answers on its own what the job cost.
/// `queue_wait_secs` is derived here rather than left to the reader: it is
/// submit-to-observed minus the scheduler's `Elapsed`, and that subtraction
/// is the only way to separate time spent waiting from time spent running —
/// the driver never sees the moment a job starts. It is an upper bound: the
/// poll interval and the accounting grace period both land inside it.
fn write_status_full(
    job_dir: &Path,
    job_id: &str,
    command: &str,
    state: &str,
    times: Option<(DateTime<Utc>, DateTime<Utc>)>,
    acct: Option<TerminalRecord>,
) -> Result<()> {
    std::fs::create_dir_all(job_dir).map_err(|e| OxoFlowError::Config {
        message: format!("cannot create {}: {e}", job_dir.display()),
    })?;
    // serde_json, not {heading} Debug — same reasoning as `emit` (#536):
    // `command` carries arbitrary user shell text whose control bytes made
    // status.json unparseable.
    let json_string = |v: &str| serde_json::Value::String(v.to_string()).to_string();
    let mut fields = vec![
        format!(r#""state":{}"#, json_string(state)),
        format!(r#""job_id":{}"#, json_string(job_id)),
        format!(r#""command":{}"#, json_string(command)),
    ];
    if let Some((submitted, observed)) = times {
        let fmt = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        fields.push(format!(r#""submitted_at":"{}""#, fmt(submitted)));
        fields.push(format!(r#""finished_at":"{}""#, fmt(observed)));
        if let Some(elapsed) = acct.and_then(|a| a.elapsed_secs) {
            let wait = (observed - submitted)
                .num_seconds()
                .saturating_sub(elapsed as i64)
                .max(0);
            fields.push(format!(r#""queue_wait_secs":{wait}"#));
        }
    }
    if let Some(a) = acct {
        if let Some(code) = a.exit_code {
            fields.push(format!(r#""exit_code":{code}"#));
        }
        if let Some(secs) = a.elapsed_secs {
            fields.push(format!(r#""elapsed_secs":{secs}"#));
        }
        if let Some(mb) = a.max_rss_mb {
            fields.push(format!(r#""max_rss_mb":{mb}"#));
        }
        if let Some(secs) = a.cpu_seconds {
            fields.push(format!(r#""cpu_seconds":{secs}"#));
        }
    }
    let body = format!("{{{}}}", fields.join(","));
    write_private(job_dir.join("status.json").as_path(), body.as_bytes())
}

/// Push a Failed record for a rule whose checkpoint re-entry hook errored
/// (issue #78): the manifest is unreadable, so the rule counts as failed
/// even though the backend reported the job Completed. Returns the record
/// so the settle-hook fire sites can pass it to the caller's fold (#1083).
fn return_record_failure(
    records: &mut Vec<JobRecord>,
    done: &mut HashMap<String, JobStatus>,
    f: &InFlight,
    e: &OxoFlowError,
) -> JobRecord {
    let record = JobRecord {
        signal: None,
        rule: f.rule.clone(),
        status: JobStatus::Failed,
        enqueued_at: Some(f.submitted_at),
        started_at: Some(f.submitted_at),
        finished_at: Some(Utc::now()),
        exit_code: Some(1),
        stdout: None,
        stderr: None,
        command: None,
        retries: 0,
        skip_reason: Some(format!("re-entry manifest: {e}")),
        max_rss_mb: None,
        cpu_seconds: None,
        caption: None,
    };
    records.push(record.clone());
    done.insert(f.rule.clone(), JobStatus::Failed);
    record
}

/// Job-directory-safe rule names: alphanumerics plus `. _ - =` survive,
/// everything else becomes `_` (rule names already use `_` separators; the
/// `=` keeps the greppable `jobs/<rule>/<key>=<value>` layout from #74).
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '=') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_json_round_trips_control_bytes() {
        // #536: Rust's {finding} Debug escapes control bytes as \u{1}
        // (invalid JSON); serde_json emits \u0001, so the resume/status
        // readers and downstream tooling can always parse the file.
        let dir = tempfile::tempdir().unwrap();
        let job_dir = dir.path().join("job");
        let hostile = "echo \u{1} hostile \u{7}; rm -rf ~";
        write_status(&job_dir, "id\u{2}x", hostile, "completed").unwrap();
        let raw = std::fs::read_to_string(job_dir.join("status.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("status.json must parse: {e}\n{raw}"));
        assert_eq!(parsed["command"], hostile);
        assert_eq!(parsed["job_id"], "id\u{2}x");
    }

    use super::*;
    use crate::backend::ScheduledPlan;
    use crate::backend::cluster::ClusterExecutor;
    use crate::cluster::{ClusterBackend, ClusterJobConfig};
    use crate::config::WorkflowConfig;
    use crate::dag::WorkflowDag;
    use crate::environment::EnvironmentResolver;
    use crate::executor::JobStatus;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn fixtures_dir() -> PathBuf {
        // <repo>/crates/oxo-flow-core → <repo>/tests/fixtures/mock-scheduler
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mock-scheduler")
    }

    struct Fixture {
        _state: tempfile::TempDir,
        workdir: tempfile::TempDir,
        run_dir: tempfile::TempDir,
    }

    fn setup() -> Fixture {
        Fixture {
            _state: tempfile::tempdir().unwrap(),
            workdir: tempfile::tempdir().unwrap(),
            run_dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cluster_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::Slurm,
            queue: None,
            account: None,
            walltime: None,
            extra_args: vec![],
        }
    }

    /// A plan with declared priorities and no dependencies (all ready).
    fn priority_plan(workdir: &Path, rules: &[(&str, i32)]) -> ScheduledPlan {
        let mut toml = String::from("[workflow]\nname = \"prio\"\n");
        for (name, priority) in rules {
            toml.push_str(&format!(
                "[[rules]]\nname = \"{name}\"\npriority = {priority}\n\
                 shell = \"true\"\noutput = [\"{name}.txt\"]\n"
            ));
        }
        let config: WorkflowConfig = toml::from_str(&toml).unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let resolver = EnvironmentResolver::with_cache_dir(&workdir.join(".oxo-flow/env-cache"));
        ScheduledPlan::build(&config, &dag, workdir, &resolver, &HashMap::new()).unwrap()
    }

    #[test]
    fn aged_ready_order_sorts_by_declared_priority_then_name() {
        let workdir = setup().workdir;
        let plan = priority_plan(
            workdir.path(),
            &[("high", 20), ("low", 10), ("mid_a", 15), ("mid_b", 15)],
        );
        let waited = HashMap::new();
        let ready = plan.order.clone();
        let ordered = BackendDriver::aged_ready_order(ready, &plan, &waited);
        assert_eq!(ordered, vec!["high", "mid_a", "mid_b", "low"]);
    }

    #[test]
    fn aged_ready_order_aged_rule_overtakes_higher_declared_priority() {
        // The cluster counterpart of the local aging guarantee (issue #134):
        // a producer passed over at the cap must eventually outrank fresh
        // high-priority rules — priority alone must never starve it forever.
        let workdir = setup().workdir;
        let plan = priority_plan(workdir.path(), &[("merge", 20), ("dump", 10)]);
        let waited = [("dump".to_string(), 15)].into_iter().collect();
        let ordered = BackendDriver::aged_ready_order(plan.order.clone(), &plan, &waited);
        assert_eq!(ordered, vec!["dump", "merge"]);
    }

    #[test]
    fn aged_ready_order_treats_missing_priority_and_waits_as_zero() {
        let workdir = setup().workdir;
        let plan = priority_plan(workdir.path(), &[("z", 0), ("a", 0)]);
        let waited = HashMap::new();
        let ordered = BackendDriver::aged_ready_order(plan.order.clone(), &plan, &waited);
        assert_eq!(ordered, vec!["a", "z"]);
    }

    /// A two-sample diamond (`a → (b ∥ c) → d`) built through the real
    /// expansion path, so `ScheduledPlan::build` records the fan-out keys
    /// the driver uses for tie-breaking (issue #1089). `boosted` gives one
    /// template `priority = 7`.
    fn diamond_plan(workdir: &Path, dispatch_order: &str, boosted: Option<&str>) -> ScheduledPlan {
        let priority = |name: &str| {
            if boosted == Some(name) {
                "\npriority = 7"
            } else {
                ""
            }
        };
        let toml = format!(
            r#"
[workflow]
name = "diamond"
[engine]
dispatch_order = "{dispatch_order}"
[[sample_groups]]
name = "batch"
samples = ["S1", "S2"]
[[rules]]
name = "a"{pa}
shell = "touch out/{{sample}}/a.txt"
output = ["out/{{sample}}/a.txt"]
[[rules]]
name = "b"{pb}
input = ["out/{{sample}}/a.txt"]
shell = "touch out/{{sample}}/b.txt"
output = ["out/{{sample}}/b.txt"]
[[rules]]
name = "c"{pc}
input = ["out/{{sample}}/a.txt"]
shell = "touch out/{{sample}}/c.txt"
output = ["out/{{sample}}/c.txt"]
[[rules]]
name = "d"{pd}
input = ["out/{{sample}}/b.txt", "out/{{sample}}/c.txt"]
shell = "touch out/{{sample}}/d.txt"
output = ["out/{{sample}}/d.txt"]
"#,
            pa = priority("a"),
            pb = priority("b"),
            pc = priority("c"),
            pd = priority("d"),
        );
        let wf = workdir.join(format!("diamond_{dispatch_order}.oxoflow"));
        std::fs::write(&wf, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
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
    fn aged_ready_order_rule_mode_keeps_name_tie_break() {
        // Regression baseline (issue #1089): the default mode still sorts
        // equal-priority instances by name — every `a` instance first, then
        // `b`, and so on.
        let workdir = setup().workdir;
        let plan = diamond_plan(workdir.path(), "rule", None);
        let ordered = BackendDriver::aged_ready_order(plan.order.clone(), &plan, &HashMap::new());
        assert_eq!(
            ordered,
            vec![
                "a_batch_S1",
                "a_batch_S2",
                "b_batch_S1",
                "b_batch_S2",
                "c_batch_S1",
                "c_batch_S2",
                "d_batch_S1",
                "d_batch_S2",
            ]
        );
    }

    #[test]
    fn aged_ready_order_sample_mode_groups_by_sample_then_declaration() {
        // Issue #1089: equal effective priority, ordering by the instance's
        // sample binding and then by `[[rules]]` declaration order — not by
        // name. All of sample 1's stages precede sample 2's.
        let workdir = setup().workdir;
        let plan = diamond_plan(workdir.path(), "sample", None);
        let ordered = BackendDriver::aged_ready_order(plan.order.clone(), &plan, &HashMap::new());
        assert_eq!(
            ordered,
            vec![
                "a_batch_S1",
                "b_batch_S1",
                "c_batch_S1",
                "d_batch_S1",
                "a_batch_S2",
                "b_batch_S2",
                "c_batch_S2",
                "d_batch_S2",
            ]
        );
    }

    #[test]
    fn aged_ready_order_sample_mode_keeps_priority_primary() {
        // Issue #1089: priority still outranks the sample key. `c` holds
        // priority 7, so both `c` instances sort before every other stage;
        // the sample key only breaks the tie between c_S1 and c_S2.
        let workdir = setup().workdir;
        let plan = diamond_plan(workdir.path(), "sample", Some("c"));
        let ordered = BackendDriver::aged_ready_order(plan.order.clone(), &plan, &HashMap::new());
        assert_eq!(
            ordered,
            vec![
                "c_batch_S1",
                "c_batch_S2",
                "a_batch_S1",
                "b_batch_S1",
                "d_batch_S1",
                "a_batch_S2",
                "b_batch_S2",
                "d_batch_S2",
            ]
        );
    }

    /// Build a plan for a chain workflow: rule N+1 consumes rule N's output
    /// (declared input → real DAG edges).
    fn chain_plan(workdir: &Path, rules: &[(&str, &str, &str)]) -> ScheduledPlan {
        let mut toml = String::from("[workflow]\nname = \"chain\"\n");
        let mut prev_output: Option<&str> = None;
        for (name, shell, output) in rules {
            match prev_output {
                Some(prev) => {
                    toml.push_str(&format!(
                        "[[rules]]\nname = \"{name}\"\ninput = [\"{prev}\"]\nshell = \"{shell}\"\noutput = [\"{output}\"]\n"
                    ));
                }
                None => {
                    toml.push_str(&format!(
                        "[[rules]]\nname = \"{name}\"\nshell = \"{shell}\"\noutput = [\"{output}\"]\n"
                    ));
                }
            }
            prev_output = Some(output);
        }
        let wf = workdir.join("wf.oxoflow");
        std::fs::write(&wf, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap()
    }

    fn driver(fx: &Fixture) -> (BackendDriver, Arc<ClusterExecutor>) {
        driver_with(fx, false)
    }

    fn driver_with(fx: &Fixture, keep_going: bool) -> (BackendDriver, Arc<ClusterExecutor>) {
        let state = fx._state.path();
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &state.to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(50),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going,
            },
        );
        (d, executor)
    }

    fn events(fx: &Fixture) -> Vec<serde_json::Value> {
        let path = fx.run_dir.path().join("events.jsonl");
        let content = std::fs::read_to_string(path).unwrap();
        content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn driver_executes_chain_and_records_success() {
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("a", "echo a > a.txt", "a.txt"),
                ("b", "cat a.txt && echo b > b.txt", "b.txt"),
            ],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            records.iter().all(|r| r.status == JobStatus::Success),
            "unexpected records: {:?}",
            records
                .iter()
                .map(|r| (r.rule.clone(), r.status, r.stderr.clone()))
                .collect::<Vec<_>>()
        );
        assert!(fx.workdir.path().join("a.txt").exists());
        assert!(fx.workdir.path().join("b.txt").exists());
        let ev = events(&fx);
        assert_eq!(ev.iter().filter(|e| e["t"] == "SUBMITTED").count(), 2);
        assert_eq!(ev.iter().filter(|e| e["t"] == "COMPLETED").count(), 2);
    }

    #[test]
    fn driver_settle_hook_observes_each_terminal_record() {
        // Issue #1083: the cluster path folds completions into the
        // checkpoint AS JOBS SETTLE. Without this hook a driver crash loses
        // every verdict since start, and the restart has no trustworthy
        // completion record — the output-existence heuristic then decides.
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("a", "echo a > a.txt", "a.txt"),
                ("b", "cat a.txt && echo b > b.txt", "b.txt"),
            ],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_hook = seen.clone();
        let on_settle = move |record: JobRecord| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<()>> + Send>,
        > {
            let seen = seen_hook.clone();
            Box::pin(async move {
                seen.lock()
                    .unwrap()
                    .push((record.rule.clone(), record.status));
                Ok(())
            })
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: Some(Box::new(on_settle)),
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2);
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            2,
            "every settled job must reach the fold hook: {seen:?}"
        );
        assert!(seen.iter().all(|(_, status)| *status == JobStatus::Success));
        let mut seen_rules: Vec<&str> = seen.iter().map(|(rule, _)| rule.as_str()).collect();
        seen_rules.sort_unstable();
        assert_eq!(seen_rules, ["a", "b"]);
    }

    #[test]
    fn settle_hook_fires_for_checkpoint_hook_failure_record() {
        // Issue #1083 F2: the re-entry-manifest failure path
        // (`return_record_failure`) pushes a Failed record that never
        // reaches the normal settle site — it must still pass the settle
        // hook, or the caller's checkpoint fold misses it and a crash
        // right after loses the verdict.
        let fx = setup();
        let workdir = fx.workdir.path();
        let wf = workdir.join("wf.oxoflow");
        std::fs::write(
            &wf,
            concat!(
                "[workflow]\nname = \"cp\"\n",
                "[[rules]]\nname = \"ck\"\nshell = \"echo hi > out.txt\"\n",
                "output = [\"out.txt\"]\ncheckpoint = true\n",
                "checkpoint_manifest = \"manifest.toml\"\n"
            ),
        )
        .unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let settled: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let hook_settled = settled.clone();
        let on_settle = move |record: JobRecord| {
            let settled = hook_settled.clone();
            Box::pin(async move {
                settled.lock().unwrap().push(record.rule);
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        };
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: Some(Box::new(|_rule: &str| {
                        Err(OxoFlowError::Config {
                            message: "manifest missing".into(),
                        })
                    })),
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: Some(Box::new(on_settle)),
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rule, "ck");
        assert_eq!(records[0].status, JobStatus::Failed);
        assert!(
            records[0]
                .skip_reason
                .as_deref()
                .unwrap_or_default()
                .starts_with("re-entry manifest:")
        );
        let settled = settled.lock().unwrap();
        assert_eq!(
            settled.as_slice(),
            &["ck".to_string()],
            "the re-entry failure record must pass the settle hook: {settled:?}"
        );
    }

    #[test]
    fn settle_hook_does_not_fire_for_signal_interrupted_records() {
        // Interrupted records bypass the settle site (they are assembled in
        // the cancel branch and returned via Err(Cancelled)); the caller
        // keeps folding them through the Cancelled arm. Pin that contract:
        // the hook must capture nothing on this path.
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Running));
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<CancelSignal>);
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: Some(cancel_rx),
                keep_going: false,
            },
        );
        let settled: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let hook_settled = settled.clone();
        let on_settle = move |record: JobRecord| {
            let settled = hook_settled.clone();
            Box::pin(async move {
                settled.lock().unwrap().push(record.rule);
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        };
        cancel_tx
            .send(Some(CancelSignal {
                signal_name: "SIGINT",
                signum: 2,
            }))
            .unwrap();
        let result = tokio::runtime::Runtime::new().unwrap().block_on(d.run(
            &mut plan,
            &to_run,
            DriverOptions {
                run_dir: fx.run_dir.path(),
                on_checkpoint: None,
                merge: None,
                sensitive_values: &[],
                on_submit: None,
                on_settle: Some(Box::new(on_settle)),
            },
        ));
        match result {
            Err(OxoFlowError::Cancelled { records, .. }) => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].status, JobStatus::Failed);
                assert_eq!(records[0].signal, Some(2));
            }
            other => panic!("expected Cancelled error, got: {other:?}"),
        }
        let settled = settled.lock().unwrap();
        assert!(
            settled.is_empty(),
            "interrupted records must not pass the settle hook: {settled:?}"
        );
        assert_eq!(backend.state().lock().unwrap().cancelled, vec!["job-1"]);
    }

    /// `--keep-going` gates ONLY the required-failure decision (issue
    /// #1093 F2b): an interrupt still cancels every in-flight job and ends
    /// the run with `Cancelled`, exactly as without the flag.
    #[test]
    fn keep_going_does_not_weaken_interrupt_cancellation() {
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Running));
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<CancelSignal>);
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: Some(cancel_rx),
                keep_going: true,
            },
        );
        cancel_tx
            .send(Some(CancelSignal {
                signal_name: "SIGINT",
                signum: 2,
            }))
            .unwrap();
        let result = tokio::runtime::Runtime::new().unwrap().block_on(d.run(
            &mut plan,
            &to_run,
            DriverOptions {
                run_dir: fx.run_dir.path(),
                on_checkpoint: None,
                merge: None,
                sensitive_values: &[],
                on_submit: None,
                on_settle: None,
            },
        ));
        match result {
            Err(OxoFlowError::Cancelled { records, .. }) => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].status, JobStatus::Failed);
                assert_eq!(records[0].signal, Some(2));
            }
            other => panic!("expected Cancelled error under --keep-going, got: {other:?}"),
        }
        assert_eq!(backend.state().lock().unwrap().cancelled, vec!["job-1"]);
    }

    #[test]
    fn driver_propagates_failure_and_skips_dependents() {
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("a", "echo a > a.txt", "a.txt"),
                ("b", "exit 3", "b.txt"),
                ("c", "echo c > c.txt", "c.txt"),
            ],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        let by_rule: std::collections::HashMap<&str, &crate::executor::JobRecord> =
            records.iter().map(|r| (r.rule.as_str(), r)).collect();
        assert_eq!(by_rule["a"].status, JobStatus::Success);
        assert_eq!(by_rule["b"].status, JobStatus::Failed);
        assert_eq!(by_rule["c"].status, JobStatus::Skipped);
        assert!(
            by_rule["c"]
                .skip_reason
                .as_deref()
                .unwrap()
                .contains("blocked by failed upstream")
        );
    }

    /// Regression (live audit finding): a rule blocked by a failed upstream
    /// must itself block ITS dependents. The dependency gate used to treat
    /// any Skipped as satisfied, so a two-levels-down rule (gather after a
    /// failed prep → skipped stats) was submitted, ran against stale
    /// inputs, and was recorded COMPLETED — a silent wrong scientific
    /// result written into the checkpoint.
    #[test]
    fn driver_blocks_transitive_dependents_of_failed_rules() {
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("prep", "exit 1", "prep.txt"),
                ("stats", "echo s > stats.txt", "stats.txt"),
                ("gather", "echo g > gather.txt", "gather.txt"),
            ],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        let by_rule: std::collections::HashMap<&str, &crate::executor::JobRecord> =
            records.iter().map(|r| (r.rule.as_str(), r)).collect();
        assert_eq!(by_rule["prep"].status, JobStatus::Failed);
        assert_eq!(by_rule["stats"].status, JobStatus::Skipped);
        assert_eq!(
            by_rule["gather"].status,
            JobStatus::Skipped,
            "gather must be blocked transitively — never submitted"
        );
        let ev = events(&fx);
        let submitted: Vec<&str> = ev
            .iter()
            .filter(|e| e["t"] == "SUBMITTED")
            .filter_map(|e| e["rule"].as_str())
            .collect();
        assert!(
            !submitted.contains(&"gather") && !submitted.contains(&"stats"),
            "blocked rules must never be submitted, got: {submitted:?}"
        );
    }

    /// Run two independent rules through the mock scheduler: `bad` fails
    /// immediately while `slow` is still in flight. `slow` polls the
    /// RELEASE marker, so the settle hook — not a timer — decides when it
    /// finishes (only in the keep-going variant); the bound just stops an
    /// orphaned mock job from looping after the test. Returns each settled
    /// record plus whether `slow` produced its output.
    fn required_failure_with_inflight_sibling(keep_going: bool) -> (Vec<JobRecord>, bool) {
        let fx = setup();
        let (d, _) = driver_with(&fx, keep_going);
        let release = fx.workdir.path().join("RELEASE");
        let slow_out = fx.workdir.path().join("slow.txt");
        let mut plan = priority_plan(fx.workdir.path(), &[("bad", 0), ("slow", 0)]);
        for (name, sr) in plan.rules.iter_mut() {
            sr.shell_cmd = if name == "bad" {
                "exit 1".to_string()
            } else {
                format!(
                    "for _ in $(seq 1 600); do if [ -f '{release}' ]; then break; fi; \
                     sleep 0.05; done; echo done > '{slow_out}'",
                    release = release.display(),
                    slow_out = slow_out.display(),
                )
            };
        }
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let on_settle: SettleHook<'_> = {
            let release = release.clone();
            Box::new(move |record: JobRecord| {
                let release = release.clone();
                Box::pin(async move {
                    // Release the sibling the moment the required failure
                    // settles — only under keep-going; on the default path
                    // the sibling has already been cancelled by then.
                    if keep_going && record.rule == "bad" && record.status == JobStatus::Failed {
                        std::fs::write(&release, b"").map_err(|e| OxoFlowError::Config {
                            message: format!("cannot create {}: {e}", release.display()),
                        })?;
                    }
                    Ok(())
                })
            })
        };
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: Some(on_settle),
                },
            ))
            .unwrap();
        // Let a surviving mock job leave its loop before the tempdir goes.
        std::fs::write(&release, b"").unwrap();
        let produced = slow_out.exists();
        (records, produced)
    }

    /// Issue #1093 F2b: `--keep-going` gates ONLY the required-failure
    /// cancel decision. The failure is still recorded and the caller still
    /// fails the run, but independent jobs already in flight finish.
    #[test]
    fn keep_going_does_not_cancel_inflight_independents() {
        let (records, slow_produced) = required_failure_with_inflight_sibling(true);
        assert!(
            slow_produced,
            "the in-flight independent must run to completion under --keep-going"
        );
        let status: HashMap<&str, JobStatus> = records
            .iter()
            .map(|r| (r.rule.as_str(), r.status))
            .collect();
        assert_eq!(status["bad"], JobStatus::Failed);
        assert_eq!(status["slow"], JobStatus::Success);
        assert!(
            !records.iter().any(|r| r.status == JobStatus::Cancelled),
            "nothing may be cancelled under --keep-going: {records:?}"
        );
    }

    /// The historical default is byte-for-byte unchanged: a required
    /// rule's failure cancels every in-flight sibling before the run exits.
    #[test]
    fn required_failure_cancels_inflight_siblings_without_keep_going() {
        let (records, slow_produced) = required_failure_with_inflight_sibling(false);
        assert!(
            !slow_produced,
            "the in-flight independent must be cancelled without --keep-going"
        );
        let status: HashMap<&str, JobStatus> = records
            .iter()
            .map(|r| (r.rule.as_str(), r.status))
            .collect();
        assert_eq!(status["bad"], JobStatus::Failed);
        assert_eq!(status["slow"], JobStatus::Cancelled);
    }

    #[test]
    fn driver_respects_max_submitted() {
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("a", "echo a > a.txt", "a.txt"),
                ("b", "cat a.txt && echo b > b.txt", "b.txt"),
                ("c", "cat b.txt && echo c > c.txt", "c.txt"),
            ],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        // max_submitted = 2: at any event index, in-flight count never exceeds 2.
        let mut inflight = 0i32;
        let mut max_seen = 0i32;
        for e in events(&fx) {
            match e["t"].as_str().unwrap() {
                "SUBMITTED" => inflight += 1,
                "COMPLETED" | "FAILED" | "CANCELLED" => inflight -= 1,
                _ => {}
            }
            max_seen = max_seen.max(inflight);
        }
        assert!(max_seen <= 2, "max in-flight observed: {max_seen}");
        assert!(max_seen >= 1);
    }

    /// A dependency the caller left out of `to_run` is already complete, not
    /// pending. Before this, `deps_ok` waited for it to appear in `done`, so
    /// every partial re-run — resume after a failure, an edit to a downstream
    /// rule — submitted nothing and died with "driver stall".
    #[test]
    fn driver_runs_a_rule_whose_dependency_is_outside_to_run() {
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[
                ("a", "echo a > a.txt", "a.txt"),
                ("b", "cat a.txt && echo b > b.txt", "b.txt"),
            ],
        );
        // `a` ran in an earlier run: its output is on disk and the caller's
        // invalidation analysis kept it out of this run's work set.
        std::fs::write(fx.workdir.path().join("a.txt"), "a\n").unwrap();
        let to_run: HashSet<String> = ["b".to_string()].into_iter().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .expect("a fresh dependency must not stall the driver");
        assert_eq!(
            records.len(),
            1,
            "only `b` belongs to this run: {records:?}"
        );
        assert_eq!(records[0].rule, "b");
        assert_eq!(records[0].status, JobStatus::Success);
        assert!(fx.workdir.path().join("b.txt").exists());
    }

    #[test]
    fn driver_errors_when_submit_binary_is_missing() {
        let fx = setup();
        // Resolve scheduler commands from an empty directory so the test is
        // environment-independent: on hosts with a real SLURM install,
        // `sbatch` in PATH would submit successfully and fail the unwrap
        // below. The empty dir makes `run_cmd` resolve `<dir>/sbatch`,
        // which never exists.
        let empty_bin_dir = tempfile::tempdir().unwrap();
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(empty_bin_dir.path().to_path_buf()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(50),
                poll_timeout: None,
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let mut plan = chain_plan(fx.workdir.path(), &[("a", "true", "a.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let err = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("sbatch"), "unexpected error: {msg}");
    }

    #[test]
    fn driver_skips_rules_without_shell() {
        let fx = setup();
        let (d, _) = driver(&fx);
        // input-only rule: valid config, but not schedulable
        let mut toml = String::from(
            "[workflow]\nname = \"w\"\n[[rules]]\nname = \"noop\"\ninput = [\"x.txt\"]\n",
        );
        toml.push_str(
            "[[rules]]\nname = \"real\"\nshell = \"echo ok > ok.txt\"\noutput = [\"ok.txt\"]\n",
        );
        let wf = fx.workdir.path().join("wf.oxoflow");
        std::fs::write(&wf, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build(
            &config,
            &dag,
            fx.workdir.path(),
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        let noop = records.iter().find(|r| r.rule == "noop").unwrap();
        assert_eq!(noop.status, JobStatus::Skipped);
        assert_eq!(
            noop.skip_reason.as_deref(),
            Some("no shell or script defined")
        );
    }

    #[test]
    fn scatter_instances_submit_as_one_array_with_identical_records() {
        // Issue #74 phase 3: same-template instances group into ONE array
        // submission (chunked at max_array_size), element-wise tracking
        // maps back to per-instance JobRecords, and index.json records the
        // array-index → instance mapping.
        let fx = setup();
        let (d, _) = driver(&fx);
        // Two instances of one template via [[values]] + a dependent rule.
        let wf_toml = r#"
[workflow]
name = "arr"
version = "1.0.0"

[[values]]
name = "assembler"
values = ["spades", "megahit"]

[[rules]]
name = "asm"
output = ["out/{assembler}.txt"]
shell = "echo asm > out/{assembler}.txt"

[[rules]]
name = "merge"
input = ["out/spades.txt", "out/megahit.txt"]
output = ["merged.txt"]
shell = "cat {input} > merged.txt"
"#;
        let wf = fx.workdir.path().join("arr.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build(
            &config,
            &dag,
            fx.workdir.path(),
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                },
            ))
            .unwrap();

        // Both instances + the dependent rule completed.
        let names: std::collections::BTreeSet<String> =
            records.iter().map(|r| r.rule.clone()).collect();
        assert_eq!(
            names,
            [
                "asm_assembler_megahit".to_string(),
                "asm_assembler_spades".to_string(),
                "merge".to_string()
            ]
            .into_iter()
            .collect(),
            "per-instance records must match the per-job path: {names:?}"
        );
        assert!(
            records.iter().all(|r| r.status == JobStatus::Success),
            "unexpected records: {:?}",
            records
                .iter()
                .map(|r| (r.rule.clone(), r.status, r.stderr.clone()))
                .collect::<Vec<_>>()
        );

        // ONE array submission: the events file records two element ids
        // sharing a base.
        let ev = events(&fx);
        let submitted: Vec<serde_json::Value> = ev
            .iter()
            .filter(|e| e["t"] == "SUBMITTED")
            .cloned()
            .collect();
        let element_ids: Vec<String> = submitted
            .iter()
            .filter(|e| e["rule"].as_str().is_some_and(|r| r.starts_with("asm_")))
            .map(|e| e["job"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(element_ids.len(), 2, "both instances submitted: {ev:?}");
        let base = element_ids[0].split('_').next().unwrap_or("");
        assert!(
            element_ids
                .iter()
                .all(|id| id.starts_with(base) && id != base),
            "both ids must be array elements of one base: {element_ids:?}"
        );

        // index.json maps the array index back to instance names.
        let index: std::collections::HashMap<String, Vec<String>> = serde_json::from_str(
            &std::fs::read_to_string(fx.run_dir.path().join("index.json")).unwrap(),
        )
        .unwrap();
        let mapped = index.get(base).expect("array base must be indexed");
        assert_eq!(mapped.len(), 2);
        assert!(mapped.contains(&"asm_assembler_spades".to_string()));
        assert!(mapped.contains(&"asm_assembler_megahit".to_string()));
    }

    #[test]
    fn driver_errors_when_index_json_is_not_writable() {
        // The array→instance map is what status/resume read back: a write
        // failure must fail the run, not be swallowed by a `let _ =`
        // (issue #136 tier-2 audit — a corrupt/missing mapping silently
        // misleads status about which instance a job id belonged to).
        let fx = setup();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        // A directory in place of the file makes the write fail.
        std::fs::create_dir(fx.run_dir.path().join("index.json")).unwrap();
        // Two instances of one template → one array submission (the only
        // path that writes index.json).
        let wf_toml = r#"
[workflow]
name = "arr"
version = "1.0.0"

[[values]]
name = "assembler"
values = ["spades", "megahit"]

[[rules]]
name = "asm"
output = ["out/{assembler}.txt"]
shell = "echo asm > out/{assembler}.txt"
"#;
        let wf = fx.workdir.path().join("arr.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build(
            &config,
            &dag,
            fx.workdir.path(),
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let err = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(
            format!("{err}").contains("index.json"),
            "the error must name the index.json write: {err}"
        );
        // Issue #1156 / CL-01: the accepted array base id must ride in the
        // terminal error together with its (confirmed) cancellation.
        assert!(
            format!("{err}").contains("job-1"),
            "the error must name the accepted array base id: {err}"
        );
        assert!(
            format!("{err}").contains("cancelled"),
            "the error must report the accepted array as cancelled: {err}"
        );
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(
            state.cancelled,
            vec!["job-1"],
            "a post-submit index write failure must cancel the accepted array"
        );
    }

    #[test]
    fn post_submit_artifact_write_failures_cancel_accepted_jobs() {
        for artifact in ["job.id", "status.json"] {
            let fx = setup();
            let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
            let to_run: HashSet<String> = plan.order.iter().cloned().collect();
            let job_dir = fx.run_dir.path().join("jobs/only");
            std::fs::create_dir_all(job_dir.join(artifact)).unwrap();
            let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
            let d = BackendDriver::new(
                backend.clone(),
                DriverConfig {
                    max_submitted: 2,
                    max_array_size: 1001,
                    no_arrays: true,
                    poll_interval: std::time::Duration::from_millis(1),
                    poll_timeout: Some(std::time::Duration::from_secs(5)),
                    unknown_settle_grace: std::time::Duration::from_secs(90),
                    cancel: None,
                    keep_going: false,
                },
            );
            let err = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(d.run(
                    &mut plan,
                    &to_run,
                    DriverOptions {
                        run_dir: fx.run_dir.path(),
                        on_checkpoint: None,
                        merge: None,
                        sensitive_values: &[],
                        on_submit: None,
                        on_settle: None,
                    },
                ))
                .unwrap_err();
            assert!(
                err.to_string().contains(artifact),
                "error must name failed artifact {artifact}: {err}"
            );
            // Issue #1156 / CL-01: the terminal error must name the accepted
            // scheduler job and its cancel outcome — never just the local
            // file, which leaves the user with no scheduler handle to check.
            assert!(
                err.to_string().contains("job-1"),
                "error must name the accepted scheduler job id: {err}"
            );
            assert!(
                err.to_string().contains("cancelled"),
                "error must report the accepted job as cancelled: {err}"
            );
            let state = backend.state();
            let state = state.lock().unwrap();
            assert_eq!(
                state.cancelled,
                vec!["job-1"],
                "{artifact} write failure must cancel the accepted job"
            );
        }
    }

    #[test]
    fn cluster_job_record_command_masks_sensitive_values() {
        // Review follow-up of #99 B1: the cluster driver recorded the fully
        // expanded shell command (with {config.*} secrets baked in) into
        // JobRecord.command, which flows into the checkpoint/report/web —
        // bypassing the local executor's capture-boundary masking.
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = chain_plan(
            fx.workdir.path(),
            &[("echo_secret", "echo token-is-s3cr3t-token-42", "out.txt")],
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &["s3cr3t-token-42".to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        let rec = records
            .iter()
            .find(|r| r.rule == "echo_secret")
            .expect("rule must produce a record");
        let command = rec.command.as_deref().unwrap_or("");
        assert!(
            !command.contains("s3cr3t-token-42"),
            "cluster JobRecord.command must mask sensitive values: {command}"
        );
        assert!(command.contains("***"), "masked marker expected: {command}");
        let status =
            std::fs::read_to_string(fx.run_dir.path().join("jobs/echo_secret/status.json"))
                .unwrap();
        assert!(
            !status.contains("s3cr3t-token-42"),
            "status.json must mask sensitive values: {status}"
        );
        assert!(status.contains("***"), "masked status marker expected");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in ["job.sh", "status.json"] {
                let mode = std::fs::metadata(fx.run_dir.path().join("jobs/echo_secret").join(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode & 0o077, 0, "{path} must not be group/world readable");
            }
        }
    }

    #[test]
    fn poll_failure_after_submission_cancels_accepted_jobs() {
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
        backend.fail_polling();
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(err.to_string().contains("injected poll failure"));
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(state.submit_count, 1);
        assert_eq!(state.cancelled, vec!["job-1"]);
    }

    // ─── issue #1156 / ledger CL-01: post-submit orphan windows ─────────────

    /// The array path has its own registration window: accepted elements are
    /// registered first, then the chunk `job.id`, `index.json`, and each
    /// element's `job.id`/`status.json` are written. A failure in that
    /// window must cancel the accepted array by its BASE id and name it in
    /// the terminal error.
    #[test]
    fn array_element_status_write_failure_cancels_accepted_array() {
        let fx = setup();
        let mut plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        // A directory in place of sib_01's status.json fails that write
        // AFTER the chunk job.id and index.json writes succeeded.
        std::fs::create_dir_all(fx.run_dir.path().join("jobs/sib_01/status.json")).unwrap();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("status.json"),
            "must name the failed write: {text}"
        );
        assert!(
            text.contains("job-1"),
            "must name the accepted array base id: {text}"
        );
        assert!(
            text.contains("cancelled"),
            "must report the accepted array as cancelled: {text}"
        );
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(state.submit_count, 1);
        assert_eq!(
            state.cancelled,
            vec!["job-1"],
            "array elements cancel through their base id, once"
        );
    }

    /// When the cleanup itself fails, the run must fail loudly with the
    /// unresolved scheduler job id — the error may never claim a cancellation
    /// that did not happen (issue #1156 / CL-01).
    #[test]
    fn cancel_failure_after_post_submit_write_error_reports_unresolved_job() {
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        // Fail the job.id write AFTER sbatch accepted the job.
        std::fs::create_dir_all(fx.run_dir.path().join("jobs/only/job.id")).unwrap();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
        backend.fail_cancelling();
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("job.id"),
            "must name the failed write: {text}"
        );
        assert!(
            text.contains("job-1"),
            "must name the unresolved scheduler job: {text}"
        );
        assert!(
            text.contains("may still be running"),
            "a failed cancel must say the job may still be running: {text}"
        );
        assert!(
            text.contains("injected cancel failure"),
            "the scheduler's cancel error must not be swallowed: {text}"
        );
        assert!(
            !text.contains("accepted scheduler job(s) cancelled"),
            "a failed cancel must never be reported as a cancellation: {text}"
        );
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(
            state.cancelled,
            vec!["job-1"],
            "the cancel attempt against the accepted id must be recorded"
        );
    }

    /// The array registration window's failed-cleanup counterpart: the
    /// unresolved BASE id must be named and the run must not claim
    /// cancellation.
    #[test]
    fn cancel_failure_after_array_index_write_reports_unresolved_base() {
        let fx = setup();
        let mut plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        std::fs::create_dir(fx.run_dir.path().join("index.json")).unwrap();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Completed));
        backend.fail_cancelling();
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("index.json"),
            "must name the failed write: {text}"
        );
        assert!(
            text.contains("job-1"),
            "must name the unresolved array base: {text}"
        );
        assert!(
            text.contains("may still be running"),
            "a failed cancel must say the array may still be running: {text}"
        );
        assert!(
            text.contains("injected cancel failure"),
            "the scheduler's cancel error must not be swallowed: {text}"
        );
        assert!(
            !text.contains("accepted scheduler job(s) cancelled"),
            "a failed cancel must never be reported as a cancellation: {text}"
        );
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(
            state.cancelled,
            vec!["job-1"],
            "the cancel attempt targets the accepted base id"
        );
    }

    /// `cancel_inflight` is best-effort across the whole set and reports
    /// exactly the unresolved scheduler ids — array elements once, by base.
    #[test]
    fn cancel_inflight_reports_unresolved_jobs_only() {
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Running));
        backend.fail_cancelling();
        let d = BackendDriver::new(backend.clone(), DriverConfig::default());
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.cancel_inflight(&[
                ("job-1".to_string(), "solo".to_string(), None),
                (
                    "arr_1".to_string(),
                    "sib_01".to_string(),
                    Some("arr".into()),
                ),
                (
                    "arr_2".to_string(),
                    "sib_02".to_string(),
                    Some("arr".into()),
                ),
            ]))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("may still be running"),
            "the error must say the unresolved jobs may still run: {text}"
        );
        assert!(text.contains("job-1"), "must list the plain job id: {text}");
        assert!(text.contains("arr"), "must list the array base id: {text}");
        let st = backend.state();
        let st = st.lock().unwrap();
        assert_eq!(
            st.cancelled,
            vec!["job-1", "arr"],
            "every target is attempted, once, even after an earlier failure"
        );
    }

    // ─── issue #136 H-items ────────────────────────────────────────────────

    /// Minimal in-test backend: submit returns synthetic ids, poll returns a
    /// configured verdict for every probed id, and every call is recorded
    /// for assertions (used by the H3/H4/H5/H6 tests below).
    struct MockBackend {
        inner: Arc<std::sync::Mutex<MockState>>,
        direct: bool,
    }
    #[derive(Default)]
    struct MockState {
        submit_count: usize,
        poll_args: Vec<Vec<String>>,
        cancelled: Vec<String>,
        verdict: Option<BackendJobStatus>,
        /// Per-id overrides for `verdict` — lets a test pin genuinely
        /// different outcomes for array elements of one submission (ledger
        /// RB-06: per-element outcomes must stay independent).
        verdicts: HashMap<String, BackendJobStatus>,
        /// Element count of every array submission, in submit order.
        array_sizes: Vec<usize>,
        /// Per-id accounting records `terminal_status` answers with. An id
        /// absent here yields `None`, exactly like a real store that never
        /// saw the job (accounting stores report the ARRAY BASE, so a test
        /// keyed by base models PBS/LSF/SGE).
        accounting: HashMap<String, TerminalRecord>,
        poll_error: bool,
        /// Every `cancel` call errors (the attempt is still recorded).
        cancel_error: bool,
    }
    impl MockBackend {
        fn new(direct: bool, verdict: BackendJobStatus) -> Self {
            Self {
                inner: Arc::new(std::sync::Mutex::new(MockState {
                    verdict: Some(verdict),
                    ..Default::default()
                })),
                direct,
            }
        }
        fn state(&self) -> Arc<std::sync::Mutex<MockState>> {
            self.inner.clone()
        }
        /// Pin a distinct verdict for one scheduler id (array elements).
        fn with_verdict(self, id: &str, verdict: BackendJobStatus) -> Self {
            self.inner
                .lock()
                .unwrap()
                .verdicts
                .insert(id.to_string(), verdict);
            self
        }
        fn fail_polling(&self) {
            self.inner.lock().unwrap().poll_error = true;
        }
        /// Pin the accounting record `terminal_status` answers for one id.
        /// The driver consults this store for the ARRAY BASE on every
        /// backend that does not poll elements directly.
        fn with_accounting(self, id: &str, rec: TerminalRecord) -> Self {
            self.inner
                .lock()
                .unwrap()
                .accounting
                .insert(id.to_string(), rec);
            self
        }
        /// Make the scheduler reject every cancellation, modelling a cancel
        /// that cannot be confirmed.
        fn fail_cancelling(&self) {
            self.inner.lock().unwrap().cancel_error = true;
        }
    }
    #[async_trait::async_trait]
    impl ExecutorBackend for MockBackend {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn render_script(&self, _rule: &ScheduledRule) -> Result<String> {
            Ok("exit 0".to_string())
        }
        fn render_array_script(
            &self,
            _rule: &crate::rule::Rule,
            _cmd_dir: &str,
            count: usize,
            _sensitive_refs: &[String],
        ) -> Result<String> {
            self.inner.lock().unwrap().array_sizes.push(count);
            Ok("exit 0".to_string())
        }
        async fn submit(&self, _script_path: &Path) -> Result<String> {
            let mut st = self.inner.lock().unwrap();
            st.submit_count += 1;
            Ok(format!("job-{}", st.submit_count))
        }
        async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>> {
            let mut st = self.inner.lock().unwrap();
            st.poll_args.push(job_ids.to_vec());
            if st.poll_error {
                return Err(OxoFlowError::Config {
                    message: "injected poll failure".to_string(),
                });
            }
            Ok(job_ids
                .iter()
                .map(|id| {
                    let verdict = st
                        .verdicts
                        .get(id)
                        .copied()
                        .or(st.verdict)
                        .expect("the mock must be configured with a verdict");
                    (id.clone(), verdict)
                })
                .collect())
        }
        async fn cancel(&self, job_id: &str) -> Result<()> {
            let mut st = self.inner.lock().unwrap();
            st.cancelled.push(job_id.to_string());
            if st.cancel_error {
                return Err(OxoFlowError::Config {
                    message: format!("injected cancel failure for {job_id}"),
                });
            }
            Ok(())
        }
        async fn logs(&self, _job_id: &str) -> Result<String> {
            Ok(String::new())
        }
        async fn terminal_status(&self, job_id: &str) -> Option<TerminalRecord> {
            self.inner.lock().unwrap().accounting.get(job_id).copied()
        }
        fn polls_elements_directly(&self) -> bool {
            self.direct
        }
    }

    /// A plan of `n` siblings sharing one template (array-eligible).
    fn sibling_plan(workdir: &Path, n: usize) -> ScheduledPlan {
        let names: Vec<String> = (1..=n).map(|i| format!("sib_{i:02}")).collect();
        let rules: Vec<(&str, i32)> = names.iter().map(|n| (n.as_str(), 0)).collect();
        let mut plan = priority_plan(workdir, &rules);
        for (name, sr) in plan.rules.iter_mut() {
            sr.template = "sib".to_string();
            sr.dependencies.clear();
            // Distinct commands so chunk-isolation assertions can tell
            // the files apart.
            sr.shell_cmd = format!("echo {name}");
        }
        plan
    }

    #[test]
    fn multi_chunk_arrays_use_per_chunk_dirs_without_overwrite() {
        // H3: two chunks of the same template must not share cmd files.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 4);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &fx._state.path().to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 8,
                max_array_size: 2, // 4 siblings → 2 chunks
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(20),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 4, "all siblings settle");
        let tpl_dir = fx.run_dir.path().join("jobs/sib");
        for chunk in ["chunk-1", "chunk-2"] {
            for i in 1..=2 {
                let cmd = tpl_dir.join(chunk).join(format!("cmd.{i}.sh"));
                assert!(cmd.exists(), "{} must exist", cmd.display());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let private_paths = ["chunk-1", "chunk-2"]
                .into_iter()
                .flat_map(|chunk| {
                    [
                        tpl_dir.join(chunk).join("job.sh"),
                        tpl_dir.join(chunk).join("cmd.1.sh"),
                        tpl_dir.join(chunk).join("job.id"),
                    ]
                })
                .chain((1..=4).map(|i| fx.run_dir.path().join(format!("jobs/sib_{i:02}/job.sh"))))
                .chain((1..=4).map(|i| fx.run_dir.path().join(format!("jobs/sib_{i:02}/job.id"))))
                .chain((1..=4).map(|i| {
                    fx.run_dir
                        .path()
                        .join(format!("jobs/sib_{i:02}/status.json"))
                }));
            for path in private_paths {
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    mode & 0o077,
                    0,
                    "{} must not be group/world readable",
                    path.display()
                );
            }
        }
        let c1 = std::fs::read_to_string(tpl_dir.join("chunk-1/cmd.1.sh")).unwrap();
        let c2 = std::fs::read_to_string(tpl_dir.join("chunk-2/cmd.1.sh")).unwrap();
        assert_ne!(
            c1, c2,
            "sibling chunks must not overwrite each other's cmd files"
        );
    }

    #[test]
    fn non_element_backends_poll_by_base_and_expand_to_elements() {
        // H4: a PBS/LSF/SGE-style backend sees BASE ids in poll and every
        // element still settles. The expansion is an aggregate, not an
        // independently observed per-element outcome, so each element record
        // names its provenance and carries no fabricated accounting
        // (issue #1132 / ledger RB-06).
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        // The accounting store knows the ARRAY BASE (PBS/LSF/SGE record the
        // array as one allocation). Its exit/memory/CPU belong to whichever
        // element the scheduler reported, but `elapsed_secs` IS how long each
        // element ran — the fixture pins all four so the test proves the
        // duration survives while the per-process numbers are withheld
        // (issue #1132 F2).
        let base_acct = TerminalRecord {
            status: BackendJobStatus::Completed,
            exit_code: Some(0),
            elapsed_secs: Some(7),
            max_rss_mb: Some(512),
            cpu_seconds: Some(70),
        };
        let backend = Arc::new(
            MockBackend::new(false, BackendJobStatus::Completed)
                .with_accounting("job-1", base_acct),
        );
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(10)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2);
        // The aggregate success is expanded, but each element record says
        // where the verdict came from and never claims an observed exit code.
        for rec in &records {
            assert_eq!(rec.status, JobStatus::Success);
            assert_eq!(
                rec.exit_code, None,
                "a base-level verdict must not fabricate an element exit code: {rec:?}"
            );
            let reason = rec.skip_reason.as_deref().unwrap_or_default();
            assert!(
                reason.contains("array base job-1") && reason.contains("per-element outcome"),
                "the record must name the aggregate verdict's provenance: {rec:?}"
            );
            // The aggregate's process-level numbers are not this element's,
            // so they stay withheld (RB-06) ...
            assert_eq!(
                rec.max_rss_mb, None,
                "the aggregate's memory must not be presented as the element's: {rec:?}"
            );
            assert_eq!(
                rec.cpu_seconds, None,
                "the aggregate's CPU must not be presented as the element's: {rec:?}"
            );
            // ... but its DURATION is the shared array allocation's, which
            // IS how long this element ran. Before the F2 fix the base-only
            // path discarded the whole record, so `started_at` fell back to
            // `submitted_at` and the recorded window absorbed queue wait.
            let started = rec.started_at.expect("started_at is always set");
            let finished = rec.finished_at.expect("finished_at is always set");
            let ran = (finished - started).num_seconds();
            assert_eq!(
                ran, 7,
                "wall time must come from the store's run duration, not submit-to-settle: {rec:?}"
            );
        }
        let st = backend.state();
        let st = st.lock().unwrap();
        assert!(!st.poll_args.is_empty());
        for args in &st.poll_args {
            for id in args {
                assert!(
                    !id.contains('_'),
                    "non-element backends must be polled by base id, got {id}"
                );
            }
        }
    }

    /// Ledger RB-06: a Completed verdict without an accounting record proves
    /// the job finished, but the exit code is unknown — it must never be
    /// reported as 0. Fail-first: before the fix the success branch
    /// hard-coded `Some(...unwrap_or(0))`.
    #[test]
    fn completed_job_without_accounting_reports_unknown_exit_code() {
        let fx = setup();
        let mut plan = priority_plan(fx.workdir.path(), &[("solo", 0)]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        // terminal_status keeps the trait default (None): the scheduler
        // poller said Completed, the accounting store knows nothing.
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend,
            DriverConfig {
                max_submitted: 1,
                max_array_size: 0,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(5),
                poll_timeout: Some(std::time::Duration::from_secs(10)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Success);
        assert_eq!(
            records[0].exit_code, None,
            "missing accounting is unknown, not exit code 0"
        );
    }

    /// Ledger RB-06: when a backend polls elements directly (SLURM, and the
    /// cloud array backends), each element settles on its own verdict — one
    /// element's failure must not be copied onto its successful siblings.
    #[test]
    fn direct_backend_settles_array_elements_independently() {
        let fx = setup();
        let mut plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        // The array submits as base `job-1`; the driver polls the element
        // ids `job-1_1` and `job-1_2`.
        let backend = Arc::new(
            MockBackend::new(true, BackendJobStatus::Running)
                .with_verdict("job-1_1", BackendJobStatus::Completed)
                .with_verdict("job-1_2", BackendJobStatus::Failed),
        );
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(5),
                poll_timeout: Some(std::time::Duration::from_secs(10)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: true,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2, "both elements settle: {records:?}");
        let success = records
            .iter()
            .find(|r| r.rule == "sib_01")
            .expect("sib_01 record");
        let failure = records
            .iter()
            .find(|r| r.rule == "sib_02")
            .expect("sib_02 record");
        assert_eq!(
            success.status,
            JobStatus::Success,
            "a successful element must not inherit a sibling's failure: {records:?}"
        );
        assert_eq!(failure.status, JobStatus::Failed);
        // One verdict per element was requested from the backend, never a
        // base-id expansion.
        let st = backend.state();
        let st = st.lock().unwrap();
        assert!(
            st.poll_args
                .iter()
                .any(|args| args.contains(&"job-1_1".to_string())
                    && args.contains(&"job-1_2".to_string())),
            "elements must be polled by their own ids: {:?}",
            st.poll_args
        );
    }

    /// Ledger RB-06 end-to-end: the real `ClusterExecutor` in Kubernetes
    /// mode submits one Indexed Job and settles each element from its own
    /// completion index. Before this change the executor was a base-only
    /// backend for the driver: the whole array inherited one verdict.
    #[cfg(unix)]
    #[test]
    fn kubernetes_indexed_array_settles_elements_independently() {
        use std::os::unix::fs::PermissionsExt;
        let fx = setup();
        let mut plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let bin = tempfile::tempdir().unwrap();
        let kubectl = bin.path().join("kubectl");
        std::fs::write(
            &kubectl,
            "#!/bin/bash\n\
             if [ \"$1\" = get ]; then\n\
             echo '1|1|2|0|1'\n\
             else\n\
             echo 'job.batch/arr-job created'\n\
             fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&kubectl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let executor = Arc::new(
            ClusterExecutor::new(
                ClusterBackend::Kubernetes,
                ClusterJobConfig {
                    backend: ClusterBackend::Kubernetes,
                    queue: None,
                    account: None,
                    walltime: None,
                    extra_args: Vec::new(),
                },
            )
            .with_scheduler_dir(bin.path().to_path_buf()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(10)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: true,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2, "both elements settle: {records:?}");
        let completed = records.iter().find(|r| r.rule == "sib_01").unwrap();
        let failed = records.iter().find(|r| r.rule == "sib_02").unwrap();
        assert_eq!(
            completed.status,
            JobStatus::Success,
            "completion index 0 succeeded and must not inherit the sibling's failure: {records:?}"
        );
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(
            completed.exit_code, None,
            "no accounting store: the exit code is unknown, not 0"
        );
        assert_eq!(failed.exit_code, None);
    }

    #[test]
    fn blind_unknown_jobs_settle_as_failed_instead_of_polling_forever() {
        // Issue #244: on a cluster without slurmdbd the live queue empties,
        // the accounting store knows nothing, and settlement polled
        // forever. After UNKNOWN_SETTLE_ROUNDS blind rounds the driver
        // settles the job as Failed with exit code unavailable.
        let fx = setup();
        let mut plan = priority_plan(fx.workdir.path(), &[("solo", 0)]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Unknown));
        let d = BackendDriver::new(
            backend,
            DriverConfig {
                max_submitted: 1,
                max_array_size: 0,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: None, // NO timeout: only the guard ends this
                unknown_settle_grace: std::time::Duration::from_millis(0),
                cancel: None,
                keep_going: false,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .expect("the blind-settlement guard must end the run");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Failed);
        assert_eq!(records[0].exit_code, None, "exit code is unknown, not 0");
    }

    #[test]
    fn cancel_inflight_cancels_array_elements_by_base_id() {
        // H4: qdel/bkill must receive the array base id, not {base}_{index}.
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Running));
        let d = BackendDriver::new(backend.clone(), DriverConfig::default());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.cancel_inflight(&[
                (
                    "arr_1".to_string(),
                    "sib_01".to_string(),
                    Some("arr".to_string()),
                ),
                (
                    "arr_2".to_string(),
                    "sib_02".to_string(),
                    Some("arr".to_string()),
                ),
                ("plain-9".to_string(), "solo".to_string(), None),
            ]))
            .unwrap();
        let st = backend.state();
        let st = st.lock().unwrap();
        assert_eq!(st.cancelled, vec!["arr".to_string(), "plain-9".to_string()]);
    }

    #[test]
    fn max_submitted_binds_per_submission_not_per_rule() {
        // H5: a multi-chunk batch must not overshoot the cap mid-rule.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 4); // 2 chunks at size 2
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Running));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 1,
                max_array_size: 2,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_millis(300)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(err.to_string().contains("poll timeout"));
        let st = backend.state();
        let st = st.lock().unwrap();
        assert_eq!(
            st.submit_count, 1,
            "the cap binds per submission: only chunk-1 may submit"
        );
    }

    #[test]
    fn on_submit_hook_records_every_accepted_job() {
        // H6: submit-time checkpoint recording sees each accepted job.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let hook_seen = seen.clone();
        let on_submit = move |rule: String, _job: String| {
            let seen = hook_seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(rule);
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        };
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &fx._state.path().to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(20),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: Some(Box::new(on_submit)),
                    on_settle: None,
                },
            ))
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            2,
            "every accepted job must hit the hook: {seen:?}"
        );
    }

    // ─── cluster-path audit findings ───────────────────────────────────────

    #[test]
    fn same_dependencies_ignores_order() {
        // The dependency list is assembled from DAG edges + depends_on, so
        // two instances of one template can carry the same producers in a
        // different order — and must still group.
        let a = vec!["prep".to_string(), "qc".to_string()];
        let b = vec!["qc".to_string(), "prep".to_string()];
        assert!(same_dependencies(&a, &b));
        assert!(!same_dependencies(&a, &["prep".to_string()]));
        assert!(!same_dependencies(
            &a,
            &["prep".to_string(), "other".to_string()]
        ));
    }

    #[test]
    fn array_elements_count_against_the_in_flight_cap() {
        // G8: a single array could blow past max_submitted — 900 elements
        // under a cap of 50 all left the door in one submission. The chunk
        // is now cut to the slots that actually remain.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 6);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Running));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 3,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(5),
                poll_timeout: Some(std::time::Duration::from_millis(300)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(err.to_string().contains("poll timeout"));
        let st = backend.state();
        let st = st.lock().unwrap();
        assert_eq!(
            st.array_sizes,
            vec![3],
            "the array must be trimmed to the remaining in-flight quota"
        );
        let submitted = events(&fx).iter().filter(|e| e["t"] == "SUBMITTED").count();
        assert_eq!(submitted, 3, "never more jobs in flight than the cap");
    }

    #[test]
    fn second_level_fanout_still_groups_into_one_array() {
        // G7: instances whose dependency LISTS differ only in order were
        // never grouped, so every array below the top level submitted one
        // job per sample.
        let fx = setup();
        let mut plan = sibling_plan(fx.workdir.path(), 2);
        // Same producers, different discovery order.
        for (name, deps) in [
            ("sib_01", vec!["prep", "qc"]),
            ("sib_02", vec!["qc", "prep"]),
        ] {
            plan.rules.get_mut(name).unwrap().dependencies =
                deps.into_iter().map(String::from).collect();
        }
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Running));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(5),
                poll_timeout: Some(std::time::Duration::from_millis(200)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let st = backend.state();
        let st = st.lock().unwrap();
        assert_eq!(st.array_sizes, vec![2], "equal dependency SETS group");
    }

    #[test]
    fn pending_jobs_report_a_heartbeat() {
        // G4: a job parked in PENDING was completely silent — a full
        // partition was indistinguishable from a hung driver.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 1);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Pending));
        let d = BackendDriver::new(
            backend,
            DriverConfig {
                max_submitted: 1,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(2),
                poll_timeout: Some(std::time::Duration::from_millis(200)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(err.to_string().contains("poll timeout"));
        let waiting = events(&fx)
            .iter()
            .filter(|e| e["t"] == "WAITING")
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            !waiting.is_empty(),
            "a parked job must produce WAITING heartbeats"
        );
        assert!(
            waiting.iter().all(|e| e["rule"] == "sib_01"
                && e["reason"]
                    .as_str()
                    .is_some_and(|r| r.starts_with("pending for "))),
            "heartbeats name the rule and the wait: {waiting:?}"
        );
        // Running jobs stay silent: a job that is working needs no nudge.
        let running = Arc::new(MockBackend::new(true, BackendJobStatus::Running));
        let fx2 = setup();
        let d = BackendDriver::new(
            running,
            DriverConfig {
                max_submitted: 1,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(2),
                poll_timeout: Some(std::time::Duration::from_millis(120)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan.clone(),
                &to_run,
                DriverOptions {
                    run_dir: fx2.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        assert!(
            !events(&fx2).iter().any(|e| e["t"] == "WAITING"),
            "a RUNNING job must not emit WAITING heartbeats"
        );
    }

    #[test]
    fn array_job_is_named_after_the_template() {
        // P7-12: the array presented itself under whichever instance sorted
        // first, so squeue/sacct reported a job named after one arbitrary
        // sample.
        let fx = setup();
        let wf_toml = r#"
[workflow]
name = "arr"
version = "1.0.0"

[[values]]
name = "assembler"
values = ["spades", "megahit"]

[[rules]]
name = "asm"
output = ["out/{assembler}.txt"]
shell = "echo asm > out/{assembler}.txt"
"#;
        let wf = fx.workdir.path().join("arr.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build(
            &config,
            &dag,
            fx.workdir.path(),
            &EnvironmentResolver::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &fx._state.path().to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        let job_sh =
            std::fs::read_to_string(fx.run_dir.path().join("jobs/asm/chunk-1/job.sh")).unwrap();
        assert!(
            job_sh.contains("#SBATCH --job-name=asm\n"),
            "the array must be named after the template: {job_sh}"
        );
        assert!(
            !job_sh.contains("--job-name=asm_assembler"),
            "no instance name may leak into the array job name: {job_sh}"
        );
    }

    /// A cancel signal mid-flight cancels every in-flight job (BASE ids for
    /// arrays) and returns a [`OxoFlowError::Cancelled`] carrying the
    /// interrupted records instead of an unreachable success value.
    #[test]
    fn cancel_signal_cancels_inflight_and_returns_cancelled_error() {
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Running));
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<super::CancelSignal>);
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: Some(cancel_rx),
                keep_going: false,
            },
        );
        cancel_tx
            .send(Some(super::CancelSignal {
                signal_name: "SIGINT",
                signum: 2,
            }))
            .unwrap();
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        let OxoFlowError::Cancelled {
            message,
            records,
            signal_name,
            exit_code,
        } = &err
        else {
            panic!("expected OxoFlowError::Cancelled, got: {err}");
        };
        assert_eq!(signal_name, "SIGINT");
        assert_eq!(*exit_code, 130);
        assert!(message.contains("interrupted by SIGINT"));
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.rule, "only");
        assert_eq!(rec.status, JobStatus::Failed);
        assert_eq!(rec.signal, Some(2));
        assert_eq!(rec.skip_reason.as_deref(), Some("interrupted by SIGINT"));
        assert_eq!(rec.exit_code, None);
        assert!(rec.enqueued_at.is_some());
        assert!(rec.started_at.is_some());
        assert!(rec.finished_at.is_some());
        // The in-flight job was actually cancelled at the scheduler.
        let state = backend.state();
        let state = state.lock().unwrap();
        assert_eq!(state.submit_count, 1);
        assert_eq!(state.cancelled, vec!["job-1"]);
        // status.json + events.jsonl record the interruption.
        let status =
            std::fs::read_to_string(fx.run_dir.path().join("jobs/only/status.json")).unwrap();
        assert!(status.contains(r#""state":"FAILED""#));
        let events = std::fs::read_to_string(fx.run_dir.path().join("events.jsonl")).unwrap();
        assert!(events.contains(r#""t":"FAILED""#));
        assert!(events.contains("interrupted by SIGINT"));
    }

    /// With no signal wired the cancel check is inert: the driver runs to
    /// completion as before.
    #[test]
    fn cancel_channel_absent_runs_to_completion() {
        let fx = setup();
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "out.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(true, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(1),
                poll_timeout: Some(std::time::Duration::from_secs(5)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: None,
                keep_going: false,
            },
        );
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Success);
        assert!(backend.state().lock().unwrap().cancelled.is_empty());
    }

    // ─── issue #1120 / RB-12: HPC secret injection via a private env file ──

    /// Recursively walk `dir` and return the first file whose raw bytes
    /// contain `marker` (issue #1258). Reads every file as BYTES so binary
    /// artifacts — checkpoints, scheduler wire files, archives — are covered
    /// too, not only the text scripts a hand-written `!contains` check would
    /// reach. Symlinks are not followed; the walk is iterative so a deep tree
    /// cannot overflow the stack.
    fn find_marker_in_tree(dir: &Path, marker: &str) -> Option<PathBuf> {
        let needle = marker.as_bytes();
        if needle.is_empty() {
            return None;
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        stack.push(entry.path());
                    }
                }
            } else if meta.is_file()
                && let Ok(bytes) = std::fs::read(&path)
                && bytes.windows(needle.len()).any(|w| w == needle)
            {
                return Some(path);
            }
        }
        None
    }

    /// Assert `marker` is recoverable from nowhere under `dir`, naming the
    /// offending path when it is. The single scanner the RB-12 recoverability
    /// tests (issue #1258) funnel through — a walk of the WHOLE run directory
    /// catches artifacts a per-file assertion would miss.
    fn assert_marker_absent(dir: &Path, marker: &str, what: &str) {
        if let Some(hit) = find_marker_in_tree(dir, marker) {
            panic!(
                "{what}: marker `{marker}` is recoverable from {}",
                hit.display()
            );
        }
    }

    /// A single-rule HPC plan whose command embeds a `sensitive = true`
    /// value, routed before the wrapper (the production ordering).
    fn secret_plan(workdir: &Path, sentinel: &str, shell: &str, output: &str) -> ScheduledPlan {
        let wf_toml =
            String::from("[workflow]\nname = \"secrets\"\n[[rules]]\nname = \"leak_probe\"\n")
                + &format!("shell = \"{shell}\"\noutput = [\"{output}\"]\n");
        let wf = workdir.join("secrets.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        ScheduledPlan::build_with_sensitive(
            &config,
            &dag,
            workdir,
            &EnvironmentResolver::new(),
            &HashMap::from([("config.api_token".to_string(), sentinel.to_string())]),
            &[sentinel.to_string()],
        )
        .unwrap()
    }

    /// Sentinel regression (ledger RB-12): an HPC job's scheduler-visible
    /// script carries only the `OXO_FLOW_SENSITIVE_0` reference plus the
    /// path of a `0o600` env file the shell sources. The value reaches the
    /// command (proved by the output), and the env file is removed when the
    /// driver observes the job terminal.
    #[test]
    fn hpc_secret_never_reaches_the_script_and_is_removed_on_settle() {
        let sentinel = "SENTINEL_SECRET_VALUE_A1H";
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = secret_plan(
            fx.workdir.path(),
            sentinel,
            "mkdir -p out && printf '%s' '{config.api_token}' > out/token.txt",
            "out/token.txt",
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[sentinel.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Success);

        // The value reached the command through the env file.
        let token = std::fs::read_to_string(fx.workdir.path().join("out/token.txt")).unwrap();
        assert_eq!(
            token, sentinel,
            "the routed variable must expand to the value"
        );

        // The driver's script carries the reference and the env-file path,
        // never the value.
        let job_dir = fx.run_dir.path().join("jobs/leak_probe");
        let script = std::fs::read_to_string(job_dir.join("job.sh")).unwrap();
        assert!(
            !script.contains(sentinel),
            "sentinel leaked into the HPC job script: {script}"
        );
        assert!(
            script.contains("${OXO_FLOW_SENSITIVE_0}"),
            "the script must carry the routed reference: {script}"
        );
        assert!(
            script.contains("secrets.env"),
            "the script must source the private env file: {script}"
        );

        // The scheduler's own copy of the script is equally clean.
        let sched_script = std::fs::read_to_string(fx._state.path().join("jobs/1/job.sh")).unwrap();
        assert!(
            !sched_script.contains(sentinel),
            "sentinel leaked into the scheduler's script copy: {sched_script}"
        );

        // The env file is gone once the job is terminal.
        assert!(
            !job_dir
                .join(crate::backend::SENSITIVE_ENV_FILE_NAME)
                .exists(),
            "the secret env file must not outlive the job"
        );

        // Records and status never carry the value either.
        let status = std::fs::read_to_string(job_dir.join("status.json")).unwrap();
        assert!(
            !status.contains(sentinel),
            "sentinel leaked into status.json"
        );
        let events = std::fs::read_to_string(fx.run_dir.path().join("events.jsonl")).unwrap();
        assert!(
            !events.contains(sentinel),
            "sentinel leaked into events.jsonl"
        );
    }

    /// Cancellation path (ledger RB-12): the env file is swept when the run
    /// ends, and neither the script nor the scheduler's copy carries the
    /// value.
    #[test]
    fn cancelled_hpc_job_leaves_no_recoverable_secret() {
        let sentinel = "SENTINEL_CANCELLED_VALUE_K7";
        let fx = setup();
        let mut plan = secret_plan(
            fx.workdir.path(),
            sentinel,
            "mkdir -p out && test -n '{config.api_token}' && touch out/started && sleep 30",
            "out/started",
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<CancelSignal>);
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &fx._state.path().to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: Some(cancel_rx),
                keep_going: false,
            },
        );
        // Give the driver time to submit and start the job, then interrupt.
        let signaller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            cancel_tx
                .send(Some(CancelSignal {
                    signal_name: "SIGINT",
                    signum: 2,
                }))
                .unwrap();
        });
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[sentinel.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        signaller.join().unwrap();
        assert!(
            matches!(err, OxoFlowError::Cancelled { .. }),
            "expected an interrupt, got: {err}"
        );

        let job_dir = fx.run_dir.path().join("jobs/leak_probe");
        let script = std::fs::read_to_string(job_dir.join("job.sh")).unwrap();
        assert!(
            !script.contains(sentinel),
            "sentinel leaked into job.sh: {script}"
        );
        assert!(
            !job_dir
                .join(crate::backend::SENSITIVE_ENV_FILE_NAME)
                .exists(),
            "the secret env file must be swept on the cancel path"
        );
        let sched = std::fs::read_to_string(fx._state.path().join("jobs/1/job.sh")).unwrap();
        assert!(
            !sched.contains(sentinel),
            "sentinel leaked into the scheduler's script copy: {sched}"
        );
        let status = std::fs::read_to_string(job_dir.join("status.json")).unwrap();
        assert!(
            !status.contains(sentinel),
            "sentinel leaked into status.json"
        );
    }

    /// Restart path (ledger RB-12): a run directory left behind by a killed
    /// driver may hold an env file; the next driver sweeps it before any
    /// submission, so a resume never leaves a recoverable value.
    #[test]
    fn driver_startup_sweeps_stale_secret_env_files() {
        let fx = setup();
        let stale_dir = fx.run_dir.path().join("jobs/stale_rule");
        std::fs::create_dir_all(&stale_dir).unwrap();
        let stale = stale_dir.join(crate::backend::SENSITIVE_ENV_FILE_NAME);
        std::fs::write(&stale, "export OXO_FLOW_SENSITIVE_0='stale-secret'\n").unwrap();

        let (d, _) = driver(&fx);
        let mut plan = chain_plan(fx.workdir.path(), &[("only", "echo ok", "ok.txt")]);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert!(
            !stale.exists(),
            "a stale secret env file must be removed by the startup sweep"
        );
    }

    /// Array path (ledger RB-12): one env file per chunk, sourced by every
    /// per-index command file, removed once the whole array settles.
    #[test]
    fn hpc_array_chunk_secret_env_file_is_sourced_and_removed() {
        let sentinel = "SENTINEL_ARRAY_VALUE_P2";
        let fx = setup();
        let (d, _) = driver(&fx);
        let wf_toml = r#"
[workflow]
name = "secret-array"
[[values]]
name = "assembler"
values = ["spades", "megahit"]
[[rules]]
name = "asm"
output = ["out/{assembler}.txt"]
shell = "mkdir -p out && printf '%s' '{config.api_token}' > out/{assembler}.txt"
"#;
        let wf = fx.workdir.path().join("arr.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let mut plan = ScheduledPlan::build_with_sensitive(
            &config,
            &dag,
            fx.workdir.path(),
            &EnvironmentResolver::new(),
            &HashMap::from([("config.api_token".to_string(), sentinel.to_string())]),
            &[sentinel.to_string()],
        )
        .unwrap();
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[sentinel.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.status == JobStatus::Success));

        // Both elements received the value through the shared env file.
        for assembler in ["spades", "megahit"] {
            let out =
                std::fs::read_to_string(fx.workdir.path().join(format!("out/{assembler}.txt")))
                    .unwrap();
            assert_eq!(out, sentinel);
        }

        let chunk_dir = fx.run_dir.path().join("jobs/asm/chunk-1");
        let job_sh = std::fs::read_to_string(chunk_dir.join("job.sh")).unwrap();
        assert!(
            !job_sh.contains(sentinel),
            "sentinel leaked into the array script"
        );
        for index in [1, 2] {
            let cmd = std::fs::read_to_string(chunk_dir.join(format!("cmd.{index}.sh"))).unwrap();
            assert!(
                !cmd.contains(sentinel),
                "sentinel leaked into cmd.{index}.sh: {cmd}"
            );
            assert!(
                cmd.contains("secrets.env"),
                "the per-index command must source the private env file: {cmd}"
            );
            assert!(
                cmd.contains("${OXO_FLOW_SENSITIVE_0}"),
                "the per-index command must carry the routed reference: {cmd}"
            );
        }
        assert!(
            !chunk_dir
                .join(crate::backend::SENSITIVE_ENV_FILE_NAME)
                .exists(),
            "the array's env file must be removed once every element settled"
        );
        let sched = std::fs::read_to_string(fx._state.path().join("jobs/1_1/job.sh")).unwrap();
        assert!(
            !sched.contains(sentinel),
            "sentinel leaked into the scheduler's array script copy: {sched}"
        );
    }

    /// Artifact inspection (ledger RB-12, issue #1258): after a normal HPC
    /// run, the sentinel is recoverable from NOWHERE in the run directory or
    /// the scheduler's own state directory — checked by recursively reading
    /// every file as bytes, so a stray copy in any artifact (job script,
    /// status, events, checkpoint, scheduler wire file) fails the test with
    /// the offending path named. This is the exit-evidence bullet RB-12 was
    /// still missing: "artifact inspection ... prove no secret is recoverable",
    /// not a per-file `!contains` on the files the author happened to think of.
    #[test]
    fn inspected_run_tree_holds_no_recoverable_secret() {
        let sentinel = "SENTINEL_INSPECTED_VALUE_N5W";
        let fx = setup();
        let (d, _) = driver(&fx);
        let mut plan = secret_plan(
            fx.workdir.path(),
            sentinel,
            "mkdir -p out && printf '%s' '{config.api_token}' > out/token.txt",
            "out/token.txt",
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[sentinel.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Success);

        // The route is still real: the value reached the command.
        let token = std::fs::read_to_string(fx.workdir.path().join("out/token.txt")).unwrap();
        assert_eq!(token, sentinel);

        // Whole-tree inspection, not a chosen-file list.
        assert_marker_absent(fx.run_dir.path(), sentinel, "run directory after success");
        assert_marker_absent(fx._state.path(), sentinel, "mock scheduler state");

        // The container-wrapper and cloud-adapter renderers are covered by
        // this same value-absence contract in `crate::backend::mod.rs`
        // (`build_with_sensitive_routes_before_the_container_wrapper` and the
        // cloud-body tests): the wrapper forwards the reference by NAME only.
        // The one deliberate exception is a literal docker bind-mount whose
        // SOURCE PATH contains the value (issue #1177) — a bind mount cannot
        // be expressed without the runtime seeing the path; that is the
        // documented docker-only caveat, not a regression. Those are unit
        // renderer tests; the live end-to-end campaign is CL-05 / #1158.
    }

    /// Restart path (ledger RB-12, issue #1258): a run directory left behind
    /// by a killed driver holds a populated env file AND a checkpoint; the
    /// next driver sweeps the stale secret before any submission, and the
    /// whole tree is value-free afterwards.
    #[test]
    fn restarted_run_tree_sweeps_seeded_secret_and_checkpoint() {
        let stale_secret = "SENTINEL_STALE_VALUE_R8Q";
        let fresh_secret = "SENTINEL_FRESH_VALUE_T2K";
        let fx = setup();

        // Simulate the crashed predecessor: a populated env file (the ONLY
        // artifact the route ever puts the value into) plus the checkpoint it
        // left behind, under the same run directory the new driver reuses.
        // The checkpoint is seeded with ordinary state — the real checkpoint
        // never carries the routed value, and this test must not manufacture a
        // leak the engine cannot produce.
        let stale_dir = fx.run_dir.path().join("jobs/leak_probe");
        std::fs::create_dir_all(&stale_dir).unwrap();
        std::fs::write(
            stale_dir.join(crate::backend::SENSITIVE_ENV_FILE_NAME),
            format!("export OXO_FLOW_SENSITIVE_0='{stale_secret}'\n"),
        )
        .unwrap();
        std::fs::write(
            fx.run_dir.path().join("checkpoint.json"),
            "{\"rules\":{\"leak_probe\":{\"status\":\"Running\"}}}\n",
        )
        .unwrap();

        // The restart re-runs the rule with a fresh sentinel; the driver's
        // startup sweep must remove the predecessor's env file first.
        let (d, _) = driver(&fx);
        let mut plan = secret_plan(
            fx.workdir.path(),
            fresh_secret,
            "mkdir -p out && printf '%s' '{config.api_token}' > out/token.txt",
            "out/token.txt",
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let records = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[fresh_secret.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, JobStatus::Success);

        // Neither the predecessor's stale value nor the new value survives
        // anywhere in the reused tree (the checkpoint file the test seeded is
        // on the run's sweep path too).
        assert_marker_absent(fx.run_dir.path(), stale_secret, "restarted run tree");
        assert_marker_absent(fx.run_dir.path(), fresh_secret, "restarted run tree");
        assert!(
            !stale_dir
                .join(crate::backend::SENSITIVE_ENV_FILE_NAME)
                .exists(),
            "the predecessor's env file must be swept before resubmission"
        );
    }

    /// Cancellation path, whole-tree form (ledger RB-12, issue #1258):
    /// interrupt a live job and prove the value is recoverable from nowhere
    /// — not only the script and env file the narrower cancel test checks.
    #[test]
    fn cancelled_run_tree_holds_no_recoverable_secret() {
        let sentinel = "SENTINEL_CANCEL_TREE_VALUE_B4M";
        let fx = setup();
        let mut plan = secret_plan(
            fx.workdir.path(),
            sentinel,
            "mkdir -p out && test -n '{config.api_token}' && touch out/started && sleep 30",
            "out/started",
        );
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None::<CancelSignal>);
        let executor = Arc::new(
            ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
                .with_scheduler_dir(fixtures_dir())
                .with_env("MOCK_SCHEDULER_DIR", &fx._state.path().to_string_lossy()),
        );
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: true,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(30)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
                cancel: Some(cancel_rx),
                keep_going: false,
            },
        );
        let signaller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            cancel_tx
                .send(Some(CancelSignal {
                    signal_name: "SIGINT",
                    signum: 2,
                }))
                .unwrap();
        });
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(d.run(
                &mut plan,
                &to_run,
                DriverOptions {
                    run_dir: fx.run_dir.path(),
                    on_checkpoint: None,
                    merge: None,
                    sensitive_values: &[sentinel.to_string()],
                    on_submit: None,
                    on_settle: None,
                },
            ))
            .unwrap_err();
        signaller.join().unwrap();
        assert!(
            matches!(err, OxoFlowError::Cancelled { .. }),
            "expected an interrupt, got: {err}"
        );

        assert_marker_absent(fx.run_dir.path(), sentinel, "cancelled run tree");
        assert_marker_absent(
            fx._state.path(),
            sentinel,
            "mock scheduler state after cancel",
        );
    }
}
