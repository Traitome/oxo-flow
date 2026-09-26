//! Server-Sent Events (SSE) support for oxo-flow-web.
//!
//! Provides real-time event broadcasting for workflow execution updates.
//!
//! Multi-tenancy (issue #82 P0-5): events carry the owning `user_id` (or
//! `null` for system-wide events). In team/hpc modes the stream requires a
//! one-time `?ticket=` (#522 — previously the long-lived session token
//! traveled as `?token=`, landing in proxy logs and browser history; the
//! ticket is issued at `POST /api/events/ticket`, expires in seconds and
//! is consumed on first use). The stream delivers only the subscriber's
//! own events plus userless ones; admins see all.

use crate::extract::ApiQuery;
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

/// One broadcast event: the owning user id (`None` = system-wide) travels
/// alongside the pre-serialized JSON payload, so subscribers filter by a
/// plain string compare instead of re-parsing every message.
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub user: Option<String>,
    pub payload: String,
}

/// Broadcast channel for Server-Sent Events (SSE).
static EVENT_TX: OnceLock<broadcast::Sender<SseEvent>> = OnceLock::new();

/// One-time SSE connection tickets: ticket -> (expiry, user_id).
///
/// EventSource cannot set an Authorization header, so the handshake needs
/// a URL credential — but a long-lived session token in a URL leaks into
/// proxy logs, browser history and Referers (#522). The ticket is short-
/// lived, single-use, and bound to the canonical user id at issuance.
static EVENT_TICKETS: OnceLock<std::sync::Mutex<HashMap<String, (std::time::Instant, String)>>> =
    OnceLock::new();

/// Ticket time-to-live: long enough for the SPA to open the stream right
/// after issuance, short enough that a leaked URL credential dies fast.
const TICKET_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Issue a one-time SSE ticket for the acting user (#522).
pub fn issue_ticket(user_id: &str) -> String {
    let store = EVENT_TICKETS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    // 128 bits of randomness: tickets are bearer credentials for one
    // handshake and must not be guessable.
    use rand::RngExt;
    let mut rng = rand::rng();
    let ticket: String = (0..32)
        .map(|_| format!("{:02x}", rng.random_range(0..=255u8)))
        .collect();
    let mut guard = store.lock().unwrap();
    // Opportunistic pruning keeps the map bounded even if tickets are
    // never consumed.
    guard.retain(|_, (exp, _)| *exp > std::time::Instant::now());
    guard.insert(
        ticket.clone(),
        (std::time::Instant::now() + TICKET_TTL, user_id.to_string()),
    );
    ticket
}

/// Consume a one-time SSE ticket, returning its bound user id.
///
/// Single use: the entry is removed before validation so a replay — even
/// a concurrent one — finds nothing.
fn consume_ticket(ticket: &str) -> Option<String> {
    let store = EVENT_TICKETS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let (exp, user_id) = store.lock().unwrap().remove(ticket)?;
    if exp > std::time::Instant::now() {
        Some(user_id)
    } else {
        None
    }
}

/// POST /api/events/ticket — mint a one-time SSE connection ticket.
///
/// Requires normal bearer authentication (the route is NOT on the
/// public-path whitelist); the returned ticket then authenticates exactly
/// one `GET /api/events?ticket=` handshake.
pub async fn events_ticket(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
) -> Response {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    if crate::server::running_mode() != "personal" && user.id == "default" {
        // resolve() falls back to the `default` pseudo-user when no auth
        // extension is present — a real session always carries an
        // authenticated id in team/hpc mode.
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            axum::Json(serde_json::json!({
                "code": "AUTH_REQUIRED",
                "message": "Authentication required to mint an event-stream ticket",
            })),
        )
            .into_response();
    }
    let ticket = issue_ticket(&user.id);
    (
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::Json(serde_json::json!({
            "ticket": ticket,
            "expires_in_secs": TICKET_TTL.as_secs(),
        })),
    )
        .into_response()
}

