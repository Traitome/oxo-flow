//! Pure execution domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

use crate::infra::db::models;

use super::diagnostics::DiagnosticsEngine;
use super::types::*;

/// Create a run from pipeline TOML. Returns execution plan with resource estimates.
pub fn create_run(
    pipeline_toml: &str,
    config: &RunConfig,
    _pipeline_id: Option<&str>,
) -> Result<CreateRunResponse, String> {
    let wf = WorkflowConfig::parse(pipeline_toml).map_err(|e| format!("Parse: {e}"))?;
    // Config expansion matches `run` (issue #707): {config.*}-routed
    // producer/consumer pairs only match after placeholder expansion.
    let dag = WorkflowDag::from_rules_with_config(&wf.rules, &wf.config_placeholder_values())
        .map_err(|e| format!("DAG: {e}"))?;
    let execution_order = dag.execution_order().map_err(|e| format!("Order: {e}"))?;
    let parallel_groups = dag.parallel_groups().unwrap_or_default();

    // When-awareness (issue #739): a rule whose gate is statically false
    // under the current [config] never runs — the spawned CLI prints
    // `[skip: when condition false]` for it — so the plan must not count
    // it. Wildcard/`{meta.*}` gates bake per instance during expansion and
    // stay in the plan, the same conservative stance the CLI dry-run
    // takes. Same evaluator call readiness and deep_check make.
    let gated_off: std::collections::HashSet<&str> = wf
        .rules
        .iter()
        .filter_map(|r| {
            let when = r.when.as_deref()?;
            (!when.contains('{')
                && !oxo_flow_core::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                    when,
                    &wf.config,
                    &std::collections::HashMap::new(),
                    wf.base_dir(),
                ))
            .then_some(r.name.as_str())
        })
        .collect();
    let execution_order: Vec<String> = execution_order
        .into_iter()
        .filter(|name| !gated_off.contains(name.as_str()))
        .collect();
    let parallel_groups: Vec<Vec<String>> = parallel_groups
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .filter(|name| !gated_off.contains(name.as_str()))
                .collect()
        })
        .filter(|group: &Vec<String>| !group.is_empty())
        .collect();

    // Estimate memory from rules. The engine's canonical parser
    // (`scheduler::parse_memory_mb`) is the single source of truth for
    // units: a heuristic guess (">1000 means already MB") silently
    // misreported legal values like `memory = "2048M"` as 2 GB in MB
    // (= 2,097,152 MB) and `memory = "8G"` as 8 MB.
    let memory_mb: u64 = wf
        .rules
        .iter()
        .filter_map(|r| r.effective_memory())
        .filter_map(oxo_flow_core::scheduler::parse_memory_mb)
        .max()
        .unwrap_or(0);

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
        note: None,
    })
}

/// Memory-pressure threshold: a rule whose sampled peak RSS reached this
/// fraction of its declared limit counts as a resource bottleneck
/// (issue #67 §4). Sampled peaks underestimate true maxima, so the
/// threshold is conservative.
const MEMORY_BOTTLENECK_THRESHOLD_PCT: u64 = 80;

/// Run-listing filter set for `GET /api/runs` (#948 P2-6).
///
/// Assembled by the handler from the query string and the authenticated
/// caller, then turned into SQL fragments by [`assemble_run_list_query`] —
/// the split keeps the pagination/filter semantics unit-testable without
/// any HTTP or database plumbing.
#[derive(Debug, Clone, Default)]
pub struct RunListFilters {
    /// Restrict results to this user's rows (the handler passes the
    /// non-admin caller's id; admins pass `None` to see the whole fleet —
    /// issue #82 P0-4 ownership scoping).
    pub user_id: Option<String>,
    /// `status` query parameter, already validated against the whitelist by
    /// the caller (unrecognized values are ignored, never 400).
    pub status: Option<String>,
    /// `q` search term — matched against `workflow_name` and run `id`.
    pub q: Option<String>,
    /// Cursor = `created_at` of the last row of the previous page. A page
    /// bound, NOT a filter: it applies to the row SELECT but never the
    /// COUNT (audit #665).
    pub cursor: Option<String>,
    /// Row cap, clamped by the caller to 1..=500 (default 100). Consumed by
    /// the CALLER — it formats the SELECT's `LIMIT` and fetches `limit + 1`
    /// to detect a next page; the assembled WHERE fragments stay cap-agnostic.
    #[allow(dead_code)]
    pub limit: i64,
}

