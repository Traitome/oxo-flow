//! HTTP handlers for observability domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::extract::ApiJson;
use axum::{Json, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::workflow::handlers::{ApiError, err};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

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
    let db_healthy = if let Some(backend) = crate::infra::db::global_backend() {
        backend.health().await.unwrap_or(false)
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
    let (active_workflows, total_requests) =
        if let Some(backend) = crate::infra::db::global_backend() {
            backend.get_run_metrics().await.unwrap_or((0, 0))
        } else {
            (0, 0)
        };

    // Process uptime via the shared START_TIME origin — the same source
    // /api/health and /api/system use. sysinfo::System::uptime() is HOST
    // boot uptime (verified live: reported ~27.7 days while the server
    // process was ~2.4h old), which contradicted the sibling endpoints.
    let uptime = service::uptime_secs();

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
    ApiJson(body): ApiJson<crate::infra::quota::QuotaConfig>,
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
    crate::extract::ApiQuery(params): crate::extract::ApiQuery<
        std::collections::HashMap<String, String>,
    >,
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

    let (entries, total) = if let Some(backend) = crate::infra::db::global_backend() {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        let (rows, total) = backend
            .list_audit_logs(&cutoff, per_page, offset)
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
    // get_hpc_status shells out synchronously to scheduler CLIs
    // (sinfo/squeue/…); on a loaded controller that takes seconds, so keep
    // it off the async worker the dashboard polls with.
    let status = tokio::task::spawn_blocking(crate::hpc::get_hpc_status)
        .await
        .unwrap_or_default();
    axum::Json(status)
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
///
/// Admin-only outside personal mode, same as PUT (#519): the configured
/// `url` conventionally embeds secret tokens, so it must not be readable
/// by every authenticated user.
pub async fn get_webhook_config(
    authenticated: Option<axum::Extension<crate::domains::auth::current_user::CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    crate::domains::auth::current_user::require_role_outside_personal(
        &user,
        "admin",
        "Admin role required to read the webhook configuration",
    )?;
    if crate::infra::db::global_backend().is_none() {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        ));
    }
    let settings = crate::domains::observability::webhook::load_settings().await;
    match settings {
        Some(s) => Ok(Json(serde_json::json!({
            "enabled": s.enabled,
            "url": s.url,
            "secret_set": s.secret.is_some(),
            "events": s.events,
            "signature_scheme": s.signature_scheme,
        }))),
        None => Ok(Json(serde_json::json!({
            "enabled": false, "url": "", "secret_set": false, "events": [],
            "signature_scheme": crate::domains::observability::webhook::DEFAULT_SIGNATURE_SCHEME,
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
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = crate::domains::auth::current_user::resolve(authenticated.as_ref());
    crate::domains::auth::current_user::require_role_outside_personal(
        &user,
        "admin",
        "Admin role required to configure webhooks",
    )?;
    if crate::infra::db::global_backend().is_none() {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        ));
    }

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
    if enabled {
        // Parse, don't prefix-match: "https://" alone passed the old
        // starts_with check (issue #907 N5). reqwest re-exports `url::Url`,
        // the same parser core's webhook delivery redaction uses. Note
        // url's special-scheme rules squash `https:///path` to host "path",
        // so requiring a non-empty host_str — not just has_host — is the
        // meaningful bar here.
        let parsed = reqwest::Url::parse(&url).ok();
        let scheme_ok = parsed
            .as_ref()
            .is_some_and(|u| matches!(u.scheme(), "http" | "https"));
        let host_ok = parsed
            .as_ref()
            .and_then(|u| u.host_str())
            .is_some_and(|h| !h.is_empty());
        if !scheme_ok || !host_ok {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "INVALID_URL",
                "url must be an absolute http:// or https:// URL with a host".into(),
            ));
        }
    }
    // An empty secret field means "keep the existing one"; the caller
    // cannot read it back, only replace it.
    let existing_settings = crate::domains::observability::webhook::load_settings().await;
    let secret: Option<String> = req
        .get("secret")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    // A webhook URL is a bearer credential — anyone who learns it can POST
    // forged delivery events to the receiver. Enabling delivery without a
    // signing secret (new or previously stored) is therefore refused; the
    // response body says so instead of silently shipping unsigned events.
    if enabled && secret.is_none() {
        // The stored value decrypts to "" when the master key is unset or
        // the ciphertext is unopenable (e.g. rotated key) — crypto::open
        // signals failure that way. Such a row is NOT a usable secret: the
        // gate must treat it as absent, or #909-NIT-1's downgrade to
        // unsigned (forgeable) deliveries silently re-arms itself.
        let has_stored_secret = existing_settings
            .as_ref()
            .and_then(|s| s.secret.as_ref())
            .is_some_and(|s| !s.is_empty());
        if !has_stored_secret {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "WEBHOOK_SECRET_REQUIRED",
                "enabling a webhook requires a signing secret (provide 'secret', or keep the previously stored one by omitting it)"
                    .into(),
            ));
        }
    }
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
    // New configurations use the RFC-2104 scheme. Omission preserves the
    // stored scheme for existing deployments, while an explicit unknown
    // value is a client error rather than a silent downgrade.
    let signature_scheme = match req.get("signature_scheme") {
        Some(value) => {
            let Some(value) = value.as_str() else {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "INVALID_SIGNATURE_SCHEME",
                    "signature_scheme must be a string: 'hmac-sha256' or 'sha256-keyed'".into(),
                ));
            };
            crate::domains::observability::webhook::canonical_signature_scheme(value).ok_or_else(
                || {
                    err(
                        StatusCode::BAD_REQUEST,
                        "INVALID_SIGNATURE_SCHEME",
                        "signature_scheme must be 'hmac-sha256' or 'sha256-keyed'".into(),
                    )
                },
            )?
        }
        None => existing_settings
            .as_ref()
            .map(|settings| {
                crate::domains::observability::webhook::canonical_signature_scheme(
                    &settings.signature_scheme,
                )
                .ok_or_else(|| {
                    err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "WEBHOOK_CONFIG_INVALID",
                        "stored webhook signature_scheme is invalid; resend an explicit supported scheme"
                            .into(),
                    )
                })
            })
            .transpose()?
            .unwrap_or(
                crate::domains::observability::webhook::DEFAULT_SIGNATURE_SCHEME,
            ),
    };
    let sealed_secret = secret.as_deref().map(crate::infra::crypto::seal);
    if let Some(backend) = crate::infra::db::global_backend() {
        backend
            .save_webhook_config(
                &url,
                sealed_secret.as_deref(),
                enabled,
                &events_json,
                signature_scheme,
            )
            .await
            .map_err(|e| {
                tracing::error!("DB error saving webhook config: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;
    }

    Ok(Json(serde_json::json!({
        "status": "saved",
        "enabled": enabled,
        "events": events,
        "signature_scheme": signature_scheme,
    })))
}
