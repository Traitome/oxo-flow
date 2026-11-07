//! Per-rule run status derived from the engine's own checkpoint state.
//!
//! The CLI owns execution and writes `.oxo-flow/checkpoint.json` after each
//! rule completes. This module reads that state directly — it is the single
//! source of truth for which rules completed or failed, with per-rule wall
//! time from the benchmark records. Currently-running rules are surfaced by
//! matching the CLI's `Running: <rule>` lines in execution.log (valid only
//! while the run is live). There is no web-side state to drift.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use oxo_flow_core::executor::CheckpointState;
use oxo_flow_core::executor::checkpoint::RuleRunRecord;

use super::types::{NodeStatus, NodeStatusItem};

/// Terminal timing from the persisted `rule_runs` record: an RFC 3339 UTC
/// start timestamp and the execution duration derived from the record's
/// start/end pair. Both stay `None` when the record is absent, predates the
/// timestamp fields (legacy checkpoint), or describes a rule that never
/// started; a field the record does carry is reported as-is.
fn recorded_timing(record: Option<&RuleRunRecord>) -> (Option<String>, Option<u64>) {
    let Some(record) = record else {
        return (None, None);
    };
    let started_at = record.started_at.map(|start| start.to_rfc3339());
    let duration_ms = record
        .started_at
        .zip(record.finished_at)
        .map(|(start, end)| end.signed_duration_since(start).num_milliseconds().max(0) as u64);
    (started_at, duration_ms)
}

