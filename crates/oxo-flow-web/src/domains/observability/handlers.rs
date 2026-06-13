//! HTTP handlers for observability domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// GET /api/health
pub async fn health() -> ApiResult<HealthResponse> {
    Ok(Json(service::health_check("personal", true)))
}

/// GET /api/system
pub async fn system_info() -> ApiResult<SystemInfoResponse> {
    Ok(Json(service::system_info()))
}

/// GET /api/metrics
pub async fn runtime_metrics() -> ApiResult<RuntimeMetricsResponse> {
    Ok(Json(RuntimeMetricsResponse {
        uptime_secs: 0,
        version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        cpu_count: num_cpus::get(),
        total_requests: 0,
        active_workflows: 0,
        host: HostResources {
            cpu_usage_percent: 0.0,
            total_memory_mb: 0,
            used_memory_mb: 0,
            total_swap_mb: 0,
            used_swap_mb: 0,
        },
    }))
}

/// GET /api/events (SSE)
pub async fn sse_events() -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({"events": []})))
}

/// GET /api/audit
pub async fn get_audit_logs(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult<AuditLogResponse> {
    let days: u8 = params.get("days").and_then(|d| d.parse().ok()).unwrap_or(7);
    Ok(Json(AuditLogResponse {
        entries: vec![],
        days,
    }))
}
