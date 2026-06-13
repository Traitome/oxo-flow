//! Saved workflow handlers.
//!
//! Handles CRUD operations for user-saved workflows.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};

use crate::{
    ApiError, ErrorResponse, SaveWorkflowRequest, SavedWorkflowResponse, db, extract_session,
};

/// Retrieve a single saved workflow by ID, including full TOML content.
#[derive(Serialize, Deserialize)]
pub struct SavedWorkflowDetail {
    pub id: String,
    pub name: String,
    pub version: String,
    pub toml_content: String,
    pub rules_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

/// `POST /api/workflows/save` — Save or update a workflow.
pub async fn save_workflow(
    headers: axum::http::HeaderMap,
    Json(req): Json<SaveWorkflowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Validate TOML before saving
    let _config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let now = chrono::Utc::now();

    if let Some(ref wf_id) = req.id {
        // Update existing workflow if owned by user
        let result = sqlx::query(
            "UPDATE workflows SET name = ?, version = ?, toml_content = ?, updated_at = ? WHERE id = ? AND user_id = ?"
        )
        .bind(&req.name)
        .bind(&req.version)
        .bind(&req.toml_content)
        .bind(now)
        .bind(wf_id)
        .bind(&user.id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Failed to update workflow", Some(e.to_string())))?;

        if result.rows_affected() == 0 {
            return Err(ApiError {
                status: StatusCode::NOT_FOUND,
                body: ErrorResponse {
                    code: "NOT_FOUND".to_string(),
                    message: "Workflow not found or not owned by user".to_string(),
                    detail: None,
                    suggestion: None,
                },
            });
        }
        let _ = db::log_action(&user.id, "update_workflow", wf_id).await;
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({"id": wf_id, "status": "updated"})),
        ))
    } else {
        // Create new workflow
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO workflows (id, user_id, name, version, toml_content, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&req.name)
        .bind(&req.version)
        .bind(&req.toml_content)
        .bind(now)
        .bind(now)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Failed to save workflow", Some(e.to_string())))?;

        let _ = db::log_action(&user.id, "save_workflow", &req.name).await;
        Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({"id": id, "status": "saved"})),
        ))
    }
}

/// `GET /api/workflows/saved` — List all saved workflows for the user.
pub async fn list_saved_workflows(
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<SavedWorkflowResponse>>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let rows = sqlx::query_as::<_, (String, String, String, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, name, version, toml_content, created_at, updated_at FROM workflows WHERE user_id = ? ORDER BY updated_at DESC"
    )
    .bind(&user.id)
    .fetch_all(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let workflows = rows
        .into_iter()
        .map(|(id, name, version, toml, created, updated)| {
            let rules_count = oxo_flow_core::WorkflowConfig::parse(&toml)
                .map(|c| c.rules.len())
                .unwrap_or(0);
            SavedWorkflowResponse {
                id,
                name,
                version,
                rules_count,
                created_at: created.to_rfc3339(),
                updated_at: updated.to_rfc3339(),
            }
        })
        .collect();

    Ok(Json(workflows))
}

/// `GET /api/workflows/saved/{id}` — Get a single saved workflow by ID.
pub async fn get_saved_workflow(
    headers: axum::http::HeaderMap,
    axum::extract::Path(wf_id): axum::extract::Path<String>,
) -> Result<Json<SavedWorkflowDetail>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let row = sqlx::query_as::<_, (String, String, String, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, name, version, toml_content, created_at, updated_at FROM workflows WHERE id = ? AND user_id = ?"
    )
    .bind(&wf_id)
    .bind(&user.id)
    .fetch_optional(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
    .ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        body: ErrorResponse {
            code: "NOT_FOUND".to_string(),
            message: "Workflow not found".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let (id, name, version, toml, created, updated) = row;
    let rules_count = oxo_flow_core::WorkflowConfig::parse(&toml)
        .map(|c| c.rules.len())
        .unwrap_or(0);

    Ok(Json(SavedWorkflowDetail {
        id,
        name,
        version,
        toml_content: toml,
        rules_count,
        created_at: created.to_rfc3339(),
        updated_at: updated.to_rfc3339(),
    }))
}

/// `DELETE /api/workflows/saved/{id}` — Delete a saved workflow (owner only).
pub async fn delete_saved_workflow(
    headers: axum::http::HeaderMap,
    axum::extract::Path(wf_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let result = sqlx::query("DELETE FROM workflows WHERE id = ? AND user_id = ?")
        .bind(&wf_id)
        .bind(&user.id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    if result.rows_affected() == 0 {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Workflow not found".to_string(),
                detail: None,
                suggestion: None,
            },
        });
    }

    let _ = db::log_action(&user.id, "delete_workflow", &wf_id).await;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({"status": "deleted"})),
    ))
}
