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

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = HealthResponse),
        (status = 500, description = "Error", body = ApiError),
    )
)]
/// GET /api/health
pub async fn health() -> ApiResult<HealthResponse> {
    let db_healthy = if let Ok(pool) = get_pool() {
        sqlx::query("SELECT 1").execute(pool).await.is_ok()
    } else {
        false
    };

    // The mode must come from the running server, not a hardcoded value —
    // issue #79 (health reported "personal" on hpc/team deployments, one
    // more piece of the status-consistency collapse).
    let mut health = service::health_check(crate::server::running_mode(), db_healthy);
    if !db_healthy {
        health.status = "degraded".to_string();
    }
    Ok(Json(health))
}

#[utoipa::path(
    get,
    path = "/api/system",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = SystemInfoResponse),
        (status = 500, description = "Error", body = ApiError),
    )
)]
/// GET /api/system
pub async fn system_info() -> ApiResult<SystemInfoResponse> {
    Ok(Json(service::system_info()))
}

#[utoipa::path(
    get,
    path = "/api/metrics",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = RuntimeMetricsResponse),
        (status = 500, description = "Error", body = ApiError),
    )
)]
/// GET /api/metrics
pub async fn runtime_metrics() -> ApiResult<RuntimeMetricsResponse> {
    // Real system metrics from the crate-wide shared sysinfo handle
    // (crate::sys): a persistent System with targeted CPU/memory refreshes.
    // A fresh System::new_all() + refresh_all() per request both blocks the
    // worker on a full process-table scan and reports a meaningless
    // global_cpu_usage() — CPU deltas need two refreshes of the SAME
    // System, spaced apart. The response includes no per-process data, so
    // no process refresh is needed here.
    let host = crate::sys::get_host_resources();

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
            cpu_usage_percent: host.cpu_usage_percent as f64,
            total_memory_mb: host.total_memory_mb,
            used_memory_mb: host.used_memory_mb,
            total_swap_mb: host.total_swap_mb,
            used_swap_mb: host.used_swap_mb,
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

#[utoipa::path(
    get,
    path = "/api/quota",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/quota — resource quota status for team mode: limits plus the
/// acting user's current usage, so a quota rejection is explainable.
pub async fn quota_status(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    let tracker = crate::infra::quota::global_quota_tracker();
    let config = tracker.config();
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    let usage = tracker.get_usage(&user.id);
    Ok(Json(serde_json::json!({
        "enabled": true,
        "limits": {
            "max_concurrent_runs": config.max_concurrent_runs,
            "max_total_threads": config.max_total_threads,
            "max_total_memory_mb": config.max_total_memory_mb,
            "max_runs_per_day": config.max_runs_per_day,
        },
        "usage": {
            "active_runs": usage.active_runs,
            "used_threads": usage.used_threads,
            "used_memory_mb": usage.used_memory_mb,
            "runs_today": usage.runs_today,
        }
    })))
}

#[utoipa::path(
    put,
    path = "/api/quota",
    tag = "observability",
    request_body = crate::infra::quota::QuotaConfig,
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = ApiError),
    )
)]
/// PUT /api/quota — update resource quota limits (admin only).
pub async fn update_quota_status(
    headers: axum::http::HeaderMap,
    Json(body): Json<crate::infra::quota::QuotaConfig>,
) -> ApiResult<serde_json::Value> {
    crate::domains::auth::handlers::require_admin(&headers).await?;
    crate::infra::quota::global_quota_tracker().update_config(body.clone());
    Ok(Json(serde_json::json!({
        "updated": true,
        "limits": {
            "max_concurrent_runs": body.max_concurrent_runs,
            "max_total_threads": body.max_total_threads,
            "max_total_memory_mb": body.max_total_memory_mb,
            "max_runs_per_day": body.max_runs_per_day,
        }
    })))
}