/// Derive node statuses from the run's checkpoint file.
///
/// `run_status` is the run record's own terminal/runtime status and drives
/// two things: the execution.log scan is gated on `"running"` (a finished
/// run must not report anything as still running), and a cancelled run's
/// killed rules surface as [`NodeStatus::Cancelled`] rather than `Failed`
/// — the process-tree kill is collateral of the operator's cancel, not a
/// rule failure. A rule with a recorded exit code
/// keeps `Failed` even in a cancelled run: that is a genuine failure that
/// happened before the cancel.
pub fn load_node_statuses(run_dir: &Path, run_status: &str) -> Vec<NodeStatusItem> {
    let is_running = run_status == "running";
    let run_cancelled = run_status == "cancelled";
    let checkpoint = CheckpointState::load_from_file(&CheckpointState::default_path(run_dir))
        .unwrap_or_else(|_| CheckpointState::new());

    let mut running: Vec<String> = Vec::new();
    if is_running {
        if !checkpoint.running.is_empty() {
            // The checkpoint's persisted in-flight set (issue #685) is
            // authoritative — written at spawn, removed at every terminal
            // transition — so the hot polled path reads O(in-flight) state
            // and never touches the log (issue #734).
            running.extend(checkpoint.running.iter().cloned());
        } else {
            // Legacy checkpoint (or genuinely nothing in flight): fall
            // back to the run log, bounded — the old code buffered the
            // whole file on every status poll, and a rule-writable
            // execution.log must not hang the poll on a planted FIFO
            // (#695/#734). Rules already completed or failed are skipped,
            // so their stale `Running:` lines cannot resurrect as phantom
            // Running nodes.
            let log = read_tail_bounded(&run_dir.join("execution.log"), 1024 * 1024);
            for line in log.lines() {
                if let Some(rest) = line.strip_prefix("Running: ") {
                    let name = rest.trim();
                    if !checkpoint.completed_rules.contains(name)
                        && !checkpoint.failed_rules.contains(name)
                    {
                        running.push(name.to_string());
                    }
                }
            }
        }
    }

    let mut items: Vec<NodeStatusItem> = Vec::new();
    for rule in &checkpoint.completed_rules {
        let record = checkpoint.rule_runs.get(rule);
        let benchmark = checkpoint.benchmarks.get(rule);
        // Synthetic up-to-date records (issue #324 F-1) carry a 0.0
        // placeholder wall time — report neither a start nor a fake "0ms":
        // the rule did not execute in this run.
        let is_synthetic = benchmark.is_some_and(|b| b.recorded_as.is_some());
        let recorded_start = if is_synthetic {
            None
        } else {
            recorded_timing(record).0
        };
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Success,
            // Execution start from the same `rule_runs` record that
            // carries the exit code; paired with the benchmark duration
            // below so a current run's completed nodes report both
            // (issue #1040 P3-3).
            started_at: recorded_start,
            duration_ms: benchmark.and_then(|b| {
                if b.recorded_as.is_some() {
                    None
                } else {
                    Some((b.wall_time_secs * 1000.0).round() as u64)
                }
            }),
            // The persisted rule_runs record (issue #758): completed rules
            // exited 0; a recorded code (legacy or 0) wins.
            exit_code: record.and_then(|r| r.exit_code).or(Some(0)),
            progress_pct: None,
            stderr_tail: None,
            stdout_tail: None,
        });
    }
    for rule in &checkpoint.failed_rules {
        let record = checkpoint.rule_runs.get(rule);
        // Issue #690: a failed_rules entry whose recorded when-verdict is
        // false was gated off, not failed — typically a stale entry from an
        // earlier run under a different config. Surface it as Skipped so the
        // web view agrees with the CLI's dry-run tally.
        //
        // P-F4: in a cancelled run a rule with no recorded exit code was
        // killed by the cancel (the CLI may not reach its own
        // demote-to-cancelled reconciliation when the web server terminates
        // the whole process tree), so surface Cancelled. An exit code means
        // the rule failed on its own — keep Failed.
        let status = if checkpoint.when_verdicts.get(rule) == Some(&false) {
            NodeStatus::Skipped
        } else if run_cancelled && record.is_none_or(|r| r.exit_code.is_none()) {
            NodeStatus::Cancelled
        } else {
            NodeStatus::Failed
        };
        let is_failed = status == NodeStatus::Failed;
        // A failed (or cancellation-demoted) rule has no benchmark record —
        // derive both its start and its duration from the execution
        // record's timestamp pair (issue #1040 P3-3). Skipped rules never
        // executed; they keep null timing.
        let (started_at, duration_ms) = if is_failed || status == NodeStatus::Cancelled {
            recorded_timing(record)
        } else {
            (None, None)
        };
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status,
            started_at,
            duration_ms,
            // The persisted failure data the CLI narrates (issue #758) —
            // the web used to hardcode None here even though the frontend
            // already renders a non-null exit code.
            exit_code: if is_failed {
                record.and_then(|r| r.exit_code)
            } else {
                None
            },
            progress_pct: None,
            stderr_tail: if is_failed {
                record.and_then(|r| r.stderr_tail.clone())
            } else {
                None
            },
            stdout_tail: if is_failed {
                record.and_then(|r| r.stdout_tail.clone())
            } else {
                None
            },
        });
    }
    // When-skipped rules have no completed/failed entry at all — the
    // executor records only `when_verdicts[name] = false` (issue #739).
    // Without this loop they fell through to `Pending` forever in the web
    // status/DAG views (inflating pending_nodes and the ETA); same
    // reconciliation the CLI's resume banner got in #690.
    {
        let accounted: HashSet<&String> = checkpoint
            .completed_rules
            .iter()
            .chain(checkpoint.failed_rules.iter())
            .collect();
        let mut skipped: Vec<&String> = checkpoint
            .when_verdicts
            .iter()
            .filter(|(name, verdict)| {
                !**verdict && !accounted.contains(name) && !running.iter().any(|r| r == *name)
            })
            .map(|(name, _)| name)
            .collect();
        skipped.sort();
        for rule in skipped {
            items.push(NodeStatusItem {
                rule: rule.clone(),
                status: NodeStatus::Skipped,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
            });
        }
    }
    // Abort-killed sibling rules (issue #767): on a fail-fast abort the
    // CLI records each not-yet-finished sibling in `rule_runs` with
    // status "cancelled" (+ skip_reason) but in NONE of the completed/
    // failed/when sets — they fell through to Pending forever, inflating
    // pending_nodes and the ETA. Derive from rule_runs, not set
    // membership: any rule_runs-only terminal state surfaces as Skipped
    // (the #739 reconciliation, extended to the cancelled source).
    {
        let accounted: HashSet<&String> = checkpoint
            .completed_rules
            .iter()
            .chain(checkpoint.failed_rules.iter())
            .collect();
        let mut cancelled: Vec<(&String, &RuleRunRecord)> = checkpoint
            .rule_runs
            .iter()
            .filter(|(name, rec)| {
                // Modern records carry status "cancelled"; pre-#498 legacy
                // records only have skip_reason + no exit_code (the status
                // field was added later) — treat both as abort-cancelled.
                let cancelled = rec.status.as_deref() == Some("cancelled")
                    || (rec.status.is_none()
                        && rec.skip_reason.is_some()
                        && rec.exit_code.is_none());
                cancelled
                    && !accounted.contains(*name)
                    && checkpoint.when_verdicts.get(*name) != Some(&false)
                    && !running.iter().any(|r| r == *name)
            })
            .collect();
        cancelled.sort_by(|a, b| a.0.cmp(b.0));
        for (rule, record) in cancelled {
            // A run-level cancel killed these too — same terminal
            // vocabulary as the failed_rules branch above. In a FAILED
            // run they stay Skipped: abort collateral, not the failure.
            let status = if run_cancelled {
                NodeStatus::Cancelled
            } else {
                NodeStatus::Skipped
            };
            // A cancelled rule that was already executing has a recorded
            // start/end pair; surface it exactly like a failed rule. Skipped
            // abort collateral never reports execution timing.
            let (started_at, duration_ms) = if status == NodeStatus::Cancelled {
                recorded_timing(Some(record))
            } else {
                (None, None)
            };
            items.push(NodeStatusItem {
                rule: rule.clone(),
                status,
                started_at,
                duration_ms,
                exit_code: None,
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
            });
        }
    }
    for rule in &running {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Running,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
            stderr_tail: None,
            stdout_tail: None,
        });
    }
    items
}

