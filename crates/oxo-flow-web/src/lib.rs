#![forbid(unsafe_code)]
//! oxo-flow-web — Web interface for the oxo-flow pipeline engine.
//!
//! Provides a REST API and web UI for building, running, and monitoring
//! bioinformatics workflows.  Includes session-based authentication,
//! role-based access control, and dual-license verification via
//! [`oxo_license`].

pub mod db;
pub mod executor;
pub mod sys;
pub mod workspace;

use axum::{
    Router,
    extract::Json,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use tower_http::cors::{Any, CorsLayer};

// ---------------------------------------------------------------------------
// Global metrics counters
// ---------------------------------------------------------------------------

static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static ACTIVE_WORKFLOWS: AtomicI64 = AtomicI64::new(0);

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
pub static OXO_FLOW_CONFIG: oxo_license::LicenseConfig = oxo_license::LicenseConfig {
    schema_version: "oxo-flow-license-v1",
    public_key_base64: "SOTbyPWS8fSF+XS9dqEg9cFyag0wPO/YMA5LhI4PXw4=",
    license_env_var: "OXO_FLOW_LICENSE",
    app_qualifier: "io",
    app_org: "traitome",
    app_name: "oxo-flow",
    license_filename: "license.oxo.json",
};

// ---------------------------------------------------------------------------
// Embedded frontend
// ---------------------------------------------------------------------------

/// Embedded single-page web application.
const FRONTEND_HTML: &str = include_str!("../static/index.html");

// Store server start time for uptime calculation.
static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn get_start_time() -> std::time::Instant {
    *START_TIME.get_or_init(std::time::Instant::now)
}

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

/// Axum middleware that enforces per-IP rate limiting.
///
/// The [`RateLimiter`] instance is extracted from request extensions
/// (added via `Extension`).  If no limiter is present the request is
/// allowed through unconditionally so that existing tests keep passing
/// without modification.
async fn rate_limit_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Extract the rate limiter from extensions (if present).
    let limiter = request.extensions().get::<RateLimiter>().cloned();

    if let Some(limiter) = limiter {
        // Derive a key from the peer IP or fall back to a fixed string.
        let key = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        if let Err(retry_after) = limiter.check_rate_limit(&key) {
            let body = RateLimitResponse {
                error: "Rate limit exceeded".to_string(),
                retry_after_secs: retry_after,
            };
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(
                    axum::http::header::RETRY_AFTER,
                    axum::http::HeaderValue::from_str(&retry_after.to_string())
                        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("60")),
                )],
                Json(body),
            )
                .into_response();
        }
    }

    next.run(request).await
}

// ---------------------------------------------------------------------------
// Authentication & authorization
// ---------------------------------------------------------------------------

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

async fn check_credentials_db(username: &str, password: &str) -> Option<db::User> {
    let user = db::get_user_by_username(username).await.ok()??;

    #[cfg(test)]
    let dev_mode = true;
    #[cfg(not(test))]
    let dev_mode = std::env::var("OXO_FLOW_DEV_MODE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let env_var = match username {
        "admin" => "OXO_FLOW_ADMIN_PASSWORD",
        "user" => "OXO_FLOW_USER_PASSWORD",
        "viewer" => "OXO_FLOW_VIEWER_PASSWORD",
        _ => "OXO_FLOW_EXTERNAL_PASSWORD",
    };

    let expected = match std::env::var(env_var) {
        Ok(p) => p,
        Err(_) => {
            if dev_mode {
                username.to_string()
            } else {
                return None;
            }
        }
    };

    if password == expected {
        Some(user)
    } else {
        None
    }
}

/// Generate a hex-encoded UUID session token.
fn generate_session_token() -> String {
    use std::fmt::Write;
    let id = uuid::Uuid::new_v4();
    let mut buf = String::with_capacity(44);
    for byte in id.as_bytes() {
        let _ = write!(buf, "{byte:02x}");
    }
    buf
}

/// Extract a session from the `Authorization: Bearer <token>` header.
async fn extract_session(headers: &axum::http::HeaderMap) -> Option<db::Session> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    db::get_session(token).await.ok().flatten()
}

