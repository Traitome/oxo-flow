//! Pure workflow domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

use super::types::*;

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse TOML content into a structured pipeline representation.
///
/// Returns [`ParseResponse`] with DAG information, rule summaries, and
/// workflow statistics.  Pure function, zero side effects.
pub fn parse_pipeline(toml_content: &str, _format_version: Option<&str>) -> Result<ParseResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse error: {e}"))?;

    let rules: Vec<RuleSummary> = config
        .rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.iter().map(|p| p.to_string()).collect(),
            outputs: r.output.iter().map(|p| p.to_string()).collect(),
            environment: Some(r.environment.kind().to_string()),
            threads: Some(r.effective_threads()),
        })
        .collect();

    let dag = WorkflowDag::from_rules(&config.rules).map_err(|e| format!("DAG build: {e}"))?;
    let parallel_groups = dag.parallel_groups().unwrap_or_default();
    let core_metrics = dag.metrics().unwrap_or_else(|_| oxo_flow_core::dag::DagMetrics {
        node_count: 0,
        edge_count: 0,
        max_depth: 0,
        max_width: 0,
        critical_path_length: 0,
        parallel_group_count: 0,
    });
    let critical_path = dag.critical_path().unwrap_or_default();

    let nodes: Vec<DagJsonNode> = config
        .rules
        .iter()
        .map(|r| {
            let env_str = r.environment.kind();
            let color = if env_str == "conda" {
                "#3B82F6"
            } else if env_str == "docker" {
                "#8B5CF6"
            } else if env_str == "singularity" {
                "#EC4899"
            } else {
                "#6B7280"
            };
            DagJsonNode {
                id: r.name.clone(),
                label: r.name.clone(),
                color: color.to_string(),
            }
        })
        .collect();

    let edges: Vec<DagJsonEdge> = config
        .rules
        .iter()
        .flat_map(|r| {
            dag.dependencies(&r.name)
                .unwrap_or_default()
                .into_iter()
                .map(|dep| DagJsonEdge {
                    from: dep,
                    to: r.name.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let stats = oxo_flow_core::format::workflow_stats(&config);

    Ok(ParseResponse {
        pipeline_id: uuid::Uuid::new_v4().to_string(),
        name: config.workflow.name.clone(),
        version: config.workflow.version.clone(),
        rules,
        dag: DagJsonResponse {
            nodes,
            edges,
            parallel_groups,
            critical_path,
            metrics: DagMetrics {
                node_count: core_metrics.node_count,
                edge_count: core_metrics.edge_count,
                max_depth: core_metrics.max_depth,
                max_width: core_metrics.max_width,
                critical_path_length: core_metrics.critical_path_length,
                parallel_group_count: core_metrics.parallel_group_count,
            },
        },
        stats: WorkflowStatsResponse {
            rule_count: stats.rule_count,
            shell_rules: stats.shell_rules,
            script_rules: stats.script_rules,
            dependency_count: stats.dependency_count,
            parallel_groups: stats.parallel_groups,
            max_depth: stats.max_depth,
            environments: stats.environments,
            total_threads: stats.total_threads,
            wildcard_count: stats.wildcard_count,
            wildcard_names: stats.wildcard_names,
        },
    })
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

/// Validate pipeline TOML content against the .oxoflow schema.
///
/// Returns structured errors with diagnostic codes and suggestions.
pub fn validate_pipeline(toml_content: &str) -> Result<ValidateResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let validation = oxo_flow_core::format::validate_format(&config);
    let lints = oxo_flow_core::format::lint_format(&config);

    let mut errors: Vec<ValidationError> = validation
        .diagnostics
        .iter()
        .map(|d| ValidationError {
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
            suggestion: d.suggestion.clone(),
        })
        .collect();

    errors.extend(lints.into_iter().map(|d| ValidationError {
        code: d.code,
        message: d.message,
        rule: d.rule,
        suggestion: d.suggestion,
    }));

    Ok(ValidateResponse {
        valid: !validation.has_errors(),
        errors,
    })
}

// ---------------------------------------------------------------------------
// Prepare
// ---------------------------------------------------------------------------

/// Prepare pipeline for execution: expand wildcards, apply defaults.
pub fn prepare_pipeline(
    toml_content: &str,
    resolve_wildcards: bool,
    apply_defaults: bool,
) -> Result<PrepareResponse, String> {
    let mut config =
        WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;

    if apply_defaults {
        config.apply_defaults();
    }

    let expanded_count = if resolve_wildcards {
        let _before = config.rules.len();
        config
            .expand_wildcards()
            .map_err(|e| format!("Wildcard: {e}"))?;
        config.rules.len()
    } else {
        config.rules.len()
    };

    let env_resolver = oxo_flow_core::environment::EnvironmentResolver::new();
    let setup_cmds: Vec<String> = config
        .rules
        .iter()
        .filter_map(|rule| {
            config
                .resolve_environment(rule)
                .and_then(|env_spec| {
                    env_resolver
                        .setup_command(&env_spec)
                        .ok()
                        .map(|cmd| format!("{}: {}", rule.name, cmd))
                })
        })
        .collect();

    Ok(PrepareResponse {
        pipeline_id: uuid::Uuid::new_v4().to_string(),
        expanded_rules_count: expanded_count,
        wildcard_combinations: 0,
        environment_setup_cmds: setup_cmds,
    })
}

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

/// Format TOML content into the canonical .oxoflow representation.
pub fn format_workflow(toml_content: &str) -> Result<FormatResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let formatted = oxo_flow_core::format::format_workflow(&config);
    Ok(FormatResponse { formatted })
}

// ---------------------------------------------------------------------------
// Lint
// ---------------------------------------------------------------------------

/// Lint pipeline TOML content and return diagnostics.
pub fn lint_workflow(toml_content: &str) -> Result<ValidateResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let diagnostics = oxo_flow_core::format::lint_format(&config);
    Ok(ValidateResponse {
        valid: true,
        errors: diagnostics
            .into_iter()
            .map(|d| ValidationError {
                code: d.code,
                message: d.message,
                rule: d.rule,
                suggestion: d.suggestion,
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Build workflow statistics from TOML content.
pub fn workflow_stats(toml_content: &str) -> Result<WorkflowStatsResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let s = oxo_flow_core::format::workflow_stats(&config);
    Ok(WorkflowStatsResponse {
        rule_count: s.rule_count,
        shell_rules: s.shell_rules,
        script_rules: s.script_rules,
        dependency_count: s.dependency_count,
        parallel_groups: s.parallel_groups,
        max_depth: s.max_depth,
        environments: s.environments,
        total_threads: s.total_threads,
        wildcard_count: s.wildcard_count,
        wildcard_names: s.wildcard_names,
    })
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// Diff two pipeline TOML contents and return structured differences.
pub fn diff_workflows(toml_a: &str, toml_b: &str) -> Result<DiffResponse, String> {
    let ca = WorkflowConfig::parse(toml_a).map_err(|e| format!("Parse A: {e}"))?;
    let cb = WorkflowConfig::parse(toml_b).map_err(|e| format!("Parse B: {e}"))?;
    let diffs = oxo_flow_core::format::diff_workflows(&ca, &cb);
    Ok(DiffResponse {
        diffs: diffs
            .into_iter()
            .map(|d| DiffEntry {
                path: d.category.clone(),
                category: d.category,
                description: d.description,
                severity: "info".to_string(),
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// DAG
// ---------------------------------------------------------------------------

/// Build a DAG representation suitable for frontend visualisation.
pub fn build_dag(toml_content: &str) -> Result<DagJsonResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let dag = WorkflowDag::from_rules(&config.rules).map_err(|e| format!("DAG: {e}"))?;

    let nodes: Vec<DagJsonNode> = config
        .rules
        .iter()
        .map(|r| {
            let env_str = r.environment.kind();
            let color = if env_str == "conda" {
                "#3B82F6"
            } else if env_str == "docker" {
                "#8B5CF6"
            } else if env_str == "singularity" {
                "#EC4899"
            } else {
                "#6B7280"
            };
            DagJsonNode {
                id: r.name.clone(),
                label: r.name.clone(),
                color: color.to_string(),
            }
        })
        .collect();

    let edges: Vec<DagJsonEdge> = config
        .rules
        .iter()
        .flat_map(|r| {
            dag.dependencies(&r.name)
                .unwrap_or_default()
                .into_iter()
                .map(|dep| DagJsonEdge {
                    from: dep,
                    to: r.name.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let groups = dag.parallel_groups().unwrap_or_default();
    let m = dag.metrics().unwrap_or_else(|_| oxo_flow_core::dag::DagMetrics {
        node_count: 0,
        edge_count: 0,
        max_depth: 0,
        max_width: 0,
        critical_path_length: 0,
        parallel_group_count: 0,
    });
    let cp = dag.critical_path().unwrap_or_default();

    Ok(DagJsonResponse {
        nodes,
        edges,
        parallel_groups: groups,
        critical_path: cp,
        metrics: DagMetrics {
            node_count: m.node_count,
            edge_count: m.edge_count,
            max_depth: m.max_depth,
            max_width: m.max_width,
            critical_path_length: m.critical_path_length,
            parallel_group_count: m.parallel_group_count,
        },
    })
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Export a pipeline as a Dockerfile or Singularity definition file.
pub fn export_pipeline(toml_content: &str, format: Option<&str>) -> Result<ExportResponse, String> {
    let config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let fmt = format.unwrap_or("docker");
    let pkg_config = oxo_flow_core::container::PackageConfig::default();
    let content = match fmt {
        "singularity" => {
            oxo_flow_core::container::generate_singularity_def(&config, &pkg_config)
                .map_err(|e| format!("Singularity: {e}"))?
        }
        _ => oxo_flow_core::container::generate_dockerfile(&config, &pkg_config)
            .map_err(|e| format!("Docker: {e}"))?,
    };
    Ok(ExportResponse {
        format: fmt.to_string(),
        content,
    })
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Search saved pipelines by keyword using simple text matching.
///
/// Queries are matched against template name, description, and tags.
/// Results are scored and sorted by relevance.
pub fn search_pipelines(
    query: &str,
    _pipelines: &[Pipeline],
    templates: &[Template],
) -> SearchResponse {
    let q = query.to_lowercase();
    let mut results: Vec<SearchResult> = Vec::new();

    for t in templates {
        let score = if t.name.to_lowercase().contains(&q) {
            0.9
        } else if t.description.to_lowercase().contains(&q) {
            0.6
        } else if t.tags.iter().any(|tag| tag.to_lowercase().contains(&q)) {
            0.5
        } else {
            continue;
        };
        results.push(SearchResult {
            id: t.id.clone(),
            name: t.name.clone(),
            source: "template".into(),
            category: Some(t.category.clone()),
            description: Some(t.description.clone()),
            tags: Some(t.tags.clone()),
            match_reason: "keyword_match".into(),
            score,
        });
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    SearchResponse {
        query: query.to_string(),
        total: results.len(),
        results,
    }
}
