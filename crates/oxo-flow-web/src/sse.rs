//! Server-Sent Events (SSE) support for oxo-flow-web.
//!
//! Provides real-time event broadcasting for workflow execution updates.

use axum::response::{
    IntoResponse,
    sse::{Event, KeepAlive, Sse},
};
use chrono::Utc;
use serde_json::Value;
use std::sync::OnceLock;
use tokio::sync::broadcast;

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

/// Send an SSE event to all connected clients.
///
/// The event is formatted as a JSON object with:
/// - `type`: The event type name
/// - `time`: ISO 8601 timestamp
/// - `data`: The provided JSON data
pub fn broadcast_event(event_type: &str, data: &Value) {
    let msg = format!(
        r#"{{"type":"{}","time":"{}","data":{}}}"#,
        event_type,
        Utc::now().to_rfc3339(),
        serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
    );
    let _ = event_tx().send(msg);
}

/// `GET /api/events` — SSE endpoint for real-time execution events.
pub async fn sse_events() -> impl IntoResponse {
    use tokio_stream::StreamExt as _;

    let mut rx = event_tx().subscribe();

    // Stream that yields events from the broadcast channel
    let event_stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    yield Ok::<_, std::convert::Infallible>(Event::default().data(msg));
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
        Ok::<_, std::convert::Infallible>(Event::default().data(msg))
    });

    // Merge the streams
    let stream = tokio_stream::StreamExt::merge(event_stream, heartbeat_stream);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}
