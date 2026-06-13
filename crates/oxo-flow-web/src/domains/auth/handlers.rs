//! HTTP handlers for auth domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

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

/// POST /api/auth/login
pub async fn login(Json(req): Json<LoginRequest>) -> ApiResult<LoginResponse> {
    service::authenticate(&req.username, &req.password)
        .map(Json)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, "AUTH_FAILED", e))
}

/// GET /api/auth/me
pub async fn auth_me(headers: axum::http::HeaderMap) -> ApiResult<AuthMeResponse> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let sessions = vec![];
    service::validate_session(token, &sessions)
        .map(Json)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, "SESSION_ERROR", e))
}

/// GET /api/users
pub async fn list_users() -> ApiResult<Vec<UserResponse>> {
    Ok(Json(vec![]))
}

/// POST /api/users
pub async fn create_user(Json(_req): Json<CreateUserRequest>) -> ApiResult<UserResponse> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "User management coming soon".into(),
    ))
}

/// DELETE /api/users/{id}
pub async fn delete_user(axum::extract::Path(_id): axum::extract::Path<String>) -> ApiResult<()> {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "User management coming soon".into(),
    ))
}

/// GET /api/license
pub async fn license_status() -> ApiResult<LicenseResponse> {
    Ok(Json(service::license_status()))
}

/// POST /api/license/upload
pub async fn upload_license(Json(_req): Json<serde_json::Value>) -> ApiResult<LicenseResponse> {
    Ok(Json(service::license_status()))
}
