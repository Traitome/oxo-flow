//! Pure execution domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path};

use crate::infra::db::models;

use super::diagnostics::DiagnosticsEngine;
use super::types::*;

/// Detect a BioTypes declaration from parsed TOML structure.
///
/// Supports both table declarations (`[types.name]`, `[rules.types]`) and
/// inline declarations (`types = { ... }`). Invalid TOML is handled by the
/// canonical workflow parser before run admission; this predicate is only a
/// conservative routing check for the resolved-admission gate.
#[cfg(test)]
fn source_rejects_unresolved_admission(source: &str) -> bool {
    let Ok(root) = toml::from_str::<toml::Table>(source) else {
        return false;
    };
    table_declares_type_contracts(&root)
}

fn table_declares_type_contracts(root: &toml::Table) -> bool {
    let has_root_types = root
        .get("types")
        .and_then(toml::Value::as_table)
        .is_some_and(|types| !types.is_empty());
    let has_rule_types = root
        .get("rules")
        .and_then(toml::Value::as_array)
        .is_some_and(|rules| {
            rules.iter().filter_map(toml::Value::as_table).any(|rule| {
                rule.get("types")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|types| !types.is_empty())
            })
        });
    has_root_types || has_rule_types
}

fn include_tables(root: &toml::Table) -> Vec<&toml::Table> {
    root.get("include")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
        .collect()
}

fn resolve_included_source(current: &str, include: &str) -> Option<String> {
    if include.starts_with("http://")
        || include.starts_with("https://")
        || Path::new(include).is_absolute()
    {
        return None;
    }
    let mut resolved = Vec::new();
    for component in Path::new(current)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(include)
        .components()
    {
        match component {
            Component::Normal(part) => resolved.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!resolved.is_empty()).then(|| resolved.join("/"))
}

/// Evidence gathered from the host and its locally staged include graph.
///
/// Unrelated staged assets are deliberately ignored: `include_files` may
/// contain environment manifests, data dictionaries, or other TOML that
/// happens to mention `types` without participating in workflow admission.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AdmissionSourceScan {
    pub host_declares_type_contracts: bool,
    pub host_has_include_graph: bool,
    pub included_source_declares_type_contracts: bool,
    pub unresolved_include: bool,
}