/// Merge checkpoint-derived statuses with the full rule list, so rules that
/// have not run yet appear as `Pending` instead of being absent.
///
/// The checkpoint stores EXPANDED instance names (`qc_S1`, `qc_S2`) while the
/// pipeline snapshot's DAG holds base rule names (`qc`). Instance statuses
/// are aggregated onto their base rule (issue #79 P1-03: the old exact-string
/// match made every wildcard rule show `Pending` and the derived status stuck
/// at `queued` for the whole run). Longest base name wins, so `qc_fast_S1`
/// is attributed to `qc_fast`, never to a shorter `qc`; instances of rules
/// absent from the snapshot are surfaced under their own names.
pub fn with_all_rules(items: Vec<NodeStatusItem>, all_rules: &[String]) -> Vec<NodeStatusItem> {
    let mut groups: HashMap<String, Vec<NodeStatusItem>> = HashMap::new();
    for item in items {
        let base = all_rules
            .iter()
            .filter(|r| item.rule == **r || item.rule.starts_with(&format!("{r}_")))
            .max_by_key(|r| r.len())
            .cloned()
            .unwrap_or_else(|| item.rule.clone());
        groups.entry(base).or_default().push(item);
    }

    let mut out = Vec::with_capacity(all_rules.len());
    let mut emitted: HashSet<String> = HashSet::new();
    for rule in all_rules {
        emitted.insert(rule.clone());
        out.push(aggregate_rule(rule, groups.remove(rule)));
    }
    // Instances of rules missing from the snapshot (workflow edited since the
    // run) are kept as their own rows rather than silently dropped. Sorted
    // by name: `groups` is a HashMap whose iteration order is
    // process-random, and the response rows must not shuffle across
    // restarts (issue #471).
    let mut orphans: Vec<(String, Vec<NodeStatusItem>)> = groups.into_iter().collect();
    orphans.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, items) in orphans {
        if !emitted.contains(&name) {
            out.push(aggregate_rule(&name, Some(items)));
        }
    }
    out
}

