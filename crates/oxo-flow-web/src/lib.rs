#![forbid(unsafe_code)]
#![allow(deprecated)] // Legacy handlers preserved for backward compat; removed v0.10.0
//! oxo-flow-web — Web interface for the oxo-flow pipeline engine.
//!
//! Provides a REST API and web UI for building, running, and monitoring
//! bioinformatics workflows.  Includes session-based authentication,
//! role-based access control, and dual-license verification via
//! [`oxo_license`].

pub mod ai_provider;
pub mod audit;
pub mod config;
pub mod db;
pub mod domains;
pub mod executor;
pub mod hpc;
pub mod infra;
pub mod openapi;
pub mod process_control;
pub mod rate_limit;
pub mod server;
pub mod sse;
pub mod sys;
pub mod workspace;

use axum::{extract::Json, http::StatusCode, response::IntoResponse};

use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// License configuration (oxo-dual-licenser integration)
// ---------------------------------------------------------------------------

/// Static license configuration for oxo-flow-web.
///
/// Uses the same Ed25519 public key as other Traitome products.  The license
/// file is discovered via (in order):
///   1. `OXO_FLOW_LICENSE` env var
///   2. Platform config directory (`io.traitome.oxo-flow/license.oxo.json`)
///   3. Legacy `~/.config/oxo-flow/license.oxo.json`
///   4. Embedded academic license (fallback)
pub static OXO_FLOW_CONFIG: oxo_license::LicenseConfig = oxo_license::LicenseConfig {
    schema_version: "oxo-flow-license-v1",
    public_key_base64: "SOTbyPWS8fSF+XS9dqEg9cFyag0wPO/YMA5LhI4PXw4=",
    license_env_var: "OXO_FLOW_LICENSE",
    app_qualifier: "io",
    app_org: "traitome",
    app_name: "oxo-flow",
    license_filename: "license.oxo.json",
};

/// Embedded academic license for default non-commercial use.
const EMBEDDED_ACADEMIC_LICENSE: &str = r#"{
  "schema": "oxo-flow-license-v1",
  "license_id": "6548e181-e352-402a-ab72-4da51f49e7b5",
  "issued_to_org": "Public Academic Test License (any academic user)",
  "license_type": "academic",
  "scope": "org",
  "perpetual": true,
  "issued_at": "2026-03-12",
  "signature": "duKJcISYPdyZkw1PbyVil5zTjvLhAYsmbzRpH0n6eRYJET90p1b0rYiHO0cJ7IGR6NLEJWqkY1wBXUkfvUvECw=="
}"#;

// ---------------------------------------------------------------------------
// Embedded frontend
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

/// Configuration for the in-memory rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum number of requests allowed within the window.
    pub max_requests: u64,
    /// Sliding window duration.
    pub window: std::time::Duration,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window: std::time::Duration::from_secs(60),
        }
    }
}

/// Simple in-memory rate limiter that tracks request timestamps per key (IP).
#[derive(Debug, Clone)]
pub struct RateLimiter {
    config: RateLimiterConfig,
    /// Maps a client key to a list of request timestamps within the current window.
    entries: Arc<dashmap::DashMap<String, Vec<std::time::Instant>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            entries: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Check whether a request from `key` is allowed.
    ///
    /// Returns `Ok(())` when the request is within the limit, or
    /// `Err(remaining_secs)` with the number of seconds until the oldest
    /// entry expires when the limit is exceeded.
    pub fn check_rate_limit(&self, key: &str) -> Result<(), u64> {
        let now = std::time::Instant::now();
        let window_start = now - self.config.window;

        let mut timestamps = self.entries.entry(key.to_owned()).or_default();

        // Evict timestamps outside the sliding window.
        timestamps.retain(|t| *t > window_start);

        if timestamps.len() as u64 >= self.config.max_requests {
            let retry_after = timestamps
                .first()
                .map(|t| {
                    self.config
                        .window
                        .saturating_sub(now.duration_since(*t))
                        .as_secs()
                        + 1
                })
                .unwrap_or(1);
            return Err(retry_after);
        }

        timestamps.push(now);
        Ok(())
    }
}

/// Response returned when the rate limit is exceeded.
#[derive(Serialize, Deserialize)]
pub struct RateLimitResponse {
    pub error: String,
    pub retry_after_secs: u64,
}

/// Login request body.
#[derive(Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response body.
#[derive(Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub role: String,
}

/// Response from `GET /api/auth/me`.
#[derive(Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub authenticated: bool,
    pub username: Option<String>,
    pub role: Option<String>,
}

/// License status response.
#[derive(Serialize, Deserialize)]
pub struct LicenseStatus {
    pub valid: bool,
    pub license_type: Option<String>,
    pub issued_to: Option<String>,
    pub schema: Option<String>,
    pub message: String,
}

