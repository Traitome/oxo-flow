//! Domain-driven server router assembly.
//!
//! This module assembles the full application router from domain handler
//! modules.  Each domain (workflow, execution, auth, observability,
//! collaboration) contributes its own route group, keeping the router definition close to
//! the domain code it serves.

use axum::{
    Router,
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};

use crate::domains::*;
use crate::infra::license::LicenseHeaderLayer;

// ---------------------------------------------------------------------------
// Embedded SPA frontend
// ---------------------------------------------------------------------------

/// Serve the React SPA index.html, reading from disk to avoid
/// compile-time embedding mismatches with frontend build hashes.
///
/// Path resolution order:
/// 1. `OXO_FLOW_FRONTEND_DIR` env var (set in Docker/deployment)
/// 2. `--frontend-dir` CLI flag (via FRONTEND_DIR env var)
/// 3. Compile-time `crates/oxo-flow-web/static/` (dev)
async fn spa_index() -> impl IntoResponse {
    let fallback = r#"<!doctype html><html><body><h1>oxo-flow</h1><p>SPA not built. Run <code>npm run build</code> in frontend/ first.</p></body></html>"#;

    let html = std::env::var("OXO_FLOW_FRONTEND_DIR")
        .ok()
        .or_else(|| std::env::var("FRONTEND_DIR").ok())
        .map(|dir| {
            let path = std::path::PathBuf::from(&dir).join("index.html");
            std::fs::read_to_string(&path).unwrap_or_else(|_| {
                tracing::warn!("SPA index not found at {}, using fallback", path.display());
                fallback.to_string()
            })
        })
        .unwrap_or_else(|| {
            let compile_time_path = concat!(env!("CARGO_MANIFEST_DIR"), "/static/index.html");
            std::fs::read_to_string(compile_time_path).unwrap_or_else(|_| fallback.to_string())
        });

    (
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-cache"),
        ],
        html,
    )
}

/// Serve embedded favicon
async fn favicon() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "image/svg+xml")],
        include_str!("../static/favicon.svg"),
    )
}

/// Serve embedded icons sprite
async fn icons() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "image/svg+xml")],
        include_str!("../static/icons.svg"),
    )
}

/// SPA fallback: serve index.html for any non-API route.
async fn spa_fallback() -> impl IntoResponse {
    spa_index().await
}

/// JSON 404 for unknown API paths — API clients must never receive HTML.
async fn api_not_found() -> impl IntoResponse {
    crate::ApiError::not_found("API endpoint not found", None)
}

/// Resolve the frontend directory at runtime.
///
/// Checks `OXO_FLOW_FRONTEND_DIR` env var first (for Docker/deployment),
/// then falls back to the compile-time static directory (for dev).
fn frontend_dir() -> std::path::PathBuf {
    std::env::var("OXO_FLOW_FRONTEND_DIR")
        .ok()
        .or_else(|| std::env::var("FRONTEND_DIR").ok())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/static")))
}

