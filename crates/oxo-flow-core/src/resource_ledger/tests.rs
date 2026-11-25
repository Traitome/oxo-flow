//! Unit tests for the normalized resource ledger.

use super::*;
use crate::executor::checkpoint::{BenchmarkRecord, CheckpointState, RuleRunRecord};
use chrono::{TimeZone, Utc};

fn benchmark(rule: &str, wall: f64) -> BenchmarkRecord {
    BenchmarkRecord {
        rule: rule.to_string(),
        wall_time_secs: wall,
        max_memory_mb: None,
        memory_limit_mb: None,
        cpu_seconds: None,
        retries: 0,
        recorded_as: None,
    }
}

/// A checkpoint with two measured rules: one local rule with every
/// optional source field, and one cluster rule with a plain benchmark.
fn checkpoint() -> CheckpointState {
    let mut cp = CheckpointState::new();
    cp.mark_completed(
        "align",
        BenchmarkRecord {
            max_memory_mb: Some(1024),
            cpu_seconds: Some(60.0),
            ..benchmark("align", 30.0)
        },
    );
    cp.mark_completed(
        "call",
        BenchmarkRecord {
            max_memory_mb: Some(512),
            cpu_seconds: Some(20.0),
            ..benchmark("call", 10.0)
        },
    );
    let started = Utc.with_ymd_and_hms(2026, 11, 25, 10, 0, 0).unwrap();
    let finished = started + chrono::Duration::seconds(40);
    cp.rule_runs.insert(
        "align".to_string(),
        RuleRunRecord {
            exit_code: Some(0),
            started_at: Some(started),
            finished_at: Some(started + chrono::Duration::seconds(30)),
            command: None,
            stderr_tail: None,
            stdout_tail: None,
            caption: None,
            status: Some("success".to_string()),
            skip_reason: None,
            signal: None,
        },
    );
    cp.rule_runs.insert(
        "call".to_string(),
        RuleRunRecord {
            exit_code: Some(0),
            started_at: Some(started + chrono::Duration::seconds(30)),
            finished_at: Some(finished),
            command: None,
            stderr_tail: None,
            stdout_tail: None,
            caption: None,
            status: Some("success".to_string()),
            skip_reason: None,
            signal: None,
        },
    );
    cp.resource_usage.insert(
        "align".to_string(),
        ResourceUsage {
            max_rss_bytes: Some(2 * 1024 * 1024 * 1024),
            cpu_seconds: Some(60.0),
            io_read_bytes: Some(4_000_000_000),
            io_write_bytes: Some(1_000_000),
            scheduler_wait_secs: Some(15),
            nodes: Some(1),
            cores: Some(8),
            source: Some(ResourceSource {
                backend: "slurm".to_string(),
                tool: "sacct".to_string(),
                version: None,
                note: None,
                raw_fields: BTreeMap::from([
                    ("MaxRSS".to_string(), "2G".to_string()),
                    ("TotalCPU".to_string(), "00:01:00".to_string()),
                ]),
            }),
        },
    );
    cp.resource_usage.insert(
        "call".to_string(),
        ResourceUsage {
            source: Some(ResourceSource {
                backend: "local".to_string(),
                tool: "sysinfo".to_string(),
                version: None,
                note: None,
                raw_fields: BTreeMap::from([("sample_interval_ms".to_string(), "200".to_string())]),
            }),
            ..ResourceUsage::default()
        },
    );
    cp
}

#[test]
fn ledger_names_are_canonical_and_rules_sorted() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    assert_eq!(ledger.schema, RESOURCE_LEDGER_SCHEMA);
    let names: Vec<&str> = ledger.rules.iter().map(|r| r.rule.as_str()).collect();
    assert_eq!(names, vec!["align", "call"]);
    assert_eq!(ledger.rules[0].status, "completed");
    assert_eq!(ledger.rules[0].max_rss_bytes, Some(2 * 1024 * 1024 * 1024));
    assert_eq!(ledger.rules[0].io_read_bytes, Some(4_000_000_000));
    assert_eq!(ledger.rules[0].scheduler_wait_secs, Some(15));
    assert_eq!(ledger.rules[0].nodes, Some(1));
    assert_eq!(ledger.rules[0].cores, Some(8));
    assert_eq!(ledger.rules[0].source.tool, "sacct");
}