pub fn check_license() -> LicenseStatus {
    // 1. Try external license file first (commercial or custom)
    match oxo_license::load_and_verify(None, &OXO_FLOW_CONFIG) {
        Ok(license) => {
            return LicenseStatus {
                valid: true,
                license_type: Some(license.payload.license_type.clone()),
                issued_to: Some(license.payload.issued_to_org.clone()),
                schema: Some(license.payload.schema.clone()),
                message: format!("License verified: {}", license.payload.license_type),
            };
        }
        Err(_) => {
            // 2. Fallback: try embedded academic license
        }
    }

    // 2. Fallback: embedded academic license (trusted, not signature-verified)
    match serde_json::from_str::<oxo_license::LicenseFile>(EMBEDDED_ACADEMIC_LICENSE) {
        Ok(embedded) => LicenseStatus {
            valid: true,
            license_type: Some(embedded.payload.license_type.clone()),
            issued_to: Some(embedded.payload.issued_to_org.clone()),
            schema: Some(embedded.payload.schema.clone()),
            message: "Academic license active - free for non-commercial use. Commercial use requires a paid license file.".to_string(),
        },
        Err(e) => LicenseStatus {
            valid: false,
            license_type: None,
            issued_to: None,
            schema: None,
            message: format!("License system error: {e}"),
        },
    }
}

use std::sync::OnceLock;
use tokio::sync::broadcast;

/// Broadcast channel for Server-Sent Events (SSE).
static EVENT_TX: OnceLock<broadcast::Sender<String>> = OnceLock::new();

fn event_tx() -> broadcast::Sender<String> {
    EVENT_TX
        .get_or_init(|| {
            let (tx, _rx) = broadcast::channel(100);
            tx
        })
        .clone()
}

/// Send an SSE event.
/// Broadcasts to both the legacy channel and the infra SSE channel
/// so all connected clients receive real-time updates.
pub fn broadcast_event(event_type: &str, data: &serde_json::Value) {
    let msg = format!(
        r#"{{"type":"{}","time":"{}","data":{}}}"#,
        event_type,
        chrono::Utc::now().to_rfc3339(),
        serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
    );
    let _ = event_tx().send(msg.clone());
    // Also forward to the infra SSE channel used by the active SSE endpoint
    crate::sse::broadcast_event(event_type, data);
}

/// Broadcast an SSE event scoped to one user's run (issue #82 P0-5) on the
/// active SSE channel.
pub fn broadcast_event_for(event_type: &str, data: &serde_json::Value, user: Option<&str>) {
    crate::sse::broadcast_event_for(event_type, data, user);
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Health check response.
#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Summary of a workflow.
#[derive(Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub version: String,
    pub rules_count: usize,
}

/// Full workflow detail including parsed rules.
#[derive(Serialize, Deserialize)]
pub struct WorkflowDetail {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub rules_count: usize,
    pub rules: Vec<RuleSummary>,
}

/// Summary of a single rule within a workflow.
#[derive(Serialize, Deserialize)]
pub struct RuleSummary {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment: String,
    pub threads: u32,
}

/// Request body for endpoints that accept TOML workflow content.
#[derive(Serialize, Deserialize)]
pub struct ValidateRequest {
    pub toml_content: String,
}

/// Response from the validation endpoint.
#[derive(Serialize, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub errors: Vec<String>,
    pub rules_count: Option<usize>,
    pub edges_count: Option<usize>,
}

/// Optional run configuration parameters.
#[derive(Serialize, Deserialize)]
pub struct RunConfig {
    pub max_jobs: Option<usize>,
    pub dry_run: Option<bool>,
    pub keep_going: Option<bool>,
}

/// Status of a workflow run (used in dry-run response).
#[derive(Serialize, Deserialize)]
pub struct RunStatus {
    pub id: String,
    pub status: String,
    pub rules_total: usize,
    pub rules_completed: usize,
    pub started_at: Option<String>,
}

/// Request body for the dry-run endpoint.
#[derive(Serialize, Deserialize)]
pub struct DryRunRequest {
    pub toml_content: String,
    #[serde(default)]
    pub config: Option<RunConfig>,
}

/// DAG visualisation response.
#[derive(Serialize, Deserialize)]
pub struct DagResponse {
    pub dot: String,
    pub nodes: usize,
    pub edges: usize,
}

/// Request body for report generation.
#[derive(Serialize, Deserialize)]
pub struct ReportRequest {
    pub toml_content: String,
    pub format: Option<String>,
}

/// Uniform JSON error body.
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub suggestion: Option<String>,
}

/// Response from the run endpoint.
#[derive(Serialize, Deserialize)]
pub struct RunResponse {
    pub run_id: String,
    pub status: String,
    pub execution_order: Vec<String>,
    pub rules_total: usize,
}

/// Response from the version endpoint.
#[derive(Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
    pub crate_name: String,
    pub rust_version: String,
}

