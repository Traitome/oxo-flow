//! Pure execution domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

use super::diagnostics::DiagnosticsEngine;
use super::types::*;

/// Create a run from pipeline TOML. Returns execution plan with resource estimates.
pub fn create_run(
    pipeline_toml: &str,
    config: &RunConfig,
    _pipeline_id: Option<&str>,
) -> Result<CreateRunResponse, String> {
    let wf = WorkflowConfig::parse(pipeline_toml).map_err(|e| format!("Parse: {e}"))?;
    let dag = WorkflowDag::from_rules(&wf.rules).map_err(|e| format!("DAG: {e}"))?;
    let execution_order = dag.execution_order().map_err(|e| format!("Order: {e}"))?;
    let parallel_groups = dag.parallel_groups().unwrap_or_default();

    // Estimate memory from rules
    let max_memory: u64 = wf
        .rules
        .iter()
        .filter_map(|r| r.effective_memory())
        .filter_map(|m| {
            m.replace("GB", "")
                .replace("G", "")
                .replace("MB", "")
                .replace("M", "")
                .trim()
                .parse::<f64>()
                .ok()
        })
        .fold(0.0_f64, |a, b| a.max(b)) as u64;
    let memory_mb = if max_memory > 1000 {
        max_memory
    } else {
        max_memory * 1024
    };

    // Rough duration estimate: 5 min per rule with parallel execution
    let max_jobs = config.max_jobs.unwrap_or(4).max(1) as u64;
    let estimated_secs = execution_order.len() as u64 * 300 / max_jobs;

    Ok(CreateRunResponse {
        run_id: uuid::Uuid::new_v4().to_string(),
        status: "queued".into(),
        estimated_resources: EstimatedResources {
            max_memory_mb: memory_mb.max(1024),
            max_threads: config.max_jobs.unwrap_or(4) as u32,
            estimated_duration_secs: estimated_secs.max(60),
        },
        execution_plan: ExecutionPlan {
            total_rules: execution_order.len(),
            parallel_groups,
            execution_order,
        },
    })
}

/// Compute overall run status from node statuses.
///
/// `db_status` is the executor-written status column. It is the terminal
/// truth — the executor attributes it from the CLI's exit code — so terminal
/// values override any node-level derivation (the checkpoint may lag the
/// final write). For live runs whose nodes have nothing to say yet (no log
/// lines, no checkpoint entries), the DB's running/paused must win over a
/// derived `Queued` — the issue #79 P1-03 "stuck at queued while running"
/// complaint.
pub fn compute_overall_status(nodes: &[NodeStatusItem], db_status: Option<&str>) -> RunStatus {
    let derived = if nodes.iter().any(|n| n.status == NodeStatus::Failed) {
        RunStatus::Failed
    } else if !nodes.is_empty()
        && nodes
            .iter()
            .all(|n| n.status == NodeStatus::Success || n.status == NodeStatus::Skipped)
    {
        RunStatus::Completed
    } else if nodes.iter().any(|n| n.status == NodeStatus::Running) {
        RunStatus::Running
    } else {
        RunStatus::Queued
    };

    match db_status {
        Some("completed") => RunStatus::Completed,
        Some("failed") => RunStatus::Failed,
        Some("cancelled") => RunStatus::Cancelled,
        Some("paused") => RunStatus::Paused,
        Some("running") if derived == RunStatus::Queued => RunStatus::Running,
        _ => derived,
    }
}

/// Compute retry plan: which rules to rerun and which to skip.
/// Reruns all failed nodes + their downstream dependents.
pub fn compute_retry_plan(
    run_nodes: &[NodeStatusItem],
    dag: &WorkflowDag,
    _from_rule: Option<&str>,
    skip_succeeded: bool,
) -> Result<RetryResponse, String> {
    let mut will_rerun: Vec<String> = run_nodes
        .iter()
        .filter(|n| n.status == NodeStatus::Failed)
        .map(|n| n.rule.clone())
        .collect();

    // Add all downstream dependents of failed nodes
    let failed_clone = will_rerun.clone();
    for failed in &failed_clone {
        if let Ok(dependents) = dag.dependents(failed) {
            for dep in dependents {
                if !will_rerun.contains(&dep) {
                    will_rerun.push(dep);
                }
            }
        }
    }

    let will_skip: Vec<String> = if skip_succeeded {
        run_nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Success && !will_rerun.contains(&n.rule))
            .map(|n| n.rule.clone())
            .collect()
    } else {
        vec![]
    };

    Ok(RetryResponse {
        new_run_id: uuid::Uuid::new_v4().to_string(),
        will_rerun,
        will_skip,
    })
}

