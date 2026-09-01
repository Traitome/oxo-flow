//! BackendDriver: executes a [`ScheduledPlan`] through an [`ExecutorBackend`].
//!
//! The driver never decides *what* to run: the caller computes `to_run` from
//! the shared invalidation predicates (run_preview). It submits waves of at
//! most `max_submitted` in-flight jobs, polls, and maps scheduler states to
//! [`JobRecord`]s — the same record type the local executor produces, so
//! checkpoint semantics are identical.

use super::{BackendJobStatus, ExecutorBackend, ScheduledPlan, ScheduledRule, TerminalRecord};
use crate::error::{OxoFlowError, Result};
use crate::executor::{JobRecord, JobStatus};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
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
    /// (declared + rounds waited, ties by name), so fresh high-priority
    /// arrivals cannot starve a producer parked at the submission cap.
    /// The caller ages the tail that is NOT submitted this round.
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
            pb.cmp(&pa).then_with(|| a.cmp(b))
        });
        ready
    }

    /// Cancel every job in `jobs` (`(job_id, rule_name, array_base)` triples),
    /// ignoring per-job errors — best effort on error paths. Array elements
    /// cancel by their BASE id: qdel/bkill do not understand `{base}_{index}`
    /// (issue #136 H4).
    pub async fn cancel_inflight(&self, jobs: &[(String, String, Option<String>)]) -> Result<()> {
        let mut targets: Vec<&str> = Vec::new();
        for (id, _rule, base) in jobs {
            let target = base.as_deref().unwrap_or(id);
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        for id in targets {
            let _ = self.backend.cancel(id).await;
        }
        Ok(())
    }

    /// Execute exactly `to_run` on `plan`, returning one [`JobRecord`] per
    /// rule. On error the caller keeps any records already returned? No —
    /// errors are terminal: everything in flight is cancelled and the error
    /// propagates (partial state lives in the run directory).
    pub async fn run(
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
        // Priority aging (issue #134): each round a ready rule spends
        // beyond the cap adds +1 to its effective priority.
        let mut waited_rounds: HashMap<String, i32> = HashMap::new();
        // Array index: base job id → element instance names, accumulated in
        // memory and persisted after each change (issue #136 H3 — the
        // read-modify-write pattern could lose earlier chunks).
        let mut array_index: HashMap<String, Vec<String>> = HashMap::new();
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
                        rule: name.clone(),
                        status: JobStatus::Skipped,
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
                            rule: name.clone(),
                            status: JobStatus::Skipped,
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
                            std::fs::create_dir_all(&job_dir).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot create {}: {e}", job_dir.display()),
                                }
                            })?;
                            for (i, (_, c_sr)) in chunk.iter().enumerate() {
                                let cmd_path = job_dir.join(format!("cmd.{}.sh", i + 1));
                                std::fs::write(&cmd_path, &c_sr.shell_cmd).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot write {}: {e}",
                                            cmd_path.display()
                                        ),
                                    }
                                })?;
                            }
                            // The array presents itself under the TEMPLATE
                            // name: naming it after `chunk[0]`'s instance
                            // made squeue/sacct report a job named after one
                            // arbitrary sample. The per-index log paths
                            // follow, so every element shares the template's
                            // log prefix.
                            let mut array_rule = chunk[0].1.rule.clone();
                            array_rule.name = chunk[0].1.template.clone();
                            let script = self.backend.render_array_script(
                                &array_rule,
                                &job_dir.to_string_lossy(),
                                chunk.len(),
                            )?;
                            let script_path = job_dir.join("job.sh");
                            std::fs::write(&script_path, &script).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot write {}: {e}", script_path.display()),
                                }
                            })?;
                            let base_id = match self.backend.submit(&script_path).await {
                                Ok(id) => id,
                                Err(e) => {
                                    let jobs: Vec<(String, String, Option<String>)> = inflight
                                        .iter()
                                        .map(|(id, f)| {
                                            (id.clone(), f.rule.clone(), f.array_base.clone())
                                        })
                                        .collect();
                                    self.cancel_inflight(&jobs).await?;
                                    return Err(e);
                                }
                            };
                            std::fs::write(job_dir.join("job.id"), &base_id).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!(
                                        "cannot write {}: {e}",
                                        job_dir.join("job.id").display()
                                    ),
                                }
                            })?;
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
                                std::fs::create_dir_all(parent).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!("cannot create {}: {e}", parent.display()),
                                    }
                                })?;
                            }
                            std::fs::write(
                                &index_path,
                                serde_json::to_string_pretty(&array_index).unwrap_or_default(),
                            )
                            .map_err(|e| OxoFlowError::Config {
                                message: format!("cannot write {}: {e}", index_path.display()),
                            })?;
                            for (i, (c_name, c_sr)) in chunk.iter().enumerate() {
                                let element_id = format!("{}_{}", base_id, i + 1);
                                // Per-INSTANCE dirs keep the run directory
                                // greppable (jobs/<instance>/job.sh + job.id)
                                // — the array stays an implementation detail
                                // (issue #74 phase 3). job.sh is a copy of
                                // the array script: it is exactly what ran
                                // for this element.
                                let inst_dir = opts.run_dir.join("jobs").join(sanitize(c_name));
                                std::fs::create_dir_all(&inst_dir).map_err(|e| {
                                    OxoFlowError::Config {
                                        message: format!(
                                            "cannot create {}: {e}",
                                            inst_dir.display()
                                        ),
                                    }
                                })?;
                                std::fs::copy(&script_path, inst_dir.join("job.sh")).map_err(
                                    |e| OxoFlowError::Config {
                                        message: format!(
                                            "cannot copy {}: {e}",
                                            inst_dir.join("job.sh").display()
                                        ),
                                    },
                                )?;
                                std::fs::write(inst_dir.join("job.id"), &element_id).map_err(
                                    |e| OxoFlowError::Config {
                                        message: format!(
                                            "cannot write {}: {e}",
                                            inst_dir.join("job.id").display()
                                        ),
                                    },
                                )?;
                                write_status(&inst_dir, &element_id, &c_sr.shell_cmd, "PENDING")?;
                                inflight.insert(
                                    element_id.clone(),
                                    InFlight {
                                        rule: c_name.clone(),
                                        submitted_at: Utc::now(),
                                        array_base: Some(base_id.clone()),
                                    },
                                );
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
                            let script = self.backend.render_script(c_sr)?;
                            let job_dir = opts.run_dir.join("jobs").join(sanitize(c_name));
                            std::fs::create_dir_all(&job_dir).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot create {}: {e}", job_dir.display()),
                                }
                            })?;
                            let script_path = job_dir.join("job.sh");
                            std::fs::write(&script_path, &script).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot write {}: {e}", script_path.display()),
                                }
                            })?;
                            let job_id = match self.backend.submit(&script_path).await {
                                Ok(id) => id,
                                Err(e) => {
                                    let jobs: Vec<(String, String, Option<String>)> = inflight
                                        .iter()
                                        .map(|(id, f)| {
                                            (id.clone(), f.rule.clone(), f.array_base.clone())
                                        })
                                        .collect();
                                    self.cancel_inflight(&jobs).await?;
                                    return Err(e);
                                }
                            };
                            std::fs::write(job_dir.join("job.id"), &job_id).map_err(|e| {
                                OxoFlowError::Config {
                                    message: format!("cannot write {}: {e}", job_dir.display()),
                                }
                            })?;
                            write_status(&job_dir, &job_id, &c_sr.shell_cmd, "PENDING")?;
                            inflight.insert(
                                job_id.clone(),
                                InFlight {
                                    rule: c_name.clone(),
                                    submitted_at: Utc::now(),
                                    array_base: None,
                                },
                            );
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
                let polled = self.backend.poll(&ids).await?;
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
                for (job_id, f, status) in settled {
                    inflight.remove(&job_id);
                    let acct = accounting.get(&job_id).copied();
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
                                        write_status(
                                            &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                            &job_id,
                                            "",
                                            "FAILED",
                                        )?;
                                        return_record_failure(&mut records, &mut done, &f, &e);
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
                                                    merge(plan, &new_names)?;
                                                    for n in &new_names {
                                                        to_run_set.insert(n.clone());
                                                    }
                                                }
                                                None => {
                                                    return Err(OxoFlowError::Config {
                                                        message: "checkpoint re-entry produced new instances but no merge hook is configured".into(),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            write_status_full(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                &plan.rules[&f.rule].shell_cmd,
                                "COMPLETED",
                                Some((f.submitted_at, observed_at)),
                                acct,
                            )?;
                            JobRecord {
                                rule: f.rule.clone(),
                                status: JobStatus::Success,
                                started_at: Some(started_at),
                                finished_at: Some(observed_at),
                                exit_code: Some(acct.and_then(|a| a.exit_code).unwrap_or(0)),
                                stdout: None,
                                stderr: None,
                                command: Some(crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                )),
                                retries: 0,
                                skip_reason: None,
                                max_rss_mb: acct.and_then(|a| a.max_rss_mb),
                                cpu_seconds: acct.and_then(|a| a.cpu_seconds).map(|s| s as f64),
                                caption: crate::executor::process::rule_report_caption(
                                    &plan.rules[&f.rule].rule,
                                    &plan.rules[&f.rule].workdir,
                                ),
                            }
                        }
                        BackendJobStatus::Failed => {
                            write_status_full(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                "",
                                "FAILED",
                                Some((f.submitted_at, observed_at)),
                                acct,
                            )?;
                            JobRecord {
                                rule: f.rule.clone(),
                                status: JobStatus::Failed,
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
                                skip_reason: None,
                                max_rss_mb: acct.and_then(|a| a.max_rss_mb),
                                cpu_seconds: acct.and_then(|a| a.cpu_seconds).map(|s| s as f64),
                                caption: crate::executor::process::rule_report_caption(
                                    &plan.rules[&f.rule].rule,
                                    &plan.rules[&f.rule].workdir,
                                ),
                            }
                        }
                        BackendJobStatus::Cancelled => JobRecord {
                            rule: f.rule.clone(),
                            status: JobStatus::Cancelled,
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
                            skip_reason: Some("cancelled".into()),
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
                self.cancel_inflight(&jobs).await?;
                return Err(OxoFlowError::Config {
                    message: "poll timeout exceeded".into(),
                });
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
        Ok(records)
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
    let line = match (job, reason) {
        (Some(j), Some(r)) => {
            format!(r#"{{"ts":"{ts}","t":"{t}","rule":{rule:?},"job":{j:?},"reason":{r:?}}}"#)
        }
        (Some(j), None) => format!(r#"{{"ts":"{ts}","t":"{t}","rule":{rule:?},"job":{j:?}}}"#),
        (None, Some(r)) => format!(r#"{{"ts":"{ts}","t":"{t}","rule":{rule:?},"reason":{r:?}}}"#),
        (None, None) => format!(r#"{{"ts":"{ts}","t":"{t}","rule":{rule:?}}}"#),
    };
    let _ = writeln!(events, "{line}");
}

fn write_status(job_dir: &Path, job_id: &str, command: &str, state: &str) -> Result<()> {
    write_status_full(job_dir, job_id, command, state, None, None)
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
    let mut fields = vec![
        format!(r#""state":{state:?}"#),
        format!(r#""job_id":{job_id:?}"#),
        format!(r#""command":{command:?}"#),
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
    std::fs::write(job_dir.join("status.json"), body).map_err(|e| OxoFlowError::Config {
        message: format!("cannot write status.json: {e}"),
    })
}

fn return_record_failure(
    records: &mut Vec<JobRecord>,
    done: &mut HashMap<String, JobStatus>,
    f: &InFlight,
    e: &OxoFlowError,
) {
    records.push(JobRecord {
        rule: f.rule.clone(),
        status: JobStatus::Failed,
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
    });
    done.insert(f.rule.clone(), JobStatus::Failed);
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
        let executor = Arc::new(ClusterExecutor::new(
            ClusterBackend::Slurm,
            cluster_config(),
        ));
        let d = BackendDriver::new(
            executor,
            DriverConfig {
                max_submitted: 2,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(50),
                poll_timeout: None,
                unknown_settle_grace: std::time::Duration::from_secs(90),
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
        let (d, _) = driver(&fx);
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
                },
            ))
            .unwrap_err();
        assert!(
            format!("{err}").contains("index.json"),
            "the error must name the index.json write: {err}"
        );
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
        /// Element count of every array submission, in submit order.
        array_sizes: Vec<usize>,
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
            Ok(job_ids
                .iter()
                .map(|id| (id.clone(), st.verdict.unwrap()))
                .collect())
        }
        async fn cancel(&self, job_id: &str) -> Result<()> {
            self.inner
                .lock()
                .unwrap()
                .cancelled
                .push(job_id.to_string());
            Ok(())
        }
        async fn logs(&self, _job_id: &str) -> Result<String> {
            Ok(String::new())
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
        // element still settles.
        let fx = setup();
        let plan = sibling_plan(fx.workdir.path(), 2);
        let to_run: HashSet<String> = plan.order.iter().cloned().collect();
        let backend = Arc::new(MockBackend::new(false, BackendJobStatus::Completed));
        let d = BackendDriver::new(
            backend.clone(),
            DriverConfig {
                max_submitted: 4,
                max_array_size: 1001,
                no_arrays: false,
                poll_interval: std::time::Duration::from_millis(10),
                poll_timeout: Some(std::time::Duration::from_secs(10)),
                unknown_settle_grace: std::time::Duration::from_secs(90),
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
                },
            ))
            .unwrap();
        assert_eq!(records.len(), 2);
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
}