#[test]
fn unknown_is_never_zero() {
    // A legacy checkpoint: only wall time and CPU were ever recorded.
    let json = r#"{
        "completed_rules": ["legacy"],
        "failed_rules": [],
        "benchmarks": {
            "legacy": {"rule": "legacy", "wall_time_secs": 5.0, "retries": 0}
        }
    }"#;
    let cp = CheckpointState::from_json(json).expect("legacy checkpoint must load");
    let ledger = build_resource_ledger(&cp, None, None, None);
    let rule = &ledger.rules[0];
    assert_eq!(rule.wall_time_secs, Some(5.0));
    assert_eq!(rule.cpu_seconds, None);
    assert_eq!(rule.max_rss_bytes, None);
    assert_eq!(rule.io_read_bytes, None);
    assert!(rule.source.note.as_deref().unwrap().contains("legacy"));

    let json = ledger.to_json().unwrap();
    // Unknown metrics are omitted, not emitted as fabricated zeroes.
    assert!(!json.contains("\"cpu_seconds\""));
    assert!(!json.contains("\"io_read_bytes\""));
    assert!(!json.contains("\"cpu_seconds\": 0"));
    assert!(!json.contains("\"max_rss_bytes\": 0"));
    // Totals do not claim completeness for a metric nobody reported.
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["run"]["totals"]["cpu_seconds_complete"], false);
    assert!(parsed["run"]["totals"].get("cpu_seconds").is_none());
}

#[test]
fn synthetic_records_are_not_measurements() {
    let mut cp = CheckpointState::new();
    cp.mark_completed(
        "up_to_date",
        BenchmarkRecord {
            recorded_as: Some("outputs up-to-date".to_string()),
            ..benchmark("up_to_date", 0.0)
        },
    );
    // Even a stale usage record from an earlier attempt must not be
    // presented as this run's cost.
    cp.resource_usage.insert(
        "up_to_date".to_string(),
        ResourceUsage {
            io_read_bytes: Some(999),
            source: Some(ResourceSource {
                backend: "local".to_string(),
                tool: "sysinfo".to_string(),
                ..ResourceSource::default()
            }),
            ..ResourceUsage::default()
        },
    );
    // A stale run record must not become a wall time either.
    let started = Utc.with_ymd_and_hms(2026, 11, 24, 10, 0, 0).unwrap();
    cp.rule_runs.insert(
        "up_to_date".to_string(),
        RuleRunRecord {
            exit_code: Some(0),
            started_at: Some(started),
            finished_at: Some(started + chrono::Duration::seconds(120)),
            command: None,
            stderr_tail: None,
            stdout_tail: None,
            caption: None,
            status: Some("success".to_string()),
            skip_reason: None,
            signal: None,
        },
    );
    let ledger = build_resource_ledger(&cp, None, None, None);
    let rule = &ledger.rules[0];
    assert_eq!(rule.wall_time_secs, None);
    assert_eq!(rule.io_read_bytes, None);
    assert!(ledger.run.sources.is_empty());
    assert_eq!(
        ledger.run.started_at, None,
        "stale attempt timestamps must not bound this run"
    );
    assert_eq!(rule.recorded_as.as_deref(), Some("outputs up-to-date"));
    assert!(
        rule.source
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("synthetic")
    );
    assert_eq!(
        ledger.run.totals.rules_executed, 0,
        "a synthetic up-to-date completion did not execute in this run"
    );
    assert_eq!(ledger.run.totals.wall_time_secs, None);
    assert!(!ledger.run.totals.wall_time_secs_complete);
}

#[test]
fn totals_sum_additive_metrics_and_peak_the_memory() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    let totals = &ledger.run.totals;
    assert_eq!(totals.rules_total, 2);
    assert_eq!(totals.rules_executed, 2);
    assert_eq!(totals.wall_time_secs, Some(40.0));
    assert!(totals.wall_time_secs_complete);
    assert_eq!(totals.cpu_seconds, Some(80.0));
    assert!(totals.cpu_seconds_complete);
    // Memory is a peak, not a sum: max(2048 MiB, 512 MiB) in bytes.
    assert_eq!(totals.max_rss_bytes, Some(2 * 1024 * 1024 * 1024));
    assert!(totals.max_rss_bytes_complete);
    // I/O was reported by one of two executed rules → partial sum, and
    // the flag says so.
    assert_eq!(totals.io_read_bytes, Some(4_000_000_000));
    assert!(!totals.io_read_bytes_complete);
    assert_eq!(totals.scheduler_wait_secs, Some(15));
    assert!(!totals.scheduler_wait_secs_complete);
}

