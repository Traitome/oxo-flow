//! Fine-grained, event-driven scheduler execution loop.
//!
//! Replaces group barriers with immediate task dispatch upon dependency satisfaction.
//! Handles disk-pressure monitoring, lost-wakeup watchdogs, signal interception,
//! task reaping, failure aborts, and post-run completion coordination.

use super::*;
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::LocalExecutor;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Parameters needed to execute the event-driven scheduler loop.
pub(crate) struct SchedulerLoopContext<'a> {
    pub jobs: usize,
    pub order: &'a mut Vec<String>,
    pub config: &'a mut WorkflowConfig,
    pub dag: &'a mut WorkflowDag,
    pub tombstone_keep: &'a HashSet<String>,
    pub workdir_actual: &'a Arc<PathBuf>,
    pub tmp_dir_resolved: Option<&'a Path>,
    pub checkpoint: &'a Arc<Mutex<CheckpointState>>,
    pub checkpoint_path: &'a Path,
    pub executor: &'a Arc<LocalExecutor>,
    pub run_log: &'a SharedRunLog,
    pub progress: &'a indicatif::ProgressBar,
    pub is_tty: bool,
    pub keep_going: bool,
    pub rerun: bool,
    pub ai_recover: bool,
    pub ai_max_retries: Option<u32>,
    pub no_report_snapshot: bool,
    pub provenance: bool,
    pub sensitive_values: &'a [String],
    pub wildcard_values: &'a Arc<HashMap<String, String>>,
    pub samples_filter: &'a [String],
    pub workflow: &'a Path,
    pub json_summary: &'a mut RunJsonSummary,
}

