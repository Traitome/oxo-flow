//! HTTP handlers for AI domain.
//!
//! All handlers return NOT_IMPLEMENTED — AI features are planned for Phase 2.

use axum::{Json, http::StatusCode};

use crate::domains::workflow::handlers::{ApiError, err};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn not_impl() -> (StatusCode, Json<ApiError>) {
    err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "AI features coming in Phase 2".into(),
    )
}

pub async fn translate(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn explain(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn interpret(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn optimize(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn get_ai_config() -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn update_ai_config(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}

pub async fn test_ai_config(Json(_req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Err(not_impl())
}