#[test]
fn failed_rules_get_wall_time_from_run_records_only() {
    let mut cp = CheckpointState::new();
    let started = Utc.with_ymd_and_hms(2026, 11, 25, 10, 0, 0).unwrap();
    cp.mark_failed("broken");
    cp.rule_runs.insert(
        "broken".to_string(),
        RuleRunRecord {
            exit_code: Some(1),
            started_at: Some(started),
            finished_at: Some(started + chrono::Duration::milliseconds(1500)),
            command: None,
            stderr_tail: None,
            stdout_tail: None,
            caption: None,
            status: Some("failed".to_string()),
            skip_reason: None,
            signal: None,
        },
    );
    let ledger = build_resource_ledger(&cp, None, None, None);
    let rule = &ledger.rules[0];
    assert_eq!(rule.status, "failed");
    assert_eq!(rule.wall_time_secs, Some(1.5));
    assert_eq!(rule.cpu_seconds, None);
    assert_eq!(ledger.run.totals.wall_time_secs, Some(1.5));
}

#[test]
fn run_bounds_come_from_recorded_timestamps() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    assert_eq!(
        ledger.run.started_at.as_deref(),
        Some("2026-11-25T10:00:00Z")
    );
    assert_eq!(
        ledger.run.finished_at.as_deref(),
        Some("2026-11-25T10:00:40Z")
    );
}

#[test]
fn json_and_csv_bytes_are_deterministic() {
    let cp = checkpoint();
    let a = build_resource_ledger(&cp, None, None, None);
    let b = build_resource_ledger(&cp, None, None, None);
    assert_eq!(a.to_json().unwrap(), b.to_json().unwrap());
    assert_eq!(a.to_csv(), b.to_csv());
    // Re-serializing reconstructed state (an order-independent checkpoint)
    // produces identical bytes.
    let round_trip = CheckpointState::from_json(&cp.to_json().unwrap()).unwrap();
    let c = build_resource_ledger(&round_trip, None, None, None);
    assert_eq!(a.to_json().unwrap(), c.to_json().unwrap());
    assert_eq!(a.to_csv(), c.to_csv());
}

#[test]
fn csv_has_stable_header_rule_rows_and_a_total_row() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    let csv = ledger.to_csv();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], CSV_HEADER.join(","));
    assert_eq!(lines.len(), 4, "header + 2 rules + TOTAL");
    assert!(lines[1].starts_with("rule,align,completed,30.000,"));
    // Unknown cells are empty, never 0.
    let call_row: Vec<&str> = lines[2].split(',').collect();
    assert_eq!(call_row[6], "", "call reported no I/O read bytes");
    let total: Vec<&str> = lines[3].split(',').collect();
    assert_eq!(total[0], "total");
    assert_eq!(total[1], "TOTAL");
    assert_eq!(total[3], "40.000");
    assert_eq!(total[4], "80.000");
    assert_eq!(
        total[20],
        "io_read_bytes;io_write_bytes;scheduler_wait_secs"
    );
}

#[test]
fn csv_quotes_special_characters() {
    let mut cp = checkpoint();
    cp.resource_usage.get_mut("align").unwrap().source = Some(ResourceSource {
        backend: "slurm".to_string(),
        tool: "sacct".to_string(),
        version: None,
        note: Some("value, with \"quotes\"\nand a newline".to_string()),
        raw_fields: BTreeMap::new(),
    });
    let csv = build_resource_ledger(&cp, None, None, None).to_csv();
    assert!(csv.contains("\"value, with \"\"quotes\"\"\nand a newline\""));
}

fn raw_totals(wall: f64, cpu: f64, rss: u64) -> RawSchedulerTotals {
    RawSchedulerTotals {
        totals: LedgerTotals {
            rules_total: 2,
            rules_executed: 2,
            wall_time_secs: Some(wall),
            wall_time_secs_complete: true,
            cpu_seconds: Some(cpu),
            cpu_seconds_complete: true,
            max_rss_bytes: Some(rss),
            max_rss_bytes_complete: true,
            ..LedgerTotals::default()
        },
        source: ResourceSource {
            backend: "slurm".to_string(),
            tool: "sacct-csv".to_string(),
            version: None,
            note: Some("imported from accounting.csv".to_string()),
            raw_fields: BTreeMap::new(),
        },
        rules: vec!["align".to_string(), "call".to_string()],
    }
}