/// Combine one rule's instance statuses into a single rule-level status.
///
/// Priority: Failed > Running > Cancelled > Success (with at least one
/// success) > Skipped > Pending. Cancelled outranks Success because a rule
/// whose instances were partly cancelled did not fully run; showing Success
/// would hide the interruption. Duration is the max across instances.
fn aggregate_rule(name: &str, items: Option<Vec<NodeStatusItem>>) -> NodeStatusItem {
    let Some(items) = items else {
        return NodeStatusItem {
            rule: name.to_string(),
            status: NodeStatus::Pending,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
            stderr_tail: None,
            stdout_tail: None,
        };
    };
    let status = if items.iter().any(|i| i.status == NodeStatus::Failed) {
        NodeStatus::Failed
    } else if items.iter().any(|i| i.status == NodeStatus::Running) {
        NodeStatus::Running
    } else if items.iter().any(|i| i.status == NodeStatus::Cancelled) {
        NodeStatus::Cancelled
    } else if items.iter().any(|i| i.status == NodeStatus::Success) {
        NodeStatus::Success
    } else if items.iter().all(|i| i.status == NodeStatus::Skipped) {
        NodeStatus::Skipped
    } else {
        NodeStatus::Pending
    };
    NodeStatusItem {
        rule: name.to_string(),
        status,
        started_at: items.iter().filter_map(|i| i.started_at.clone()).next(),
        duration_ms: items.iter().filter_map(|i| i.duration_ms).max(),
        exit_code: items.iter().find_map(|i| i.exit_code),
        progress_pct: items.iter().filter_map(|i| i.progress_pct).max(),
        stderr_tail: items.iter().find_map(|i| i.stderr_tail.clone()),
        stdout_tail: items.iter().find_map(|i| i.stdout_tail.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_checkpoint(dir: &std::path::Path, json: &str) {
        let dir = dir.join(".oxo-flow");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("checkpoint.json"), json).unwrap();
    }

    #[test]
    fn maps_completed_failed_and_pending_from_checkpoint() {
        let dir = std::env::temp_dir().join("cp-test-1");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc"],
                "failed_rules": ["align"],
                "benchmarks": {"fastqc": {"rule": "fastqc", "wall_time_secs": 1.5, "retries": 0}}
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        assert_eq!(items.len(), 2);
        let fastqc = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert!(matches!(fastqc.status, NodeStatus::Success));
        assert_eq!(fastqc.duration_ms, Some(1500));
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Failed));
    }

    #[test]
    fn terminal_nodes_report_started_at_and_duration() {
        // Issue #1040 P3-3: completed and failed nodes used to report
        // `duration_ms` without `started_at` (and failed nodes no duration at
        // all) because the checkpoint writer dropped the execution
        // timestamps. Both fields must come from the same `rule_runs`
        // record, and fields with no underlying data stay null together.
        let dir = std::env::temp_dir().join("cp-terminal-timing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc", "cached"],
                "failed_rules": ["align"],
                "running": ["sort"],
                "benchmarks": {
                    "fastqc": {"rule": "fastqc", "wall_time_secs": 1.5, "retries": 0},
                    "cached": {"rule": "cached", "wall_time_secs": 0.0, "retries": 0, "recorded_as": "outputs up-to-date"}
                },
                "rule_runs": {
                    "fastqc": {"exit_code": 0, "started_at": "2026-01-01T00:00:00Z", "finished_at": "2026-01-01T00:00:01.500Z"},
                    "cached": {"exit_code": 0, "started_at": "2025-12-31T00:00:00Z", "finished_at": "2025-12-31T00:00:01Z"},
                    "align": {"exit_code": 1, "started_at": "2026-01-01T00:01:00Z", "finished_at": "2026-01-01T00:01:02.500Z"}
                }
            }"#,
        );
        let items = load_node_statuses(&dir, "running");
        let fastqc = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert_eq!(
            fastqc.started_at.as_deref(),
            Some("2026-01-01T00:00:00+00:00"),
            "a completed node must carry its execution start"
        );
        assert_eq!(fastqc.duration_ms, Some(1500));
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert_eq!(
            align.started_at.as_deref(),
            Some("2026-01-01T00:01:00+00:00"),
            "a failed node must carry its execution start"
        );
        assert_eq!(
            align.duration_ms,
            Some(2500),
            "a failed node's duration derives from the recorded start/end pair"
        );
        // Synthetic "outputs up-to-date" completions did not execute in this
        // run: neither field may be reported, even when a stale record keeps
        // timestamps from an earlier execution.
        let cached = items.iter().find(|i| i.rule == "cached").unwrap();
        assert_eq!(cached.started_at, None);
        assert_eq!(cached.duration_ms, None);
        // A running rule has no recorded start/end yet; a pending rule has
        // no record at all. Both keep sensible nulls.
        let running = items.iter().find(|i| i.rule == "sort").unwrap();
        assert_eq!(running.started_at, None);
        assert_eq!(running.duration_ms, None);
        let all = with_all_rules(
            items,
            &[
                "fastqc".into(),
                "cached".into(),
                "align".into(),
                "sort".into(),
                "pending".into(),
            ],
        );
        let pending = all.iter().find(|i| i.rule == "pending").unwrap();
        assert_eq!(pending.started_at, None);
        assert_eq!(pending.duration_ms, None);
    }

    #[test]
    fn running_rules_come_from_execution_log() {
        let dir = std::env::temp_dir().join("cp-test-2");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{"completed_rules": ["fastqc"], "failed_rules": [], "benchmarks": {}}"#,
        );
        fs::write(
            dir.join("execution.log"),
            "Running: align\n✓ fastqc (0.1s)\n",
        )
        .unwrap();
        let items = load_node_statuses(&dir, "running");
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Running));
        // Without a live run, the same log must not claim anything is running.
        // Pending rules are expressed by absence — the caller merges with the
        // full rule list from the pipeline snapshot.
        let items = load_node_statuses(&dir, "completed");
        assert!(!items.iter().any(|i| i.rule == "align"));
    }

    #[test]
    fn rule_runs_exit_code_and_stderr_surface_on_failed_nodes() {
        // #758: the checkpoint persists rich failure data in rule_runs,
        // but the web hardcoded exit_code: None — the frontend already
        // renders a non-null exit code, and the AI assistant needs the
        // stderr tail to answer "why did my rule fail?".
        let dir = std::env::temp_dir().join("cp-rule-runs-exit");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc"],
                "failed_rules": ["trim"],
                "benchmarks": {},
                "rule_runs": {
                    "fastqc": {"exit_code": 0},
                    "trim": {
                        "exit_code": -1,
                        "stderr_tail": "output pattern contains unbound wildcard {sample}",
                        "stdout_tail": "root cause printed on stdout"
                    }
                }
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let trim = items.iter().find(|i| i.rule == "trim").unwrap();
        assert!(matches!(trim.status, NodeStatus::Failed));
        assert_eq!(trim.exit_code, Some(-1));
        assert_eq!(
            trim.stderr_tail.as_deref(),
            Some("output pattern contains unbound wildcard {sample}")
        );
        assert_eq!(
            trim.stdout_tail.as_deref(),
            Some("root cause printed on stdout"),
            "stdout tail must surface too — root causes live there for some tools (#765)"
        );
        let fastqc = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert_eq!(fastqc.exit_code, Some(0));
        assert!(fastqc.stderr_tail.is_none());
    }

    #[test]
    fn missing_checkpoint_yields_empty() {
        let dir = std::env::temp_dir().join("cp-test-3");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(load_node_statuses(&dir, "completed").is_empty());
    }

    #[test]
    fn read_tail_bounded_returns_whole_small_file() {
        let dir = std::env::temp_dir().join("cp-tail-small");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("execution.log"), "line one\nline two\n").unwrap();
        assert_eq!(
            read_tail_bounded(&dir.join("execution.log"), 256 * 1024),
            "line one\nline two\n"
        );
    }

    #[test]
    fn read_tail_bounded_reads_only_the_tail_window() {
        let dir = std::env::temp_dir().join("cp-tail-window");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let big = "x".repeat(4096);
        let mut log = String::new();
        for i in 0..100 {
            log.push_str(&format!("{big} line {i}\n"));
        }
        fs::write(dir.join("execution.log"), &log).unwrap();
        let tail = read_tail_bounded(&dir.join("execution.log"), 8192);
        // Bounded well under the file size…
        assert!(tail.len() <= 8192, "tail is {} bytes", tail.len());
        // …starts on a whole line (no torn prefix)…
        assert!(tail.starts_with("xxxx"));
        // …and carries the newest content.
        assert!(tail.contains("line 99"));
        assert!(!tail.contains("line 0\n"));
    }

    #[test]
    fn read_tail_bounded_missing_file_is_empty() {
        let dir = std::env::temp_dir().join("cp-tail-missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            read_tail_bounded(&dir.join("execution.log"), 256 * 1024),
            ""
        );
    }

    #[test]
    fn abort_killed_siblings_surface_as_skipped_not_pending() {
        // #767: the CLI records abort-killed siblings in rule_runs with
        // status "cancelled" but in none of the completed/failed/when
        // sets — the web showed them Pending forever (inflating
        // pending_nodes and the ETA).
        let dir = std::env::temp_dir().join("cp-cancelled-siblings");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc"],
                "failed_rules": ["trim"],
                "benchmarks": {},
                "rule_runs": {
                    "trim": {"exit_code": 1},
                    "align": {
                        "status": "cancelled",
                        "skip_reason": "run aborted before this rule finished — required rule 'trim' failed"
                    }
                }
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let align = items
            .iter()
            .find(|i| i.rule == "align")
            .expect("abort-killed sibling must appear");
        assert!(
            matches!(align.status, NodeStatus::Skipped),
            "abort-killed sibling must be Skipped, not absent/Pending: {:?}",
            align.status
        );
        let trim = items.iter().find(|i| i.rule == "trim").unwrap();
        assert!(matches!(trim.status, NodeStatus::Failed));
    }

    #[test]
    fn legacy_cancelled_entry_without_status_field_surfaces_as_skipped() {
        // Pre-#498 checkpoints record aborts as skip_reason + no exit_code
        // without the status field — the same Skipped surfacing applies.
        let dir = std::env::temp_dir().join("cp-cancelled-legacy");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": [],
                "failed_rules": ["trim"],
                "benchmarks": {},
                "rule_runs": {
                    "align": {
                        "skip_reason": "run aborted before this rule finished — required rule 'trim' failed"
                    }
                }
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(
            matches!(align.status, NodeStatus::Skipped),
            "{:?}",
            align.status
        );
    }

    #[test]
    fn when_skipped_rules_surface_as_skipped_not_pending() {
        // #739: the executor records only `when_verdicts[name] = false` for
        // a gated-off rule — no completed/failed entry. The web status and
        // DAG views must show Skipped, not a forever-Pending node that
        // inflates pending_nodes and the ETA.
        let dir = std::env::temp_dir().join("cp-when-skip");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["align"],
                "failed_rules": [],
                "benchmarks": {},
                "when_verdicts": {"qc": false}
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let qc = items
            .iter()
            .find(|i| i.rule == "qc")
            .expect("when-skipped rule must appear");
        assert!(matches!(qc.status, NodeStatus::Skipped), "{:?}", qc.status);
    }

    #[test]
    fn running_rules_prefer_checkpoint_set_over_log_scan() {
        // #734: the persisted in-flight set (#685) is authoritative — the
        // polled path must not buffer the log at all when it is present.
        // The stale `Running: align` line in the log must also lose to the
        // completed entry instead of resurrecting a phantom Running node.
        let dir = std::env::temp_dir().join("cp-running-prefer");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["align"],
                "failed_rules": [],
                "benchmarks": {},
                "running": ["samtools_sort"]
            }"#,
        );
        fs::write(
            dir.join("execution.log"),
            "Running: align\n✓ align\nRunning: samtools_sort\n",
        )
        .unwrap();
        let items = load_node_statuses(&dir, "running");
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(
            matches!(align.status, NodeStatus::Success),
            "stale log line must not resurrect a completed rule: {:?}",
            align.status
        );
        let sort = items.iter().find(|i| i.rule == "samtools_sort").unwrap();
        assert!(matches!(sort.status, NodeStatus::Running));
    }

    #[test]
    fn read_head_bounded_returns_whole_lines_from_the_head() {
        let dir = std::env::temp_dir().join("cp-head-bounded");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = "first line\nsecond line\nthird line\n";
        fs::write(dir.join("execution.log"), log).unwrap();
        // A window far larger than the file yields the whole content.
        assert_eq!(read_head_bounded(&dir.join("execution.log"), 4096), log);
        // A tiny window ends on a whole line, never a torn record.
        let head = read_head_bounded(&dir.join("execution.log"), 20);
        assert!(head.ends_with('\n'), "no torn line: {head:?}");
        assert!(head.starts_with("first line"));
        // Missing file → empty.
        assert_eq!(read_head_bounded(&dir.join("absent.log"), 4096), "");
    }

    #[test]
    fn when_gated_failed_entries_show_as_skipped() {
        // Issue #690: a stale failed_rules entry whose recorded when-verdict
        // is false was gated off, not failed — the web view must show
        // Skipped, matching the CLI's dry-run tally, not an alarming Failed.
        let dir = std::env::temp_dir().join("cp-test-690");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc"],
                "failed_rules": ["primers_map_primers", "align"],
                "when_verdicts": {"primers_map_primers": false},
                "benchmarks": {}
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let gated = items
            .iter()
            .find(|i| i.rule == "primers_map_primers")
            .unwrap();
        assert!(matches!(gated.status, NodeStatus::Skipped));
        let real = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(real.status, NodeStatus::Failed));
    }

    #[test]
    fn with_all_rules_fills_pending_entries() {
        let dir = std::env::temp_dir().join("cp-test-4");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{"completed_rules": ["a"], "failed_rules": [], "benchmarks": {}}"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let all = with_all_rules(items, &["a".into(), "b".into(), "c".into()]);
        assert_eq!(all.len(), 3);
        let statuses: Vec<&NodeStatus> = all.iter().map(|i| &i.status).collect();
        assert!(matches!(statuses[0], NodeStatus::Success));
        assert!(matches!(statuses[1], NodeStatus::Pending));
        assert!(matches!(statuses[2], NodeStatus::Pending));
    }

    #[test]
    fn stale_when_false_entries_render_skipped_not_failed() {
        // Issue #690: a checkpoint written by an older version keeps a
        // when-gated rule in failed_rules even though the gate evaluated
        // to false — it must surface as Skipped, never as ✗ Failed.
        let dir = std::env::temp_dir().join("cp-test-5");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": [],
                "failed_rules": ["align", "real_fail"],
                "when_verdicts": {"align": false},
                "benchmarks": {}
            }"#,
        );
        let items = load_node_statuses(&dir, "completed");
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Skipped));
        let real_fail = items.iter().find(|i| i.rule == "real_fail").unwrap();
        assert!(matches!(real_fail.status, NodeStatus::Failed));
    }

    /// Report finding P-F4: cancelling a run kills the in-flight rule. The
    /// checkpoint shows it in failed_rules with no exit code (the CLI never
    /// reached its own demote-to-cancelled reconciliation), and the web used
    /// to report it as `failed`. With the run record cancelled, the node is
    /// `cancelled` — the cancel is the cause, not a rule failure.
    #[test]
    fn cancelled_run_maps_killed_rule_to_cancelled_not_failed() {
        let dir = std::env::temp_dir().join("cp-run-cancelled-killed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": ["fastqc"],
                "failed_rules": ["sleep"],
                "benchmarks": {},
                "rule_runs": {"sleep": {}}
            }"#,
        );
        let items = load_node_statuses(&dir, "cancelled");
        let killed = items.iter().find(|i| i.rule == "sleep").unwrap();
        assert!(
            matches!(killed.status, NodeStatus::Cancelled),
            "killed rule must be Cancelled: {:?}",
            killed.status
        );
        assert_eq!(killed.exit_code, None);
        let done = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert!(matches!(done.status, NodeStatus::Success));
    }

    /// A genuine failure that happened BEFORE the cancel keeps its exit
    /// code and must stay `failed` — cancellation must not launder real
    /// failures (report finding P-F4).
    #[test]
    fn cancelled_run_keeps_genuine_failure_failed() {
        let dir = std::env::temp_dir().join("cp-run-cancelled-real-fail");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": [],
                "failed_rules": ["align"],
                "benchmarks": {},
                "rule_runs": {"align": {"exit_code": 1, "stderr_tail": "boom"}}
            }"#,
        );
        let items = load_node_statuses(&dir, "cancelled");
        let failed = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(
            matches!(failed.status, NodeStatus::Failed),
            "a recorded exit code is a real failure: {:?}",
            failed.status
        );
        assert_eq!(failed.exit_code, Some(1));
        assert_eq!(failed.stderr_tail.as_deref(), Some("boom"));
    }

    /// The CLI's own abort-cancelled records (status "cancelled", not in
    /// failed_rules) follow the same run-level vocabulary: Cancelled when
    /// the run was cancelled, Skipped when it failed.
    #[test]
    fn cancelled_run_maps_rule_runs_cancelled_entries_to_cancelled() {
        let dir = std::env::temp_dir().join("cp-run-cancelled-siblings");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_checkpoint(
            &dir,
            r#"{
                "completed_rules": [],
                "failed_rules": [],
                "benchmarks": {},
                "rule_runs": {
                    "align": {
                        "status": "cancelled",
                        "skip_reason": "run cancelled by operator"
                    }
                }
            }"#,
        );
        let cancelled = load_node_statuses(&dir, "cancelled");
        let align = cancelled.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Cancelled));
        // Same checkpoint on a FAILED run: abort collateral stays Skipped,
        // so the failed-run view does not grow a cancelled vocabulary.
        let failed = load_node_statuses(&dir, "failed");
        let align = failed.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Skipped));
    }
}