/// Check the oxo-flow license status.
fn check_license() -> LicenseStatus {
    match oxo_license::load_and_verify(None, &OXO_FLOW_CONFIG) {
        Ok(license) => LicenseStatus {
            valid: true,
            license_type: Some(license.payload.license_type.clone()),
            issued_to: Some(license.payload.issued_to_org.clone()),
            schema: Some(license.payload.schema.clone()),
            message: "License verified successfully".to_string(),
        },
        Err(e) => LicenseStatus {
            valid: false,
            license_type: None,
            issued_to: None,
            schema: None,
            message: format!("No valid license: {e}"),
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
pub fn broadcast_event(event_type: &str, data: &serde_json::Value) {
    let msg = format!(
        r#"{{"type":"{}","time":"{}","data":{}}}"#,
        event_type,
        chrono::Utc::now().to_rfc3339(),
        serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
    );
    let _ = event_tx().send(msg);
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Health check response.
#[derive(Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Workflow list response.
#[derive(Serialize, Deserialize)]
struct WorkflowListResponse {
    workflows: Vec<WorkflowSummary>,
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

/// Environment backend info.
#[derive(Serialize, Deserialize)]
struct EnvInfo {
    available: Vec<String>,
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
struct DryRunRequest {
    toml_content: String,
    #[serde(default)]
    config: Option<RunConfig>,
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
    pub error: String,
    pub detail: Option<String>,
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
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn bad_request(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }

    fn unprocessable(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Existing endpoints
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn list_workflows(
    headers: axum::http::HeaderMap,
) -> Result<Json<WorkflowListResponse>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .ok()
        .flatten()
        .unwrap();

    let user_dir = std::path::Path::new("workspace")
        .join("users")
        .join(&user.username)
        .join("templates");

    let mut workflows = Vec::new();

    if user_dir.exists()
        && let Ok(entries) = std::fs::read_dir(user_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("oxoflow")
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(config) = oxo_flow_core::WorkflowConfig::parse(&content)
            {
                workflows.push(WorkflowSummary {
                    name: config.workflow.name.clone(),
                    version: config.workflow.version.clone(),
                    rules_count: config.rules.len(),
                });
            }
        }
    }

    Ok(Json(WorkflowListResponse { workflows }))
}

async fn list_environments() -> Json<EnvInfo> {
    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
    Json(EnvInfo {
        available: resolver
            .available_backends()
            .into_iter()
            .map(String::from)
            .collect(),
    })
}

async fn not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "Not found".to_string(),
            detail: None,
        }),
    )
}

// ---------------------------------------------------------------------------
// New endpoints
// ---------------------------------------------------------------------------

/// `POST /api/workflows/validate` — Parse + validate a workflow TOML.
async fn validate_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    let config = match oxo_flow_core::WorkflowConfig::parse(&req.toml_content) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: None,
                edges_count: None,
            }));
        }
    };

    let dag = match oxo_flow_core::WorkflowDag::from_rules(&config.rules) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: Some(config.rules.len()),
                edges_count: None,
            }));
        }
    };

    if let Err(e) = dag.validate() {
        return Ok(Json(ValidateResponse {
            valid: false,
            errors: vec![e.to_string()],
            rules_count: Some(dag.node_count()),
            edges_count: Some(dag.edge_count()),
        }));
    }

    Ok(Json(ValidateResponse {
        valid: true,
        errors: vec![],
        rules_count: Some(dag.node_count()),
        edges_count: Some(dag.edge_count()),
    }))
}

/// `POST /api/workflows/parse` — Parse a workflow and return full detail.
async fn parse_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<WorkflowDetail>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let rules: Vec<RuleSummary> = config
        .rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.to_vec(),
            outputs: r.output.to_vec(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    Ok(Json(WorkflowDetail {
        name: config.workflow.name.clone(),
        version: config.workflow.version.clone(),
        description: config.workflow.description.clone(),
        author: config.workflow.author.clone(),
        rules_count: rules.len(),
        rules,
    }))
}

/// `POST /api/workflows/dag` — Build a DAG and return its DOT representation.
async fn build_dag(Json(req): Json<ValidateRequest>) -> Result<Json<DagResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    Ok(Json(DagResponse {
        dot: dag.to_dot(),
        nodes: dag.node_count(),
        edges: dag.edge_count(),
    }))
}