#[test]
fn agreeing_raw_totals_produce_no_conflict() {
    let ledger = build_resource_ledger(
        &checkpoint(),
        None,
        None,
        Some(&raw_totals(40.0, 80.0, 2 * 1024 * 1024 * 1024)),
    );
    assert!(ledger.run.conflicts.is_empty());
    assert_eq!(
        ledger.run.scheduler_totals.as_ref().unwrap().wall_time_secs,
        Some(40.0)
    );
    // Review I1: the raw side carries its own source, not just conflicts.
    let source = ledger
        .run
        .scheduler_source
        .as_ref()
        .expect("scheduler totals always name their source");
    assert_eq!(source.tool, "sacct-csv");
    // Review M3: an agreeing comparison is distinguishable from none.
    let reconciliation = ledger
        .run
        .reconciliation
        .as_ref()
        .expect("a supplied export records its outcome");
    assert_eq!(
        reconciliation.compared_metrics,
        vec!["wall_time_secs", "cpu_seconds", "max_rss_bytes"]
    );
    assert_eq!(reconciliation.covered_rules, vec!["align", "call"]);
    assert!(
        reconciliation.note.contains("no disagreement"),
        "{}",
        reconciliation.note
    );
}

#[test]
fn disagreeing_raw_totals_produce_named_conflicts() {
    let ledger = build_resource_ledger(
        &checkpoint(),
        None,
        None,
        // CPU and memory far off; wall time within tolerance (1 s).
        Some(&raw_totals(40.5, 160.0, 4 * 1024 * 1024 * 1024)),
    );
    let metrics: Vec<&str> = ledger
        .run
        .conflicts
        .iter()
        .map(|c| c.metric.as_str())
        .collect();
    assert_eq!(metrics, vec!["cpu_seconds", "max_rss_bytes"]);
    let cpu = &ledger.run.conflicts[0];
    assert_eq!(cpu.per_rule_total, 80.0);
    assert_eq!(cpu.scheduler_total, 160.0);
    assert_eq!(cpu.unit, "seconds");
    assert_eq!(cpu.scheduler_source.tool, "sacct-csv");
    assert!(!cpu.per_rule_sources.is_empty());
    let rss = &ledger.run.conflicts[1];
    assert_eq!(rss.unit, "bytes");
    let reconciliation = ledger.run.reconciliation.as_ref().unwrap();
    assert!(
        reconciliation.note.contains("2 of 3"),
        "{}",
        reconciliation.note
    );
}

#[test]
fn raw_export_missing_a_rule_is_not_a_conflict() {
    let mut raw = raw_totals(400.0, 800.0, 8 * 1024 * 1024 * 1024);
    raw.rules = vec!["align".to_string()];
    let ledger = build_resource_ledger(&checkpoint(), None, None, Some(&raw));
    assert!(
        ledger.run.conflicts.is_empty(),
        "a partial export is a coverage gap, not a numeric disagreement"
    );
    let reconciliation = ledger.run.reconciliation.as_ref().unwrap();
    assert!(reconciliation.compared_metrics.is_empty());
    assert!(
        reconciliation.note.contains("not compared"),
        "{}",
        reconciliation.note
    );
    assert_eq!(reconciliation.covered_rules, vec!["align"]);
    assert!(
        reconciliation
            .skipped_metrics
            .iter()
            .all(|m| m.contains("does not cover"))
    );
}

#[test]
fn incomplete_side_never_conflicts() {
    let mut cp = checkpoint();
    // Drop CPU from one rule: the per-rule sum is partial, so it cannot
    // be compared against a complete raw export.
    cp.benchmarks.get_mut("call").unwrap().cpu_seconds = None;
    let ledger = build_resource_ledger(
        &cp,
        None,
        None,
        Some(&raw_totals(40.0, 999.0, 2 * 1024 * 1024 * 1024)),
    );
    assert!(
        ledger
            .run
            .conflicts
            .iter()
            .all(|c| c.metric != "cpu_seconds")
    );
    let reconciliation = ledger.run.reconciliation.as_ref().unwrap();
    assert!(
        reconciliation
            .skipped_metrics
            .iter()
            .any(|m| m.starts_with("cpu_seconds:") && m.contains("incomplete")),
        "{:?}",
        reconciliation.skipped_metrics
    );
}