/// Scan the host and locally staged sources reachable through `[[include]]`.
pub(crate) fn scan_admission_sources<'a>(
    host_source: &str,
    include_sources: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> AdmissionSourceScan {
    let sources: HashMap<&str, &str> = include_sources.into_iter().collect();
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([("workflow.oxoflow".to_string(), host_source)]);
    let mut scan = AdmissionSourceScan::default();

    while let Some((path, source)) = pending.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let Ok(root) = toml::from_str::<toml::Table>(source) else {
            if path != "workflow.oxoflow" {
                scan.unresolved_include = true;
            }
            continue;
        };
        if table_declares_type_contracts(&root) {
            if path == "workflow.oxoflow" {
                scan.host_declares_type_contracts = true;
            } else {
                scan.included_source_declares_type_contracts = true;
            }
        }
        let includes = include_tables(&root);
        if path == "workflow.oxoflow" && !includes.is_empty() {
            scan.host_has_include_graph = true;
        }
        for include in includes {
            if include.get("repo").is_some() {
                scan.unresolved_include = true;
                continue;
            }
            let Some(include_path) = include.get("path").and_then(toml::Value::as_str) else {
                scan.unresolved_include = true;
                continue;
            };
            let Some(resolved) = resolve_included_source(&path, include_path) else {
                scan.unresolved_include = true;
                continue;
            };
            let Some(content) = sources.get(resolved.as_str()) else {
                scan.unresolved_include = true;
                continue;
            };
            pending.push_back((resolved, content));
        }
    }
    scan
}

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

    // Web admission uses the same canonical biological contract gate as the
    // CLI preflight. Rejecting here prevents an invalid run from being
    // persisted or queued while deferring the failure to the spawned CLI.
    if let Err(type_errors) = oxo_flow_core::types::validate_workflow_types(&wf, &dag) {
        let findings = type_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "declared biological type contracts are violated: {findings}"
        ));
    }

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
    fn create_run_rejects_declared_type_contract_violations() {
        let toml = r#"
[workflow]
name = "type-gate"
version = "0.1.0"

[types.sam_unsorted]
format = "sam"
sort_order = "unsorted"

[types.bam_sorted_indexed]
format = "bam"
sort_order = "coordinate"
index = "bai"

[[rules]]
name = "align"
output = ["aligned.sam"]
shell = "touch aligned.sam"

[rules.types]
outputs = { default = "sam_unsorted" }

[[rules]]
name = "call"
input = ["aligned.sam"]
output = ["calls.vcf"]
shell = "touch calls.vcf"

[rules.types]
inputs = { default = "bam_sorted_indexed" }
"#;
        let config = RunConfig {
            max_jobs: Some(2),
            dry_run: None,
            keep_going: None,
            resource_budget: None,
        };
        let err = create_run(toml, &config, None)
            .expect_err("web admission must reject declared type violations");
        assert!(err.contains("TYPE-001"), "{err}");
        assert!(err.contains("TYPE-002"), "{err}");
        assert!(err.contains("TYPE-003"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn typed_contract_source_scan_handles_tables_inline_tables_and_comments() {
        assert!(source_rejects_unresolved_admission(
            "[types.sam]\nformat = \"sam\""
        ));
        assert!(source_rejects_unresolved_admission(
            "[workflow]\nname = \"typed\"\n[[rules]]\nname = \"r\"\n  [rules.types] # contract\noutputs = {}"
        ));
        assert!(!source_rejects_unresolved_admission(
            "# [types.sam]\nformat = \"sam\""
        ));
        assert!(!source_rejects_unresolved_admission("[env]\nchannels = []"));
        assert!(source_rejects_unresolved_admission(
            "types = { sam = { format = \"sam\" } }\n[workflow]\nname = \"typed\""
        ));
        assert!(source_rejects_unresolved_admission(
            "[workflow]\nname = \"typed\"\n[[rules]]\nname = \"r\"\ntypes = { outputs = { default = \"sam\" } }"
        ));
        assert!(source_rejects_unresolved_admission(
            "[workflow]\nname = \"typed\"\n[[rules]]\nname = \"r\"\ntypes.outputs = { default = \"sam\" }"
        ));
    }

    #[test]
    fn typed_contract_scan_follows_only_reachable_local_includes() {
        let host = r#"
[workflow]
name = "host"

[[include]]
path = "modules/plain.oxoflow"
"#;
        let plain = r#"
[workflow]
name = "plain"

[[include]]
path = "nested/typed.oxoflow"
"#;
        let typed = r#"
[workflow]
name = "typed"

[[rules]]
name = "r"
types = { outputs = { default = "sam" } }
"#;
        let unrelated = r#"
[types.sam]
format = "sam"
"#;
        let sources = [
            ("modules/plain.oxoflow", plain),
            ("modules/nested/typed.oxoflow", typed),
            ("assets/unrelated.toml", unrelated),
        ];
        let scan = scan_admission_sources(host, sources);
        assert!(!scan.host_declares_type_contracts);
        assert!(scan.host_has_include_graph);
        assert!(scan.included_source_declares_type_contracts);
        assert!(!scan.unresolved_include);

        let host_without_nested_types = r#"
[workflow]
name = "host"

[[include]]
path = "modules/plain.oxoflow"
"#;
        let plain_without_includes = r#"
[workflow]
name = "plain"
"#;
        let unrelated_only = [
            ("modules/plain.oxoflow", plain_without_includes),
            ("assets/unrelated.toml", unrelated),
        ];
        let scan = scan_admission_sources(host_without_nested_types, unrelated_only);
        assert!(!scan.host_declares_type_contracts);
        assert!(scan.host_has_include_graph);
        assert!(!scan.included_source_declares_type_contracts);
        assert!(!scan.unresolved_include);

        let host_with_unstaged_include = r#"
[workflow]
name = "host"

[[include]]
path = "modules/missing.oxoflow"
"#;
        let scan = scan_admission_sources(host_with_unstaged_include, unrelated_only);
        assert!(scan.host_has_include_graph);
        assert!(scan.unresolved_include);

        let host_with_repo_include = r#"
[workflow]
name = "host"

[[include]]
path = "modules/plain.oxoflow"
repo = "https://example.com/workflows.git"
ref = "v1"
"#;
        let staged_shadow = [("modules/plain.oxoflow", "[workflow]\nname = \"shadow\"\n")];
        let scan = scan_admission_sources(host_with_repo_include, staged_shadow);
        assert!(scan.host_has_include_graph);
        assert!(scan.unresolved_include);
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
            // symlink_metadata deliberately does not follow symlinks — the
            // same rule the zip path (files.rs) applies: a rule-created link
            // may point outside the workdir, and both views of the tree must
            // agree on what is exposed.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
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
mod run_list_item_tests {
    use super::*;

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
}
