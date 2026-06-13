//! HTTP handlers for observability domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::ApiError;
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn err(s: StatusCode, c: &str, m: String) -> (StatusCode, Json<ApiError>) {
    (
        s,
        Json(ApiError {
            code: c.into(),
            message: m,
            detail: None,
            suggestion: None,
        }),
    )
}

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

/// GET /api/health
pub async fn health() -> ApiResult<HealthResponse> {
    let db_healthy = if let Ok(pool) = get_pool() {
        sqlx::query("SELECT 1").execute(pool).await.is_ok()
    } else {
        false
    };

    let mut health = service::health_check("personal", db_healthy);
    if !db_healthy {
        health.status = "degraded".to_string();
    }
    Ok(Json(health))
}

/// GET /api/system
pub async fn system_info() -> ApiResult<SystemInfoResponse> {
    Ok(Json(service::system_info()))
}

/// GET /api/metrics
pub async fn runtime_metrics() -> ApiResult<RuntimeMetricsResponse> {
    // Collect real system metrics via sysinfo
    let mut sys = sysinfo::System::new_all();
    sys.refresh_all();

    let total_memory_mb = sys.total_memory() / 1024;
    let used_memory_mb = sys.used_memory() / 1024;
    let total_swap_mb = sys.total_swap() / 1024;
    let used_swap_mb = sys.used_swap() / 1024;
    let cpu_usage = sys.global_cpu_usage();

    // Count active runs from DB
    let (active_workflows, total_requests) = if let Ok(pool) = get_pool() {
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE status IN ('running', 'queued')")
                .fetch_one(pool)
                .await
                .unwrap_or(0);
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        (active, total as u64)
    } else {
        (0, 0)
    };

    let uptime = sysinfo::System::uptime();

    Ok(Json(RuntimeMetricsResponse {
        uptime_secs: uptime,
        version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        cpu_count: num_cpus::get(),
        total_requests,
        active_workflows,
        host: HostResources {
            cpu_usage_percent: cpu_usage as f64,
            total_memory_mb,
            used_memory_mb,
            total_swap_mb,
            used_swap_mb,
        },
    }))
}

/// GET /api/events (SSE — Server-Sent Events)
pub async fn sse_events() -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    let events: Vec<serde_json::Value> = if let Ok(pool) = get_pool() {
        let rows: Vec<models::AuditLogRow> =
            sqlx::query_as("SELECT * FROM audit_logs ORDER BY timestamp DESC LIMIT 50")
                .fetch_all(pool)
                .await
                .unwrap_or_default();

        rows.into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "user_id": r.user_id,
                    "action": r.action,
                    "target": r.target,
                    "timestamp": r.timestamp,
                })
            })
            .collect()
    } else {
        vec![]
    };

    // Return as SSE formatted text
    let sse_body = events
        .iter()
        .map(|e| format!("data: {}\n", serde_json::to_string(e).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n");
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("connection", "keep-alive"),
        ],
        sse_body,
    )
}

/// GET /api/audit
pub async fn get_audit_logs(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult<AuditLogResponse> {
    let days: u8 = params.get("days").and_then(|d| d.parse().ok()).unwrap_or(7);

    let entries: Vec<AuditEntry> = if let Ok(pool) = get_pool() {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        let rows: Vec<models::AuditLogRow> = sqlx::query_as(
            "SELECT * FROM audit_logs WHERE timestamp >= ? ORDER BY timestamp DESC LIMIT 500",
        )
        .bind(&cutoff)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|r| AuditEntry {
                timestamp: r.timestamp,
                user: r.user_id,
                action: r.action,
                resource: r.target,
            })
            .collect()
    } else {
        vec![]
    };

    Ok(Json(AuditLogResponse { entries, days }))
}
