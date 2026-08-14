//! Per-rule run status derived from the engine's own checkpoint state.
//!
//! The CLI owns execution and writes `.oxo-flow/checkpoint.json` after each
//! rule completes. This module reads that state directly — it is the single
//! source of truth for which rules completed or failed, with per-rule wall
//! time from the benchmark records. Currently-running rules are surfaced by
//! matching the CLI's "Running: <rule>" lines in execution.log (valid only
//! while the run is live). There is no web-side state to drift.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxo_flow_core::executor::CheckpointState;

use super::types::{NodeStatus, NodeStatusItem};

/// Derive node statuses from the run's checkpoint file.
///
/// `is_running` gates the execution.log scan: a finished run must not report
/// anything as still running.
pub fn load_node_statuses(run_dir: &Path, is_running: bool) -> Vec<NodeStatusItem> {
    let checkpoint = CheckpointState::load_from_file(&CheckpointState::default_path(run_dir))
        .unwrap_or_else(|_| CheckpointState::new());

    let mut running: Vec<String> = Vec::new();
    if is_running && let Ok(log) = std::fs::read_to_string(run_dir.join("execution.log")) {
        for line in log.lines() {
            if let Some(rest) = line.strip_prefix("Running: ") {
                running.push(rest.trim().to_string());
            }
        }
    }

    let mut items: Vec<NodeStatusItem> = Vec::new();
    for rule in &checkpoint.completed_rules {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Success,
            started_at: None,
            duration_ms: checkpoint
                .benchmarks
                .get(rule)
                .map(|b| (b.wall_time_secs * 1000.0).round() as u64),
            exit_code: None,
            progress_pct: None,
        });
    }
    for rule in &checkpoint.failed_rules {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Failed,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
        });
    }
    for rule in &running {
        items.push(NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Running,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
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
    // run) are kept as their own rows rather than silently dropped.
    for (name, items) in groups {
        if !emitted.contains(&name) {
            out.push(aggregate_rule(&name, Some(items)));
        }
    }
    out
}

/// Combine one rule's instance statuses into a single rule-level status.
///
/// Priority: Failed > Running > Success (with at least one success) >
/// Skipped > Pending. Duration is the max across instances.
fn aggregate_rule(name: &str, items: Option<Vec<NodeStatusItem>>) -> NodeStatusItem {
    let Some(items) = items else {
        return NodeStatusItem {
            rule: name.to_string(),
            status: NodeStatus::Pending,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
        };
    };
    let status = if items.iter().any(|i| i.status == NodeStatus::Failed) {
        NodeStatus::Failed
    } else if items.iter().any(|i| i.status == NodeStatus::Running) {
        NodeStatus::Running
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
        let items = load_node_statuses(&dir, false);
        assert_eq!(items.len(), 2);
        let fastqc = items.iter().find(|i| i.rule == "fastqc").unwrap();
        assert!(matches!(fastqc.status, NodeStatus::Success));
        assert_eq!(fastqc.duration_ms, Some(1500));
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Failed));
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
        let items = load_node_statuses(&dir, true);
        let align = items.iter().find(|i| i.rule == "align").unwrap();
        assert!(matches!(align.status, NodeStatus::Running));
        // Without a live run, the same log must not claim anything is running.
        // Pending rules are expressed by absence — the caller merges with the
        // full rule list from the pipeline snapshot.
        let items = load_node_statuses(&dir, false);
        assert!(!items.iter().any(|i| i.rule == "align"));
    }

    #[test]
    fn missing_checkpoint_yields_empty() {
        let dir = std::env::temp_dir().join("cp-test-3");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(load_node_statuses(&dir, false).is_empty());
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
        let items = load_node_statuses(&dir, false);
        let all = with_all_rules(items, &["a".into(), "b".into(), "c".into()]);
        assert_eq!(all.len(), 3);
        let statuses: Vec<&NodeStatus> = all.iter().map(|i| &i.status).collect();
        assert!(matches!(statuses[0], NodeStatus::Success));
        assert!(matches!(statuses[1], NodeStatus::Pending));
        assert!(matches!(statuses[2], NodeStatus::Pending));
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
) -> HashMap<String, oxo_flow_core::executor::checkpoint::BenchmarkRecord> {
    CheckpointState::load_from_file(&CheckpointState::default_path(run_dir))
        .map(|ck| ck.benchmarks)
        .unwrap_or_default()
}