/// `POST /api/workflows/dry-run` — Simulate execution and return the plan.
async fn dry_run(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let rules: Vec<RuleSummary> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.to_vec(),
            outputs: r.output.to_vec(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    let run_config = req.config.unwrap_or(RunConfig {
        max_jobs: None,
        dry_run: None,
        keep_going: None,
    });

    let status = RunStatus {
        id: uuid::Uuid::new_v4().to_string(),
        status: "dry-run".to_string(),
        rules_total: rules.len(),
        rules_completed: 0,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
    };

    #[derive(Serialize)]
    struct DryRunResponse {
        status: RunStatus,
        execution_order: Vec<String>,
        rules: Vec<RuleSummary>,
        config: RunConfig,
    }

    Ok(Json(DryRunResponse {
        status,
        execution_order: order,
        rules,
        config: run_config,
    }))
}

/// `POST /api/reports/generate` — Generate a report from a workflow.
async fn generate_report(Json(req): Json<ReportRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    // Build a report with workflow overview and rule details.
    let mut report = oxo_flow_core::report::Report::new(
        &format!("{} — Workflow Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );

    report.add_metadata("rules_count", &config.rules.len().to_string());
    report.add_metadata("edges_count", &dag.edge_count().to_string());

    // Overview section
    let overview = oxo_flow_core::report::ReportSection {
        title: "Workflow Overview".to_string(),
        id: "overview".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue {
            pairs: vec![
                ("Name".to_string(), config.workflow.name.clone()),
                ("Version".to_string(), config.workflow.version.clone()),
                (
                    "Description".to_string(),
                    config
                        .workflow
                        .description
                        .clone()
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
                ("Rules".to_string(), config.rules.len().to_string()),
                ("DAG edges".to_string(), dag.edge_count().to_string()),
            ],
        },
        subsections: vec![],
    };
    report.add_section(overview);

    // Execution order section
    let exec_section = oxo_flow_core::report::ReportSection {
        title: "Execution Order".to_string(),
        id: "execution-order".to_string(),
        content: oxo_flow_core::report::ReportContent::Table {
            headers: vec![
                "Step".to_string(),
                "Rule".to_string(),
                "Threads".to_string(),
                "Environment".to_string(),
            ],
            rows: order
                .iter()
                .enumerate()
                .filter_map(|(i, name)| {
                    config.get_rule(name).map(|r| {
                        vec![
                            (i + 1).to_string(),
                            r.name.clone(),
                            r.effective_threads().to_string(),
                            r.environment.kind().to_string(),
                        ]
                    })
                })
                .collect(),
        },
        subsections: vec![],
    };
    report.add_section(exec_section);

    let format = req.format.unwrap_or_else(|| "html".to_string());

    match format.as_str() {
        "json" => {
            let json = report.to_json().map_err(|e| {
                ApiError::unprocessable("Report generation failed", Some(e.to_string()))
            })?;
            Ok((StatusCode::OK, [("content-type", "application/json")], json))
        }
        _ => {
            let html = report.to_html();
            Ok((StatusCode::OK, [("content-type", "text/html")], html))
        }
    }
}

/// `POST /api/workflows/run` — Initialize a run and start it in the background.
async fn run_workflow(
    headers: axum::http::HeaderMap,
    Json(req): Json<DryRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    // Fetch full user details for auth_type and os_user
    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found in DB", None))?;

    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let run_id = uuid::Uuid::new_v4().to_string();

    // 1. Initialize physical sandbox
    workspace::initialize_sandbox(&user.username, &run_id, &req.toml_content)
        .map_err(|e| ApiError::unprocessable("Failed to setup sandbox", Some(e.to_string())))?;

    // 2. Insert run record into DB
    let run = db::Run {
        id: run_id.clone(),
        user_id: user.id.clone(),
        workflow_name: config.workflow.name.clone(),
        status: "pending".to_string(),
        pid: None,
        started_at: None,
        finished_at: None,
    };
    db::insert_run(&run)
        .await
        .map_err(|e| ApiError::unprocessable("Failed to save run record", Some(e.to_string())))?;

    // 3. Log the action
    let _ = db::log_action(&user.id, "run", &config.workflow.name).await;

    // 4. Spawn background executor
    executor::spawn_background_run(
        run_id.clone(),
        user.username.clone(),
        user.auth_type.clone(),
        user.os_user.clone(),
    );

    ACTIVE_WORKFLOWS.fetch_add(1, Ordering::Relaxed);

    Ok(Json(RunResponse {
        run_id,
        status: "started".to_string(),
        execution_order: order,
        rules_total: config.rules.len(),
    }))
}

/// `GET /api/version` — Return crate version and build info.
async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        crate_name: env!("CARGO_PKG_NAME").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// `POST /api/workflows/clean` — List output files that would be cleaned.
async fn clean_workflow(Json(req): Json<ValidateRequest>) -> Result<Json<CleanResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let files_to_clean: Vec<String> = config
        .rules
        .iter()
        .flat_map(|r| r.output.to_vec())
        .collect();

    Ok(Json(CleanResponse {
        workflow_name: config.workflow.name.clone(),
        total_files: files_to_clean.len(),
        files_to_clean,
    }))
}

// ---------------------------------------------------------------------------
// Request ID middleware
// ---------------------------------------------------------------------------

/// Middleware that attaches a unique `x-request-id` header to every response.
async fn add_request_id(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("x-request-id"),
        axum::http::HeaderValue::from_str(&request_id).unwrap(),
    );
    response
}

// ---------------------------------------------------------------------------
// Export endpoint
// ---------------------------------------------------------------------------

/// `POST /api/workflows/export` — Generate a Dockerfile or Singularity def.
async fn export_workflow(Json(req): Json<ExportRequest>) -> Result<Json<ExportResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let format = req.format.unwrap_or_else(|| "docker".to_string());
    let pkg_config = oxo_flow_core::container::PackageConfig::default();

    let content = match format.as_str() {
        "singularity" => oxo_flow_core::container::generate_singularity_def(&config, &pkg_config)
            .map_err(|e| {
            ApiError::unprocessable("Singularity def generation failed", Some(e.to_string()))
        })?,
        _ => oxo_flow_core::container::generate_dockerfile(&config, &pkg_config).map_err(|e| {
            ApiError::unprocessable("Dockerfile generation failed", Some(e.to_string()))
        })?,
    };

    let actual_format = match format.as_str() {
        "singularity" => "singularity".to_string(),
        _ => "docker".to_string(),
    };

    Ok(Json(ExportResponse {
        format: actual_format,
        content,
    }))
}

// ---------------------------------------------------------------------------
// Frontend & new endpoints
// ---------------------------------------------------------------------------

/// Serve the embedded frontend HTML.
async fn frontend() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        FRONTEND_HTML,
    )
}