/// Build the full application router for the given serve mode.
///
/// * `personal` — bind to 127.0.0.1, no auth required
/// * `team` — bind to 0.0.0.0, auth required
/// * `hpc` — bind to 0.0.0.0, scheduler awareness
pub fn build_router(mode: &str) -> Router {
    tracing::info!("Building router for mode: {mode}");

    // Mode flag: auth is required for team and hpc modes
    let auth_required = mode == "team" || mode == "hpc";
    if auth_required {
        tracing::info!("Auth middleware enabled for {mode} mode");
    }

    // ---- Frontend / SPA routes ----
    let frontend_routes = Router::new()
        .route("/favicon.svg", get(favicon))
        .route("/icons.svg", get(icons))
        .nest_service(
            "/assets",
            tower_http::services::ServeDir::new(frontend_dir().join("assets")),
        )
        .route("/", get(spa_index));

    // ---- Workflow routes ----
    let workflow_routes = Router::new()
        .route(
            "/api/pipelines/parse",
            post(workflow::handlers::parse_pipeline),
        )
        .route(
            "/api/pipelines/validate",
            post(workflow::handlers::validate_pipeline),
        )
        .route(
            "/api/pipelines/prepare",
            post(workflow::handlers::prepare_pipeline),
        )
        .route("/api/pipelines/dag", post(workflow::handlers::build_dag))
        .route(
            "/api/pipelines/format",
            post(workflow::handlers::format_pipeline),
        )
        .route(
            "/api/pipelines/lint",
            post(workflow::handlers::lint_pipeline),
        )
        .route(
            "/api/pipelines/stats",
            post(workflow::handlers::pipeline_stats),
        )
        .route(
            "/api/pipelines/diff",
            post(workflow::handlers::diff_pipelines),
        )
        .route(
            "/api/pipelines/export",
            post(workflow::handlers::export_pipeline),
        )
        .route(
            "/api/pipelines/search",
            post(workflow::handlers::search_pipelines),
        )
        .route(
            "/api/plugins/validate",
            post(workflow::handlers::validate_plugin),
        )
        .route("/api/pipelines", post(workflow::handlers::save_pipeline))
        .route("/api/pipelines", get(workflow::handlers::list_pipelines))
        .route("/api/pipelines/{id}", get(workflow::handlers::get_pipeline))
        .route(
            "/api/pipelines/{id}",
            put(workflow::handlers::update_pipeline),
        )
        .route(
            "/api/pipelines/{id}",
            delete(workflow::handlers::delete_pipeline),
        );

    // ---- Run routes ----
    let run_routes = Router::new()
        .route("/api/runs", post(execution::handlers::create_run))
        .route("/api/runs", get(execution::handlers::list_runs))
        .route("/api/runs/{id}", get(execution::handlers::get_run))
        .route(
            "/api/runs/{id}/status",
            get(execution::handlers::get_run_status),
        )
        .route(
            "/api/runs/{id}/dag-status",
            get(execution::handlers::get_dag_status),
        )
        .route(
            "/api/runs/{id}/diagnostics",
            get(execution::handlers::get_diagnostics),
        )
        .route(
            "/api/runs/{id}/logs",
            get(execution::handlers::get_run_logs),
        )
        .route(
            "/api/runs/{id}/results",
            get(execution::handlers::get_run_results),
        )
        .route("/api/runs/{id}/retry", post(execution::handlers::retry_run))
        .route(
            "/api/runs/{id}/cancel",
            post(execution::handlers::cancel_run),
        )
        .route("/api/runs/{id}/pause", post(execution::handlers::pause_run))
        .route(
            "/api/runs/{id}/resume",
            post(execution::handlers::resume_run),
        )
        .route(
            "/api/runs/{id}/ai-status",
            get(execution::handlers::get_ai_status),
        )
        .route(
            "/api/runs/{id}/report",
            get(execution::handlers::get_run_report),
        )
        .route(
            "/api/runs/{id}/report/ask",
            post(execution::handlers::ask_report_question),
        )
        .route(
            "/api/runs/{id}/report/visualize",
            post(execution::handlers::visualize_report),
        );

    // ---- Data routes ----
    let data_routes = Router::new()
        .route("/api/data/analyze", post(workflow::handlers::analyze_data))
        .route(
            "/api/data/reference",
            post(workflow::handlers::discover_reference),
        )
        .route(
            "/api/data/perceive",
            post(workflow::handlers::perceive_data),
        )
        .route(
            "/api/data/reference/status",
            get(workflow::handlers::reference_status),
        )
        .route(
            "/api/data/samplesheet/parse",
            post(workflow::handlers::parse_samplesheet),
        );

    // ---- Template routes ----
    let template_routes = Router::new()
        .route("/api/templates", get(workflow::handlers::list_templates))
        .route("/api/templates/{id}", get(workflow::handlers::get_template))
        .route("/api/templates", post(workflow::handlers::save_template))
        .route(
            "/api/templates/{id}",
            delete(workflow::handlers::delete_template),
        );

    // ---- Auth routes ----
    let auth_routes = Router::new()
        .route("/api/auth/login", post(auth::handlers::login))
        .route("/api/auth/me", get(auth::handlers::auth_me))
        .route("/api/users", get(auth::handlers::list_users))
        .route("/api/users", post(auth::handlers::create_user))
        .route("/api/users/{id}", delete(auth::handlers::delete_user))
        .route(
            "/api/auth/oauth/authorize",
            post(auth::handlers::oauth_authorize),
        )
        .route(
            "/api/auth/oauth/callback",
            post(auth::handlers::oauth_callback),
        );

    // ---- License routes ----
    let license_routes = Router::new()
        .route("/api/license", get(auth::handlers::license_status))
        .route("/api/license/upload", post(auth::handlers::upload_license));

    // ---- Chat routes (v0.8 AI Companion) ----
    let chat_routes = Router::new()
        .route("/api/chat/send", post(chat::handlers::chat_send))
        .route("/api/chat/send/json", post(chat::handlers::chat_send_json))
        .route("/api/chat/sessions", get(chat::handlers::list_sessions));

    // ---- DAG Edit routes ----
    let dag_edit_routes = Router::new()
        .route(
            "/api/pipeline/{id}/command",
            post(dag::handlers::edit_command),
        )
        .route("/api/pipeline/{id}/undo", post(dag::handlers::undo_command))
        .route("/api/pipeline/{id}/redo", post(dag::handlers::redo_command));

    // ---- AI routes ----
    let ai_routes = Router::new()
        .route("/api/ai/translate", post(ai::handlers::translate))
        .route(
            "/api/ai/translate/stream",
            post(ai::handlers::translate_sse),
        )
        .route("/api/ai/explain", post(ai::handlers::explain))
        .route("/api/ai/interpret", post(ai::handlers::interpret))
        .route("/api/ai/optimize", post(ai::handlers::optimize))
        .route("/api/ai/config", get(ai::handlers::get_ai_config))
        .route("/api/ai/config", post(ai::handlers::update_ai_config))
        .route("/api/ai/test", post(ai::handlers::test_ai_config))
        .route("/api/ai/config/user", get(ai::handlers::get_user_ai_config))
        .route(
            "/api/ai/config/user",
            put(ai::handlers::update_user_ai_config),
        )
        .route(
            "/api/ai/config/server",
            get(ai::handlers::get_server_ai_config),
        )
        .route(
            "/api/ai/config/server",
            put(ai::handlers::update_server_ai_config),
        )
        .route(
            "/api/ai/config/effective",
            get(ai::handlers::get_ai_config_effective),
        );

    // ---- Collaboration routes ----
    let collaboration_routes = Router::new()
        .route(
            "/api/pipelines/{id}/fork",
            post(collaboration::handlers::fork_pipeline),
        )
        .route(
            "/api/pipelines/{id}/share",
            post(collaboration::handlers::share_pipeline),
        )
        .route(
            "/api/pipelines/import",
            post(collaboration::handlers::import_pipeline),
        );

    // ---- Observability routes ----
    let obs_routes = Router::new()
        .route("/api/health", get(observability::handlers::health))
        .route("/api/system", get(observability::handlers::system_info))
        .route(
            "/api/openapi.json",
            get(|| async {
                (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    include_str!("../static/openapi.json"),
                )
            }),
        )
        .route(
            "/api/metrics",
            get(observability::handlers::runtime_metrics),
        )
        .route("/api/events", get(crate::sse::sse_events))
        .route("/api/audit", get(observability::handlers::get_audit_logs))
        .route("/api/quota", get(observability::handlers::quota_status));

    // ---- HPC routes ----
    let hpc_routes = Router::new().route("/api/hpc", get(crate::handlers::system::hpc_status));

    // ---- API 404: unknown /api/* paths return JSON, never HTML ----
    let api_fallback = Router::new().nest("/api", Router::new().fallback(api_not_found));

    // ---- SPA fallback: any unknown non-API route serves index.html ----
    let spa_fallback = Router::new().fallback(spa_fallback);

    // ---- Assemble ----
    let mut router = Router::new()
        .merge(frontend_routes)
        .merge(workflow_routes)
        .merge(run_routes)
        .merge(data_routes)
        .merge(template_routes)
        .merge(auth_routes)
        .merge(license_routes)
        .merge(chat_routes)
        .merge(dag_edit_routes)
        .merge(ai_routes)
        .merge(collaboration_routes)
        .merge(obs_routes);

    // HPC mode: include HPC-specific routes (job submit/cancel)
    if mode == "hpc" {
        router = router.merge(hpc_routes);
    }

    // Team/HPC mode: apply auth layer to non-auth, non-health endpoints
    if auth_required {
        router = router.layer(axum::middleware::from_fn(require_auth));
    }

    // Rate limiter: 100 requests per 60 seconds per client IP (login brute-force protection).
    let rate_limiter = std::sync::Arc::new(crate::rate_limit::RateLimiter::new(
        crate::rate_limit::RateLimiterConfig::default(),
    ));

    // Build CORS layer: restrictive by default, configurable via OXO_FLOW_ALLOWED_ORIGINS.
    let cors = {
        let allowed = std::env::var("OXO_FLOW_ALLOWED_ORIGINS").unwrap_or_default();
        if allowed.is_empty() {
            // Default: only localhost origins (browser dev + local deployments)
            tower_http::cors::CorsLayer::new()
                .allow_origin([
                    "http://localhost:3000".parse().unwrap(),
                    "http://localhost:5173".parse().unwrap(),
                    "http://127.0.0.1:3000".parse().unwrap(),
                    "http://127.0.0.1:5173".parse().unwrap(),
                ])
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ])
        } else {
            // User-specified origins (comma-separated)
            let origins: Vec<axum::http::HeaderValue> = allowed
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            tower_http::cors::CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ])
        }
    };

    router
        .merge(api_fallback)
        .merge(spa_fallback)
        .layer(LicenseHeaderLayer)
        .layer(axum::middleware::from_fn(security_headers))
        .layer(axum::Extension(rate_limiter))
        .layer(axum::middleware::from_fn(
            crate::rate_limit::rate_limit_middleware,
        ))
        .layer(cors)
}