/// Get or initialize the broadcast channel sender.
pub fn event_tx() -> broadcast::Sender<SseEvent> {
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
    let payload = format!(
        r#"{{"type":"{}","time":"{}","user":{},"data":{}}}"#,
        event_type,
        Utc::now().to_rfc3339(),
        user_json,
        serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
    );
    let _ = event_tx().send(SseEvent {
        user: user.map(String::from),
        payload,
    });
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
/// Team/hpc modes require a one-time `?ticket=` minted at
/// `POST /api/events/ticket` (#522 — EventSource cannot set an
/// Authorization header, and a session token in the URL leaks into proxy
/// logs). The stream is then filtered to the subscriber's own events;
/// admins receive everything.
pub async fn sse_events(ApiQuery(params): ApiQuery<HashMap<String, String>>) -> Response {
    let me = if crate::server::running_mode() == "personal" {
        None
    } else {
        let user_id = params
            .get("ticket")
            .map(String::as_str)
            .filter(|t| !t.is_empty())
            .and_then(consume_ticket);
        let Some(user_id) = user_id else {
            return (
                StatusCode::UNAUTHORIZED,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                axum::Json(serde_json::json!({
                    "code": "INVALID_TOKEN",
                    "message": "A fresh one-time ?ticket= from POST /api/events/ticket is required for the event stream in team/hpc mode",
                })),
            )
                .into_response();
        };
        // Resolve role from the canonical row; fail closed when the user
        // has vanished since the ticket was minted (#516 policy).
        let role = match crate::infra::db::sqlite::try_pool() {
            Ok(pool) => sqlx::query_as::<_, (String,)>("SELECT role FROM users WHERE id = ?")
                .bind(&user_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .map(|r| r.0),
            Err(_) => None,
        };
        let Some(role) = role else {
            return (
                StatusCode::UNAUTHORIZED,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                axum::Json(serde_json::json!({
                    "code": "INVALID_TOKEN",
                    "message": "Ticket user no longer exists",
                })),
            )
                .into_response();
        };
        Some(crate::domains::auth::current_user::CurrentUser { id: user_id, role })
    };

    let mut rx = event_tx().subscribe();

    // Stream that yields events from the broadcast channel, filtered by
    // ownership: userless events reach everyone; user-scoped events reach
    // their owner and admins only. The owner id travels beside the payload,
    // so the filter is a string compare — no per-subscriber JSON re-parse.
    let event_stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(me) = &me {
                        let user_ok = event.user.is_none()
                            || event.user.as_deref() == Some(me.id.as_str())
                            || me.is_admin();
                        if !user_ok {
                            continue;
                        }
                    }
                    yield Ok::<_, Infallible>(Event::default().data(event.payload));
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    // The client fell behind the 100-slot ring buffer and
                    // silently lost events. Emit a synthetic marker so the
                    // frontend can refetch/invalidate instead of missing
                    // run state transitions.
                    yield Ok::<_, Infallible>(Event::default().data(format!(
                        r#"{{"type":"lagged","time":"{}","data":{{"missed":{missed}}}}}"#,
                        Utc::now().to_rfc3339()
                    )));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    // Keepalive comes from axum's KeepAlive alone (15s comment ping) — a
    // second merged heartbeat stream would double the traffic.
    Sse::new(event_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_carries_owner_and_system_events_in_order() {
        // Both assertions share ONE subscription: the process-global
        // broadcast channel is shared across tests in a binary, so two
        // separate tests subscribing concurrently can drain each other's
        // events (verified root cause of the flake, issue #268 item 2.2).
        // A single receiver observing both broadcasts in FIFO order is
        // deterministic.
        let mut rx = event_tx().subscribe();
        broadcast_event_for(
            "run_started",
            &serde_json::json!({"run_id": "r1"}),
            Some("alice"),
        );
        let event = rx.recv().await.expect("event arrives");
        assert_eq!(event.user.as_deref(), Some("alice"));
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert_eq!(payload["type"], "run_started");
        assert_eq!(payload["user"], "alice");
        assert_eq!(payload["data"]["run_id"], "r1");

        broadcast_event("engine_ready", &serde_json::json!({}));
        let event = rx.recv().await.expect("event arrives");
        assert!(event.user.is_none());
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert!(payload["user"].is_null());
    }
}
