//! Template library handlers.
//!
//! Handles template CRUD operations for the workflow template library.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, ErrorResponse, db, extract_session};

/// Request body for creating/updating a template.
#[derive(Debug, Serialize, Deserialize)]
pub struct SaveTemplateRequest {
    pub name: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: String,
    pub toml_content: String,
    /// Optional template ID for updates.
    pub id: Option<String>,
}

fn default_category() -> String {
    "general".to_string()
}

/// Template response.
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tags: String,
    pub is_system: bool,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /api/templates` — List all templates.
pub async fn list_templates() -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    let rows: Vec<(String, String, String, String, String, String, i64, Option<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, name, category, description, tags, toml_content, is_system, created_by, created_at, updated_at FROM templates ORDER BY category, name",
        )
        .fetch_all(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let templates: Vec<TemplateResponse> = rows
        .into_iter()
        .map(
            |(
                id,
                name,
                category,
                description,
                tags,
                _toml,
                is_system,
                created_by,
                created_at,
                updated_at,
            )| {
                TemplateResponse {
                    id,
                    name,
                    category,
                    description,
                    tags,
                    is_system: is_system != 0,
                    created_by,
                    created_at: created_at.to_rfc3339(),
                    updated_at: updated_at.to_rfc3339(),
                }
            },
        )
        .collect();

    Ok(Json(templates))
}

/// `GET /api/templates/{id}` — Get a single template with full TOML content.
pub async fn get_template(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<(String, String, String, String, String, String, i64, Option<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, name, category, description, tags, toml_content, is_system, created_by, created_at, updated_at FROM templates WHERE id = ?",
        )
        .bind(&id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    match row {
        Some((
            id,
            name,
            category,
            description,
            tags,
            toml,
            is_system,
            created_by,
            created_at,
            updated_at,
        )) => Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "category": category,
            "description": description,
            "tags": tags,
            "toml_content": toml,
            "is_system": is_system != 0,
            "created_by": created_by,
            "created_at": created_at.to_rfc3339(),
            "updated_at": updated_at.to_rfc3339(),
        }))),
        None => Err(ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                error: "Template not found".to_string(),
                detail: None,
            },
        }),
    }
}

/// `POST /api/templates` — Create or update a template.
pub async fn save_template(
    headers: axum::http::HeaderMap,
    Json(req): Json<SaveTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
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

    let now = chrono::Utc::now();

    if let Some(ref existing_id) = req.id {
        // Update existing template
        // Check it's not a system template
        let is_sys: Option<(i64,)> = sqlx::query_as("SELECT is_system FROM templates WHERE id = ?")
            .bind(existing_id)
            .fetch_optional(db::pool())
            .await
            .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

        match is_sys {
            Some((1,)) => {
                return Err(ApiError::bad_request(
                    "Cannot modify system templates",
                    None,
                ));
            }
            Some(_) => {
                sqlx::query(
                    "UPDATE templates SET name=?, category=?, description=?, tags=?, toml_content=?, updated_at=? WHERE id=?",
                )
                .bind(&req.name)
                .bind(&req.category)
                .bind(&req.description)
                .bind(&req.tags)
                .bind(&req.toml_content)
                .bind(now)
                .bind(existing_id)
                .execute(db::pool())
                .await
                .map_err(|e| ApiError::bad_request("Update failed", Some(e.to_string())))?;

                let _ = db::log_action(&user.id, "update_template", existing_id).await;
                Ok((
                    StatusCode::OK,
                    Json(serde_json::json!({"id": existing_id, "status": "updated"})),
                ))
            }
            None => Err(ApiError {
                status: StatusCode::NOT_FOUND,
                body: ErrorResponse {
                    error: "Template not found".to_string(),
                    detail: None,
                },
            }),
        }
    } else {
        // Create new template
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO templates (id, name, category, description, tags, toml_content, is_system, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&req.name)
        .bind(&req.category)
        .bind(&req.description)
        .bind(&req.tags)
        .bind(&req.toml_content)
        .bind(&user.id)
        .bind(now)
        .bind(now)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Create failed", Some(e.to_string())))?;

        let _ = db::log_action(&user.id, "create_template", &req.name).await;
        Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({"id": id, "name": req.name, "status": "created"})),
        ))
    }
}

/// `DELETE /api/templates/{id}` — Delete a template (system templates protected).
pub async fn delete_template(
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
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

    // Prevent deleting system templates
    let is_sys: Option<(i64,)> = sqlx::query_as("SELECT is_system FROM templates WHERE id = ?")
        .bind(&id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    match is_sys {
        Some((1,)) => {
            return Err(ApiError::bad_request(
                "Cannot delete system templates",
                None,
            ));
        }
        None => {
            return Err(ApiError {
                status: StatusCode::NOT_FOUND,
                body: ErrorResponse {
                    error: "Template not found".to_string(),
                    detail: None,
                },
            });
        }
        _ => {}
    }

    sqlx::query("DELETE FROM templates WHERE id = ?")
        .bind(&id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Delete failed", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "delete_template", &id).await;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({"status": "deleted"})),
    ))
}
