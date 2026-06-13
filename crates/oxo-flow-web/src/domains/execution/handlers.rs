//! HTTP handlers for execution domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, extract::Path, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn err(s: StatusCode, c: &str, m: String) -> (StatusCode, Json<ApiError>) {
    (
        s,
        Json(ApiError {
            code: c.into(),
            message: m,
            detail: None,
            suggestion: None,
        }),
    )
}

/// POST /api/runs
pub async fn create_run(Json(req): Json<serde_json::Value>) -> ApiResult<CreateRunResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "toml_content required".into(),
            )
        })?;
    let max_jobs = req.get("max_jobs").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
    let config = RunConfig {
        max_jobs: Some(max_jobs),
        dry_run: None,
        keep_going: None,
        resource_budget: None,
    };
    service::create_run(toml, &config, None)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "RUN_ERROR", e))
}

/// GET /api/runs
pub async fn list_runs() -> ApiResult<Vec<serde_json::Value>> {
    Ok(Json(vec![]))
}

/// GET /api/runs/{id}
pub async fn get_run(Path(_id): Path<String>) -> ApiResult<serde_json::Value> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// GET /api/runs/{id}/status
pub async fn get_run_status(Path(_id): Path<String>) -> ApiResult<RunStatusResponse> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// GET /api/runs/{id}/dag-status
pub async fn get_dag_status(Path(_id): Path<String>) -> ApiResult<DagStatusResponse> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// GET /api/runs/{id}/diagnostics
pub async fn get_diagnostics(Path(_id): Path<String>) -> ApiResult<DiagnosticsResponse> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// GET /api/runs/{id}/logs
pub async fn get_run_logs(Path(_id): Path<String>) -> ApiResult<String> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// GET /api/runs/{id}/results
pub async fn get_run_results(Path(_id): Path<String>) -> ApiResult<Vec<serde_json::Value>> {
    Ok(Json(vec![]))
}

/// POST /api/runs/{id}/retry
pub async fn retry_run(Path(_id): Path<String>) -> ApiResult<RetryResponse> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}

/// POST /api/runs/{id}/cancel
pub async fn cancel_run(Path(_id): Path<String>) -> ApiResult<serde_json::Value> {
    Err(err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "Run not found".into(),
    ))
}
