//! BackendDriver: executes a [`ScheduledPlan`] through an [`ExecutorBackend`].
//!
//! The driver never decides *what* to run: the caller computes `to_run` from
//! the shared invalidation predicates (run_preview). It submits waves of at
//! most `max_submitted` in-flight jobs, polls, and maps scheduler states to
//! [`JobRecord`]s — the same record type the local executor produces, so
//! checkpoint semantics are identical.

use super::{BackendJobStatus, ExecutorBackend, ScheduledPlan, ScheduledRule};
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
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            max_submitted: 50,
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: Duration::from_secs(5),
            poll_timeout: None,
        }
    }
}

/// Checkpoint completion hook: returns the names of newly created rule
/// instances, or an error that fails the checkpoint rule.
pub type CheckpointHook<'a> = Box<dyn FnMut(&str) -> Result<Vec<String>> + Send + 'a>;

/// Plan-merge hook: folds newly created instances into the plan (the
/// caller's re-expansion has already produced them).
pub type PlanMerge<'a> = Box<dyn FnMut(&mut ScheduledPlan, &[String]) -> Result<()> + Send + 'a>;

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

impl BackendDriver {
    pub fn new(backend: Arc<dyn ExecutorBackend>, config: DriverConfig) -> Self {
        Self { backend, config }
    }