/// Response from the clean endpoint.
#[derive(Serialize, Deserialize)]
pub struct CleanResponse {
    pub workflow_name: String,
    pub files_to_clean: Vec<String>,
    pub total_files: usize,
}

/// Request body for the export endpoint.
#[derive(Serialize, Deserialize)]
pub struct ExportRequest {
    pub toml_content: String,
    pub format: Option<String>, // "docker" or "singularity", default "docker"
}

/// Response from the export endpoint.
#[derive(Serialize, Deserialize)]
pub struct ExportResponse {
    pub format: String,
    pub content: String,
}

/// Query parameters for paginated list endpoints.
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    /// Page number (1-based). Defaults to 1.
    #[serde(default = "default_page")]
    pub page: usize,
    /// Items per page. Defaults to 20, max 100.
    #[serde(default = "default_per_page")]
    pub per_page: usize,
}

fn default_page() -> usize {
    1
}

fn default_per_page() -> usize {
    20
}

impl PaginationParams {
    /// Clamp per_page to the allowed range [1, 100].
    pub fn clamped_per_page(&self) -> usize {
        self.per_page.clamp(1, 100)
    }

    /// Returns the offset for database-style slicing.
    pub fn offset(&self) -> usize {
        (self.page.saturating_sub(1)) * self.clamped_per_page()
    }
}

/// Pagination metadata included in paginated responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct PaginationMeta {
    /// Current page number (1-based).
    pub page: usize,
    /// Items per page.
    pub per_page: usize,
    /// Total number of items.
    pub total_items: usize,
    /// Total number of pages.
    pub total_pages: usize,
    /// Whether there is a next page.
    pub has_next: bool,
    /// Whether there is a previous page.
    pub has_prev: bool,
}

impl PaginationMeta {
    pub fn new(page: usize, per_page: usize, total_items: usize) -> Self {
        let total_pages = if total_items == 0 {
            1
        } else {
            total_items.div_ceil(per_page)
        };
        Self {
            page,
            per_page,
            total_items,
            total_pages,
            has_next: page < total_pages,
            has_prev: page > 1,
        }
    }
}

/// Request body for lint endpoint.
#[derive(Serialize, Deserialize)]
pub struct LintRequest {
    pub toml_content: String,
}

/// Response from lint endpoint.
#[derive(Serialize, Deserialize)]
pub struct LintResponse {
    pub diagnostics: Vec<DiagnosticItem>,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}

/// Single diagnostic item in lint/validate response.
#[derive(Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub rule: Option<String>,
}

/// Response from format endpoint.
#[derive(Serialize, Deserialize)]
pub struct FormatResponse {
    pub formatted: String,
}

/// Paginated response from lint endpoint.
#[derive(Serialize, Deserialize)]
pub struct PaginatedLintResponse {
    pub diagnostics: Vec<DiagnosticItem>,
    pub pagination: PaginationMeta,
    pub summary: LintSummary,
}

/// Summary counts for lint results.
#[derive(Serialize, Deserialize)]
pub struct LintSummary {
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}

/// Response from stats endpoint.
#[derive(Serialize, Deserialize)]
pub struct StatsResponse {
    pub rule_count: usize,
    pub shell_rules: usize,
    pub script_rules: usize,
    pub dependency_count: usize,
    pub parallel_groups: usize,
    pub max_depth: usize,
    pub environments: Vec<String>,
    pub total_threads: u32,
    pub wildcard_count: usize,
    pub wildcard_names: Vec<String>,
}

/// System information response.
#[derive(Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub rust_version: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub uptime_secs: f64,
}

/// Runtime metrics for monitoring and observability.
#[derive(Debug, Serialize)]
pub struct RuntimeMetrics {
    pub uptime_secs: f64,
    pub version: String,
    pub pid: u32,
    pub os: String,
    pub arch: String,
    /// Number of available CPU cores.
    pub cpu_count: usize,
    /// Total number of requests processed.
    pub total_requests: u64,
    /// Current number of active/running workflows.
    pub active_workflows: i64,
    /// Host resource usage.
    pub host: sys::HostResources,
}

/// Request body for comparing two workflows.
#[derive(Deserialize)]
pub struct DiffRequest {
    /// TOML content of the first workflow.
    pub toml_a: String,
    /// TOML content of the second workflow.
    pub toml_b: String,
}

/// Response from workflow diff.
#[derive(Serialize)]
pub struct DiffResponse {
    /// Number of differences found.
    pub diff_count: usize,
    /// List of differences.
    pub diffs: Vec<DiffEntry>,
}

/// A single difference entry.
#[derive(Serialize)]
pub struct DiffEntry {
    pub category: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

/// Wrap an `ErrorResponse` with an HTTP status code so it can be returned from
/// any handler via `Result<impl IntoResponse, ApiError>`.
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorResponse,
}