/// `POST /api/workflows/format` — Format a workflow TOML into canonical form.
async fn format_workflow_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<FormatResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let formatted = oxo_flow_core::format::format_workflow(&config);

    Ok(Json(FormatResponse { formatted }))
}

/// `POST /api/workflows/lint` — Lint a workflow for best practices.
async fn lint_workflow(Json(req): Json<LintRequest>) -> Result<Json<LintResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint_diags = oxo_flow_core::format::lint_format(&config);

    let mut diagnostics = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;

    for d in validation.diagnostics.iter().chain(lint_diags.iter()) {
        let severity = match d.severity {
            oxo_flow_core::format::Severity::Error => {
                error_count += 1;
                "error"
            }
            oxo_flow_core::format::Severity::Warning => {
                warning_count += 1;
                "warning"
            }
            oxo_flow_core::format::Severity::Info => {
                info_count += 1;
                "info"
            }
        };
        diagnostics.push(DiagnosticItem {
            severity: severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }

    Ok(Json(LintResponse {
        diagnostics,
        error_count,
        warning_count,
        info_count,
    }))
}

/// `POST /api/workflows/lint/paginated` — Lint with paginated results.
async fn lint_workflow_paginated(
    pagination: axum::extract::Query<PaginationParams>,
    Json(req): Json<LintRequest>,
) -> Result<Json<PaginatedLintResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("parse error", Some(e.to_string())))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint = oxo_flow_core::format::lint_format(&config);

    let mut all_diagnostics: Vec<DiagnosticItem> = Vec::new();
    for d in &validation.diagnostics {
        all_diagnostics.push(DiagnosticItem {
            severity: d.severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }
    for d in &lint {
        all_diagnostics.push(DiagnosticItem {
            severity: d.severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }

    let error_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .count();
    let warning_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "warning")
        .count();
    let info_count = all_diagnostics
        .iter()
        .filter(|d| d.severity == "info")
        .count();

    let total = all_diagnostics.len();
    let per_page = pagination.clamped_per_page();
    let offset = pagination.offset();

    let page_items: Vec<DiagnosticItem> = all_diagnostics
        .into_iter()
        .skip(offset)
        .take(per_page)
        .collect();

    Ok(Json(PaginatedLintResponse {
        diagnostics: page_items,
        pagination: PaginationMeta::new(pagination.page, per_page, total),
        summary: LintSummary {
            error_count,
            warning_count,
            info_count,
        },
    }))
}

/// `POST /api/workflows/stats` — Return workflow statistics.
async fn workflow_stats_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<StatsResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let stats = oxo_flow_core::format::workflow_stats(&config);

    Ok(Json(StatsResponse {
        rule_count: stats.rule_count,
        shell_rules: stats.shell_rules,
        script_rules: stats.script_rules,
        dependency_count: stats.dependency_count,
        parallel_groups: stats.parallel_groups,
        max_depth: stats.max_depth,
        environments: stats.environments,
        total_threads: stats.total_threads,
        wildcard_count: stats.wildcard_count,
        wildcard_names: stats.wildcard_names,
    }))
}

/// `POST /api/workflows/diff` — Compare two workflow configurations.
async fn diff_workflows_endpoint(
    Json(req): Json<DiffRequest>,
) -> Result<Json<DiffResponse>, ApiError> {
    let config_a = oxo_flow_core::WorkflowConfig::parse(&req.toml_a)
        .map_err(|e| ApiError::bad_request("Invalid first workflow TOML", Some(e.to_string())))?;
    let config_b = oxo_flow_core::WorkflowConfig::parse(&req.toml_b)
        .map_err(|e| ApiError::bad_request("Invalid second workflow TOML", Some(e.to_string())))?;

    let diffs = oxo_flow_core::format::diff_workflows(&config_a, &config_b);

    Ok(Json(DiffResponse {
        diff_count: diffs.len(),
        diffs: diffs
            .into_iter()
            .map(|d| DiffEntry {
                category: d.category,
                description: d.description,
            })
            .collect(),
    }))
}

/// `GET /api/system` — Return system information.
async fn system_info() -> Json<SystemInfo> {
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
async fn runtime_metrics() -> Json<RuntimeMetrics> {
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

/// `GET /api/events` — SSE endpoint for real-time execution events.
async fn sse_events() -> impl IntoResponse {
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

// ---------------------------------------------------------------------------
// Authentication & license endpoints
// ---------------------------------------------------------------------------

/// `POST /api/auth/login` — Authenticate and obtain a session token.
async fn login(Json(req): Json<LoginRequest>) -> Result<Json<LoginResponse>, ApiError> {
    let user = check_credentials_db(&req.username, &req.password)
        .await
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                error: "Invalid credentials".to_string(),
                detail: None,
            },
        })?;

    let role = user.role.clone();
    let token = generate_session_token();

    // Set expiration to 24 hours from now
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(24);

    let session = db::Session {
        token: token.clone(),
        user_id: user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at,
    };

    db::create_session(&session).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: ErrorResponse {
            error: "Failed to create session".to_string(),
            detail: Some(e.to_string()),
        },
    })?;

    Ok(Json(LoginResponse {
        token,
        username: user.username,
        role,
    }))
}

