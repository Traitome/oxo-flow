//! Workflow-related handlers.
//!
//! Handles workflow parsing, validation, DAG visualization, execution,
//! formatting, linting, statistics, and comparison.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use std::sync::atomic::Ordering;

use crate::{
    ACTIVE_WORKFLOWS, ApiError, CleanResponse, DagResponse, DiagnosticItem, DiffEntry, DiffRequest,
    DiffResponse, DryRunRequest, ErrorResponse, ExportRequest, ExportResponse, FormatResponse,
    LintRequest, LintResponse, LintSummary, PaginatedLintResponse, PaginationMeta,
    PaginationParams, RuleSummary, RunConfig, RunResponse, RunStatus, StatsResponse,
    ValidateRequest, ValidateResponse, WorkflowDetail, db, executor, extract_session, workspace,
};

/// `POST /api/workflows/validate` — Parse + validate a workflow TOML.
pub async fn validate_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    let config = match oxo_flow_core::WorkflowConfig::parse(&req.toml_content) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: None,
                edges_count: None,
            }));
        }
    };

    let dag = match oxo_flow_core::WorkflowDag::from_rules(&config.rules) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: Some(config.rules.len()),
                edges_count: None,
            }));
        }
    };

    if let Err(e) = dag.validate() {
        return Ok(Json(ValidateResponse {
            valid: false,
            errors: vec![e.to_string()],
            rules_count: Some(dag.node_count()),
            edges_count: Some(dag.edge_count()),
        }));
    }

    Ok(Json(ValidateResponse {
        valid: true,
        errors: vec![],
        rules_count: Some(dag.node_count()),
        edges_count: Some(dag.edge_count()),
    }))
}

/// `POST /api/workflows/parse` — Parse a workflow and return full detail.
pub async fn parse_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<WorkflowDetail>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let rules: Vec<RuleSummary> = config
        .rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.to_vec(),
            outputs: r.output.to_vec(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    Ok(Json(WorkflowDetail {
        name: config.workflow.name.clone(),
        version: config.workflow.version.clone(),
        description: config.workflow.description.clone(),
        author: config.workflow.author.clone(),
        rules_count: rules.len(),
        rules,
    }))
}

/// `POST /api/workflows/dag` — Build a DAG and return its DOT representation.
pub async fn build_dag(Json(req): Json<ValidateRequest>) -> Result<Json<DagResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    Ok(Json(DagResponse {
        dot: dag.to_dot(),
        nodes: dag.node_count(),
        edges: dag.edge_count(),
    }))
}

/// `POST /api/workflows/dry-run` — Simulate execution and return the plan.
pub async fn dry_run(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let rules: Vec<RuleSummary> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.to_vec(),
            outputs: r.output.to_vec(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    let run_config = req.config.unwrap_or(RunConfig {
        max_jobs: None,
        dry_run: None,
        keep_going: None,
    });

    let status = RunStatus {
        id: uuid::Uuid::new_v4().to_string(),
        status: "dry-run".to_string(),
        rules_total: rules.len(),
        rules_completed: 0,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
    };

    #[derive(Serialize)]
    struct DryRunResponse {
        status: RunStatus,
        execution_order: Vec<String>,
        rules: Vec<RuleSummary>,
        config: RunConfig,
    }

    Ok(Json(DryRunResponse {
        status,
        execution_order: order,
        rules,
        config: run_config,
    }))
}

/// `POST /api/workflows/run` — Initialize a run and start it in the background.
pub async fn run_workflow(
    headers: axum::http::HeaderMap,
    Json(req): Json<DryRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    // Fetch full user details for auth_type and os_user
    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found in DB", None))?;

    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let run_id = uuid::Uuid::new_v4().to_string();

    // 1. Initialize physical sandbox
    workspace::initialize_sandbox(&user.username, &run_id, &req.toml_content)
        .map_err(|e| ApiError::unprocessable("Failed to setup sandbox", Some(e.to_string())))?;

    // 2. Insert run record into DB
    let run = db::Run {
        id: run_id.clone(),
        user_id: user.id.clone(),
        workflow_name: config.workflow.name.clone(),
        status: "pending".to_string(),
        pid: None,
        started_at: None,
        finished_at: None,
    };
    db::insert_run(&run)
        .await
        .map_err(|e| ApiError::unprocessable("Failed to save run record", Some(e.to_string())))?;

    // 3. Log the action
    let _ = db::log_action(&user.id, "run", &config.workflow.name).await;

    // 4. Spawn background executor
    executor::spawn_background_run(
        run_id.clone(),
        user.username.clone(),
        user.auth_type.clone(),
        user.os_user.clone(),
    );

    ACTIVE_WORKFLOWS.fetch_add(1, Ordering::Relaxed);

    Ok(Json(RunResponse {
        run_id,
        status: "started".to_string(),
        execution_order: order,
        rules_total: config.rules.len(),
    }))
}

/// `POST /api/workflows/clean` — List output files that would be cleaned.
pub async fn clean_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<CleanResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let files_to_clean: Vec<String> = config
        .rules
        .iter()
        .flat_map(|r| r.output.to_vec())
        .collect();

    Ok(Json(CleanResponse {
        workflow_name: config.workflow.name.clone(),
        total_files: files_to_clean.len(),
        files_to_clean,
    }))
}