#[allow(dead_code)]
impl ApiError {
    /// Map an HTTP status code to a machine-readable error code.
    fn code_for_status(status: StatusCode) -> &'static str {
        match status {
            StatusCode::BAD_REQUEST => "BAD_REQUEST",
            StatusCode::UNAUTHORIZED => "AUTH_REQUIRED",
            StatusCode::NOT_FOUND => "NOT_FOUND",
            StatusCode::UNPROCESSABLE_ENTITY => "UNPROCESSABLE_ENTITY",
            StatusCode::TOO_MANY_REQUESTS => "RATE_LIMITED",
            StatusCode::INTERNAL_SERVER_ERROR => "INTERNAL_ERROR",
            StatusCode::CONFLICT => "CONFLICT",
            _ => "UNKNOWN_ERROR",
        }
    }

    /// Create an ApiError with an inferred code from the HTTP status.
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorResponse {
                code: Self::code_for_status(status).to_string(),
                message: message.into(),
                detail: None,
                suggestion: None,
            },
        }
    }

    /// Create a BAD_REQUEST error.
    fn bad_request(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorResponse {
                code: "BAD_REQUEST".to_string(),
                message: error.into(),
                detail: detail.into(),
                suggestion: None,
            },
        }
    }

    /// Create an UNPROCESSABLE_ENTITY error.
    fn unprocessable(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ErrorResponse {
                code: "UNPROCESSABLE_ENTITY".to_string(),
                message: error.into(),
                detail: detail.into(),
                suggestion: None,
            },
        }
    }

    /// Create an UNAUTHORIZED error.
    fn unauthorized(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "AUTH_REQUIRED".to_string(),
                message: error.into(),
                detail: detail.into(),
                suggestion: None,
            },
        }
    }

    /// Create a NOT_FOUND error.
    fn not_found(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: error.into(),
                detail: detail.into(),
                suggestion: None,
            },
        }
    }

    /// Create an INTERNAL_SERVER_ERROR.
    fn internal_error(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ErrorResponse {
                code: "INTERNAL_ERROR".to_string(),
                message: error.into(),
                detail: detail.into(),
                suggestion: None,
            },
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

pub async fn start_server_with_mode(
    mode: &str,
    host: &str,
    port: u16,
    base_path: &str,
) -> anyhow::Result<()> {
    crate::db::init_db("sqlite://oxo-flow.db").await?;
    crate::db::recover_orphaned_runs().await?;
    crate::infra::db::sqlite::init_pool("sqlite://oxo-flow.db").await;

    // Cluster definitions from the platform config file are imported here
    // (the shared entry for BOTH the `oxo-flow serve` subcommand and the
    // standalone web binary) — idempotent, existing DB rows win.
    if let Some(cfg) = crate::config::load() {
        crate::domains::clusters::handlers::import_from_config(&cfg.clusters).await;
    }

    // Initialize structured logging
    let log_dir = std::path::PathBuf::from("logs");
    if let Err(e) = crate::domains::observability::logging::init_logging(&log_dir) {
        tracing::warn!("Failed to initialize structured logging: {e}");
    }

    // Initialize AI provider
    crate::ai_provider::AiProviderRegistry::global().init_from_env();

    // Normalize defensively: axum's nest() panics on a mount path without a
    // leading slash, so whatever the caller passed must become "/x" or "".
    let normalized = crate::server::normalize_base_path(base_path);
    crate::server::set_base_path(&normalized);
    let app = crate::server::build_router(mode);
    let app = if normalized.is_empty() {
        app
    } else {
        // See main.rs: `nest` leaves the trailing-slash mount root unrouted.
        axum::Router::new()
            .route(
                &format!("{normalized}/"),
                axum::routing::get(crate::server::spa_index),
            )
            .nest(&normalized, app)
    };

    let addr = format!("{host}:{port}");
    tracing::info!("Starting oxo-flow web server in {mode} mode on {addr}");

    // The daily run quota is a rolling window that must reset — without
    // this runs_today only ever grows and users hit 429 until a restart.
    spawn_daily_quota_reset();

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Reset the daily run quota once per UTC day (at minute 1, so DST and
/// scheduler races on the hour mark don't skip or double-fire it).
fn spawn_daily_quota_reset() {
    tokio::spawn(async {
        loop {
            let now = chrono::Utc::now();
            let next = (now + chrono::Duration::days(1))
                .date_naive()
                .and_hms_opt(0, 1, 0)
                .expect("00:01:00 is a valid time")
                .and_utc();
            let wait = (next - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(60));
            tokio::time::sleep(wait).await;
            crate::infra::quota::global_quota_tracker().reset_daily();
        }
    });
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl+C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down gracefully...");
        },
        () = terminate => {
            tracing::info!("Received SIGTERM, shutting down gracefully...");
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