/// `GET /api/auth/me` — Return the identity of the current session.
async fn auth_me(headers: axum::http::HeaderMap) -> Json<AuthMeResponse> {
    match extract_session(&headers).await {
        Some(session) => {
            if let Ok(Some(user)) = db::get_user_by_id(&session.user_id).await {
                Json(AuthMeResponse {
                    authenticated: true,
                    username: Some(user.username),
                    role: Some(user.role),
                })
            } else {
                Json(AuthMeResponse {
                    authenticated: false,
                    username: None,
                    role: None,
                })
            }
        }
        None => Json(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        }),
    }
}

/// `GET /api/license` — Return current license status.
async fn license_status() -> Json<LicenseStatus> {
    Json(check_license())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the web application router.
pub fn build_router() -> Router {
    build_router_inner(None)
}

/// Build the web application router with an optional rate limiter.
pub fn build_router_with_rate_limiter(limiter: RateLimiter) -> Router {
    build_router_inner(Some(limiter))
}

async fn list_runs(headers: axum::http::HeaderMap) -> Result<Json<Vec<db::Run>>, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let runs = sqlx::query_as::<_, db::Run>(
        "SELECT * FROM runs WHERE user_id = ? ORDER BY started_at DESC",
    )
    .bind(&user.id)
    .fetch_all(db::pool())
    .await
    .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    Ok(Json(runs))
}

async fn get_run_logs(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify ownership
    let _run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                error: "Run not found".to_string(),
                detail: None,
            },
        })?;

    let run_dir = workspace::get_run_directory(&user.username, &run_id);
    let log_path = run_dir.join("execution.log");

    if !log_path.exists() {
        return Err(ApiError::bad_request("Log file not found", None));
    }

    let content = std::fs::read_to_string(log_path)
        .map_err(|e| ApiError::unprocessable("Failed to read log", Some(e.to_string())))?;

    Ok(content)
}

async fn cancel_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Authentication required".to_string(),
            detail: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    // Verify ownership
    let run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                error: "Run not found".to_string(),
                detail: None,
            },
        })?;

    if run.status != "running" && run.status != "pending" {
        return Err(ApiError::bad_request(
            "Run is not in a cancellable state",
            Some(run.status),
        ));
    }

    // Cancel the run (kill the process if it exists)
    if let Some(pid) = run.pid {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        if let Some(process) = sys.process(sysinfo::Pid::from_u32(pid as u32)) {
            process.kill();
        }
    }

    sqlx::query("UPDATE runs SET status = 'cancelled', finished_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(&run_id)
        .execute(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

    let _ = db::log_action(&user.id, "cancel_run", &run_id).await;

    // Broadcast cancellation event
    broadcast_event(
        "run_cancelled",
        &serde_json::json!({
            "run_id": run_id,
            "status": "cancelled"
        }),
    );

    Ok((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "status": "cancelled" })),
    ))
}

/// Build the web application router_inner function with new endpoints.
fn build_router_inner(limiter: Option<RateLimiter>) -> Router {
    // Initialize start time
    get_start_time();

    // Check for custom CORS origins
    let origins: Vec<axum::http::HeaderValue> = std::env::var("OXO_FLOW_ALLOWED_ORIGINS")
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_default();

    let cors = if origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_credentials(true)
    };

    let mut router = Router::new()
        // Frontend
        .route("/", get(frontend))
        // API endpoints
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/system", get(system_info))
        .route("/api/metrics", get(runtime_metrics))
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/validate", post(validate_workflow))
        .route("/api/workflows/parse", post(parse_workflow))
        .route("/api/workflows/dag", post(build_dag))
        .route("/api/workflows/dry-run", post(dry_run))
        .route("/api/workflows/run", post(run_workflow))
        .route("/api/workflows/clean", post(clean_workflow))
        .route("/api/workflows/export", post(export_workflow))
        .route("/api/workflows/format", post(format_workflow_endpoint))
        .route("/api/workflows/lint", post(lint_workflow))
        .route(
            "/api/workflows/lint/paginated",
            post(lint_workflow_paginated),
        )
        .route("/api/workflows/stats", post(workflow_stats_endpoint))
        .route("/api/workflows/diff", post(diff_workflows_endpoint))
        .route("/api/environments", get(list_environments))
        .route("/api/reports/generate", post(generate_report))
        .route("/api/events", get(sse_events))
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{id}", delete(cancel_run))
        .route("/api/runs/{id}/logs", get(get_run_logs))
        // Authentication & license
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(auth_me))
        .route("/api/license", get(license_status))
        .fallback(not_found)
        .layer(middleware::from_fn(add_request_id))
        .layer(middleware::from_fn(rate_limit_middleware));

    if let Some(limiter) = limiter {
        router = router.layer(axum::Extension(limiter));
    }

    router.layer(cors)
}

/// Build a router mounted under a configurable base path.
pub fn build_router_with_base(base_path: &str) -> Router {
    let app = build_router();
    if base_path.is_empty() || base_path == "/" {
        app
    } else {
        Router::new().nest(base_path, app)
    }
}

