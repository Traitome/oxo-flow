//! System handlers.
//!
//! Handles health checks, version info, system metrics, environment listing,
//! and Server-Sent Events (SSE) for real-time updates.

use axum::{extract::Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

use crate::{
    ACTIVE_WORKFLOWS, HealthResponse, RuntimeMetrics, SystemInfo, TOTAL_REQUESTS, VersionResponse,
    event_tx, get_start_time, sys,
};

/// Environment backend info.
#[derive(Serialize, Deserialize)]
pub struct EnvInfo {
    pub available: Vec<String>,
}

/// `GET /api/health` — Health check endpoint.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// `GET /api/version` — Return crate version and build info.
pub async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        crate_name: env!("CARGO_PKG_NAME").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// `GET /api/system` — Return system information.
pub async fn system_info() -> Json<SystemInfo> {
    let uptime = get_start_time().elapsed().as_secs_f64();
    Json(SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        pid: std::process::id(),
        uptime_secs: uptime,
    })
}

/// `GET /api/metrics` — Runtime metrics for monitoring and observability.
pub async fn runtime_metrics() -> Json<RuntimeMetrics> {
    let resources = sys::get_host_resources();
    let uptime = get_start_time().elapsed().as_secs_f64();
    Json(RuntimeMetrics {
        uptime_secs: uptime,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count: std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1),
        total_requests: TOTAL_REQUESTS.load(Ordering::Relaxed),
        active_workflows: ACTIVE_WORKFLOWS.load(Ordering::Relaxed),
        host: resources,
    })
}

/// `GET /api/environments` — List available environment backends.
pub async fn list_environments() -> Json<EnvInfo> {
    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
    Json(EnvInfo {
        available: resolver
            .available_backends()
            .into_iter()
            .map(String::from)
            .collect(),
    })
}

/// `GET /api/events` — SSE endpoint for real-time execution events.
pub async fn sse_events() -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use tokio_stream::StreamExt as _;

    let mut rx = event_tx().subscribe();

    // Stream that yields events from the broadcast channel
    let event_stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    yield Ok::<_, std::convert::Infallible>(Event::default().data(msg));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Skip lagged messages
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
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
            chrono::Utc::now().to_rfc3339()
        );
        Ok::<_, std::convert::Infallible>(Event::default().data(msg))
    });

    // Merge the streams
    let stream = tokio_stream::StreamExt::merge(event_stream, heartbeat_stream);

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}
