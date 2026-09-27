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
pub mod extract;
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
use std::net::ToSocketAddrs;
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

impl ApiError {
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
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// The host a server should actually bind, given the requested mode and
/// host. Single source of truth for both entry points (`oxo-flow serve`
/// and the standalone web binary):
///
/// - **Personal mode has no sign-in** — a non-loopback bind would expose
///   unauthenticated management endpoints to the network, so the bind is
///   forced to `127.0.0.1` with a loud warning.
/// - **`OXO_FLOW_DEV_MODE=1`** accepts `password == username` logins for
///   any user — refusing to start on a non-loopback bind.
pub fn effective_bind_host(mode: &str, host: &str) -> anyhow::Result<String> {
    let dev_mode = std::env::var("OXO_FLOW_DEV_MODE").as_deref() == Ok("1");
    effective_bind_host_with(mode, host, dev_mode)
}

/// Pure core of [`effective_bind_host`] — `dev_mode` is passed in so tests
/// need no environment mutation.
fn effective_bind_host_with(mode: &str, host: &str, dev_mode: bool) -> anyhow::Result<String> {
    fn is_loopback_host(host: &str) -> bool {
        matches!(host, "127.0.0.1" | "::1" | "localhost")
    }
    if mode == "personal" {
        if is_loopback_host(host) {
            return Ok(host.to_string());
        }
        tracing::warn!(
            "personal mode requires sign-in credentials that are not \
             enforced, forcing loopback bind instead of '{host}'"
        );
        return Ok("127.0.0.1".to_string());
    }
    if dev_mode && !is_loopback_host(host) {
        anyhow::bail!(
            "OXO_FLOW_DEV_MODE=1 accepts password==username logins for \
             any user and is only safe on a loopback bind; refusing to \
             start on '{host}'. Unset OXO_FLOW_DEV_MODE or bind to 127.0.0.1."
        );
    }
    Ok(host.to_string())
}

/// Resolve the effective host into a concrete [`std::net::SocketAddr`].
///
/// `oxo-flow serve` binds `TcpListener::bind(format!("{host}:{port}"))`,
/// which accepts hostnames ("localhost") as-is; the standalone binary
/// instead needs a `SocketAddr`, and a bare `IpAddr` parse rejects the
/// "localhost" that `effective_bind_host` deliberately passes through
/// (issue #573). IPs parse directly; anything else resolves via
/// `ToSocketAddrs`, preferring IPv4 so the bind stays deterministic —
/// Linux resolves "localhost" to `::1` first, which silently locks out
/// IPv4 clients (and the IPv4-based integration checks); unresolvable
/// hosts fail with an actionable message.
pub fn resolve_bind_addr(host: &str, port: u16) -> anyhow::Result<std::net::SocketAddr> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(std::net::SocketAddr::new(ip, port));
    }
    let mut addrs = (host, port).to_socket_addrs().map_err(|e| {
        anyhow::anyhow!(
            "failed to resolve bind host {host:?}: {e} — use an IP address \
             such as 127.0.0.1 or 0.0.0.0, or a hostname resolvable on this \
             machine"
        )
    })?;
    let pick = addrs.find(|addr| addr.is_ipv4()).or_else(|| addrs.next());
    pick.ok_or_else(|| {
        anyhow::anyhow!(
            "bind host {host:?} resolved to no addresses — \
             use an IP address such as 127.0.0.1 or 0.0.0.0"
        )
    })
}

/// Capability matrix advertised at startup when `DATABASE_URL` selects
/// PostgreSQL.
///
/// It must describe what this build actually serves: the HTTP layer is not
/// yet PostgreSQL-aware — every domain handler (auth sessions included)
/// reads the SQLite pool, so a PG deployment answers 503 `DB_ERROR` on
/// DB-backed routes instead of silently handing out session tokens that are
/// never persisted (logins then 401 forever).
pub const PG_CAPABILITY_MATRIX: &str = "PostgreSQL deployment capability matrix:\n  \
     available: /api/health · /api/license · /api/openapi.json · SPA assets\n  \
     unavailable 503 DB_ERROR (SQLite-only handlers): auth sessions — login tokens are \
     NOT persisted, later requests 401 — plus pipelines, templates, shares, clusters, \
     audit, AI, chat\n  \
     gated 503 RUNS_REQUIRE_SQLITE: every /api/runs* endpoint \
     (run execution is SQLite-only)";