/// Start the web server with graceful shutdown support.
pub async fn start_server(host: &str, port: u16) -> anyhow::Result<()> {
    crate::db::init_db("sqlite://oxo-flow.db").await?;
    crate::db::recover_orphaned_runs().await?;
    let app = build_router();
    let addr = format!("{host}:{port}");
    tracing::info!("Starting oxo-flow web server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Start the web server with an optional base path and graceful shutdown.
pub async fn start_server_with_base(host: &str, port: u16, base_path: &str) -> anyhow::Result<()> {
    crate::db::init_db("sqlite://oxo-flow.db").await?;
    crate::db::recover_orphaned_runs().await?;
    let app = build_router_with_base(base_path);
    let addr = format!("{host}:{port}");
    tracing::info!(
        "Starting oxo-flow web server on {} (base: {})",
        addr,
        if base_path.is_empty() { "/" } else { base_path }
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A minimal valid workflow TOML used across tests.
    const VALID_TOML: &str = r#"
[workflow]
name = "test-pipeline"
version = "1.0.0"
description = "A test workflow"
author = "Test Author"

[[rules]]
name = "step_a"
input = ["raw/{sample}.fastq"]
output = ["trimmed/{sample}.fastq"]
shell = "trim {input} > {output}"
threads = 4

[[rules]]
name = "step_b"
input = ["trimmed/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell = "align {input} > {output}"
threads = 8
[rules.environment]
docker = "biocontainers/bwa:0.7.17"
"#;

    /// Helper: initialize SQLite for tests that need DB.
    ///
    /// Uses a per-process temp file so that all parallel test threads within
    /// one binary run share the same schema and seed data, while different
    /// test binary invocations start fresh.
    async fn init_test_db() {
        // Use process-specific temp database - each test process gets its own DB
        // This avoids locking issues when tests run in parallel within the same process
        let db_path = std::env::temp_dir().join(format!("oxo-flow-test-{}.db", std::process::id()));
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        db::init_db(&url)
            .await
            .expect("Failed to initialize test database");
    }

    /// Helper: send a POST request with a JSON body and return the response.
    async fn post_json(uri: &str, body: impl Serialize) -> axum::http::Response<Body> {
        let app = build_router();
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    /// Helper: send a POST request with auth header.
    async fn post_json_auth(
        uri: &str,
        body: impl Serialize,
        token: &str,
    ) -> axum::http::Response<Body> {
        let app = build_router();
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    /// Helper: send a GET request with auth header.
    async fn get_auth(uri: &str, token: &str) -> axum::http::Response<Body> {
        let app = build_router();
        app.oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    // -- Existing endpoint tests -------------------------------------------------

    #[tokio::test]
    async fn health_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "ok");
    }

    #[tokio::test]
    async fn workflows_endpoint() {
        init_test_db().await;
        // Login first to get a token
        let login_resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "admin".to_string(),
            },
        )
        .await;
        let body = axum::body::to_bytes(login_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let login: LoginResponse = serde_json::from_slice(&body).unwrap();

        let response = get_auth("/api/workflows", &login.token).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn environments_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/environments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn not_found_fallback() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.error, "Not found");
    }

    // -- Validate endpoint -------------------------------------------------------

    #[tokio::test]
    async fn validate_valid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.valid);
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.rules_count, Some(2));
        assert!(parsed.edges_count.is_some());
    }

    #[tokio::test]
    async fn validate_invalid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: "this is not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.valid);
        assert!(!parsed.errors.is_empty());
    }

    // -- Parse endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn parse_valid_workflow() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: WorkflowDetail = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail.name, "test-pipeline");
        assert_eq!(detail.version, "1.0.0");
        assert_eq!(detail.description.as_deref(), Some("A test workflow"));
        assert_eq!(detail.author.as_deref(), Some("Test Author"));
        assert_eq!(detail.rules_count, 2);
        assert_eq!(detail.rules[0].name, "step_a");
        assert_eq!(detail.rules[0].threads, 4);
        assert_eq!(detail.rules[1].environment, "docker");
    }

    #[tokio::test]
    async fn parse_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: "not valid".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    // -- DAG endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn dag_generation() {
        let resp = post_json(
            "/api/workflows/dag",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dag: DagResponse = serde_json::from_slice(&body).unwrap();
        assert!(dag.dot.contains("digraph"));
        assert_eq!(dag.nodes, 2);
        assert!(dag.edges >= 1);
    }

    // -- Dry-run endpoint --------------------------------------------------------

    #[tokio::test]
    async fn dry_run_endpoint() {
        let resp = post_json(
            "/api/workflows/dry-run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: Some(RunConfig {
                    max_jobs: Some(4),
                    dry_run: Some(true),
                    keep_going: Some(false),
                }),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let status = &value["status"];
        assert_eq!(status["status"], "dry-run");
        assert_eq!(status["rules_total"], 2);
        assert_eq!(status["rules_completed"], 0);

        let order = value["execution_order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        // step_a must come before step_b (dependency)
        assert_eq!(order[0], "step_a");
        assert_eq!(order[1], "step_b");

        let rules = value["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
    }

    // -- Report generation endpoint ----------------------------------------------

    #[tokio::test]
    async fn report_generate_html() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: None, // default → html
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-pipeline"));
    }

    #[tokio::test]
    async fn report_generate_json() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: Some("json".to_string()),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/json");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["workflow_name"], "test-pipeline");
        assert!(value["sections"].as_array().unwrap().len() >= 2);
    }

    // -- Run endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn run_workflow_endpoint() {
        init_test_db().await;
        // Login first to get a token
        let login_resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "admin".to_string(),
            },
        )
        .await;
        let body = axum::body::to_bytes(login_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let login: LoginResponse = serde_json::from_slice(&body).unwrap();

        let resp = post_json_auth(
            "/api/workflows/run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: None,
            },
            &login.token,
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: RunResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "started");
        assert_eq!(parsed.rules_total, 2);
        assert!(!parsed.run_id.is_empty());
        assert_eq!(parsed.execution_order.len(), 2);
        assert_eq!(parsed.execution_order[0], "step_a");
        assert_eq!(parsed.execution_order[1], "step_b");
    }

    // -- Version endpoint --------------------------------------------------------

    #[tokio::test]
    async fn version_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: VersionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(parsed.crate_name, "oxo-flow-web");
    }

    // -- Clean endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn clean_workflow_endpoint() {
        let resp = post_json(
            "/api/workflows/clean",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: CleanResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.workflow_name, "test-pipeline");
        assert_eq!(parsed.total_files, 2);
        assert!(
            parsed
                .files_to_clean
                .contains(&"trimmed/{sample}.fastq".to_string())
        );
        assert!(
            parsed
                .files_to_clean
                .contains(&"aligned/{sample}.bam".to_string())
        );
    }

    // -- Additional error-path & edge-case tests --------------------------------

    #[tokio::test]
    async fn run_invalid_toml_returns_400() {
        init_test_db().await;
        // Login first to get a token
        let login_resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "admin".to_string(),
            },
        )
        .await;
        let body = axum::body::to_bytes(login_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let login: LoginResponse = serde_json::from_slice(&body).unwrap();

        let resp = post_json_auth(
            "/api/workflows/run",
            &DryRunRequest {
                toml_content: "not valid toml {{{{".to_string(),
                config: None,
            },
            &login.token,
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn clean_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/clean",
            &ValidateRequest {
                toml_content: "not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn validate_workflow_with_cycle() {
        let cycle_toml = r#"
[workflow]
name = "cycle-test"
version = "1.0.0"

[[rules]]
name = "step_a"
input = ["b_output.txt"]
output = ["a_output.txt"]
shell = "echo a"

[[rules]]
name = "step_b"
input = ["a_output.txt"]
output = ["b_output.txt"]
shell = "echo b"
"#;

        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: cycle_toml.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.valid);
        assert!(!parsed.errors.is_empty());
        let joined = parsed.errors.join(" ");
        assert!(
            joined.to_lowercase().contains("cycle"),
            "expected cycle error, got: {joined}"
        );
    }

    #[tokio::test]
    async fn dag_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/dag",
            &ValidateRequest {
                toml_content: "not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn dry_run_without_config() {
        let resp = post_json(
            "/api/workflows/dry-run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: None,
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let status = &value["status"];
        assert_eq!(status["status"], "dry-run");
        assert_eq!(status["rules_total"], 2);

        let order = value["execution_order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], "step_a");
        assert_eq!(order[1], "step_b");
    }

    // -- Export endpoint ---------------------------------------------------------

    #[tokio::test]
    async fn export_workflow_docker() {
        let resp = post_json(
            "/api/workflows/export",
            &ExportRequest {
                toml_content: VALID_TOML.to_string(),
                format: None, // default → docker
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ExportResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.format, "docker");
        assert!(parsed.content.contains("FROM"));
        assert!(parsed.content.contains("test-pipeline"));
    }

    // -- Frontend endpoint -------------------------------------------------------

    #[tokio::test]
    async fn frontend_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("oxo-flow"));
        assert!(html.contains("Command Center"));
    }

    // -- System info endpoint ----------------------------------------------------

    #[tokio::test]
    async fn system_info_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/system")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let info: SystemInfo = serde_json::from_slice(&body).unwrap();
        assert!(!info.version.is_empty());
        assert!(!info.os.is_empty());
    }

    // -- Format endpoint ---------------------------------------------------------

    #[tokio::test]
    async fn format_endpoint() {
        let body = ValidateRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/format", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: FormatResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp.formatted.contains("[workflow]"));
    }

    #[tokio::test]
    async fn format_endpoint_invalid_toml() {
        let body = ValidateRequest {
            toml_content: "broken!!!".to_string(),
        };
        let response = post_json("/api/workflows/format", &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- Lint endpoint -----------------------------------------------------------

    #[tokio::test]
    async fn lint_endpoint() {
        let body = LintRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/lint", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: LintResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.error_count, 0);
    }

    #[tokio::test]
    async fn lint_endpoint_invalid_toml() {
        let body = LintRequest {
            toml_content: "not valid toml [[[".to_string(),
        };
        let response = post_json("/api/workflows/lint", &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- Stats endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn stats_endpoint() {
        let body = ValidateRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/stats", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: StatsResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.rule_count, 2);
    }

    // -- SSE events endpoint -----------------------------------------------------

    #[tokio::test]
    async fn events_endpoint_returns_sse() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // -- Auth endpoints ----------------------------------------------------------

    #[tokio::test]
    async fn login_valid_admin() {
        init_test_db().await;
        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "admin".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.username, "admin");
        assert_eq!(parsed.role, "admin");
        assert!(!parsed.token.is_empty());
    }

    #[tokio::test]
    async fn login_valid_user() {
        init_test_db().await;
        // Create a test user first
        let user_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(user_id)
        .bind("user")
        .bind("user")
        .bind("sudo")
        .bind("oxo-flow")
        .bind(now)
        .execute(db::pool())
        .await
        .unwrap();

        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "user".to_string(),
                password: "user".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.role, "user");
    }

    #[tokio::test]
    async fn login_invalid_credentials() {
        init_test_db().await;
        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "wrong".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(err.error.contains("Invalid"));
    }

    #[tokio::test]
    async fn auth_me_without_token() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: AuthMeResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.authenticated);
        assert!(parsed.username.is_none());
    }

    // -- License endpoint --------------------------------------------------------

    #[tokio::test]
    async fn license_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/license")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: LicenseStatus = serde_json::from_slice(&body).unwrap();
        // No license file installed in test environment
        assert!(!status.valid);
        assert!(!status.message.is_empty());
    }

    // ---- /api/metrics endpoint test -----------------------------------------

    #[tokio::test]
    async fn metrics_endpoint_returns_runtime_metrics() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let metrics: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(metrics.get("uptime_secs").is_some());
        assert!(metrics.get("version").is_some());
        assert!(metrics.get("pid").is_some());
        assert!(metrics.get("os").is_some());
        assert!(metrics.get("arch").is_some());
        assert!(metrics.get("cpu_count").is_some());
        assert!(metrics["cpu_count"].as_u64().unwrap() >= 1);
    }

    // ---- /api/workflows/diff endpoint tests ---------------------------------

    #[tokio::test]
    async fn diff_endpoint_identical_workflows() {
        let resp = post_json(
            "/api/workflows/diff",
            &serde_json::json!({
                "toml_a": VALID_TOML,
                "toml_b": VALID_TOML,
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["diff_count"], 0);
        assert!(parsed["diffs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn diff_endpoint_different_workflows() {
        let toml_b = r#"
[workflow]
name = "test-pipeline-v2"
version = "2.0.0"
description = "A test workflow"
author = "Test Author"

[[rules]]
name = "step_a"
input = ["raw/{sample}.fastq"]
output = ["trimmed/{sample}.fastq"]
shell = "trim {input} > {output}"
threads = 4
"#;
        let resp = post_json(
            "/api/workflows/diff",
            &serde_json::json!({
                "toml_a": VALID_TOML,
                "toml_b": toml_b,
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed["diff_count"].as_u64().unwrap() > 0);
        assert!(!parsed["diffs"].as_array().unwrap().is_empty());
    }

    // ---- build_router_with_base tests ---------------------------------------

    #[tokio::test]
    async fn router_with_base_path_nested() {
        let app = build_router_with_base("/oxo-flow");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/oxo-flow/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_with_root_base_path() {
        let app = build_router_with_base("/");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_with_empty_base_path() {
        let app = build_router_with_base("");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_pagination_params() {
        let params = PaginationParams {
            page: 1,
            per_page: 20,
        };
        assert_eq!(params.offset(), 0);
        assert_eq!(params.clamped_per_page(), 20);

        let params = PaginationParams {
            page: 3,
            per_page: 10,
        };
        assert_eq!(params.offset(), 20);

        let params = PaginationParams {
            page: 1,
            per_page: 200,
        };
        assert_eq!(params.clamped_per_page(), 100); // Clamped
    }

    #[tokio::test]
    async fn test_pagination_meta() {
        let meta = PaginationMeta::new(1, 10, 25);
        assert_eq!(meta.total_pages, 3);
        assert!(meta.has_next);
        assert!(!meta.has_prev);

        let meta = PaginationMeta::new(3, 10, 25);
        assert_eq!(meta.total_pages, 3);
        assert!(!meta.has_next);
        assert!(meta.has_prev);

        let meta = PaginationMeta::new(1, 10, 0);
        assert_eq!(meta.total_pages, 1);
        assert!(!meta.has_next);
        assert!(!meta.has_prev);
    }

    #[tokio::test]
    async fn test_paginated_lint_endpoint() {
        let app = build_router();
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            input = ["input.txt"]
            output = ["output.txt"]
            shell = "echo hello"
        "#;
        let body = serde_json::json!({ "toml_content": toml });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workflows/lint/paginated?page=1&per_page=5")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