#[test]
fn latest_skip_marker_suppresses_previous_run_metrics() {
    // Review I4: the common `mark_completed_quiet` path preserves an older
    // benchmark while the skip marker records the latest attempt. The ledger
    // must not present the previous run's work as this run's.
    let mut cp = checkpoint();
    // The real behavior: a pre-skip or executor skip records the marker
    // while the last execution's rule_runs record — here "success" with
    // timestamps — stays intact.
    cp.record_skipped_attempt("align", "outputs up-to-date");
    let ledger = build_resource_ledger(&cp, None, None, None);
    let align = ledger
        .rules
        .iter()
        .find(|r| r.rule == "align")
        .expect("align recorded");
    assert_eq!(align.status, "skipped");
    assert_eq!(align.wall_time_secs, None);
    assert_eq!(align.cpu_seconds, None);
    assert_eq!(align.max_rss_bytes, None);
    assert_eq!(align.io_read_bytes, None);
    assert!(
        align
            .source
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("not executed in this run"),
        "{:?}",
        align.source
    );
    // Only the rule that actually ran is counted; totals are its alone.
    let totals = &ledger.run.totals;
    assert_eq!(totals.rules_total, 2);
    assert_eq!(totals.rules_executed, 1);
    assert_eq!(totals.wall_time_secs, Some(10.0));
    assert!(totals.wall_time_secs_complete);
    assert_eq!(totals.cpu_seconds, Some(20.0));
    // Run bounds ignore the skipped rule's stale timestamps.
    assert_eq!(
        ledger.run.started_at.as_deref(),
        Some("2026-11-25T10:00:30Z")
    );
    // The skipped rule is not a run-level data source.
    assert!(
        ledger.run.sources.iter().all(|s| s.tool != "sacct"),
        "{:?}",
        ledger.run.sources
    );
}

#[test]
fn failed_rule_keeps_scheduler_cpu_without_a_benchmark() {
    // Review I5/I6: a failed rule has no BenchmarkRecord, but a scheduler
    // that measured CPU must still contribute it.
    let mut cp = CheckpointState::new();
    cp.mark_failed("broken");
    cp.resource_usage.insert(
        "broken".to_string(),
        ResourceUsage {
            cpu_seconds: Some(7200.0),
            source: Some(ResourceSource {
                backend: "slurm".to_string(),
                tool: "sacct".to_string(),
                version: None,
                note: None,
                raw_fields: BTreeMap::from([("TotalCPU".to_string(), "02:00:00".to_string())]),
            }),
            ..ResourceUsage::default()
        },
    );
    let ledger = build_resource_ledger(&cp, None, None, None);
    let rule = &ledger.rules[0];
    assert_eq!(rule.status, "failed");
    assert_eq!(rule.cpu_seconds, Some(7200.0));
    assert_eq!(rule.wall_time_secs, None);
    assert_eq!(ledger.run.totals.cpu_seconds, Some(7200.0));
    assert!(ledger.run.totals.cpu_seconds_complete);
    assert_eq!(ledger.run.totals.rules_executed, 1);
}

#[test]
fn executed_rule_names_filter_skips_and_synthetic_records() {
    let mut cp = checkpoint();
    cp.benchmarks.insert(
        "up_to_date".to_string(),
        BenchmarkRecord {
            recorded_as: Some("outputs up-to-date".to_string()),
            ..benchmark("up_to_date", 0.0)
        },
    );
    cp.completed_rules.insert("up_to_date".to_string());
    // A rule whose latest attempt was skipped (marker path) is not executed.
    cp.completed_rules.insert("skipped".to_string());
    cp.record_skipped_attempt("skipped", "outputs up-to-date");
    let names = executed_rule_names(&cp);
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec!["align".to_string(), "call".to_string()]
    );
}

#[test]
fn no_raw_export_means_no_scheduler_section() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    assert!(ledger.run.scheduler_totals.is_none());
    assert!(ledger.run.scheduler_source.is_none());
    assert!(ledger.run.reconciliation.is_none());
    let json = ledger.to_json().unwrap();
    assert!(!json.contains("\"scheduler_totals\""));
    assert!(!json.contains("\"scheduler_source\""));
    assert!(!json.contains("\"reconciliation\""));
}

#[test]
fn cost_and_gpu_stay_absent_without_a_source() {
    let ledger = build_resource_ledger(&checkpoint(), None, None, None);
    let json = ledger.to_json().unwrap();
    assert!(!json.contains("\"cost\""));
    assert!(!json.contains("\"gpu_seconds\""));
    assert!(!json.contains("\"currency\""));
}
