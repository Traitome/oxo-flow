//! HTTP handlers for collaboration domain (Phase 3 stubs).
//!
//! All endpoints return HTTP 501 Not Implemented until Phase 3 ships
//! the full collaboration features (fork, share, import).

use axum::{Json, http::StatusCode};

use crate::domains::workflow::handlers::ApiError;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn not_impl() -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiError {
            code: "NOT_IMPLEMENTED".into(),
            message: "Collaboration features coming in Phase 3".into(),
            detail: None,
            suggestion: None,
        }),
    )
}

pub async fn fork_pipeline(
    axum::extract::Path(_id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn share_pipeline(
    axum::extract::Path(_id): axum::extract::Path<String>,
    Json(_req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn import_pipeline(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}
