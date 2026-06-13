//! HTTP handlers for auth domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;
use crate::infra::db::models;

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

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
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

    // Try DB session first
    let sessions = if let Ok(pool) = get_pool() {
        let token_str = token.to_string();
        let rows: Vec<models::SessionRow> =
            sqlx::query_as("SELECT * FROM sessions WHERE token = ?")
                .bind(&token_str)
                .fetch_all(pool)
                .await
                .unwrap_or_default();

        rows.into_iter()
            .map(|r| crate::domains::auth::types::Session {
                token: r.token,
                user_id: r.user_id,
                created_at: r.created_at,
                expires_at: r.expires_at,
            })
            .collect()
    } else {
        vec![]
    };

    service::validate_session(token, &sessions)
        .map(Json)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, "SESSION_ERROR", e))
}

/// GET /api/users
pub async fn list_users() -> ApiResult<Vec<UserResponse>> {
    let pool = get_pool()?;

    let rows: Vec<models::UserRow> = sqlx::query_as("SELECT * FROM users ORDER BY created_at ASC")
        .fetch_all(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let users: Vec<UserResponse> = rows
        .into_iter()
        .map(|r| UserResponse {
            id: r.id,
            username: r.username,
            role: r.role,
            auth_type: Some(r.auth_type),
            os_user: r.os_user,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(users))
}

/// POST /api/users
pub async fn create_user(Json(req): Json<CreateUserRequest>) -> ApiResult<UserResponse> {
    let pool = get_pool()?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();

    sqlx::query(
        "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.username)
    .bind(&req.role)
    .bind("password")
    .bind(None::<String>)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| {
        err(
            StatusCode::CONFLICT,
            "DB_ERROR",
            format!("Failed to create user: {e}"),
        )
    })?;

    Ok(Json(UserResponse {
        id,
        username: req.username,
        role: req.role.unwrap_or_else(|| "user".to_string()),
        auth_type: Some("password".to_string()),
        os_user: None,
        created_at: now,
    }))
}

/// DELETE /api/users/{id}
pub async fn delete_user(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    let pool = get_pool()?;

    let existing: Option<models::UserRow> = sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if existing.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("User {id} not found"),
        ));
    }

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

/// GET /api/license
pub async fn license_status() -> ApiResult<LicenseResponse> {
    Ok(Json(service::license_status()))
}

// ---------------------------------------------------------------------------
// OAuth2 handlers
// ---------------------------------------------------------------------------

/// POST /api/auth/oauth/authorize
///
/// Initiates an OAuth2 authorization flow. Returns the provider's
/// authorization URL that the user should be redirected to.
pub async fn oauth_authorize(
    Json(req): Json<OAuthAuthorizeRequest>,
) -> ApiResult<OAuthAuthorizeResponse> {
    let redirect_uri = req
        .redirect_uri
        .as_deref()
        .unwrap_or("http://localhost:8777/api/auth/oauth/callback");

    super::service::initiate_oauth(&req.provider, redirect_uri)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "OAUTH_ERROR", e))
}

/// POST /api/auth/oauth/callback
///
/// Handles the OAuth2 callback after the user authorizes the application.
/// Exchanges the authorization code for an access token and creates a session.
pub async fn oauth_callback(
    Json(req): Json<OAuthCallbackRequest>,
) -> ApiResult<OAuthCallbackResponse> {
    let provider = req.provider.as_deref().unwrap_or("orcid");
    let redirect_uri = std::env::var("OXO_FLOW_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:8777/api/auth/oauth/callback".to_string());

    super::service::handle_oauth_callback(provider, &req.code, &req.state, &redirect_uri)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "OAUTH_CALLBACK_ERROR", e))
}

/// POST /api/license/upload
pub async fn upload_license(Json(req): Json<serde_json::Value>) -> ApiResult<LicenseResponse> {
    // Log the upload attempt
    if let Ok(pool) = get_pool() {
        let license_data = req
            .get("license_data")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !license_data.is_empty() {
            let _ = sqlx::query(
                "INSERT INTO audit_logs (id, user_id, action, target, metadata, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind("system")
            .bind("upload_license")
            .bind("license")
            .bind(Some(license_data))
            .bind(now_iso())
            .execute(pool)
            .await;
        }
    }

    Ok(Json(service::license_status()))
}