#[cfg(test)]
mod aggregation_tests {
    use super::*;

    fn item(rule: &str, status: NodeStatus) -> NodeStatusItem {
        NodeStatusItem {
            rule: rule.into(),
            status,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
            stderr_tail: None,
            stdout_tail: None,
        }
    }

    #[test]
    fn wildcard_instances_aggregate_onto_base_rule() {
        // The exact scenario of issue #79 P1-03: checkpoint holds expanded
        // instance names, the snapshot DAG holds the base name.
        let items = vec![
            item("qc_S1", NodeStatus::Success),
            item("qc_S2", NodeStatus::Success),
        ];
        let all = with_all_rules(items, &["qc".into(), "report".into()]);
        assert_eq!(all.len(), 2);
        let qc = all.iter().find(|i| i.rule == "qc").unwrap();
        assert_eq!(qc.status, NodeStatus::Success);
        let report = all.iter().find(|i| i.rule == "report").unwrap();
        assert_eq!(report.status, NodeStatus::Pending);
    }

    #[test]
    fn partial_instance_failure_marks_rule_failed() {
        let items = vec![
            item("qc_S1", NodeStatus::Success),
            item("qc_S2", NodeStatus::Failed),
        ];
        let all = with_all_rules(items, &["qc".into()]);
        assert_eq!(all[0].status, NodeStatus::Failed);
    }