/// Whether a database URL selects the PostgreSQL backend.
pub fn is_postgres_url(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// Initialize the database layer from `DATABASE_URL` — shared startup step
/// for every serving entry point (standalone binary, `oxo-flow serve`,
/// desktop shell).
///
/// A `postgres://`/`postgresql://` URL selects the PostgreSQL backend on
/// builds with the `postgres` feature (issue #207's capability matrix is
/// logged so operators see the served/gated split before the first 503);
/// anything else is treated as a SQLite URL, defaulting to
/// `sqlite://oxo-flow.db` in the working directory.
pub async fn init_database() -> anyhow::Result<()> {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://oxo-flow.db".to_string());

    if is_postgres_url(&database_url) {
        #[cfg(feature = "postgres")]
        {
            tracing::info!("Initializing PostgreSQL backend");
            crate::infra::db::postgres::init_pool(&database_url).await;
            tracing::warn!("{}", PG_CAPABILITY_MATRIX);
        }
        #[cfg(not(feature = "postgres"))]
        {
            let _ = database_url;
            tracing::error!(
                "DATABASE_URL is a PostgreSQL URL but the 'postgres' feature is not enabled. \
                 Rebuild with: cargo build --features postgres"
            );
            anyhow::bail!("PostgreSQL support not compiled in — rebuild with --features postgres");
        }
    } else {
        crate::db::init_db(&database_url).await?;
        crate::db::recover_orphaned_runs().await?;
        // Also initialize the v0.8 domain-driven DB pool for domain handlers.
        crate::infra::db::sqlite::init_pool(&database_url).await;
    }
    Ok(())
}

/// Initialize the AI provider stack — shared startup step.
///
/// Tier order: environment variables → at-rest-encryption notice →
/// DB-persisted settings (when env did not configure a provider) → platform
/// config file (lowest; secrets stay in env vars referenced by
/// `api_key_env`, never inline). Issue #569: this sequence was previously
/// standalone-binary-only, so `oxo-flow serve` and the desktop shell lost
/// saved provider keys on every restart.
pub async fn init_ai_provider(platform_config: Option<&crate::config::WebConfig>) {
    crate::ai_provider::AiProviderRegistry::global().init_from_env();
    // Issue #205: at-rest encryption is opt-in. The shared notice lives in
    // infra::crypto so every entry point emits the same warning.
    crate::infra::crypto::warn_if_plaintext_key();
    // Restore the DB-persisted tier (settings UI) when env did not configure
    // a provider — otherwise a saved key would be lost on restart.
    crate::domains::ai::handlers::restore_ai_config_from_db().await;
    if let Some(cfg) = platform_config
        && crate::ai_provider::AiProviderRegistry::global()
            .get_config()
            .provider
            == "disabled"
        && let Some(provider) = cfg.ai.provider.as_deref()
    {
        let api_key = cfg
            .ai
            .api_key_env
            .as_deref()
            .and_then(|key_env| std::env::var(key_env).ok());
        if let Err(e) = crate::ai_provider::AiProviderRegistry::global().reconfigure(
            provider,
            api_key,
            cfg.ai.api_url.clone(),
            cfg.ai.model.clone(),
        ) {
            tracing::warn!("AI config file tier rejected: {e}");
        }
    }
    tracing::info!(
        "AI provider: {}",
        crate::ai_provider::AiProviderRegistry::global()
            .get_config()
            .provider
    );
}

pub async fn start_server_with_mode(
    mode: &str,
    host: &str,
    port: u16,
    base_path: &str,
) -> anyhow::Result<()> {
    // Auth-boundary enforcement (shared with the standalone binary).
    let host = effective_bind_host(mode, host)?;
    crate::init_database().await?;

    // Cluster definitions from the platform config file are imported by both
    // serving entry points (this one and the standalone web binary), each
    // calling the same idempotent import — existing DB rows win.
    let platform_config = crate::config::load();
    if let Some(cfg) = &platform_config {
        crate::domains::clusters::handlers::import_from_config(&cfg.clusters).await;
    }

    // Initialize structured logging
    let log_dir = std::path::PathBuf::from("logs");
    if let Err(e) = crate::domains::observability::logging::init_logging(&log_dir) {
        tracing::warn!("Failed to initialize structured logging: {e}");
    }

    // Initialize AI provider (env → DB → config file tiers; shared with the
    // standalone binary via the hoisted helper — issue #569).
    crate::init_ai_provider(platform_config.as_ref()).await;

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

    // Background maintenance every serving entry point needs (idempotent).
    start_background_tasks();

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // Share URLs report the real bound port (issue #82 P0-6): with port 0
    // or an already-taken port the requested number is not what we got.
    let bound = listener.local_addr()?.port();
    crate::server::set_bound_port(bound);
    // The connect-info service feeds the rate limiter's peer-address key;
    // without it the limiter can only fall back to one shared bucket.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

/// Start the background maintenance tasks every serving entry point needs.
///
/// Idempotent — a second call is a no-op — so both `oxo-flow serve` (this
/// library) and the standalone `oxo-flow-web` binary can call it without
/// double-spawning. The daily quota reset is the load-bearing one: without
/// it `runs_today` only ever grows and every `POST /api/runs` answers 429
/// until the process restarts.
pub fn start_background_tasks() {
    if BACKGROUND_TASKS_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    spawn_daily_quota_reset();
}

static BACKGROUND_TASKS_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether [`start_background_tasks`] has run in this process (test seam).
#[cfg(test)]
fn background_tasks_started() -> bool {
    BACKGROUND_TASKS_STARTED.load(std::sync::atomic::Ordering::SeqCst)
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

/// Process-global shutdown broadcast: fired once when [`shutdown_signal`]
/// resolves (SIGTERM/Ctrl+C), before the axum graceful drain begins.
///
/// Endpoints holding a connection open indefinitely — the `/api/events`
/// SSE stream — must end on shutdown, otherwise `.with_graceful_shutdown`
/// waits for them forever and the process only dies when the supervisor
/// SIGKILLs it after its grace period (issue #572).
static SHUTDOWN_TX: OnceLock<tokio::sync::watch::Sender<bool>> = OnceLock::new();

/// Subscribe to the process shutdown signal.
pub fn shutdown_rx() -> tokio::sync::watch::Receiver<bool> {
    SHUTDOWN_TX
        .get_or_init(|| tokio::sync::watch::channel(false).0)
        .subscribe()
}

fn fire_shutdown() {
    // `send` only fails with no receivers — nothing to notify then.
    let _ = SHUTDOWN_TX
        .get_or_init(|| tokio::sync::watch::channel(false).0)
        .send(true);
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
    // Release long-lived connections before the drain starts, so
    // `.with_graceful_shutdown` can actually finish (#572).
    fire_shutdown();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod effective_bind_host_tests {
    use super::*;

    /// The PG capability log is the operator-facing contract; it must not
    /// advertise SQLite-only surfaces (auth sessions) as available.
    #[test]
    fn pg_capability_matrix_is_truthful_about_auth() {
        let line = |prefix: &str| {
            PG_CAPABILITY_MATRIX
                .lines()
                .find(|l| l.trim_start().starts_with(prefix))
                .unwrap_or_else(|| panic!("matrix lacks a '{prefix}' line: {PG_CAPABILITY_MATRIX}"))
        };

        let available = line("available:");
        assert!(
            !available.contains("auth"),
            "auth sessions are SQLite-only and must not be advertised as available: {available}"
        );
        let unavailable = line("unavailable");
        assert!(
            unavailable.contains("auth sessions"),
            "the SQLite-only auth limitation must be spelled out: {unavailable}"
        );
        assert!(
            PG_CAPABILITY_MATRIX.contains("RUNS_REQUIRE_SQLITE"),
            "the runs gate must stay in the matrix"
        );
    }

    /// Both serving entry points call `start_background_tasks`; a second
    /// call must not double-spawn the quota-reset loop.
    #[tokio::test]
    async fn background_tasks_start_exactly_once() {
        assert!(!background_tasks_started());
        start_background_tasks();
        assert!(background_tasks_started());
        start_background_tasks();
        assert!(background_tasks_started());
    }

    #[test]
    fn personal_mode_forces_loopback_for_non_loopback_hosts() {
        assert_eq!(
            effective_bind_host_with("personal", "0.0.0.0", false).unwrap(),
            "127.0.0.1"
        );
        assert_eq!(
            effective_bind_host_with("personal", "192.168.1.5", false).unwrap(),
            "127.0.0.1"
        );
        // Loopback hosts pass through untouched.
        assert_eq!(
            effective_bind_host_with("personal", "127.0.0.1", false).unwrap(),
            "127.0.0.1"
        );
        assert_eq!(
            effective_bind_host_with("personal", "localhost", false).unwrap(),
            "localhost"
        );
    }

    #[test]
    fn team_mode_binds_any_host_and_dev_mode_refuses_non_loopback() {
        assert_eq!(
            effective_bind_host_with("team", "0.0.0.0", false).unwrap(),
            "0.0.0.0"
        );
        assert!(effective_bind_host_with("team", "0.0.0.0", true).is_err());
        assert!(effective_bind_host_with("team", "127.0.0.1", true).is_ok());
    }

    /// Both prefixes accepted by the postgres gate; anything else —
    /// including URLs that merely mention postgres — stays SQLite.
    /// Bare `"postgres://"` matches the prefix and routes to PG, same as
    /// the standalone binary's gate this helper mirrors.
    #[test]
    fn postgres_url_detection_matches_standalone_gate() {
        assert!(is_postgres_url("postgres://u:p@db:5432/oxo"));
        assert!(is_postgres_url("postgresql://u:p@db:5432/oxo"));
        assert!(is_postgres_url("postgres://"));
        assert!(!is_postgres_url("sqlite://oxo-flow.db"));
        assert!(!is_postgres_url("sqlite::memory:"));
        // A SQLite file named like postgres must not be misrouted.
        assert!(!is_postgres_url("sqlite:postgres://weird.db"));
    }

    /// `resolve_bind_addr` must accept what `effective_bind_host` passes
    /// through: IPs parse directly, and "localhost" — deliberately not
    /// rewritten — resolves via `ToSocketAddrs` with IPv4 preferred, so
    /// the loopback bind is deterministic even where Linux resolves
    /// "localhost" to `::1` first (issue #573, parity with `oxo-flow serve`).
    #[test]
    fn resolve_bind_addr_accepts_ips_and_localhost() {
        assert_eq!(
            resolve_bind_addr("127.0.0.1", 8080).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(
            resolve_bind_addr("0.0.0.0", 3000).unwrap(),
            std::net::SocketAddr::from(([0, 0, 0, 0], 3000))
        );
        assert_eq!(
            resolve_bind_addr("::1", 8080).unwrap(),
            std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 8080))
        );
        // The regression: "localhost" must resolve, not fail an IP parse,
        // and prefer the IPv4 loopback over a `::1` first-hit.
        assert_eq!(
            resolve_bind_addr("localhost", 8080).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
    }

    /// Unresolvable hosts fail with the actionable message the issue
    /// asks for, not a bare parse error. An empty host is rejected by
    /// getaddrinfo itself, so the assertion holds on machines whose DNS
    /// wildcard-resolves bogus names (fake-ip VPN tunnels).
    #[test]
    fn resolve_bind_addr_names_unresolvable_hosts_actionably() {
        let err = resolve_bind_addr("", 8080).expect_err("empty host must fail");
        let msg = err.to_string();
        assert!(msg.contains("resolve"), "message should say why: {msg}");
        assert!(
            msg.contains("127.0.0.1") && msg.contains("0.0.0.0"),
            "message must point at usable alternatives: {msg}"
        );
    }
}
