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
/// Runs the REAL agent loop (oxo-flow-ai Orchestrator with the web tool
/// registry) and forwards its events as typed SSE:
/// `status` → `tool_call` → `tool_result` → `text` → `action` → `done` | `error`.
pub async fn chat_send(
    Json(req): Json<ChatRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let message = req.message.clone();
    let context = req.context.clone();
    let session_id = req
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let run = service::spawn_chat_agent(message, session_id.clone(), context, req.run_id);

    let stream = async_stream::stream! {
        let mut events = run.events;
        while let Some(event) = events.recv().await {
            match event {
                service::ChatStreamEvent::Agent(oxo_flow_ai::agent::events::AgentEvent::Status(msg)) => {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("status")
                        .data(serde_json::json!({"message": msg}).to_string()));
                }
                service::ChatStreamEvent::Agent(oxo_flow_ai::agent::events::AgentEvent::ToolCall { name, args }) => {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("tool_call")
                        .data(serde_json::json!({"name": name, "args": args}).to_string()));
                }
                service::ChatStreamEvent::Agent(oxo_flow_ai::agent::events::AgentEvent::ToolResult { name, summary }) => {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("tool_result")
                        .data(serde_json::json!({"name": name, "summary": summary}).to_string()));
                }
                service::ChatStreamEvent::Agent(oxo_flow_ai::agent::events::AgentEvent::Text(text)) => {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("text")
                        .data(serde_json::json!({"chunk": text}).to_string()));
                }
                service::ChatStreamEvent::Agent(other) => {
                    yield Ok::<_, Infallible>(Event::default()
                        .event("status")
                        .data(serde_json::json!({"message": format!("{other:?}")}).to_string()));
                }
                service::ChatStreamEvent::Outcome(boxed) => {
                    let outcome = match *boxed {
                        Ok(o) => o,
                        Err(e) => {
                            yield Ok::<_, Infallible>(Event::default()
                                .event("error")
                                .data(serde_json::json!({
                                    "code": "CHAT_ERROR",
                                    "message": e
                                }).to_string()));
                            continue;
                        }
                    };
                    if let Some(toml) = outcome.content.as_deref() {
                        let validation = crate::domains::workflow::service::validate_pipeline(toml)
                            .map(|v| serde_json::json!({
                                "valid": v.valid,
                                "errors": v.errors.iter().map(|e| serde_json::json!({
                                    "code": e.code, "message": e.message, "suggestion": e.suggestion
                                })).collect::<Vec<_>>()
                            }))
                            .unwrap_or(serde_json::json!({"valid": false, "errors": []}));
                        yield Ok::<_, Infallible>(Event::default()
                            .event("action")
                            .data(serde_json::json!({
                                "action_type": "pipeline_ready",
                                "data": {
                                    "toml_content": toml,
                                    "validation": validation,
                                }
                            }).to_string()));
                    }
                    yield Ok::<_, Infallible>(Event::default()
                        .event("done")
                        .data(serde_json::json!({
                            "session_id": session_id,
                            "rounds": outcome.rounds,
                        }).to_string()));
                }
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
pub async fn chat_send_json(Json(req): Json<ChatRequest>) -> ApiResult<serde_json::Value> {
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

    let session_id = req
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    match service::process_chat(
        &req.message,
        Some(&session_id),
        req.context.as_ref(),
        &templates,
    )
    .await
    {
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