/// SQL fragments for one run-listing page: the row SELECT (filters + cursor)
/// and the filter-only COUNT (audit #665), each with its ordered binds.
///
/// All fragments are static strings pushed by this function — every piece of
/// user input rides in the binds — which is what makes the assembled SQL safe
/// to pass to `sqlx::AssertSqlSafe` at the call site.
#[derive(Debug, Clone)]
pub struct RunListQuery {
    /// `SELECT * FROM runs<page_where> ORDER BY created_at DESC LIMIT <limit + 1>`
    /// — ready for `format!` substitution of the numeric LIMIT.
    pub page_where_sql: String,
    /// Binds for the SELECT, in placeholder order (admin scoping → status
    /// → q → cursor).
    pub page_binds: Vec<String>,
    /// `SELECT COUNT(*) FROM runs<filter_where>` — cursor deliberately
    /// excluded so the total stays stable across pages.
    pub filter_where_sql: String,
    /// Binds for the COUNT (admin scoping → status → q).
    pub filter_binds: Vec<String>,
}

/// Assemble the run-listing SELECT and COUNT fragments from the filters.
///
/// Clause order is part of the wire-stable behavior the tests pin down:
/// `user_id = ?` (ownership, issue #82 P0-4) → `status = ?` →
/// `(workflow_name LIKE ? OR id LIKE ?)` → page-only `created_at < ?`.
/// An unrecognized `status` is silently dropped (the handler's long-standing
/// contract), as is a whitespace-only `q`. The row cap is NOT folded in
/// here — the caller formats the numeric `LIMIT` itself (fetching
/// `limit + 1`), so the fragments stay cap-agnostic.
pub fn assemble_run_list_query(filters: &RunListFilters) -> RunListQuery {
    let mut where_clauses: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(user_id) = &filters.user_id {
        where_clauses.push("user_id = ?".to_string());
        binds.push(user_id.clone());
    }
    if let Some(status) = &filters.status
        && matches!(
            status.as_str(),
            "queued" | "running" | "paused" | "completed" | "failed" | "cancelled"
        )
    {
        where_clauses.push("status = ?".to_string());
        binds.push(status.clone());
    }
    if let Some(q) = &filters.q
        && !q.trim().is_empty()
    {
        where_clauses.push("(workflow_name LIKE ? OR id LIKE ?)".to_string());
        let pattern = format!("%{}%", q.trim());
        binds.push(pattern.clone());
        binds.push(pattern);
    }
    // The cursor is a page bound, not a filter: it applies to the row
    // SELECT but NOT to the total COUNT, which reports the whole matching
    // population and must not shrink with every page (audit #665).
    let mut page_clauses = where_clauses.clone();
    let mut page_binds = binds.clone();
    if let Some(cursor) = &filters.cursor {
        page_clauses.push("created_at < ?".to_string());
        page_binds.push(cursor.clone());
    }
    let page_where_sql = if page_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", page_clauses.join(" AND "))
    };
    let filter_where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    RunListQuery {
        page_where_sql,
        page_binds,
        filter_where_sql,
        filter_binds: binds,
    }
}

