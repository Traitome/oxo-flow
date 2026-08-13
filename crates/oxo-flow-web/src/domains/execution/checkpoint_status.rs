//! Per-rule run status derived from the engine's own checkpoint state.
//!
//! The CLI owns execution and writes `.oxo-flow/checkpoint.json` after each
//! rule completes. This module reads that state directly — it is the single
//! source of truth for which rules completed or failed, with per-rule wall
//! time from the benchmark records. Currently-running rules are surfaced by
//! matching the CLI's "Running: <rule>" lines in execution.log (valid only
//! while the run is live). There is no web-side state to drift.

use std::collections::HashMap;
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
pub fn with_all_rules(items: Vec<NodeStatusItem>, all_rules: &[String]) -> Vec<NodeStatusItem> {
    let mut by_rule: HashMap<String, NodeStatusItem> =
        items.into_iter().map(|i| (i.rule.clone(), i)).collect();
    let mut out = Vec::with_capacity(all_rules.len());
    for rule in all_rules {
        out.push(by_rule.remove(rule).unwrap_or_else(|| NodeStatusItem {
            rule: rule.clone(),
            status: NodeStatus::Pending,
            started_at: None,
            duration_ms: None,
            exit_code: None,
            progress_pct: None,
        }));
    }
    out
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