#[utoipa::path(
    get,
    path = "/api/audit",
    tag = "observability",
    params(
        ("days" = Option<u8>, Query, description = "Number of days of audit history to return (default 7)"),
        ("page" = Option<u32>, Query, description = "Page number, 1-based (default 1)"),
        ("per_page" = Option<u32>, Query, description = "Items per page, max 200 (default 50)"),
    ),
    responses(
        (status = 200, description = "Success", body = AuditLogResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/audit — paginated audit trail for compliance review.
pub async fn get_audit_logs(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> ApiResult<AuditLogResponse> {
    // The audit trail records every user's actions — team/hpc mode exposes
    // it to admins only (personal mode keeps the localhost trust model).
    crate::domains::auth::handlers::require_admin(&headers).await?;
    let days: u8 = params.get("days").and_then(|d| d.parse().ok()).unwrap_or(7);
    let page: u32 = params
        .get("page")
        .and_then(|d| d.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: u32 = params
        .get("per_page")
        .and_then(|d| d.parse().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let offset = (page - 1) * per_page;

    let (entries, total) = if let Ok(pool) = get_pool() {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        let rows: Vec<models::AuditLogRow> = sqlx::query_as(
            "SELECT * FROM audit_logs WHERE timestamp >= ? ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        )
        .bind(&cutoff)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let entries = rows
            .into_iter()
            .map(|r| AuditEntry {
                timestamp: r.timestamp,
                user: r.user_id,
                action: r.action,
                resource: r.target,
                result: r.result,
            })
            .collect();

        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE timestamp >= ?")
            .bind(&cutoff)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

        (entries, total as u32)
    } else {
        (vec![], 0)
    };

    Ok(Json(AuditLogResponse {
        entries,
        days,
        page,
        per_page,
        total,
    }))
}

#[utoipa::path(
    get,
    path = "/api/hpc",
    tag = "hpc",
    responses(
        (status = 200, description = "Success", body = crate::hpc::HpcStatus),
    )
)]
/// GET /api/hpc — scheduler status (hpc mode only).
pub async fn hpc_status() -> axum::Json<crate::hpc::HpcStatus> {
    axum::Json(crate::hpc::get_hpc_status())
}

// ---------------------------------------------------------------------------
// Webhook configuration (issue #82 P1-12)
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/webhook",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/webhook — current settings (the secret is never echoed back).
pub async fn get_webhook_config(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    let _user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;
    let row = sqlx::query_as::<_, (String, Option<String>, i64, String, String)>(
        "SELECT url, secret, enabled, events, signature_scheme FROM webhook_config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    match row {
        Some((url, secret, enabled, events, scheme)) => Ok(Json(serde_json::json!({
            "enabled": enabled != 0,
            "url": url,
            "secret_set": secret.is_some(),
            "events": serde_json::from_str::<serde_json::Value>(&events).unwrap_or(serde_json::json!([])),
            "signature_scheme": scheme,
        }))),
        None => Ok(Json(serde_json::json!({
            "enabled": false, "url": "", "secret_set": false, "events": [],
            "signature_scheme": "sha256-keyed",
        }))),
    }
}

#[utoipa::path(
    put,
    path = "/api/webhook",
    tag = "observability",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// PUT /api/webhook — configure the endpoint (admin-only outside personal
/// mode; the webhook fires for every user's runs, it is shared
/// infrastructure).
pub async fn put_webhook_config(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    if crate::server::running_mode() != "personal" && !user.is_admin() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Admin role required to configure webhooks".into(),
        ));
    }
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let enabled = req
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let url = req
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if enabled && !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_URL",
            "url must start with http:// or https://".into(),
        ));
    }
    // An empty secret field means "keep the existing one"; the caller
    // cannot read it back, only replace it.
    let secret: Option<String> = req
        .get("secret")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let events: Vec<String> = req
        .get("events")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["workflow_completed".into(), "workflow_failed".into()]);
    let events_json = serde_json::to_string(&events).unwrap_or_else(|_| "[]".into());
    // Default 'sha256-keyed' keeps pre-v0.11 webhook consumers verifying
    // after upgrade; 'hmac-sha256' (RFC 2104) is the explicit opt-in.
    let signature_scheme = match req
        .get("signature_scheme")
        .and_then(|v| v.as_str())
        .unwrap_or("sha256-keyed")
    {
        "hmac-sha256" => "hmac-sha256",
        _ => "sha256-keyed",
    };
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO webhook_config (id, url, secret, enabled, events, signature_scheme, updated_at) VALUES (1, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET url = excluded.url, \
         secret = COALESCE(excluded.secret, webhook_config.secret), \
         enabled = excluded.enabled, events = excluded.events, \
         signature_scheme = excluded.signature_scheme, updated_at = excluded.updated_at",
    )
    .bind(&url)
    .bind(secret)
    .bind(enabled as i64)
    .bind(&events_json)
    .bind(signature_scheme)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error saving webhook config: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(
        serde_json::json!({"status": "saved", "enabled": enabled, "events": events}),
    ))
}
