//! HTTP handlers for Chat domain — SSE streaming conversational AI.
//!
//! Thin adapters: parse HTTP request → call service → stream SSE response.

use axum::Extension;
use axum::Json;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use std::convert::Infallible;

use super::service;
use super::types::*;
use crate::domains::auth::current_user::{CurrentUser, resolve};
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

/// Server-side chat persistence (issue #81): upsert the session row and
/// append the user's message. Best-effort — chat works without a DB.
async fn persist_user_message(
    pool: Option<&sqlx::SqlitePool>,
    user: &CurrentUser,
    session_id: &str,
    message: &str,
) {
    let Some(pool) = pool else { return };
    let now = chrono::Utc::now().to_rfc3339();
    let title: String = message.chars().take(60).collect();
    let _ = sqlx::query(
        "INSERT INTO chat_sessions (id, user_id, title, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, updated_at = excluded.updated_at",
    )
    .bind(session_id)
    .bind(&user.id)
    .bind(&title)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "INSERT INTO chat_messages (id, session_id, role, content, meta, created_at) \
         VALUES (?, ?, 'user', ?, NULL, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(session_id)
    .bind(message)
    .bind(&now)
    .execute(pool)
    .await;
}

/// Append the assistant's final answer to the session.
async fn persist_assistant_message(
    pool: Option<&sqlx::SqlitePool>,
    session_id: &str,
    content: &str,
) {
    let Some(pool) = pool else { return };
    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "INSERT INTO chat_messages (id, session_id, role, content, meta, created_at) \
         VALUES (?, ?, 'assistant', ?, NULL, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(session_id)
    .bind(content)
    .bind(&now)
    .execute(pool)
    .await;
    let _ = sqlx::query("UPDATE chat_sessions SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(session_id)
        .execute(pool)
        .await;
}

#[utoipa::path(
    post,
    path = "/api/chat/send",
    tag = "chat",
    responses(
        (status = 200, description = "Success", content_type = "text/event-stream"),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/chat/send — SSE streaming chat endpoint.
///
/// Runs the REAL agent loop (oxo-flow-ai Orchestrator with the web tool
/// registry) and forwards its events as typed SSE:
/// `status` → `tool_call` → `tool_result` → `text` → `action` → `done` | `error`.
pub async fn chat_send(
    authenticated: Option<Extension<CurrentUser>>,
    Json(req): Json<ChatRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let user = resolve(authenticated.as_ref());
    let message = req.message.clone();
    let context = req.context.clone();
    let session_id = req
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Server-side persistence (issue #81): the session + user message are
    // recorded up front; the assistant's answer is appended on completion.
    let pool = crate::infra::db::sqlite::try_pool().ok();
    persist_user_message(pool, &user, &session_id, &message).await;

    // Chat runs on the acting user's own AI provider (isolation fix).
    let provider = crate::ai_provider::provider_for(&user.id).await;
    let run = service::spawn_chat_agent(message, session_id.clone(), context, req.run_id, provider);

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
                        let validation = crate::domains::workflow::service::validate_pipeline(toml, None)
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
                    if let Some(content) = outcome.content.as_deref() {
                        persist_assistant_message(pool, &session_id, content).await;
                    }
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

#[utoipa::path(
    post,
    path = "/api/chat/send/json",
    tag = "chat",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/chat/send/json — non-streaming JSON response.
pub async fn chat_send_json(
    authenticated: Option<Extension<CurrentUser>>,
    Json(req): Json<ChatRequest>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
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

    let pool = crate::infra::db::sqlite::try_pool().ok();
    persist_user_message(pool, &user, &session_id, &req.message).await;

    let provider = crate::ai_provider::provider_for(&user.id).await;
    match service::process_chat(
        &req.message,
        Some(&session_id),
        req.context.as_ref(),
        &templates,
        &provider,
    )
    .await
    {
        Ok((text, data)) => {
            persist_assistant_message(pool, &session_id, &text).await;
            Ok(Json(data))
        }
        Err(e) => Err(err(StatusCode::BAD_REQUEST, "CHAT_ERROR", e)),
    }
}

#[utoipa::path(
    get,
    path = "/api/chat/sessions",
    tag = "chat",
    responses(
        (status = 200, description = "Success", body = Vec<ChatSession>),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/chat/sessions — list the acting user's chat sessions
/// (issue #81: server-side persistence + per-user scoping).
pub async fn list_sessions(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<Vec<ChatSession>> {
    let user = resolve(authenticated.as_ref());
    let sessions = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::ChatSessionRow>(
            "SELECT * FROM chat_sessions WHERE user_id = ? ORDER BY updated_at DESC LIMIT 20",
        )
        .bind(&user.id)
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