pub(crate) async fn run_scheduler_loop(ctx: SchedulerLoopContext<'_>) -> anyhow::Result<()> {
    let SchedulerLoopContext {
        jobs,
        order,
        config,
        dag,
        tombstone_keep,
        workdir_actual,
        tmp_dir_resolved,
        checkpoint,
        checkpoint_path,
        executor,
        run_log,
        progress,
        is_tty,
        keep_going,
        rerun,
        ai_recover,
        ai_max_retries,
        no_report_snapshot,
        provenance,
        sensitive_values,
        wildcard_values,
        samples_filter,
        workflow,
        json_summary,
    } = ctx;

    let success_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provenance_recorded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let non_required_fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_rules_set: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let skipped_no_output: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let failures: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let blocked: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let rule_names: Vec<String> = order.to_vec();
    let rule_name_refs: Vec<&str> = rule_names.iter().map(String::as_str).collect();
    let mut sched = oxo_flow_core::scheduler::SchedulerState::new(&rule_name_refs);
    let mut order_set: std::collections::HashSet<String> = order.iter().cloned().collect();
    let run_started = std::time::Instant::now();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs.max(1)));
    let mut join_set = tokio::task::JoinSet::new();
    let mut submitted: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Task id → rule name for panic attribution (issue #136 fix 2): a
    // panicked task leaves the JoinSet without its payload, so the id is
    // the only way to learn which rule died and release its scheduler slot.
    let mut task_rule: std::collections::HashMap<tokio::task::Id, String> =
        std::collections::HashMap::new();
    // Task id → abort handle (issue #872): the watchdog requeue aborts a
    // specific stuck spawn task; `JoinSet` itself cannot abort by id.
    let mut task_abort: std::collections::HashMap<tokio::task::Id, tokio::task::AbortHandle> =
        std::collections::HashMap::new();

    // Pre-process checkpoint-completed rules so they are never re-submitted.
    {
        let ck = checkpoint.lock().await;
        for rule_name in order.iter() {
            // A rule still marked running (issue #685) is never "already
            // completed": the previous run died with it mid-flight, its
            // outputs may be half-written, and treating it as done would
            // silently skip real work.
            if ck.is_running(rule_name) {
                continue;
            }
            if ck.is_completed(rule_name)
                && !rerun
                && order_set.contains(rule_name.as_str())
                && !submitted.contains(rule_name.as_str())
            {
                // Shared with dry-run's read-only preview (issue #66).
                // Tombstoned rules count as up to date while no dependent
                // needs their outputs.
                let outputs_ok = tombstone_keep.contains(rule_name)
                    || config
                        .get_rule(rule_name)
                        .map(|rule| {
                            crate::commands::run_preview::rule_outputs_exist(
                                rule,
                                workdir_actual.as_ref(),
                                wildcard_values,
                            )
                        })
                        .unwrap_or(true);
                if outputs_ok {
                    submitted.insert(rule_name.clone());
                    sched.mark_completed(oxo_flow_core::executor::JobRecord {
                        signal: None,
                        rule: rule_name.clone(),
                        status: oxo_flow_core::executor::JobStatus::Success,
                        enqueued_at: None,
                        started_at: None,
                        finished_at: None,
                        exit_code: Some(0),
                        stdout: None,
                        stderr: None,
                        command: None,
                        retries: 0,
                        skip_reason: None,
                        max_rss_mb: None,
                        cpu_seconds: None,
                        caption: config.get_rule(rule_name).and_then(|r| {
                            oxo_flow_core::executor::process::rule_report_caption(
                                r,
                                workdir_actual.as_ref(),
                            )
                        }),
                    });
                    skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !is_tty {
                        diagnostic_narrate(
                            format_args!("  {} {} (already completed)", "⊝".dimmed(), rule_name),
                            Some(run_log),
                        );
                    }
                    progress.inc(1);
                }
            }
        }
    }

    // Ready-list ordering (age_ready_list below, called every dispatch
    // round): the scheduler's ready rules are filtered to this run's order
    // set minus already-submitted rules, then sorted by EFFECTIVE priority
    // — declared priority plus the rounds a rule has waited ready-but-not-
    // submitted (the aging counter of issue #123, cluster-side issue #134)
    // — descending, ties broken by name for determinism. Priorities are
    // captured ONCE up front: the config mutates mid-run across checkpoint
    // re-entry (issue #78 P3), and re-reading priorities per round would
    // drift the ordering.
    let priority_map: std::collections::HashMap<String, i32> = config
        .rules
        .iter()
        .map(|r| (r.name.clone(), r.priority))
        .collect();
    // Fair-dispatch aging (issue #123): every dispatch round a rule spends
    // ready-but-not-submitted adds AGING_STEP to its effective priority, so
    // high-priority rules that keep failing/re-occupying slots cannot starve
    // their lower-priority producers forever (live: auto-sra dumps at p10
    // starved behind merges at p20 sharing the limit_merge group). Aging
    // only matters under contention — with free slots everything submits
    // immediately and the relative order is unchanged.
    const AGING_STEP: i32 = 1;
    let task_ctx = Arc::new(TaskContext {
        executor: executor.clone(),
        checkpoint: checkpoint.clone(),
        checkpoint_path: checkpoint_path.to_path_buf(),
        workdir_actual: workdir_actual.clone(),
        task_run_log: Arc::clone(run_log),
        sensitive_values: sensitive_values.to_vec(),
        failures: failures.clone(),
        failed_rules_set: failed_rules_set.clone(),
        skipped_no_output: skipped_no_output.clone(),
        success_count: success_count.clone(),
        fail_count: fail_count.clone(),
        non_required_fail_count: non_required_fail_count.clone(),
        skipped_count: skipped_count.clone(),
        provenance_recorded: provenance_recorded.clone(),
        provenance,
        keep_going,
        is_tty,
        semaphore: semaphore.clone(),
    });

    let mut waited_rounds: std::collections::HashMap<String, i32> = Default::default();

    // Spawn watchdog (issue #685): wall-clock time each rule was marked
    // running, used to detect rules stuck in Running without ever spawning
    // a child process (lost wakeup wedge). Entries are removed in the
    // common completion funnel below; cleanup on every terminal path.
    let mut running_since: std::collections::HashMap<String, std::time::Instant> =
        Default::default();
    // Watchdog fire counters (issue #872): the watchdog re-arms every
    // threshold interval instead of reporting once per rule per run, and
    // counts consecutive fires per rule. A rule reaching
    // `watchdog_requeue_fires` on the ERROR branch (dispatched but never
    // forked) is requeued to Pending; queued (WARN) rules never escalate.
    let mut watchdog_fires: std::collections::HashMap<String, u32> = Default::default();
    let watchdog_requeue_fires = watchdog_requeue_fires();
    let watchdog_threshold = spawn_watchdog_threshold();
    let mut watchdog_tick =
        tokio::time::interval(std::time::Duration::from_secs(SPAWN_WATCHDOG_TICK_SECS));

    // #843 disk-pressure monitor: level thresholds resolved once from the
    // validated config (None = feature off — zero monitoring overhead).
    let disk_thresholds = config.engine.as_ref().and_then(|e| e.disk_thresholds());
    let mut disk_monitor = oxo_flow_core::scheduler::DiskPressureMonitor::new(
        disk_thresholds.map_or(0, |t| t.0),
        disk_thresholds.map_or(0, |t| t.1),
    );
    let mut disk_tick = tokio::time::interval(std::time::Duration::from_secs(disk_tick_secs()));
    // Grace rounds accumulated while parked under Hold with nothing in
    // flight; disk recovery resets it.
    let mut disk_hold_rounds: u32 = 0;
    let disk_hold_abort_rounds = disk_hold_abort_rounds();

    // ---- main event loop -------------------------------------------------

    // Termination signals (audit C14): SIGINT/SIGTERM/SIGHUP stop the run
    // through `terminate_on_signal` instead of killing the process outright.
    // A watch channel keeps the wait re-armable across loop iterations, the
    // way a oneshot receiver could only be awaited once.
    let (signal_tx, mut signal_rx) = tokio::sync::watch::channel(None::<InterruptSignal>);
    tokio::spawn(async move {
        if let Some(signal) = wait_for_termination_signal().await
            && signal_tx.send(Some(signal)).is_err()
        {
            // The run loop is gone but the handlers are still installed, so
            // a late signal must still end the process.
            std::process::exit(signal.exit_code());
        }
    });

    loop {
        // #843 disk-pressure gate — measure BEFORE any dispatch decision so
        // a floor crossing narrates once (crossing dedup inside the monitor)
        // and enforces its policy this round.
        if disk_thresholds.is_some() {
            let mut n = |args: std::fmt::Arguments<'_>| {
                diagnostic_narrate(args, Some(run_log));
            };
            let level = disk_monitor.observe(workdir_actual.as_ref(), &mut n);
            let hold = level >= oxo_flow_core::scheduler::DiskPressureLevel::Hold;

            // Reclaim BEFORE the Hold-park decision (review F1): a run that
            // starts below the floor — or falls through to Hold within a
            // single tick — must still get a reclaim pass before parking,
            // or the "nothing left to reclaim" abort below is untrue and
            // reclaimable intermediates are never released.
            if level >= oxo_flow_core::scheduler::DiskPressureLevel::Reclaim {
                // Completed temporary rules whose dependents are all done
                // no longer need their outputs.
                let wildcards = wildcard_values.clone();
                let candidates = {
                    let ck = checkpoint.lock().await;
                    oxo_flow_core::scheduler::reclaim_candidates(
                        &config.rules,
                        dag,
                        &ck,
                        workdir_actual.as_ref(),
                        &wildcards,
                        &config.expansion_templates,
                    )
                };
                for candidate in candidates {
                    if let Err(reclaim_err) = reclaim_rule_outputs(
                        &candidate,
                        workdir_actual.as_ref(),
                        checkpoint,
                        checkpoint_path,
                        run_log,
                    )
                    .await
                    {
                        diagnostic_narrate(
                            format_args!(
                                "  {} reclaim of '{}' failed: {reclaim_err}",
                                "!".yellow(),
                                candidate.rule
                            ),
                            Some(run_log),
                        );
                    }
                }
            }
            if hold && !sched.is_complete() && sched.running_count() == 0 && join_set.is_empty() {
                // Nothing in flight and nothing dispatchable: park for a
                // tick (external space may free up) and re-measure. After
                // disk_hold_abort_rounds consecutive parked rounds below the
                // floor, abort budget-abort style — reclaim has already fired
                // above, before the park decision; if the disk is still full,
                // no progress is possible.
                if disk_hold_rounds >= disk_hold_abort_rounds {
                    {
                        let ck = checkpoint.lock().await;
                        ck.save_to_file_async(checkpoint_path).await.ok();
                    }
                    progress.finish_and_clear();
                    json_summary.emit("failed", &RunCounts::default(), vec![]);
                    anyhow::bail!(
                        "disk pressure: {}MB free is below min_free_disk ({}MB) with no rules \
                         in flight and nothing left to reclaim — the run cannot make progress. \
                         Free space manually (delete intermediates or run `oxo-flow clean`), \
                         then resume from the checkpoint, or lower [engine] min_free_disk.",
                        oxo_flow_core::scheduler::check_available_disk_mb(workdir_actual.as_ref())
                            .unwrap_or(0),
                        disk_thresholds.map_or(0, |t| t.0),
                    );
                }
                disk_hold_rounds += 1;
                tokio::time::sleep(std::time::Duration::from_secs(disk_tick_secs())).await;
                continue;
            }
            // Any pass that is not a parked hold clears the grace counter.
            disk_hold_rounds = 0;
        }

        // Check deadlock before each scheduling round.
        if !sched.is_complete() && sched.running_count() == 0 && join_set.is_empty() {
            sched.check_deadlock(dag)?;
        }

        // Submit every rule whose dependencies are now satisfied — capped to
        // the free -j slots so the semaphore queue never builds, and the
        // ready list (aged by age_ready_list) decides dispatch order fairly.
        let ready = age_ready_list(
            sched.ready_rules(dag)?,
            &order_set,
            &submitted,
            &priority_map,
            &waited_rounds,
        );
        // jobs was clamped to >= 1 at run_command entry, so a raw 0 can
        // never silently suppress all submissions (issue #136 fix 1).
        // Under #843 Hold, new dispatches are withheld entirely — in-flight
        // rules continue to completion.
        let available = if disk_thresholds.is_some()
            && disk_monitor.last_level() >= oxo_flow_core::scheduler::DiskPressureLevel::Hold
        {
            0
        } else {
            jobs.saturating_sub(sched.running_count())
        };
        let to_submit: Vec<String> = ready.iter().take(available).cloned().collect();
        for name in ready.iter().skip(available) {
            *waited_rounds.entry(name.clone()).or_insert(0) += AGING_STEP;
        }
        for rule_name in &to_submit {
            // Skip rules blocked by a failed — or no-output-skipped — upstream
            // dependency. Optional rules are exempt: they evaluate their own
            // inputs and skip gracefully when they're absent (issue #200).
            let rule_is_optional = config
                .get_rule(rule_name)
                .is_some_and(|r| r.optional.is_optional());
            let blocked_by = if rule_is_optional {
                None
            } else {
                let frs = failed_rules_set.lock().await;
                let sno = skipped_no_output.lock().await;
                dag.dependencies(rule_name).ok().and_then(|deps| {
                    deps.into_iter()
                        .find(|d| frs.contains(d.as_str()) || sno.contains(d.as_str()))
                })
            };
            if let Some(dep) = blocked_by {
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Propagate transitively: this rule produced nothing, so its
                // own dependents are blocked the same way.
                let dep_skipped = {
                    let sno = skipped_no_output.lock().await;
                    sno.contains(dep.as_str())
                };
                if dep_skipped {
                    let mut sno = skipped_no_output.lock().await;
                    sno.insert(rule_name.clone());
                } else {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                blocked.lock().await.push((rule_name.clone(), dep));
                submitted.insert(rule_name.clone());
                sched.mark_completed(oxo_flow_core::executor::JobRecord {
                    signal: None,
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Skipped,
                    enqueued_at: None,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    skip_reason: Some(
                        if dep_skipped {
                            "blocked by skipped upstream dependency"
                        } else {
                            "blocked by failed upstream dependency"
                        }
                        .into(),
                    ),
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: config.get_rule(rule_name).and_then(|r| {
                        oxo_flow_core::executor::process::rule_report_caption(
                            r,
                            workdir_actual.as_ref(),
                        )
                    }),
                });
                if !is_tty {
                    narrate_console_line(format_args!(
                        "  {} {} (blocked by {} dependency)",
                        "⊘".yellow(),
                        rule_name,
                        if dep_skipped { "skipped" } else { "failed" }
                    ));
                }
                progress.inc(1);
                continue;
            }

            submitted.insert(rule_name.clone());
            sched.mark_running(rule_name);
            // Spawn-time persistence (issue #685, ask 3): the checkpoint's
            // running set is written when the rule actually enters flight,
            // so a crashed run leaves an honest record for the next start.
            {
                let mut ck = checkpoint.lock().await;
                ck.mark_running(rule_name);
                if let Err(e) = ck.save_to_file_async(checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint (running mark): {e}");
                }
            }
            running_since.insert(rule_name.clone(), std::time::Instant::now());

            // ---- spawn task via execute_rule_task (task.rs) -----
            let Some(rule) = config.get_rule(rule_name).cloned() else {
                anyhow::bail!("rule '{rule_name}' not found in workflow");
            };
            let instance_bindings = config.instance_bindings(rule_name);
            progress.set_message(format!("executing {}", rule_name));
            if !is_tty {
                progress_narrate(
                    format_args!("  {} {}", "Running:".bold().cyan(), rule_name),
                    run_log,
                );
            }
            emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleStarted {
                rule: rule_name.clone(),
                command: None,
            });

            let typed_config = config.config.clone();
            let rule_caption = config.get_rule(rule_name).and_then(|r| {
                oxo_flow_core::executor::process::rule_report_caption(r, workdir_actual.as_ref())
            });
            let when_gated_off = rule
                .when
                .as_deref()
                .is_some_and(|when| {
                    !when.contains('{')
                        && !oxo_flow_core::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                            when,
                            &config.config,
                            wildcard_values,
                            Some(workdir_actual.as_ref().as_path()),
                        )
                });
            let task_rule_name = rule_name.clone();
            let handle = join_set.spawn(execute_rule_task(
                TaskInvocation {
                    rule,
                    rule_caption,
                    when_gated_off,
                    instance_bindings,
                    wildcard_values: wildcard_values.clone(),
                    typed_config,
                },
                task_ctx.clone(),
            ));
            task_rule.insert(handle.id(), task_rule_name);
            task_abort.insert(handle.id(), handle);
        }

        // ---- wait for completions -----------------------------------------

        // If nothing is in-flight, we're done (all rules completed, failed, or blocked).
        if join_set.is_empty() {
            break;
        }

        // Wait for the next rule to finish — or for a termination signal
        // (audit C14): the in-flight rules must be torn down and the
        // checkpoint persisted before the process exits, instead of the
        // default disposition killing oxo-flow outright and orphaning every
        // child (PPID 1) with a half-written checkpoint.
        let join_result = tokio::select! {
            // `biased` with join-next first: a result that is already
            // reapable is ALWAYS drained before the signal branch can be
            // taken. Without this, a signal landing in the same poll
            // cycle as a completion races it at random — the teardown
            // would then see the finished rule's id still in `task_rule`
            // and record it Failed in the checkpoint.
            biased;
            result = join_set.join_next_with_id() => Some(result),
            received = async {
                match signal_rx.changed().await {
                    Ok(()) => *signal_rx.borrow_and_update(),
                    // The watcher died before the run did; the run then
                    // keeps the signals' default dispositions.
                    Err(_) => None,
                }
            } => match received {
                Some(signal) => {
                    terminate_on_signal(
                        signal,
                        executor,
                        &mut join_set,
                        task_rule.values(),
                        checkpoint,
                        checkpoint_path,
                        json_summary,
                        config,
                    )
                    .await
                }
                // The watcher died before the run did; the run then keeps
                // the signals' default dispositions.
                None => None,
            },
            // Spawn watchdog (issue #685): a rule stuck in Running without
            // ever spawning a child process wedged the engine in a futex
            // wait (empty checkpoint running-set, zero completions). Every
            // SPAWN_WATCHDOG_TICK_SECS, compare the scheduler's running set
            // with the executor's live child pids: any rule running longer
            // than the threshold is re-reported each interval, with the
            // consecutive-fire count. Issue #847 splits the report: a rule
            // still queued in the resource pool is a benign wait — the
            // scheduler's own 60s blockage WARNs already narrate holders
            // and FIFO position — so it only earns a WARN here and never
            // escalates. The #685 ERROR is reserved for rules NOT queued in
            // the pool (env-setup stragglers have their own throttled WARN;
            // anything else is the genuine lost-wakeup signature). After
            // `watchdog_requeue_fires` consecutive ERROR fires the rule is
            // requeued to Pending (issue #872): both live wedge recoveries
            // needed ~9× the threshold to self-heal, so a bounded requeue
            // beats indefinite lane-freezing keep-waiting.
            _ = watchdog_tick.tick() => {
                let live_rules: std::collections::HashSet<String> = executor
                    .active_pids()
                    .into_iter()
                    .map(|(rule, _)| rule)
                    .collect();
                let queued: std::collections::HashSet<String> =
                    executor.resource_waiters().await.into_iter().collect();
                let n = |args: std::fmt::Arguments<'_>| {
                    diagnostic_narrate(args, Some(run_log));
                };
                for rule in sched.running_rules() {
                    if live_rules.contains(&rule) {
                        watchdog_fires.remove(&rule);
                        continue;
                    }
                    let Some(started) = running_since.get(&rule) else {
                        continue;
                    };
                    if started.elapsed() < watchdog_threshold {
                        continue;
                    }
                    let running_secs = started.elapsed().as_secs();
                    if queued.contains(&rule) {
                        let fires = watchdog_fires.entry(rule.clone()).or_insert(0);
                        *fires += 1;
                        n(format_args!(
                            "{} rule '{rule}' has been Running for {running_secs}s while \
                             queued in the resource pool — benign wait (watchdog fire {}), \
                             the scheduler reports blockage details every 60s (issue #847). \
                             The engine will keep waiting.",
                            "Watchdog:".yellow().bold(),
                            *fires
                        ));
                        continue;
                    }
                    // Not queued and no live child: dispatched but never
                    // forked (issue #872 dispatch-state wording).
                    let fires = watchdog_fires.entry(rule.clone()).or_insert(0);
                    *fires += 1;
                    if *fires < watchdog_requeue_fires {
                        n(format_args!(
                            "{} rule '{rule}' has been Running for {running_secs}s but never \
                             spawned a child process (dispatched but not forked, fire {} of {}) \
                             — likely causes: environment setup still in progress, or a lost \
                             scheduler wakeup (issue #685). The engine will keep waiting; at \
                             fire {} the rule is requeued to Pending.",
                            "Watchdog ERROR:".yellow().bold(),
                            *fires,
                            watchdog_requeue_fires,
                            watchdog_requeue_fires
                        ));
                        tracing::error!(
                            rule = %rule,
                            running_secs,
                            fires = *fires,
                            state = "dispatched-not-forked",
                            "rule has been Running for over {}s but never spawned a child \
                             process (issue #685)",
                            watchdog_threshold.as_secs()
                        );
                        continue;
                    }
                    // Escalation: requeue the stuck rule (issue #872). Abort
                    // its spawn task so the submit-cap slot is freed; the
                    // reap path routes the resulting non-panic cancellation
                    // to a no-op — all cleanup happens here.
                    n(format_args!(
                        "{} rule '{rule}' has been Running for {running_secs}s but never \
                         spawned a child process after {} watchdog fires (~{}s) — requeueing \
                         it to Pending. Its submit slot is released and the rule is \
                         re-dispatched when it next becomes ready; if it wedges again the \
                         watchdog will re-report and requeue it (issue #872).",
                        "Watchdog ERROR:".yellow().bold(),
                        *fires,
                        running_secs
                    ));
                    tracing::error!(
                        rule = %rule,
                        running_secs,
                        fires = *fires,
                        state = "requeued",
                        "rule requeued to Pending after {} watchdog fires without a child \
                         process (issue #872)",
                        *fires
                    );
                    let stuck_id = task_rule
                        .iter()
                        .find_map(|(id, name)| (name == &rule).then_some(*id));
                    match stuck_id.and_then(|id| task_abort.get(&id).map(|h| (id, h))) {
                        Some((task_id, abort)) => {
                            // Tree-kill before abort: the wedged task may have
                            // spawned grand-children that kill_on_drop would
                            // orphan; they could race the re-dispatched rule's
                            // outputs. Snapshot the pid while the task is
                            // still live, then sweep after the abort — same
                            // shape as terminate_on_signal (#524) and the
                            // first-failure abort. The fresh-pid check here
                            // also closes the requeue-vs-reap race (review
                            // NIT 6): a rule reaped between snapshot and
                            // abort has no pid left to kill.
                            let stuck_pids: Vec<u32> = executor
                                .active_pids()
                                .into_iter()
                                .filter(|(name, _)| name == &rule)
                                .map(|(_, pid)| pid)
                                .collect();
                            abort.abort();
                            task_abort.remove(&task_id);
                            task_rule.remove(&task_id);
                            if !stuck_pids.is_empty() {
                                // kill_process_tree's grace poll sleeps up to
                                // 10 s — run the sweep on the blocking pool,
                                // not a runtime worker (#524).
                                let rule_name = rule.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    for pid in stuck_pids {
                                        if let Err(e) = oxo_flow_core::executor::timeout::
                                            kill_process_tree(pid)
                                        {
                                            tracing::warn!(
                                                rule = %rule_name,
                                                pid,
                                                "watchdog requeue: failed to kill stuck \
                                                 process tree: {e}"
                                            );
                                        }
                                    }
                                })
                                .await;
                            }
                        }
                        None => {
                            // No task link: the id→rule registration is the only
                            // path back to the spawn task. Without it we cannot
                            // free the submit slot, so fail the run loudly
                            // instead of silently shrinking the cap (mirrors the
                            // reap-path internal-error invariant).
                            json_summary.emit(
                                "failed",
                                &RunCounts {
                                    succeeded: success_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    skipped: skipped_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    failed: fail_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    non_required_failed: non_required_fail_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    blocked: blocked.lock().await.len(),
                                },
                                vec![],
                            );
                            return Err(anyhow::anyhow!(
                                "internal error: watchdog requeue found no task for rule '{rule}'"
                            ));
                        }
                    }
                    executor.clear_resource_waiters().await;
                    if let Some(rule_cfg) = config.get_rule(&rule) {
                        executor.release_resources(rule_cfg).await;
                    }
                    // The spawn task may still be reaped later this tick; the
                    // scheduler running-set entry it would otherwise hold is
                    // already cleared above, so dependents re-admit normally.
                    sched.reset_to_pending(&rule);
                    {
                        let mut ck = checkpoint.lock().await;
                        ck.clear_running(&rule);
                        if let Err(save_err) = ck.save_to_file_async(checkpoint_path).await {
                            tracing::warn!("Failed to save checkpoint: {save_err}");
                        }
                    }
                    running_since.remove(&rule);
                    submitted.remove(&rule);
                    watchdog_fires.remove(&rule);
                }
                None
            }
            // #843 disk-pressure tick: while rules are in flight, re-measure
            // free space periodically so Reclaim fires even without a
            // completion boundary (e.g. a single long-running rule writing
            // hundreds of GB). Expression is None — the measured state is
            // enforced at the top of the next loop iteration.
            _ = disk_tick.tick() => {
                // F4: skip the statfs entirely while the feature is off —
                // otherwise a disabled config still pays one measurement
                // per tick for nothing.
                if disk_thresholds.is_some() {
                    let mut n = |args: std::fmt::Arguments<'_>| {
                        diagnostic_narrate(args, Some(run_log));
                    };
                    disk_monitor.observe(workdir_actual.as_ref(), &mut n);
                }
                None
            }
        };
        let Some(join_result) = join_result else {
            continue;
        };
        let (completed_rule, status, record) = match join_result {
            // Reaped: drop the id→rule link so a later signal teardown
            // cannot record this rule Failed. (Only the panic branch
            // used to remove the entry — every success or rule-level
            // failure lingered here, so Ctrl-C marked ALL finished
            // rules Failed in the checkpoint.)
            Some(Ok((id, v))) => {
                task_rule.remove(&id);
                task_abort.remove(&id);
                v
            }
            Some(Err(e)) => {
                // Watchdog-initiated requeue (issue #872): a non-panic
                // cancellation is the abort issued by the watchdog's
                // escalation path, which already released the pool
                // reservation, requeued the rule to Pending, and cleared the
                // checkpoint running-set. All the engine-fault bookkeeping
                // below (fail_count, ck.mark_failed, failures.push) must be
                // skipped — otherwise the requeue would double-count as a
                // failure. The id→rule link is dropped here so the
                // "no recorded rule" invariant stays intact for real panics.
                // Invariant (review NIT 8): ONLY a non-panic cancellation
                // may take this early-continue. The loop is inline in
                // `run_command` and not unit-testable without a refactor,
                // so this branch is guarded by review, not by a test — a
                // regression here (dropping the `!e.is_panic()` guard, say)
                // would double-count every watchdog requeue as a rule
                // failure.
                if e.is_cancelled() && !e.is_panic() {
                    task_rule.remove(&e.id());
                    task_abort.remove(&e.id());
                    continue;
                }
                if e.is_panic() {
                    tracing::error!("Task panicked: {e}");
                }
                // A panicked task is an ENGINE fault, not a rule-level
                // best-effort failure — always counts as required (the run
                // fails), regardless of the rule's `required` flag.
                // Its pool registration (if it was waiting) must not hold
                // the FIFO line hostage — live waiters re-register.
                executor.clear_resource_waiters().await;
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // The task left the JoinSet without its payload, so the id
                // is the only link back to the rule (issue #136 fix 2).
                // Without marking it completed the rule would stay in the
                // scheduler's running set forever, permanently shrinking
                // the submit cap — with -j 1 every remaining rule would
                // silently never run. Fall through to the common failure
                // bookkeeping instead of `continue`, so dependents are
                // blocked, the checkpoint records the engine fault, and
                // the standard abort path fails the run loudly.
                let Some(rule_name) = task_rule.remove(&e.id()) else {
                    task_abort.remove(&e.id());
                    // Internal invariant: every spawned task registers its
                    // name at spawn. Fail the run rather than leak the cap.
                    // The engine fault counts as a failure in the summary
                    // (issue #142 H6).
                    json_summary.emit(
                        "failed",
                        &RunCounts {
                            succeeded: success_count.load(std::sync::atomic::Ordering::Relaxed),
                            skipped: skipped_count.load(std::sync::atomic::Ordering::Relaxed),
                            failed: fail_count.load(std::sync::atomic::Ordering::Relaxed),
                            non_required_failed: non_required_fail_count
                                .load(std::sync::atomic::Ordering::Relaxed),
                            blocked: blocked.lock().await.len(),
                        },
                        vec![],
                    );
                    return Err(anyhow::anyhow!(
                        "internal error: task {} has no recorded rule",
                        e.id()
                    ));
                };
                {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                let record = oxo_flow_core::executor::JobRecord {
                    signal: None,
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Failed,
                    enqueued_at: None,
                    started_at: None,
                    finished_at: None,
                    exit_code: Some(-1),
                    stdout: None,
                    stderr: Some(oxo_flow_core::executor::process::mask_sensitive(
                        &e.to_string(),
                        sensitive_values,
                    )),
                    command: None,
                    retries: 0,
                    skip_reason: None,
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: config.get_rule(&rule_name).and_then(|r| {
                        oxo_flow_core::executor::process::rule_report_caption(
                            r,
                            workdir_actual.as_ref(),
                        )
                    }),
                };
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&rule_name);
                if let Err(save_err) = ck.save_to_file_async(checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
                let mut f = failures.lock().await;
                f.push((rule_name.clone(), format!("task panicked: {e}")));
                (
                    rule_name,
                    oxo_flow_core::executor::JobStatus::Failed,
                    record,
                )
            }
            None => break,
        };

        sched.mark_completed(record);
        running_since.remove(&completed_rule);
        progress.inc(1);

        // ── Checkpoint re-entry processing (issue #78 P3) ───────────────────
        // A successful checkpoint rule may declare new samples in its manifest;
        // they merge into the plan and execute in this same run.
        if status == oxo_flow_core::executor::JobStatus::Success
            && let Some(cp_rule) = config.get_rule(&completed_rule).cloned()
            && cp_rule.checkpoint
            && let Err(e) = process_reentry(
                &mut *config,
                &cp_rule,
                workdir_actual.as_ref(),
                wildcard_values,
                &mut sched,
                &mut *order,
                &mut order_set,
                &mut *dag,
                checkpoint,
            )
            .await
        {
            tracing::error!(rule = %completed_rule, error = %e, "checkpoint re-entry failed");
            // Fail the checkpoint rule and propagate like any failure.
            // Build the record first so the checkpoint captures the same
            // diagnostics the report will show (issue #83 WS2).
            let record = oxo_flow_core::executor::JobRecord {
                signal: None,
                rule: completed_rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: Some(1),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: Some(format!("re-entry manifest: {e}")),
                max_rss_mb: None,
                cpu_seconds: None,
                caption: config.get_rule(&completed_rule).and_then(|r| {
                    oxo_flow_core::executor::process::rule_report_caption(
                        r,
                        workdir_actual.as_ref(),
                    )
                }),
            };
            {
                let mut frs = failed_rules_set.lock().await;
                frs.insert(completed_rule.clone());
            }
            {
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&completed_rule);
                if let Err(save_err) = ck.save_to_file_async(checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
            }
            {
                let mut f = failures.lock().await;
                let reason = if cp_rule.required {
                    format!("re-entry manifest: {e}")
                } else {
                    format!("re-entry manifest: {e} (non-required)")
                };
                f.push((completed_rule.clone(), reason));
            }
            if cp_rule.required {
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                non_required_fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            sched.mark_completed(record);
        }

        // ── Runtime-discovered output_pattern processing (issue #227 item 5) ─
        // A producer instance succeeded: scan its output_pattern domain,
        // persist it, and — once ALL of the producer's instances are
        // completed (the domain union is final) — instantiate the deferred
        // consumers and insert them into the remaining plan. A discovery
        // failure fails the producer like a re-entry failure: the run aborts
        // with attribution and the next run re-executes the producer.
        if status == oxo_flow_core::executor::JobStatus::Success
            && config
                .get_rule(&completed_rule)
                .is_some_and(|r| r.output_pattern.is_some())
            && let Err(e) = process_output_pattern_discovery(
                &mut *config,
                &completed_rule,
                workdir_actual.as_ref(),
                &mut sched,
                &mut *order,
                &mut order_set,
                &mut *dag,
                checkpoint,
                checkpoint_path,
            )
            .await
        {
            tracing::error!(rule = %completed_rule, error = %e, "output_pattern discovery failed");
            let record = oxo_flow_core::executor::JobRecord {
                signal: None,
                rule: completed_rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: Some(1),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: Some(format!("output_pattern discovery: {e}")),
                max_rss_mb: None,
                cpu_seconds: None,
                caption: config.get_rule(&completed_rule).and_then(|r| {
                    oxo_flow_core::executor::process::rule_report_caption(
                        r,
                        workdir_actual.as_ref(),
                    )
                }),
            };
            {
                let mut frs = failed_rules_set.lock().await;
                frs.insert(completed_rule.clone());
            }
            {
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&completed_rule);
                if let Err(save_err) = ck.save_to_file_async(checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
            }
            {
                let mut f = failures.lock().await;
                let reason = match config.get_rule(&completed_rule).map(|r| r.required) {
                    Some(false) => format!("output_pattern discovery: {e} (non-required)"),
                    _ => format!("output_pattern discovery: {e}"),
                };
                f.push((completed_rule.clone(), reason));
            }
            // Mirror the re-entry failure attribution above: honor the
            // rule's `required` flag so a non-required producer's discovery
            // hiccup doesn't fail the whole run.
            if config
                .get_rule(&completed_rule)
                .map(|r| r.required)
                .unwrap_or(true)
            {
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                non_required_fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            sched.mark_completed(record);
        }

        // Failure attribution for output_pattern producers (issue #227
        // item 5): a producer that failed leaves its deferred consumers
        // uninstantiated — name them so the failure is attributable, and
        // record them for the failure summary.
        if (status == oxo_flow_core::executor::JobStatus::Failed
            || status == oxo_flow_core::executor::JobStatus::TimedOut
            || status == oxo_flow_core::executor::JobStatus::Cancelled)
            && let Some(producer_template) = config.output_pattern_template_of(&completed_rule)
        {
            let consumers = config.pending_output_pattern_consumers_of(&producer_template);
            if !consumers.is_empty() {
                narrate_console_line(format_args!(
                    "{} {} not instantiated — output_pattern producer '{}' failed",
                    "Warning:".bold().yellow(),
                    consumers.join(", "),
                    producer_template
                ));
                let mut f = failures.lock().await;
                for c in consumers {
                    f.push((
                        c,
                        format!(
                            "not instantiated — output_pattern producer '{}' failed",
                            producer_template
                        ),
                    ));
                }
            }
        }

        // Skipped producers (issue #297 item 4): an outputs-up-to-date or
        // condition-false skip never reaches discovery, so the producer's
        // deferred consumers silently vanish from THIS run's plan. A
        // checkpointed domain from an earlier success is replayed on resume,
        // but a skip with no recorded domain leaves them uninstantiated for
        // good — name both sides so the disappearance is attributable. A
        // skip is not a failure, so unlike the block above this stays a
        // warning and does not enter the failure summary.
        if status == oxo_flow_core::executor::JobStatus::Skipped
            && let Some(producer_template) = config.output_pattern_template_of(&completed_rule)
        {
            let consumers = config.pending_output_pattern_consumers_of(&producer_template);
            if !consumers.is_empty() {
                narrate_console_line(format_args!(
                    "{} {} not instantiated — output_pattern producer '{}' was skipped",
                    "Warning:".bold().yellow(),
                    consumers.join(", "),
                    producer_template
                ));
                tracing::warn!(
                    producer = %producer_template,
                    consumers = ?consumers,
                    "skipped output_pattern producer leaves deferred consumers uninstantiated"
                );
            }
        }

        // Abort on first failure when not in keep_going mode.
        let fc = fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if fc > 0
            && !keep_going
            && (status == oxo_flow_core::executor::JobStatus::Failed
                || status == oxo_flow_core::executor::JobStatus::TimedOut
                || status == oxo_flow_core::executor::JobStatus::Cancelled)
        {
            progress.finish_and_clear();
            return handle_run_failure_abort(
                &completed_rule,
                status,
                AbortContext {
                    executor,
                    join_set: &mut join_set,
                    task_rule: &task_rule,
                    failed_rules_set: &failed_rules_set,
                    checkpoint,
                    checkpoint_path,
                    failures: &failures,
                    fail_count: &fail_count,
                    non_required_fail_count: &non_required_fail_count,
                    success_count: &success_count,
                    skipped_count: &skipped_count,
                    blocked: &blocked,
                    config,
                    workflow,
                    workdir_actual: workdir_actual.as_ref(),
                    tmp_dir_resolved,
                    run_log,
                    json_summary,
                    is_tty,
                    ai_recover,
                    ai_max_retries,
                },
            )
            .await;
        }

        // Propagate failure transitively: every rule that (transitively)
        // depends on a failed rule is marked as skipped/blocked.
        let nrf = non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if (fc > 0 && keep_going) || nrf > 0 {
            propagate_transitive_failures(TransitiveFailureContext {
                dag,
                rule_names: &rule_names,
                submitted: &mut submitted,
                failed_rules_set: &failed_rules_set,
                skipped_count: &skipped_count,
                blocked: &blocked,
                sched: &mut sched,
                config,
                workdir_actual: workdir_actual.as_ref(),
                is_tty,
                progress,
            })
            .await;
        }
    }

    // #540: the run loop is done — drop the receiver so a Ctrl-C landing
    // in the post-run work (report snapshot, webhook POST, AI report)
    // makes the watcher's `send` fail and its process-exit fallback
    // engage. With the receiver alive but never polled again, the late
    // signal was swallowed: nothing happened for potentially minutes.
    drop(signal_rx);

    let success_count = success_count.load(std::sync::atomic::Ordering::Relaxed);
    let fail_count = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let non_required_fail_count =
        non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let skipped_count = skipped_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut checkpoint_guard = checkpoint.lock().await;
    let failures = failures.lock().await;
    let blocked = blocked.lock().await;

    progress.finish_and_clear();

    handle_run_completion(CompletionContext {
        run_started,
        success_count,
        fail_count,
        non_required_fail_count,
        skipped_count,
        checkpoint: &mut checkpoint_guard,
        checkpoint_path,
        failures: &failures,
        blocked: &blocked,
        workflow,
        workdir_actual: workdir_actual.as_ref(),
        tmp_dir_resolved,
        run_log,
        no_report_snapshot,
        provenance,
        provenance_recorded: provenance_recorded.load(std::sync::atomic::Ordering::Relaxed),
        config,
        order,
        wildcard_values,
        disk_thresholds,
        dag,
        samples_filter,
        json_summary,
    })
    .await
}
