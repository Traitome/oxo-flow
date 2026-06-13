//! HTTP handlers for Chat domain — SSE streaming conversational AI.
//!
//! Thin adapters: parse HTTP request → call service → stream SSE response.

use axum::Json;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use std::convert::Infallible;

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::{ApiError, err};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

/// POST /api/chat/send — SSE streaming chat endpoint.
///
/// Sends a message to the AI companion and streams the response as SSE events.
/// Events: agent (status updates) → text (response chunks) → action (structured actions) → done (completion).
pub async fn chat_send(
    Json(req): Json<ChatRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    // Load templates
    let templates: Vec<String> = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY usage_count DESC LIMIT 20",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
    } else {
        vec![]
    };

    let message = req.message.clone();
    let context = req.context.clone();
    let session_id = req.session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let stream = async_stream::stream! {
        // Event: processing started
        yield Ok::<_, Infallible>(Event::default()
            .event("agent")
            .data(serde_json::json!({
                "agent": "Orchestrator",
                "status": "Understanding your intent...",
                "progress": 0.1
            }).to_string()));

        let intent = if let Some(ref ctx) = context {
            if let Some(ref i) = ctx.intent { i.clone() } else {
                service::infer_intent(&message)
            }
        } else {
            service::infer_intent(&message)
        };

        yield Ok::<_, Infallible>(Event::default()
            .event("agent")
            .data(serde_json::json!({
                "agent": "Orchestrator",
                "status": format!("Detected intent: {intent}"),
                "progress": 0.2
            }).to_string()));

        // Data Agent if paths provided
        if let Some(ref ctx) = context
            && let Some(ref paths) = ctx.data_paths
            && !paths.is_empty()
        {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("agent")
                        .data(serde_json::json!({
                            "agent": "DataAgent",
                            "status": "Scanning data files...",
                            "progress": 0.3
                        }).to_string()));

                    let data_report = service::analyze_data_paths(paths);
                    if let Some(ref report) = data_report {
                        yield Ok::<_, Infallible>(Event::default()
                            .event("action")
                            .data(serde_json::json!({
                                "action_type": "data_report",
                                "data": report
                            }).to_string()));
                    }
        }

        // Generate pipeline
        yield Ok::<_, Infallible>(Event::default()
            .event("agent")
            .data(serde_json::json!({
                "agent": "ToolExpert",
                "status": "Generating pipeline...",
                "progress": 0.5
            }).to_string()));

        let result = service::process_chat(&message, Some(&session_id), context.as_ref(), &templates).await;

        match result {
            Ok((ai_text, pipeline_data)) => {
                // Stream the AI response text in chunks
                let chunks: Vec<&str> = ai_text.split_inclusive(['.', '\n']).collect();
                for chunk in chunks {
                    if !chunk.trim().is_empty() {
                        yield Ok::<_, Infallible>(Event::default()
                            .event("text")
                            .data(serde_json::json!({"chunk": chunk}).to_string()));
                    }
                }

                // Validate
                yield Ok::<_, Infallible>(Event::default()
                    .event("agent")
                    .data(serde_json::json!({
                        "agent": "ValidatorAgent",
                        "status": "Validating pipeline...",
                        "progress": 0.85
                    }).to_string()));

                let valid = pipeline_data["validation"]["valid"].as_bool().unwrap_or(false);
                yield Ok::<_, Infallible>(Event::default()
                    .event("agent")
                    .data(serde_json::json!({
                        "agent": "ValidatorAgent",
                        "status": if valid { "Pipeline valid ✓" } else { "Pipeline has warnings" },
                        "progress": 0.9
                    }).to_string()));

                // Send the pipeline action
                yield Ok::<_, Infallible>(Event::default()
                    .event("action")
                    .data(serde_json::json!({
                        "action_type": "pipeline_ready",
                        "data": pipeline_data
                    }).to_string()));

                // Done
                yield Ok::<_, Infallible>(Event::default()
                    .event("done")
                    .data(serde_json::json!({
                        "session_id": session_id,
                        "pipeline_id": pipeline_data["pipeline_id"]
                    }).to_string()));
            }
            Err(e) => {
                yield Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({
                        "code": "CHAT_ERROR",
                        "message": e
                    }).to_string()));
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// POST /api/chat/send/json — non-streaming JSON response.
pub async fn chat_send_json(
    Json(req): Json<ChatRequest>,
) -> ApiResult<serde_json::Value> {
    let templates: Vec<String> = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY usage_count DESC LIMIT 20",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
    } else {
        vec![]
    };

    let session_id = req.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    match service::process_chat(&req.message, Some(&session_id), req.context.as_ref(), &templates).await {
        Ok((_text, data)) => Ok(Json(data)),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, "CHAT_ERROR", e)),
    }
}

/// GET /api/chat/sessions — list chat sessions.
pub async fn list_sessions() -> ApiResult<Vec<ChatSession>> {
    let sessions = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::ChatSessionRow>(
            "SELECT * FROM chat_sessions ORDER BY updated_at DESC LIMIT 20",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| ChatSession {
            id: r.id,
            title: r.title,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect()
    } else {
        vec![]
    };
    Ok(Json(sessions))
}