    /// Cancel every job in `jobs` (`(job_id, rule_name)` pairs), ignoring
    /// per-job errors — best effort on error paths.
    pub async fn cancel_inflight(&self, jobs: &[(String, String)]) -> Result<()> {
        for (id, _rule) in jobs {
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
                let blocked = plan
                    .rules
                    .get(&name)
                    .is_some_and(|sr| sr.dependencies.iter().any(|d| failed.contains(d)));
                if blocked {
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
                        timeout: None,
                        skip_reason: Some("blocked by failed upstream dependency".into()),
                        max_rss_mb: None,
                        cpu_seconds: None,
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

            // 2. Submit a wave of ready rules up to the in-flight cap.
            let mut submitted_this_round = 0usize;
            if inflight.len() < self.config.max_submitted {
                for name in plan.order.clone() {
                    if inflight.len() >= self.config.max_submitted {
                        break;
                    }
                    if !to_run_set.contains(&name)
                        || done.contains_key(&name)
                        || inflight.values().any(|f| f.rule == name)
                        || !deps_ok(&name, &done, &to_run_set, plan)
                    {
                        continue;
                    }
                    if !plan.rules.contains_key(&name) {
                        // Not schedulable (no shell/script) — mirror the local
                        // executor's skip.
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
                            timeout: None,
                            skip_reason: Some("no shell or script defined".into()),
                            max_rss_mb: None,
                            cpu_seconds: None,
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
                                    && deps_ok(o, &done, &to_run_set, plan)
                                    && plan.rules.get(*o).is_some_and(|o_sr| {
                                        o_sr.template == sr.template
                                            && array_key(o_sr) == array_key(&sr)
                                            && o_sr.dependencies == sr.dependencies
                                    })
                            })
                            .map(|o| (o.clone(), plan.rules[o].clone()))
                            .collect()
                    };
                    let mut batch: Vec<(String, ScheduledRule)> = vec![(name.clone(), sr.clone())];
                    batch.extend(siblings);
                    let chunks = batch.chunks(self.config.max_array_size.max(1));
                    for chunk in chunks {
                        if chunk.len() > 1 {
                            // Array submission: one script + per-index
                            // command files under the TEMPLATE's job dir.
                            let job_dir = opts.run_dir.join("jobs").join(sanitize(&sr.template));
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
                            let script = self.backend.render_array_script(
                                &sr.rule,
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
                                    let jobs: Vec<(String, String)> = inflight
                                        .iter()
                                        .map(|(id, f)| (id.clone(), f.rule.clone()))
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
                            // own").
                            {
                                let index_path = opts.run_dir.join("index.json");
                                let mut index: HashMap<String, Vec<String>> =
                                    std::fs::read_to_string(&index_path)
                                        .ok()
                                        .and_then(|raw| serde_json::from_str(&raw).ok())
                                        .unwrap_or_default();
                                index.insert(
                                    base_id.clone(),
                                    chunk.iter().map(|(n, _)| n.clone()).collect(),
                                );
                                if let Some(parent) = index_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::write(
                                    &index_path,
                                    serde_json::to_string_pretty(&index).unwrap_or_default(),
                                );
                            }
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
                                    },
                                );
                                emit(&mut events, "SUBMITTED", c_name, Some(&element_id), None);
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
                                    let jobs: Vec<(String, String)> = inflight
                                        .iter()
                                        .map(|(id, f)| (id.clone(), f.rule.clone()))
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
                                },
                            );
                            submitted_this_round += 1;
                            emit(&mut events, "SUBMITTED", c_name, Some(&job_id), None);
                        }
                    }
                }
            }

            // 3. Poll and settle terminal jobs.
            let mut settled_this_round = 0usize;
            if !inflight.is_empty() {
                let ids: Vec<String> = inflight.keys().cloned().collect();
                let mut statuses = self.backend.poll(&ids).await?;
                // Short jobs vanish from the live queue the instant they
                // finish (squeue/qstat only list active jobs), so a missing
                // id is NOT "still running" — settle it from the accounting
                // store once it is old enough (a fresh submission can
                // legitimately be absent for a moment; the accounting
                // record itself takes a few seconds to appear).
                const ACCOUNT_GRACE_SECS: i64 = 5;
                let now = Utc::now();
                for (id, f) in &inflight {
                    if statuses.contains_key(id) {
                        continue;
                    }
                    if (now - f.submitted_at).num_seconds() < ACCOUNT_GRACE_SECS {
                        continue;
                    }
                    if let Some(st) = self.backend.terminal_status(id).await
                        && st != BackendJobStatus::Unknown
                    {
                        statuses.insert(id.clone(), st);
                    }
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
                for (job_id, f, status) in settled {
                    inflight.remove(&job_id);
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
                            write_status(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                &plan.rules[&f.rule].shell_cmd,
                                "COMPLETED",
                            )?;
                            JobRecord {
                                rule: f.rule.clone(),
                                status: JobStatus::Success,
                                started_at: Some(f.submitted_at),
                                finished_at: Some(Utc::now()),
                                exit_code: Some(0),
                                stdout: None,
                                stderr: None,
                                command: Some(crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                )),
                                retries: 0,
                                timeout: None,
                                skip_reason: None,
                                max_rss_mb: None,
                                cpu_seconds: None,
                            }
                        }
                        BackendJobStatus::Failed => {
                            write_status(
                                &opts.run_dir.join("jobs").join(sanitize(&f.rule)),
                                &job_id,
                                "",
                                "FAILED",
                            )?;
                            JobRecord {
                                rule: f.rule.clone(),
                                status: JobStatus::Failed,
                                started_at: Some(f.submitted_at),
                                finished_at: Some(Utc::now()),
                                exit_code: Some(1),
                                stdout: None,
                                stderr: None,
                                command: Some(crate::executor::process::mask_sensitive(
                                    &plan.rules[&f.rule].shell_cmd,
                                    opts.sensitive_values,
                                )),
                                retries: 0,
                                timeout: None,
                                skip_reason: None,
                                max_rss_mb: None,
                                cpu_seconds: None,
                            }
                        }
                        BackendJobStatus::Cancelled => JobRecord {
                            rule: f.rule.clone(),
                            status: JobStatus::Cancelled,
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
                            timeout: None,
                            skip_reason: Some("cancelled".into()),
                            max_rss_mb: None,
                            cpu_seconds: None,
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
                let jobs: Vec<(String, String)> = inflight
                    .iter()
                    .map(|(id, f)| (id.clone(), f.rule.clone()))
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
fn deps_ok(
    name: &str,
    done: &HashMap<String, JobStatus>,
    to_run: &HashSet<String>,
    plan: &ScheduledPlan,
) -> bool {
    plan.rules.get(name).is_none_or(|sr| {
        sr.dependencies.iter().all(|d| {
            !to_run.contains(d)
                || matches!(done.get(d), Some(JobStatus::Success | JobStatus::Skipped))
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

fn emit(events: &mut std::fs::File, t: &str, rule: &str, job: Option<&str>, reason: Option<&str>) {
    let line = match (job, reason) {
        (Some(j), Some(r)) => {
            format!(r#"{{"t":"{t}","rule":{rule:?},"job":{j:?},"reason":{r:?}}}"#)
        }
        (Some(j), None) => format!(r#"{{"t":"{t}","rule":{rule:?},"job":{j:?}}}"#),
        (None, Some(r)) => format!(r#"{{"t":"{t}","rule":{rule:?},"reason":{r:?}}}"#),
        (None, None) => format!(r#"{{"t":"{t}","rule":{rule:?}}}"#),
    };
    let _ = writeln!(events, "{line}");
}

fn write_status(job_dir: &Path, job_id: &str, command: &str, state: &str) -> Result<()> {
    std::fs::create_dir_all(job_dir).map_err(|e| OxoFlowError::Config {
        message: format!("cannot create {}: {e}", job_dir.display()),
    })?;
    let body = format!(r#"{{"state":{state:?},"job_id":{job_id:?},"command":{command:?}}}"#);
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
        timeout: None,
        skip_reason: Some(format!("re-entry manifest: {e}")),
        max_rss_mb: None,
        cpu_seconds: None,
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
}