/// `POST /api/workflows/export` — Generate a Dockerfile or Singularity def.
pub async fn export_workflow(
    Json(req): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let format = req.format.unwrap_or_else(|| "docker".to_string());
    let pkg_config = oxo_flow_core::container::PackageConfig::default();

    let content = match format.as_str() {
        "singularity" => oxo_flow_core::container::generate_singularity_def(&config, &pkg_config)
            .map_err(|e| {
            ApiError::unprocessable("Singularity def generation failed", Some(e.to_string()))
        })?,
        _ => oxo_flow_core::container::generate_dockerfile(&config, &pkg_config).map_err(|e| {
            ApiError::unprocessable("Dockerfile generation failed", Some(e.to_string()))
        })?,
    };

    let actual_format = match format.as_str() {
        "singularity" => "singularity".to_string(),
        _ => "docker".to_string(),
    };

    Ok(Json(ExportResponse {
        format: actual_format,
        content,
    }))
}

/// `POST /api/workflows/format` — Format a workflow TOML into canonical form.
pub async fn format_workflow_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<FormatResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let formatted = oxo_flow_core::format::format_workflow(&config);

    Ok(Json(FormatResponse { formatted }))
}

/// `POST /api/workflows/lint` — Lint a workflow for best practices.
pub async fn lint_workflow(Json(req): Json<LintRequest>) -> Result<Json<LintResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint_diags = oxo_flow_core::format::lint_format(&config);

    let mut diagnostics = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;

    for d in validation.diagnostics.iter().chain(lint_diags.iter()) {
        let severity = match d.severity {
            oxo_flow_core::format::Severity::Error => {
                error_count += 1;
                "error"
            }
            oxo_flow_core::format::Severity::Warning => {
                warning_count += 1;
                "warning"
            }
            oxo_flow_core::format::Severity::Info => {
                info_count += 1;
                "info"
            }
        };
        diagnostics.push(DiagnosticItem {
            severity: severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }

    Ok(Json(LintResponse {
        diagnostics,
        error_count,
        warning_count,
        info_count,
    }))
}

/// `POST /api/workflows/lint/paginated` — Lint with paginated results.
pub async fn lint_workflow_paginated(
    pagination: axum::extract::Query<PaginationParams>,
    Json(req): Json<LintRequest>,
) -> Result<Json<PaginatedLintResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("parse error", Some(e.to_string())))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint = oxo_flow_core::format::lint_format(&config);

    let mut all_diagnostics: Vec<DiagnosticItem> = Vec::new();
    for d in &validation.diagnostics {
        all_diagnostics.push(DiagnosticItem {
            severity: d.severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }
    for d in &lint {
        all_diagnostics.push(DiagnosticItem {
            severity: d.severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }

    let error_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .count();
    let warning_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "warning")
        .count();
    let info_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "info")
        .count();

    let total = all_diagnostics.len();
    let per_page = pagination.clamped_per_page();
    let offset = pagination.offset();

    let page_items: Vec<DiagnosticItem> = all_diagnostics
        .into_iter()
        .skip(offset)
        .take(per_page)
        .collect();

    Ok(Json(PaginatedLintResponse {
        diagnostics: page_items,
        pagination: PaginationMeta::new(pagination.page, per_page, total),
        summary: LintSummary {
            error_count,
            warning_count,
            info_count,
        },
    }))
}

/// `POST /api/workflows/stats` — Return workflow statistics.
pub async fn workflow_stats_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<StatsResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let stats = oxo_flow_core::format::workflow_stats(&config);

    Ok(Json(StatsResponse {
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
    }))
}

/// `POST /api/workflows/diff` — Compare two workflow configurations.
pub async fn diff_workflows_endpoint(
    Json(req): Json<DiffRequest>,
) -> Result<Json<DiffResponse>, ApiError> {
    let config_a = oxo_flow_core::WorkflowConfig::parse(&req.toml_a)
        .map_err(|e| ApiError::bad_request("Invalid first workflow TOML", Some(e.to_string())))?;
    let config_b = oxo_flow_core::WorkflowConfig::parse(&req.toml_b)
        .map_err(|e| ApiError::bad_request("Invalid second workflow TOML", Some(e.to_string())))?;

    let diffs = oxo_flow_core::format::diff_workflows(&config_a, &config_b);

    Ok(Json(DiffResponse {
        diff_count: diffs.len(),
        diffs: diffs
            .into_iter()
            .map(|d| DiffEntry {
                category: d.category,
                description: d.description,
            })
            .collect(),
    }))
}
