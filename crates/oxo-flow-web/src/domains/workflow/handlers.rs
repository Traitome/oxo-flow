//! HTTP handlers for workflow domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{extract::Path, http::StatusCode, Json};

use super::service;
use super::types::*;
use crate::domains::observability::types::*;

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn err(status: StatusCode, code: &str, msg: String) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: msg,
            detail: None,
            suggestion: None,
        }),
    )
}

// ---------------------------------------------------------------------------
// Pipeline lifecycle
// ---------------------------------------------------------------------------

/// POST /api/pipelines/parse
pub async fn parse_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<ParseResponse> {
    service::parse_pipeline(&req.toml_content, req.format_version.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PARSE_ERROR", e))
}

/// POST /api/pipelines/validate
///
/// Accepts TOML content directly so the endpoint is self-contained.
pub async fn validate_pipeline(
    Json(req): Json<serde_json::Value>,
) -> ApiResult<ValidateResponse> {
    // Accept either { toml_content } or { pipeline_id } for flexibility
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    service::validate_pipeline(toml)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "VALIDATE_ERROR", e))
}

/// POST /api/pipelines/prepare
pub async fn prepare_pipeline(
    Json(req): Json<serde_json::Value>,
) -> ApiResult<PrepareResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    let resolve = req
        .get("resolve_wildcards")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let apply = req
        .get("apply_defaults")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    service::prepare_pipeline(toml, resolve, apply)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PREPARE_ERROR", e))
}

/// POST /api/pipelines/dag
pub async fn build_dag(Json(req): Json<serde_json::Value>) -> ApiResult<DagJsonResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    service::build_dag(toml)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DAG_ERROR", e))
}

/// POST /api/pipelines/format
pub async fn format_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<FormatResponse> {
    service::format_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "FORMAT_ERROR", e))
}

/// POST /api/pipelines/lint
pub async fn lint_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<ValidateResponse> {
    service::lint_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "LINT_ERROR", e))
}

/// POST /api/pipelines/stats
pub async fn pipeline_stats(
    Json(req): Json<ParseRequest>,
) -> ApiResult<WorkflowStatsResponse> {
    service::workflow_stats(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "STATS_ERROR", e))
}

/// POST /api/pipelines/diff
pub async fn diff_pipelines(Json(req): Json<DiffRequest>) -> ApiResult<DiffResponse> {
    let a = req.pipeline_a_id.as_deref().unwrap_or("");
    let b = req.pipeline_b_id.as_deref().unwrap_or("");
    service::diff_workflows(a, b)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DIFF_ERROR", e))
}

/// POST /api/pipelines/export
pub async fn export_pipeline(Json(req): Json<ExportRequest>) -> ApiResult<ExportResponse> {
    let id = req.pipeline_id.as_deref().unwrap_or("");
    service::export_pipeline(id, req.format.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "EXPORT_ERROR", e))
}

/// POST /api/pipelines/search
pub async fn search_pipelines(
    Json(req): Json<SearchRequest>,
) -> ApiResult<SearchResponse> {
    // For v0.8, search only matches templates (saved pipeline search comes later)
    let empty_pipelines = vec![];
    let empty_templates = vec![];
    Ok(Json(service::search_pipelines(
        &req.query,
        &empty_pipelines,
        &empty_templates,
    )))
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

/// GET /api/pipelines
pub async fn list_pipelines() -> ApiResult<Vec<Pipeline>> {
    Ok(Json(vec![]))
}

/// GET /api/pipelines/{id}
pub async fn get_pipeline(Path(_id): Path<String>) -> ApiResult<Pipeline> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Pipeline not found".into(),
    ))
}

/// PUT /api/pipelines/{id}
pub async fn update_pipeline(Path(_id): Path<String>) -> ApiResult<Pipeline> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "Not yet implemented".into(),
    ))
}

/// DELETE /api/pipelines/{id}
pub async fn delete_pipeline(Path(_id): Path<String>) -> ApiResult<()> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "Not yet implemented".into(),
    ))
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// GET /api/templates
pub async fn list_templates() -> ApiResult<Vec<Template>> {
    Ok(Json(vec![]))
}

/// GET /api/templates/{id}
pub async fn get_template(Path(_id): Path<String>) -> ApiResult<Template> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Template not found".into(),
    ))
}

/// POST /api/templates
pub async fn save_template(
    Json(_req): Json<serde_json::Value>,
) -> ApiResult<Template> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "Not yet implemented".into(),
    ))
}

/// DELETE /api/templates/{id}
pub async fn delete_template(Path(_id): Path<String>) -> ApiResult<()> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "Not yet implemented".into(),
    ))
}

// ---------------------------------------------------------------------------
// Data discovery
// ---------------------------------------------------------------------------

/// POST /api/data/analyze
pub async fn analyze_data(
    Json(req): Json<DataAnalysisRequest>,
) -> ApiResult<DataAnalysisResponse> {
    super::data::analyze_files(&req.paths, req.max_depth)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DATA_ERROR", e))
}

/// POST /api/data/reference
pub async fn discover_reference(
    Json(req): Json<ReferenceRequest>,
) -> ApiResult<ReferenceResponse> {
    super::data::discover_reference(&req.genome, &req.components)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "REF_ERROR", e))
}
