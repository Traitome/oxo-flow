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
pub fn compute_overall_status(nodes: &[NodeStatusItem]) -> RunStatus {
    if nodes.iter().any(|n| n.status == NodeStatus::Failed) {
        RunStatus::Failed
    } else if nodes
        .iter()
        .all(|n| n.status == NodeStatus::Success || n.status == NodeStatus::Skipped)
    {
        RunStatus::Completed
    } else if nodes.iter().any(|n| n.status == NodeStatus::Running) {
        RunStatus::Running
    } else {
        RunStatus::Queued
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

/// Diagnose a failed run using the deterministic diagnostics engine.
pub fn diagnose_run(run_nodes: &[NodeStatusItem], log_output: &str) -> DiagnosticsResponse {
    let engine = DiagnosticsEngine::new();
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
        resource_bottlenecks: vec![],
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
        assert_eq!(compute_overall_status(&nodes), RunStatus::Completed);
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
        assert_eq!(compute_overall_status(&nodes), RunStatus::Failed);
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
        assert_eq!(compute_overall_status(&nodes), RunStatus::Running);
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
        let resp = diagnose_run(&nodes, log);
        assert_eq!(resp.failed_nodes.len(), 1);
        assert_eq!(
            resp.failed_nodes[0].error_pattern.as_deref(),
            Some("oom_killed")
        );
        assert_eq!(resp.warnings.len(), 1);
        assert_eq!(resp.warnings[0].pattern, "skipped");
    }
}
