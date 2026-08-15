//! Server-Sent Events (SSE) support for oxo-flow-web.
//!
//! Provides real-time event broadcasting for workflow execution updates.
//!
//! Multi-tenancy (issue #82 P0-5): events carry the owning `user_id` (or
//! `null` for system-wide events). In team/hpc modes the stream requires a
//! `?token=` session token (EventSource cannot set headers) and delivers
//! only the subscriber's own events plus userless ones; admins see all.

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{
    IntoResponse, Response,
    sse::{Event, KeepAlive, Sse},
};
use chrono::Utc;
use serde_json::Value;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::OnceLock;
use tokio::sync::broadcast;
use tokio_stream::StreamExt as _;

/// Broadcast channel for Server-Sent Events (SSE).
static EVENT_TX: OnceLock<broadcast::Sender<String>> = OnceLock::new();

/// Get or initialize the broadcast channel sender.
pub fn event_tx() -> broadcast::Sender<String> {
    EVENT_TX
        .get_or_init(|| {
            let (tx, _rx) = broadcast::channel(100);
            tx
        })
        .clone()
}

/// Send a system-wide SSE event to all connected clients (user = null).
///
/// The event is formatted as a JSON object with:
/// - `type`: The event type name
/// - `time`: ISO 8601 timestamp
/// - `user`: owning user id, or null for system-wide events
/// - `data`: The provided JSON data
pub fn broadcast_event(event_type: &str, data: &Value) {
    broadcast_event_for(event_type, data, None);
}

/// Send an SSE event scoped to one user's run (issue #82 P0-5).
pub fn broadcast_event_for(event_type: &str, data: &Value, user: Option<&str>) {
    let user_json = match user {
        Some(u) => serde_json::to_string(u).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };
    let msg = format!(
        r#"{{"type":"{}","time":"{}","user":{},"data":{}}}"#,
        event_type,
        Utc::now().to_rfc3339(),
        user_json,
        serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
    );
    let _ = event_tx().send(msg);
}

/// Validate a `?token=` session token; returns the acting user on success.
async fn validate_event_token(
    token: &str,
) -> Option<crate::domains::auth::current_user::CurrentUser> {
    let pool = crate::infra::db::sqlite::try_pool().ok()?;
    let now = chrono::Utc::now().to_rfc3339();
    let session = sqlx::query_as::<_, (String,)>(
        "SELECT user_id FROM sessions WHERE token = ? AND expires_at > ?",
    )
    .bind(token)
    .bind(&now)
    .fetch_optional(pool)
    .await
    .unwrap_or(None)?;
    let user_id = session.0;

    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT id, role FROM users WHERE id = ? OR username = ?",
    )
    .bind(&user_id)
    .bind(&user_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    match row {
        Some((id, role)) => Some(crate::domains::auth::current_user::CurrentUser { id, role }),
        None => Some(crate::domains::auth::current_user::CurrentUser {
            id: user_id.clone(),
            role: if user_id == "admin" {
                "admin".into()
            } else {
                "user".into()
            },
        }),
    }
}

#[utoipa::path(
    get,
    path = "/api/events",
    tag = "observability",
    params(("token" = Option<String>, Query, description = "Session token (team/hpc modes — EventSource cannot set Authorization headers)")),
    responses(
        (status = 200, description = "Success", content_type = "text/event-stream"),
        (status = 401, description = "Error", body = crate::domains::workflow::handlers::ApiError),
    )
)]
/// `GET /api/events` — SSE endpoint for real-time execution events.
///
/// Team/hpc modes require `?token=<session token>` (EventSource cannot set
/// an Authorization header). The stream is then filtered to the
/// subscriber's own events; admins receive everything.
pub async fn sse_events(Query(params): Query<HashMap<String, String>>) -> Response {
    let me = if crate::server::running_mode() == "personal" {
        None
    } else {
        match params.get("token").filter(|t| !t.is_empty()) {
            Some(token) => match validate_event_token(token).await {
                Some(user) => Some(user),
                None => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        axum::Json(serde_json::json!({
                            "code": "INVALID_TOKEN",
                            "message": "A valid ?token= session token is required for the event stream in team/hpc mode",
                        })),
                    )
                        .into_response();
                }
            },
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    axum::Json(serde_json::json!({
                        "code": "AUTH_REQUIRED",
                        "message": "?token= is required for the event stream in team/hpc mode",
                    })),
                )
                    .into_response();
            }
        }
    };

    let mut rx = event_tx().subscribe();

    // Stream that yields events from the broadcast channel, filtered by
    // ownership: userless events reach everyone; user-scoped events reach
    // their owner and admins only.
    let event_stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if let Some(me) = &me {
                        let user_ok = serde_json::from_str::<serde_json::Value>(&msg)
                            .map(|v| {
                                let u = v.get("user").and_then(|u| u.as_str());
                                u.is_none() || u == Some(me.id.as_str()) || me.is_admin()
                            })
                            .unwrap_or(false);
                        if !user_ok {
                            continue;
                        }
                    }
                    yield Ok::<_, Infallible>(Event::default().data(msg));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Skip lagged messages
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    // Heartbeat stream every 5 seconds
    let heartbeat_stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(5),
    ))
    .map(|_| {
        let msg = format!(
            r#"{{"type":"heartbeat","time":"{}"}}"#,
            Utc::now().to_rfc3339()
        );
        Ok::<_, Infallible>(Event::default().data(msg))
    });

    // Merge the streams
    let stream = tokio_stream::StreamExt::merge(event_stream, heartbeat_stream);

    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}