    #[test]
    fn running_instance_marks_rule_running() {
        let items = vec![
            item("qc_S1", NodeStatus::Success),
            item("qc_S2", NodeStatus::Running),
        ];
        let all = with_all_rules(items, &["qc".into()]);
        assert_eq!(all[0].status, NodeStatus::Running);
    }

    #[test]
    fn cancelled_instance_outranks_success_in_aggregation() {
        // A partly-run wildcard rule was interrupted: showing Success for
        // the base rule would hide the cancellation of its other instances.
        let items = vec![
            item("qc_S1", NodeStatus::Success),
            item("qc_S2", NodeStatus::Cancelled),
        ];
        let all = with_all_rules(items, &["qc".into()]);
        assert_eq!(all[0].status, NodeStatus::Cancelled);
        // A genuine failure still outranks the cancellation.
        let items = vec![
            item("qc_S1", NodeStatus::Failed),
            item("qc_S2", NodeStatus::Cancelled),
        ];
        let all = with_all_rules(items, &["qc".into()]);
        assert_eq!(all[0].status, NodeStatus::Failed);
    }

    #[test]
    fn longest_base_rule_wins_attribution() {
        // `qc_fast_S1` must belong to `qc_fast`, never to `qc`.
        let items = vec![item("qc_fast_S1", NodeStatus::Success)];
        let all = with_all_rules(items, &["qc".into(), "qc_fast".into()]);
        let qc = all.iter().find(|i| i.rule == "qc").unwrap();
        assert_eq!(qc.status, NodeStatus::Pending);
        let fast = all.iter().find(|i| i.rule == "qc_fast").unwrap();
        assert_eq!(fast.status, NodeStatus::Success);
    }

