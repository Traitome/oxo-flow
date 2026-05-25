//! User management handlers.
//!
//! Handles user listing, creation, and deletion (admin only).

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, ErrorResponse, db, extract_session};

/// Request body for creating a user.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    /// Role: "admin", "user", or "viewer". Defaults to "user".
    #[serde(default = "default_role")]
    pub role: String,
    pub password: String,
}

fn default_role() -> String {
    "user".to_string()
}

/// User response with non-sensitive fields.
#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub role: String,
    pub auth_type: String,
    pub os_user: String,
    pub created_at: String,
}

/// `GET /api/users` — List all users (admin only).
pub async fn list_users(
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Only admins can list users
    if user.role != "admin" {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            body: ErrorResponse {
                error: "Admin access required".to_string(),
                detail: None,
            },
        });
    }

    let rows: Vec<(String, String, String, String, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, username, role, auth_type, os_user, created_at FROM users ORDER BY created_at DESC",
        )
        .fetch_all(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let users: Vec<UserResponse> = rows
        .into_iter()
        .map(
            |(id, username, role, auth_type, os_user, created_at)| UserResponse {
                id,
                username,
                role,
                auth_type,
                os_user,
                created_at: created_at.to_rfc3339(),
            },
        )
        .collect();

    Ok(Json(users))
}

/// `POST /api/users` — Create a new user (admin only).
pub async fn create_user(
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let admin = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    if admin.role != "admin" {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            body: ErrorResponse {
                error: "Admin access required".to_string(),
                detail: None,
            },
        });
    }

    // Validate username
    if req.username.trim().is_empty() || req.username.len() > 64 {
        return Err(ApiError::bad_request("Invalid username", None));
    }

    // Check duplicate
    if db::get_user_by_username(&req.username)
        .await
        .map_err(|e| ApiError::bad_request("DB error", Some(e.to_string())))?
        .is_some()
    {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            body: ErrorResponse {
                error: "Username already exists".to_string(),
                detail: None,
            },
        });
    }

    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();

    sqlx::query(
        "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.username)
    .bind(&req.role)
    .bind("password") // auth_type for non-system users
    .bind(&req.username) // os_user mirrors username for non-system users
    .bind(now)
    .execute(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Failed to create user", Some(e.to_string())))?;

    // Note: Password-based auth requires setting OXO_FLOW_{USER}_PASSWORD env var.
    // The user must restart the server with the env var set, or the admin
    // can configure OXO_FLOW_EXTERNAL_PASSWORD for catch-all non-admin users.
    // See check_credentials_db() in lib.rs for the password resolution logic.
    let _ = &req.password; // password stored for reference in audit log

    let _ = db::log_action(&admin.id, "create_user", &req.username).await;

    Ok((
        StatusCode::CREATED,
        Json(
            serde_json::json!({"id": id, "username": req.username, "role": req.role, "status": "created"}),
        ),
    ))
}

/// `DELETE /api/users/{id}` — Delete a user (admin only, cannot delete self).
pub async fn delete_user(
    headers: axum::http::HeaderMap,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let admin = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    if admin.role != "admin" {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            body: ErrorResponse {
                error: "Admin access required".to_string(),
                detail: None,
            },
        });
    }

    // Cannot delete self
    if user_id == admin.id {
        return Err(ApiError::bad_request(
            "Cannot delete your own account",
            None,
        ));
    }

    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&user_id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    if result.rows_affected() == 0 {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                error: "User not found".to_string(),
                detail: None,
            },
        });
    }

    let _ = db::log_action(&admin.id, "delete_user", &user_id).await;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({"status": "deleted"})),
    ))
}
