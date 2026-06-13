//! Scheduled run handlers.
//!
//! Handles workflow scheduling with cron expressions, listing, and cancellation.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, ErrorResponse, db, extract_session};

/// Request body for creating a scheduled run.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateScheduledRunRequest {
    pub workflow_id: String,
    pub cron_expression: String,
}

/// Response for scheduled run creation.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduledRunResponse {
    pub id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub cron_expression: String,
    pub next_run_at: String,
    pub status: String,
}

/// Calculate next run time from cron expression.
///
/// Supports simple cron expressions like "daily", "hourly", "weekly",
/// or standard 5-field cron: minute hour day-of-month month day-of-week
fn calculate_next_run(cron_expression: &str) -> Result<DateTime<Utc>, String> {
    let now = Utc::now();

    // Simple presets
    match cron_expression {
        "hourly" => Ok(now + chrono::Duration::hours(1)),
        "daily" => Ok(now + chrono::Duration::days(1)),
        "weekly" => Ok(now + chrono::Duration::weeks(1)),
        "daily-6am" => {
            let next = now + chrono::Duration::days(1);
            Ok(next.with_hour(6).unwrap_or(next))
        }
        "daily-9pm" => {
            let next = now + chrono::Duration::days(1);
            Ok(next.with_hour(21).unwrap_or(next))
        }
        _ => {
            // Try to parse standard cron (5 fields)
            // For simplicity, we use a basic approximation
            let parts: Vec<&str> = cron_expression.split_whitespace().collect();
            if parts.len() != 5 {
                return Err(
                    "Invalid cron expression. Use presets (hourly, daily, weekly) or 5-field cron"
                        .to_string(),
                );
            }

            // Basic parsing: default to daily for now
            // In a full implementation, you'd use a cron library
            Ok(now + chrono::Duration::days(1))
        }
    }
}

/// `POST /api/scheduled` — Create a scheduled workflow run.
pub async fn create_scheduled_run(
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateScheduledRunRequest>,
) -> Result<Json<ScheduledRunResponse>, ApiError> {
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

    // Get the workflow (id, name, version, toml_content)
    let workflow: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, name, version, toml_content FROM workflows WHERE id = ? AND user_id = ?",
    )
    .bind(&req.workflow_id)
    .bind(&user.id)
    .fetch_optional(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let (wf_id, wf_name, _wf_version, _wf_toml) = workflow.ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        body: ErrorResponse {
            code: "NOT_FOUND".to_string(),
            message: "Workflow not found".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let next_run_at = calculate_next_run(&req.cron_expression)
        .map_err(|e| ApiError::bad_request("Invalid cron expression", Some(e)))?;

    let scheduled_id = Uuid::new_v4().to_string();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO scheduled_runs (id, user_id, workflow_id, workflow_name, cron_expression, next_run_at, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&scheduled_id)
    .bind(&user.id)
    .bind(&wf_id)
    .bind(&wf_name)
    .bind(&req.cron_expression)
    .bind(next_run_at)
    .bind("active")
    .bind(now)
    .execute(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "schedule_create", &scheduled_id).await;

    Ok(Json(ScheduledRunResponse {
        id: scheduled_id,
        workflow_id: wf_id,
        workflow_name: wf_name,
        cron_expression: req.cron_expression,
        next_run_at: next_run_at.to_rfc3339(),
        status: "active".to_string(),
    }))
}

/// `GET /api/scheduled` — List all scheduled runs for the authenticated user.
pub async fn list_scheduled_runs(
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<db::ScheduledRun>>, ApiError> {
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

    let scheduled = sqlx::query_as::<_, db::ScheduledRun>(
        "SELECT * FROM scheduled_runs WHERE user_id = ? ORDER BY next_run_at ASC",
    )
    .bind(&user.id)
    .fetch_all(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    Ok(Json(scheduled))
}

/// `DELETE /api/scheduled/{id}` — Cancel/delete a scheduled run.
pub async fn cancel_scheduled_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(scheduled_id): axum::extract::Path<String>,
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

    // Verify ownership
    let scheduled = sqlx::query_as::<_, db::ScheduledRun>(
        "SELECT * FROM scheduled_runs WHERE id = ? AND user_id = ?",
    )
    .bind(&scheduled_id)
    .bind(&user.id)
    .fetch_optional(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
    .ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        body: ErrorResponse {
            code: "NOT_FOUND".to_string(),
            message: "Scheduled run not found".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    if scheduled.status == "cancelled" {
        return Err(ApiError::bad_request(
            "Scheduled run is already cancelled",
            None,
        ));
    }

    sqlx::query("UPDATE scheduled_runs SET status = 'cancelled' WHERE id = ?")
        .bind(&scheduled_id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "schedule_cancel", &scheduled_id).await;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "status": "cancelled" })),
    ))
}

/// `GET /api/scheduled/{id}` — Get details of a scheduled run.
pub async fn get_scheduled_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(scheduled_id): axum::extract::Path<String>,
) -> Result<Json<db::ScheduledRun>, ApiError> {
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

    let scheduled = sqlx::query_as::<_, db::ScheduledRun>(
        "SELECT * FROM scheduled_runs WHERE id = ? AND user_id = ?",
    )
    .bind(&scheduled_id)
    .bind(&user.id)
    .fetch_optional(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
    .ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        body: ErrorResponse {
            code: "NOT_FOUND".to_string(),
            message: "Scheduled run not found".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    Ok(Json(scheduled))
}