/// Add standard security headers to every response.
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // Prevent MIME-type sniffing
    headers.insert(
        axum::http::header::HeaderName::from_static("x-content-type-options"),
        "nosniff".parse().unwrap(),
    );
    // Prevent clickjacking
    headers.insert(
        axum::http::header::HeaderName::from_static("x-frame-options"),
        "DENY".parse().unwrap(),
    );
    // Basic CSP: only allow same-origin resources
    headers.insert(
        axum::http::header::HeaderName::from_static("content-security-policy"),
        "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'".parse().unwrap(),
    );
    // Referrer policy
    headers.insert(
        axum::http::header::HeaderName::from_static("referrer-policy"),
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    // Permissions policy: restrict sensitive browser features
    headers.insert(
        axum::http::header::HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()".parse().unwrap(),
    );

    response
}

/// Middleware: require authentication for team/hpc mode.
///
/// Validates the Bearer token against the `sessions` table — the token must
/// exist and not be expired.  Returns 401 for missing, malformed, or invalid
/// tokens.  Public endpoints (login, OAuth, health, etc.) bypass this check.
async fn require_auth(
    headers: axum::http::HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();

    // Allow public endpoints without auth
    if path == "/api/auth/login"
        || path == "/api/auth/me"
        || path.starts_with("/api/auth/oauth")
        || path == "/api/health"
        || path == "/api/openapi.json"
        || path == "/api/license"
        || path == "/api/system"
        || path == "/api/metrics"
        || path == "/api/ai/config"
        || path == "/api/ai/test"
        || path == "/api/hpc"
        || path == "/api/events"
        || path == "/"
        || path.starts_with("/assets/")
        || path == "/favicon.svg"
        || path == "/icons.svg"
    {
        return next.run(request).await;
    }

    // Extract Bearer token
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                axum::Json(serde_json::json!({
                    "code": "AUTH_REQUIRED",
                    "message": "Authentication required in team/hpc mode",
                    "suggestion": "Login at POST /api/auth/login to obtain a session token"
                })),
            )
                .into_response();
        }
    };

    // Validate the token against the sessions table
    let valid = match crate::infra::db::sqlite::try_pool() {
        Ok(pool) => {
            let now = chrono::Utc::now().to_rfc3339();
            let session: Option<crate::infra::db::models::SessionRow> =
                sqlx::query_as("SELECT * FROM sessions WHERE token = ? AND expires_at > ?")
                    .bind(&token)
                    .bind(&now)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);
            session.is_some()
        }
        Err(_) => {
            // DB not available — reject all tokens (fail-secure)
            tracing::error!("require_auth: DB pool not available, rejecting request");
            false
        }
    };

    if valid {
        return next.run(request).await;
    }

    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::Json(serde_json::json!({
            "code": "INVALID_TOKEN",
            "message": "Invalid or expired session token",
            "suggestion": "Login again at POST /api/auth/login to obtain a fresh token"
        })),
    )
        .into_response()
}