/// Memory-pressure threshold: a rule whose sampled peak RSS reached this
/// fraction of its declared limit counts as a resource bottleneck
/// (issue #67 §4). Sampled peaks underestimate true maxima, so the
/// threshold is conservative.
const MEMORY_BOTTLENECK_THRESHOLD_PCT: u64 = 80;

/// Diagnose a failed run using the deterministic diagnostics engine.
///
/// `benchmarks` carries the engine's per-rule measurements (sampled peak
/// RSS + declared limit) from the run's checkpoint — the source for the
/// resource-bottleneck list.
pub fn diagnose_run(
    run_nodes: &[NodeStatusItem],
    log_output: &str,
    benchmarks: &std::collections::HashMap<
        String,
        oxo_flow_core::executor::checkpoint::BenchmarkRecord,
    >,
) -> DiagnosticsResponse {
    let engine = DiagnosticsEngine::new();

    // Sampled peak RSS at ≥80% of the declared memory limit = sustained
    // memory pressure worth surfacing.
    let resource_bottlenecks: Vec<ResourceBottleneck> = benchmarks
        .iter()
        .filter_map(|(rule, b)| {
            let actual = b.max_memory_mb?;
            let limit = b.memory_limit_mb?;
            if limit == 0 || actual * 100 < limit * MEMORY_BOTTLENECK_THRESHOLD_PCT {
                return None;
            }
            Some(ResourceBottleneck {
                rule: rule.clone(),
                metric: "max_memory_mb".into(),
                actual: actual as f64,
                limit: limit as f64,
            })
        })
        .collect();

    // If there are no nodes at all, the run failed before execution (e.g. parsing/validation).
    if run_nodes.is_empty() && !log_output.is_empty() {
        let lines: Vec<&str> = log_output.lines().rev().take(5).collect();
        return DiagnosticsResponse {
            failed_nodes: vec![FailedNode {
                rule: "workflow".into(),
                error_pattern: Some("pre_execution_failure".into()),
                likely_cause:
                    "Pipeline failed before execution (parsing, validation, or preparation error)."
                        .into(),
                suggestions: vec![
                    "Check the pipeline TOML for syntax errors.".into(),
                    "Ensure all referenced tools are available in the environment.".into(),
                    "Review the pipeline configuration for missing required fields.".into(),
                ],
                auto_fixable: false,
                fix_action: None,
                relevant_log_lines: lines.into_iter().map(|s| s.to_string()).collect(),
            }],
            warnings: vec![],
            resource_bottlenecks,
        };
    }

    let failed_nodes: Vec<FailedNode> = run_nodes
        .iter()
        .filter(|n| n.status == NodeStatus::Failed)
        .flat_map(|n| {
            let results = engine.analyze(&n.rule, log_output, n.exit_code);
            results
                .into_iter()
                .map(|r| FailedNode {
                    rule: r.rule,
                    error_pattern: r.error_pattern,
                    likely_cause: r.likely_cause,
                    suggestions: r.suggestions,
                    auto_fixable: r.auto_fixable,
                    fix_action: r.fix_action.map(|fa| FixAction {
                        description: fa.description,
                        config_change: fa.config_change.map(|cc| ConfigChange {
                            path: cc.path,
                            old_value: cc.old_value,
                            new_value: cc.new_value,
                        }),
                        command: fa.command,
                    }),
                    relevant_log_lines: r.relevant_log_lines,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let warnings: Vec<DiagnosticWarning> = run_nodes
        .iter()
        .filter(|n| n.status == NodeStatus::Skipped)
        .map(|n| DiagnosticWarning {
            rule: n.rule.clone(),
            pattern: "skipped".into(),
            suggestion: "This rule was skipped due to upstream failure.".into(),
        })
        .collect();

    DiagnosticsResponse {
        failed_nodes,
        warnings,
        resource_bottlenecks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_run() {
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"
[[rules]]
name = "hello"
shell = "echo hi"
output = ["hi.txt"]
"#;
        let config = RunConfig {
            max_jobs: Some(2),
            dry_run: None,
            keep_going: None,
            resource_budget: None,
        };
        let resp = create_run(toml, &config, None).unwrap();
        assert_eq!(resp.execution_plan.total_rules, 1);
        assert_eq!(resp.estimated_resources.max_threads, 2);
    }

    #[test]
    fn test_compute_retry() {
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"
[[rules]]
name = "step1"
shell = "echo 1"
output = ["a.txt"]
[[rules]]
name = "step2"
shell = "echo 2"
input = ["a.txt"]
output = ["b.txt"]
"#;
        let wf = WorkflowConfig::parse(toml).unwrap();
        let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
        let nodes = vec![
            NodeStatusItem {
                rule: "step1".into(),
                status: NodeStatus::Failed,
                started_at: None,
                duration_ms: None,
                exit_code: Some(1),
                progress_pct: None,
            },
            NodeStatusItem {
                rule: "step2".into(),
                status: NodeStatus::Pending,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
            },
        ];
        let plan = compute_retry_plan(&nodes, &dag, None, true).unwrap();
        assert!(plan.will_rerun.contains(&"step1".to_string()));
        assert!(plan.will_rerun.contains(&"step2".to_string()));
    }

    #[test]
    fn test_compute_overall_status_all_success() {
        let nodes = vec![NodeStatusItem {
            rule: "r1".into(),
            status: NodeStatus::Success,
            started_at: None,
            duration_ms: None,
            exit_code: Some(0),
            progress_pct: None,
        }];
        assert_eq!(compute_overall_status(&nodes, None), RunStatus::Completed);
    }

    #[test]
    fn test_compute_overall_status_one_failed() {
        let nodes = vec![
            NodeStatusItem {
                rule: "r1".into(),
                status: NodeStatus::Success,
                started_at: None,
                duration_ms: None,
                exit_code: Some(0),
                progress_pct: None,
            },
            NodeStatusItem {
                rule: "r2".into(),
                status: NodeStatus::Failed,
                started_at: None,
                duration_ms: None,
                exit_code: Some(1),
                progress_pct: None,
            },
        ];
        assert_eq!(compute_overall_status(&nodes, None), RunStatus::Failed);
    }

    #[test]
    fn test_compute_overall_status_running() {
        let nodes = vec![
            NodeStatusItem {
                rule: "r1".into(),
                status: NodeStatus::Success,
                started_at: None,
                duration_ms: None,
                exit_code: Some(0),
                progress_pct: None,
            },
            NodeStatusItem {
                rule: "r2".into(),
                status: NodeStatus::Running,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
            },
        ];
        assert_eq!(compute_overall_status(&nodes, None), RunStatus::Running);
    }

    #[test]
    fn test_diagnose_run() {
        let nodes = vec![
            NodeStatusItem {
                rule: "oom_rule".into(),
                status: NodeStatus::Failed,
                started_at: None,
                duration_ms: None,
                exit_code: Some(137),
                progress_pct: None,
            },
            NodeStatusItem {
                rule: "skipped_rule".into(),
                status: NodeStatus::Skipped,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
            },
        ];
        let log = "FATAL: out of memory\nprocess killed";
        let resp = diagnose_run(&nodes, log, &Default::default());
        assert_eq!(resp.failed_nodes.len(), 1);
        assert_eq!(
            resp.failed_nodes[0].error_pattern.as_deref(),
            Some("oom_killed")
        );
        assert_eq!(resp.warnings.len(), 1);
        assert_eq!(resp.warnings[0].pattern, "skipped");
    }
}

/// One entry in a recursive workdir listing.
#[derive(Debug, Clone)]
pub struct FileListingEntry {
    pub name: String,
    pub path: String,
    pub size_bytes: i64,
    pub is_dir: bool,
}

/// Recursive workdir listing for result browsers and reports (issue #79
/// P1-08: the old top-level-only `read_dir` hid real products nested in
/// output directories — peaks.bed, sam/bam, taxa.tsv were invisible).
///
/// The engine's internal `.oxo-flow` directory is skipped. Depth and count
/// caps keep pathological workdirs (thousands of chunk files) from bloating
/// responses.
pub fn list_files_recursive(root: &std::path::Path) -> Vec<FileListingEntry> {
    const MAX_DEPTH: usize = 4;
    const MAX_ENTRIES: usize = 500;

    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<FileListingEntry>) {
        if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<std::fs::DirEntry> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if out.len() >= MAX_ENTRIES {
                return;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if depth == 0 && name == ".oxo-flow" {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = path.metadata() else {
                continue;
            };
            let is_dir = meta.is_dir();
            out.push(FileListingEntry {
                name,
                path: path.to_string_lossy().to_string(),
                size_bytes: if is_dir { 0 } else { meta.len() as i64 },
                is_dir,
            });
            if is_dir {
                walk(&path, depth + 1, out);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

#[cfg(test)]
mod file_listing_tests {
    use super::*;

    #[test]
    fn recursive_listing_reaches_nested_outputs_and_skips_internals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("results/sample1")).unwrap();
        std::fs::create_dir_all(dir.path().join(".oxo-flow")).unwrap();
        std::fs::write(dir.path().join("top.txt"), "a").unwrap();
        std::fs::write(dir.path().join("results/sample1/peaks.bed"), "b").unwrap();
        std::fs::write(dir.path().join(".oxo-flow/checkpoint.json"), "{}").unwrap();

        let files = list_files_recursive(dir.path());
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"top.txt"));
        assert!(names.contains(&"results"));
        assert!(
            names.contains(&"peaks.bed"),
            "nested product must be listed: {names:?}"
        );
        assert!(
            !files.iter().any(|f| f.path.contains(".oxo-flow")),
            "engine internals must be excluded: {files:?}"
        );
        let bed = files.iter().find(|f| f.name == "peaks.bed").unwrap();
        assert_eq!(bed.size_bytes, 1);
        assert!(!bed.is_dir);
    }

    #[test]
    fn caps_bound_listing_size() {
        let dir = tempfile::tempdir().unwrap();
        // 600 files at depth 1 — the cap must hold.
        for i in 0..600 {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let files = list_files_recursive(dir.path());
        assert!(files.len() <= 500);
    }

    #[test]
    fn diagnose_flags_memory_bottleneck_at_or_above_80pct() {
        use oxo_flow_core::executor::checkpoint::BenchmarkRecord;
        let mut benchmarks = std::collections::HashMap::new();
        benchmarks.insert(
            "tight_rule".to_string(),
            BenchmarkRecord {
                rule: "tight_rule".into(),
                wall_time_secs: 10.0,
                max_memory_mb: Some(810),
                memory_limit_mb: Some(1000),
                cpu_seconds: None,
                retries: 0,
            },
        );
        benchmarks.insert(
            "comfy_rule".to_string(),
            BenchmarkRecord {
                rule: "comfy_rule".into(),
                wall_time_secs: 5.0,
                max_memory_mb: Some(790),
                memory_limit_mb: Some(1000),
                cpu_seconds: None,
                retries: 0,
            },
        );
        let resp = diagnose_run(&[], "", &benchmarks);
        assert_eq!(resp.resource_bottlenecks.len(), 1);
        let b = &resp.resource_bottlenecks[0];
        assert_eq!(b.rule, "tight_rule");
        assert_eq!(b.metric, "max_memory_mb");
        assert_eq!(b.actual, 810.0);
        assert_eq!(b.limit, 1000.0);
    }

    #[test]
    fn diagnose_degrades_to_empty_bottlenecks_without_measurements() {
        let resp = diagnose_run(&[], "", &Default::default());
        assert!(resp.resource_bottlenecks.is_empty());
        // A measured peak without a declared limit is not a bottleneck.
        let mut benchmarks = std::collections::HashMap::new();
        benchmarks.insert(
            "no_limit".to_string(),
            oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                rule: "no_limit".into(),
                wall_time_secs: 1.0,
                max_memory_mb: Some(5000),
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
            },
        );
        let resp = diagnose_run(&[], "", &benchmarks);
        assert!(resp.resource_bottlenecks.is_empty());
    }
}