    #[test]
    fn unknown_instances_are_kept_not_dropped() {
        // Rule removed from the workflow after the run — its checkpoint rows
        // must not silently disappear.
        let items = vec![item("deleted_rule_S1", NodeStatus::Success)];
        let all = with_all_rules(items, &["kept".into()]);
        assert!(all.iter().any(|i| i.rule == "deleted_rule_S1"));
        assert!(all.iter().any(|i| i.rule == "kept"));
    }
}

/// Benchmark records from the run's checkpoint (empty when unavailable) —
/// the `actual` side of resource-bottleneck detection (issue #67 §4).
pub fn load_benchmarks(
    run_dir: &Path,
) -> BTreeMap<String, oxo_flow_core::executor::checkpoint::BenchmarkRecord> {
    CheckpointState::load_from_file(&CheckpointState::default_path(run_dir))
        .map(|ck| ck.benchmarks)
        .unwrap_or_default()
}

/// Load the run's sampled resource telemetry from `workdir/metrics.jsonl`
/// (written by the executor's per-run sampler, issue #82 P1-2). Returns
/// JSON samples in chronological order: `{ts, memory_mb, cpu_pct,
/// processes}`. An absent or unreadable file yields an empty list — the
/// monitor then shows "no telemetry" instead of fabricated numbers.
///
/// Returns only the newest `max_bytes` of the telemetry file. Telemetry
/// grows once per sample for the whole run lifetime and status endpoints
/// re-read it on every poll, so the read is bounded. When truncated, the
/// leading partial line is skipped and only whole lines within the window
/// are returned (chronological order preserved).
pub fn load_metrics_bounded(run_dir: &Path, max_bytes: u64) -> Vec<serde_json::Value> {
    read_tail_bounded(&run_dir.join("metrics.jsonl"), max_bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

/// Read at most the first `max_bytes` of a file (issue #734): the head
/// window is where run-start artifacts live — the dry-run preview JSON and
/// the invalidation summary print before any rule executes, so a tail
/// window would miss them in a long log. The window ends on a whole line.
/// A missing, unreadable, or non-regular file yields an empty string.
pub fn read_head_bounded(path: &Path, max_bytes: u64) -> String {
    if !oxo_flow_core::result::is_regular_file(path) {
        return String::new();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut content = String::new();
    {
        use std::io::Read;
        if std::io::BufReader::new(file)
            .take(max_bytes)
            .read_to_string(&mut content)
            .is_err()
        {
            return String::new();
        }
    }
    // End on a whole line so consumers never see a torn final record.
    match content.rfind('\n') {
        Some(i) => content[..=i].to_owned(),
        None => content,
    }
}

/// Read at most the newest `max_bytes` of a file, seeking to the tail
/// first so a multi-gigabyte log is never buffered whole (issue #710).
/// When the window starts mid-line the torn first line is dropped, so
/// consumers only ever see whole lines. A missing, unreadable, or
/// non-regular file (FIFO/socket/device — `File::open` would block on a
/// writer-less FIFO, #695 family) yields an empty string.
pub fn read_tail_bounded(path: &Path, max_bytes: u64) -> String {
    // Stat first: `exists()` never blocks but does not exclude special
    // files, and opening those can hang the request.
    if !oxo_flow_core::result::is_regular_file(path) {
        return String::new();
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let skip = file_len.saturating_sub(max_bytes);
    // Seek near the end first so we never read the whole file into memory.
    if skip > 0 && std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(skip)).is_err() {
        return String::new();
    }
    let mut content = String::new();
    {
        use std::io::Read;
        if std::io::BufReader::new(file)
            .take(max_bytes)
            .read_to_string(&mut content)
            .is_err()
        {
            return String::new();
        }
    }
    // Drop the first line when we skipped into the middle of one.
    let start = if skip > 0 {
        content.find('\n').map(|i| i + 1).unwrap_or(content.len())
    } else {
        0
    };
    content[start..].to_owned()
}