/// Shape one `RunRow` into the API's public list-item JSON (#948 P2-6).
///
/// Deliberately NO `pid`: the host process id is engine bookkeeping (cancel
/// and liveness probes use it server-side) and discloses host internals to
/// API consumers.
pub fn run_row_to_list_item(r: &models::RunRow) -> serde_json::Value {
    let duration_secs = match (&r.started_at, &r.finished_at) {
        (Some(s), Some(f)) => {
            let start = chrono::DateTime::parse_from_rfc3339(s).ok();
            let end = chrono::DateTime::parse_from_rfc3339(f).ok();
            match (start, end) {
                (Some(a), Some(b)) => Some((b - a).num_seconds().max(0)),
                _ => None,
            }
        }
        _ => None,
    };
    serde_json::json!({
        "id": r.id,
        "user_id": r.user_id,
        "pipeline_id": r.pipeline_id,
        "workflow_name": r.workflow_name,
        "status": r.status,
        "phase": r.phase,
        "dry_run": r.dry_run,
        "workdir": r.workdir,
        "started_at": r.started_at,
        "finished_at": r.finished_at,
        "created_at": r.created_at,
        "duration_secs": duration_secs,
    })
}

/// Diagnose a failed run using the deterministic diagnostics engine.
///
/// `benchmarks` carries the engine's per-rule measurements (sampled peak
/// RSS + declared limit) from the run's checkpoint — the source for the
/// resource-bottleneck list.
pub fn diagnose_run(
    run_nodes: &[NodeStatusItem],
    log_output: &str,
    benchmarks: &std::collections::BTreeMap<
        String,
        oxo_flow_core::executor::checkpoint::BenchmarkRecord,
    >,
) -> DiagnosticsResponse {
    // Shared engine: the static pattern table's regexes compile once (#708).
    let engine = DiagnosticsEngine::global();

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
            // Every Skipped item this path produces is a when-gate skip
            // (checkpoint_status keys both sources on
            // `when_verdicts == false`) — the old "upstream failure"
            // reason was never true here (issue #739).
            suggestion: "This rule's when-condition evaluated false — it did not run \
                         in this configuration."
                .into(),
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
    fn create_run_rejects_reserved_values_name() {
        // #760: the reserved-name collision must fail at PLAN time — the
        // web used to accept the run, persist it as queued, and fail 13 ms
        // after spawn.
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"

[[values]]
name = "sample"
values = ["s1", "s2"]
"#;
        let config = RunConfig {
            max_jobs: Some(2),
            dry_run: None,
            keep_going: None,
            resource_budget: None,
        };
        let err = create_run(toml, &config, None).expect_err("reserved name must fail");
        assert!(
            err.contains("collides with a built-in wildcard"),
            "error must name the collision: {err}"
        );
    }

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
    fn create_run_plan_excludes_when_gated_off_rules() {
        // #739: a rule whose gate is statically false never runs — the
        // spawned CLI prints `[skip: when condition false]` for it — so the
        // plan's order and rule count must not include it.
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"

[config]
run_qc = false

[[rules]]
name = "hello"
shell = "echo hi"
output = ["hi.txt"]

[[rules]]
name = "qc"
when = "config.run_qc"
shell = "echo qc"
output = ["qc.txt"]
"#;
        let config = RunConfig {
            max_jobs: Some(2),
            dry_run: None,
            keep_going: None,
            resource_budget: None,
        };
        let resp = create_run(toml, &config, None).unwrap();
        assert_eq!(
            resp.execution_plan.total_rules, 1,
            "gated-off rule must not count"
        );
        assert!(
            !resp
                .execution_plan
                .execution_order
                .iter()
                .any(|name| name == "qc"),
            "gated-off rule must not appear in the plan: {:?}",
            resp.execution_plan.execution_order
        );
    }

    #[test]
    fn create_run_memory_estimate_uses_canonical_units() {
        // Legal `8G` must report 8192 MB, not the heuristic's 8 MB
        // (bare "8" parsed, <1000, multiplied by 1024 — units guessed
        // from magnitude). Likewise `2048M` is 2048 MB, not 2 GB in MB.
        let toml = r#"
[workflow]
name = "test"
version = "0.1.0"
[[rules]]
name = "a"
memory = "8G"
shell = "true"
output = ["a.txt"]
[[rules]]
name = "b"
resources.memory = "2048M"
shell = "true"
output = ["b.txt"]
"#;
        let config = RunConfig {
            max_jobs: Some(2),
            dry_run: None,
            keep_going: None,
            resource_budget: None,
        };
        let resp = create_run(toml, &config, None).unwrap();
        assert_eq!(resp.estimated_resources.max_memory_mb, 8192);
    }

    #[test]
    fn create_run_memory_estimate_empty_workflow_floors_at_1gb() {
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
        assert_eq!(resp.estimated_resources.max_memory_mb, 1024);
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
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeStatusItem {
                rule: "step2".into(),
                status: NodeStatus::Pending,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
            },
        ];
        let plan = compute_retry_plan(&nodes, &dag, true).unwrap();
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
            stderr_tail: None,
            stdout_tail: None,
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
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeStatusItem {
                rule: "r2".into(),
                status: NodeStatus::Failed,
                started_at: None,
                duration_ms: None,
                exit_code: Some(1),
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
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
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeStatusItem {
                rule: "r2".into(),
                status: NodeStatus::Running,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
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
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeStatusItem {
                rule: "skipped_rule".into(),
                status: NodeStatus::Skipped,
                started_at: None,
                duration_ms: None,
                exit_code: None,
                progress_pct: None,
                stderr_tail: None,
                stdout_tail: None,
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
        let mut benchmarks = std::collections::BTreeMap::new();
        benchmarks.insert(
            "tight_rule".to_string(),
            BenchmarkRecord {
                rule: "tight_rule".into(),
                wall_time_secs: 10.0,
                max_memory_mb: Some(810),
                memory_limit_mb: Some(1000),
                cpu_seconds: None,
                retries: 0,
                recorded_as: None,
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
                recorded_as: None,
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
        let mut benchmarks = std::collections::BTreeMap::new();
        benchmarks.insert(
            "no_limit".to_string(),
            oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                rule: "no_limit".into(),
                wall_time_secs: 1.0,
                max_memory_mb: Some(5000),
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
                recorded_as: None,
            },
        );
        let resp = diagnose_run(&[], "", &benchmarks);
        assert!(resp.resource_bottlenecks.is_empty());
    }
}

#[cfg(test)]
mod run_list_query_tests {
    use super::*;

    fn filters(
        user_id: Option<&str>,
        status: Option<&str>,
        q: Option<&str>,
        cursor: Option<&str>,
    ) -> RunListFilters {
        RunListFilters {
            user_id: user_id.map(str::to_string),
            status: status.map(str::to_string),
            q: q.map(str::to_string),
            cursor: cursor.map(str::to_string),
            limit: 100,
        }
    }

    #[test]
    fn no_filters_yields_empty_where_and_no_binds() {
        let q = assemble_run_list_query(&filters(None, None, None, None));
        assert_eq!(q.page_where_sql, "");
        assert!(q.page_binds.is_empty());
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }

    #[test]
    fn non_admin_scope_adds_user_id_clause() {
        // Ownership scoping (issue #82 P0-4): non-admins see their own runs
        // only — first clause, before any user-supplied filter.
        let q = assemble_run_list_query(&filters(Some("u1"), Some("failed"), Some("rna"), None));
        assert_eq!(
            q.filter_where_sql,
            " WHERE user_id = ? AND status = ? AND (workflow_name LIKE ? OR id LIKE ?)"
        );
        assert_eq!(q.filter_binds, vec!["u1", "failed", "%rna%", "%rna%"]);
    }

    #[test]
    fn admin_sees_whole_fleet_no_user_clause() {
        let q = assemble_run_list_query(&filters(None, Some("running"), None, None));
        assert_eq!(q.filter_where_sql, " WHERE status = ?");
        assert_eq!(q.filter_binds, vec!["running"]);
    }

    #[test]
    fn unrecognized_status_is_silently_dropped() {
        let q = assemble_run_list_query(&filters(None, Some("DROP TABLE"), None, None));
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }

    #[test]
    fn empty_or_whitespace_q_is_dropped() {
        let q = assemble_run_list_query(&filters(None, None, Some("   "), None));
        assert_eq!(q.filter_where_sql, "");
        // A meaningful term still binds twice: LIKE pattern for both columns.
        let q2 = assemble_run_list_query(&filters(None, None, Some("  rna  "), None));
        assert_eq!(
            q2.filter_where_sql,
            " WHERE (workflow_name LIKE ? OR id LIKE ?)"
        );
        assert_eq!(q2.filter_binds, vec!["%rna%", "%rna%"]);
    }

    #[test]
    fn cursor_applies_to_select_but_not_count() {
        // Audit #665: the cursor is a page bound, not a filter — the COUNT
        // must not shrink with every page.
        let q = assemble_run_list_query(&filters(
            Some("u1"),
            None,
            None,
            Some("2026-01-01T00:00:00Z"),
        ));
        assert_eq!(q.page_where_sql, " WHERE user_id = ? AND created_at < ?");
        assert_eq!(q.page_binds, vec!["u1", "2026-01-01T00:00:00Z"]);
        assert_eq!(q.filter_where_sql, " WHERE user_id = ?");
        assert_eq!(q.filter_binds, vec!["u1"]);
    }

    #[test]
    fn full_assembled_select_sql_is_parameterized() {
        // The handler concatenates `page_where_sql` into the final SELECT —
        // assert the whole string contains nothing but static fragments and
        // the numeric limit.
        let q =
            assemble_run_list_query(&filters(Some("u1"), Some("queued"), Some("x"), Some("c1")));
        let sql = format!(
            "SELECT * FROM runs{} ORDER BY created_at DESC LIMIT {}",
            q.page_where_sql,
            100 + 1
        );
        assert_eq!(
            sql,
            "SELECT * FROM runs WHERE user_id = ? AND status = ? AND (workflow_name LIKE ? OR id LIKE ?) AND created_at < ? ORDER BY created_at DESC LIMIT 101"
        );
        assert_eq!(q.page_binds, vec!["u1", "queued", "%x%", "%x%", "c1"]);
        // A quote in the search term never reaches the SQL text — it rides
        // in the bind.
        let evil =
            assemble_run_list_query(&filters(None, None, Some("'; DROP TABLE runs;--"), None));
        assert!(!evil.page_where_sql.contains('\''));
        assert_eq!(
            evil.page_binds,
            vec!["%'; DROP TABLE runs;--%", "%'; DROP TABLE runs;--%"]
        );
    }

    #[test]
    fn list_item_omits_pid_and_computes_duration() {
        let row = models::RunRow {
            id: "r1".into(),
            user_id: "u1".into(),
            pipeline_id: None,
            pipeline_snapshot: String::new(),
            workflow_name: Some("wf".into()),
            status: "completed".into(),
            phase: "executing".into(),
            dry_run: false,
            pid: Some(4242),
            workdir: None,
            config_overrides: None,
            started_at: Some("2026-01-01T00:00:00Z".into()),
            finished_at: Some("2026-01-01T00:01:10Z".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let item = run_row_to_list_item(&row);
        assert!(item.get("pid").is_none(), "pid must never be public");
        assert_eq!(item["id"], "r1");
        assert_eq!(item["user_id"], "u1");
        assert_eq!(item["status"], "completed");
        assert_eq!(item["duration_secs"], 70);
        // A run without both timestamps has no duration to report.
        let open = models::RunRow {
            finished_at: None,
            pid: None,
            ..row
        };
        assert_eq!(
            run_row_to_list_item(&open)["duration_secs"],
            serde_json::Value::Null
        );
        // Clock skew (finished_at before started_at) clamps at 0, never
        // goes negative.
        let skewed = models::RunRow {
            started_at: Some("2026-01-01T00:01:10Z".into()),
            finished_at: Some("2026-01-01T00:00:00Z".into()),
            ..open
        };
        assert_eq!(run_row_to_list_item(&skewed)["duration_secs"], 0);
    }

    #[test]
    fn cursor_only_yields_bare_page_clause() {
        // The one filter-shape the other tests don't pin: a cursor with no
        // other filters — the admin's Load-More path.
        let q = assemble_run_list_query(&filters(None, None, None, Some("2026-01-01T00:00:00Z")));
        assert_eq!(q.page_where_sql, " WHERE created_at < ?");
        assert_eq!(q.page_binds, vec!["2026-01-01T00:00:00Z"]);
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }
}
