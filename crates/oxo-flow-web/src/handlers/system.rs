//! System handlers.
//!
//! Handles health checks, version info, system metrics, environment listing,
//! Server-Sent Events (SSE) for real-time updates, and audit logs.

use axum::{extract::Json, extract::Query, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

use crate::{
    ACTIVE_WORKFLOWS, HealthResponse, RuntimeMetrics, SystemInfo, TOTAL_REQUESTS, VersionResponse,
    audit, event_tx, get_start_time, hpc, sys,
};

/// Environment backend info.
#[derive(Serialize, Deserialize)]
pub struct EnvInfo {
    pub available: Vec<String>,
}

/// Query parameters for audit log requests.
#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    /// Number of days to look back (1-30, default 7).
    #[serde(default = "default_audit_days")]
    pub days: u8,
}

fn default_audit_days() -> u8 {
    7
}

/// Response from the audit log endpoint.
#[derive(Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub entries: Vec<audit::AuditEntry>,
    pub days: u8,
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

/// `GET /api/audit` — Audit log viewer for enterprise governance.
///
/// Returns audit log entries from the last `days` days.
pub async fn get_audit_logs(Query(query): Query<AuditLogQuery>) -> Json<AuditLogResponse> {
    let days = query.days.clamp(1, 30);

    // Get raw JSON lines and parse them into entries
    let raw_lines = audit::get_recent_audit_logs(days).unwrap_or_default();
    let entries: Vec<audit::AuditEntry> = raw_lines
        .into_iter()
        .filter_map(|line| serde_json::from_str::<audit::AuditEntry>(&line).ok())
        .collect();

    Json(AuditLogResponse { entries, days })
}

/// `GET /api/hpc` — HPC scheduler status for cluster monitoring.
///
/// Detects the local scheduler and returns queue, node, and job status.
pub async fn hpc_status() -> Json<crate::hpc::HpcStatus> {
    Json(hpc::get_hpc_status())
}
