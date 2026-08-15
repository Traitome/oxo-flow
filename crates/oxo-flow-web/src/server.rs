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

use crate::domains::clusters;
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
pub async fn spa_index() -> impl IntoResponse {
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
            // Same resolution chain as the asset serving (frontend_dir()):
            // env override → bundle-relative → compile-time.
            std::fs::read_to_string(frontend_dir().join("index.html"))
                .unwrap_or_else(|_| fallback.to_string())
        });

    // Always inject a <base> so the SPA's relative asset URLs resolve
    // from the mount root instead of the CURRENT ROUTE: without it,
    // /runs/<id> would request /runs/assets/... and get the SPA fallback
    // (MIME mismatch, blank page). Root mounts get href="/"; sub-path
    // mounts get the mount prefix.
    let base = base_path();
    let effective_base = if base.is_empty() {
        "/".to_string()
    } else {
        format!("{base}/")
    };
    let tag = format!(
        "<base href=\"{effective_base}\"><script>window.__OXO_BASE__=\"{}\";</script>",
        if base.is_empty() { "" } else { base }
    );
    // Insert right after <head>: <base> only affects URLs that FOLLOW it,
    // and vite emits its <link>/<script> tags immediately — injecting at
    // </head> would leave them resolving against the current route.
    let html = match html.find("<head>") {
        Some(head_start) => {
            let insert_at = head_start + "<head>".len();
            format!("{}{}{}", &html[..insert_at], tag, &html[insert_at..])
        }
        None => html,
    };

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

/// `GET /api/openapi.json` — the code-generated OpenAPI 3.1 specification
/// (issue #82 P1-13: derived from `#[utoipa::path]` annotations on every
/// route handler, no hand-maintained static file).
#[utoipa::path(
    get,
    path = "/api/openapi.json",
    tag = "observability",
    responses(
        (status = 200, description = "Generated OpenAPI 3.1 specification", content_type = "application/json"),
        (status = 500, description = "Error", body = crate::domains::workflow::handlers::ApiError),
    )
)]
pub async fn openapi_json() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        crate::openapi::spec_json(),
    )
}

/// Resolve the frontend directory at runtime, first hit wins:
///
/// 1. `OXO_FLOW_FRONTEND_DIR` / `FRONTEND_DIR` env (Docker/deployment)
/// 2. `../Resources/static` next to the executable (macOS .app bundle —
///    the desktop packaging ships the SPA build in Contents/Resources)
/// 3. `static` next to the executable (flat portable layout)
/// 4. `../share/oxo-flow/static` (Linux prefix installs)
/// 5. Compile-time `crates/oxo-flow-web/static/` (source checkout/dev)
fn frontend_dir() -> std::path::PathBuf {
    let explicit = std::env::var("OXO_FLOW_FRONTEND_DIR")
        .ok()
        .or_else(|| std::env::var("FRONTEND_DIR").ok())
        .map(std::path::PathBuf::from);
    if let Some(dir) = explicit {
        return dir;
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()));
    if let Some(exe_dir) = &exe_dir {
        for candidate in [
            exe_dir.join("../Resources/static"),
            exe_dir.join("static"),
            exe_dir.join("../share/oxo-flow/static"),
        ] {
            if candidate.join("index.html").exists() {
                return candidate;
            }
        }
    }

    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/static"))
}

/// Build the full application router for the given serve mode.
///
/// * `personal` — bind to 127.0.0.1, no auth required
/// * `team` — bind to 0.0.0.0, auth required
/// * `hpc` — bind to 0.0.0.0, scheduler awareness
pub fn build_router(mode: &str) -> Router {
    tracing::info!("Building router for mode: {mode}");
    set_running_mode(mode);

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
        )
        .route(
            "/api/pipelines/{id}/revisions",
            get(workflow::handlers::list_revisions),
        )
        .route(
            "/api/pipelines/{id}/revisions/{rev}",
            get(workflow::handlers::get_revision),
        )
        .route(
            "/api/pipelines/{id}/rollback",
            post(workflow::handlers::rollback_pipeline),
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
            "/api/runs/{id}/instances",
            get(execution::handlers::get_run_instances),
        )
        .route("/api/runs/{id}/clean", post(execution::handlers::clean_run))
        .route(
            "/api/runs/{id}/resume-checkpoint",
            post(execution::handlers::resume_checkpoint),
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
            "/api/runs/{id}/preview",
            get(execution::handlers::get_run_preview),
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
        )
        .route("/api/runs/{id}/files", get(execution::files::get_run_file));

    // ---- File service routes (issue #82 P0-1/P0-2) ----
    let file_routes = Router::new()
        .route("/api/files", post(execution::files::upload_files))
        .route("/api/files", get(execution::files::list_uploaded_files));

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
        )
        .route("/api/auth/keys", get(auth::handlers::list_api_keys))
        .route("/api/auth/keys", post(auth::handlers::create_api_key))
        .route(
            "/api/auth/keys/{id}",
            delete(auth::handlers::revoke_api_key),
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
        .route("/api/knowledge/tools", get(ai::handlers::knowledge_tools))
        .route("/api/knowledge/skills", get(ai::handlers::knowledge_skills))
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

    // ---- Cluster connections (SSH endpoints) ----
    let cluster_routes = Router::new()
        .route(
            "/api/clusters",
            get(clusters::handlers::list_clusters).post(clusters::handlers::upsert_cluster),
        )
        .route(
            "/api/clusters/{id}",
            axum::routing::delete(clusters::handlers::delete_cluster),
        )
        .route(
            "/api/clusters/{id}/probe",
            post(clusters::handlers::probe_cluster),
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
        )
        .route(
            "/api/share/{token}",
            get(collaboration::handlers::get_share_landing),
        );

    // ---- Observability routes ----
    let obs_routes = Router::new()
        .route("/api/health", get(observability::handlers::health))
        .route("/api/system", get(observability::handlers::system_info))
        .route("/api/openapi.json", get(openapi_json))
        .route(
            "/api/metrics",
            get(observability::handlers::runtime_metrics),
        )
        .route("/api/events", get(crate::sse::sse_events))
        .route("/api/audit", get(observability::handlers::get_audit_logs))
        .route("/api/quota", get(observability::handlers::quota_status))
        .route(
            "/api/webhook",
            get(observability::handlers::get_webhook_config),
        )
        .route(
            "/api/webhook",
            put(observability::handlers::put_webhook_config),
        );

    // ---- HPC routes ----
    let hpc_routes = Router::new().route("/api/hpc", get(observability::handlers::hpc_status));

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
        .merge(file_routes)
        .merge(cluster_routes)
        .merge(dag_edit_routes)
        .merge(ai_routes)
        .merge(collaboration_routes)
        .merge(obs_routes);

    // HPC mode: include HPC-specific routes (job submit/cancel)
    if mode == "hpc" {
        router = router.merge(hpc_routes);
    }

    // Audit layer sits INSIDE require_auth so it sees the authenticated user
    // id the auth middleware inserts into request extensions (issue #79
    // P1-05: the single write point for all mutation handlers).
    router = router.layer(axum::middleware::from_fn(crate::audit::audit_middleware));

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
        // Layer order matters (issue #79 P1-04): in axum the LAST layer is
        // outermost, and the middleware must sit INSIDE the Extension layer
        // to see the RateLimiter in the request extensions. The previous
        // order made the limiter invisible — the middleware silently skipped
        // rate limiting entirely (40/120/150-request bursts passed with zero
        // 429s in the evaluation).
        .layer(axum::middleware::from_fn(
            crate::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::Extension(rate_limiter))
        .layer(cors)
}

/// Deployment mode of the running server ("personal" | "team" | "hpc"),
/// set once at router construction. Handlers consult it for mode-specific
/// trust decisions (e.g. personal-mode management endpoints).
static RUNNING_MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Base path the app is mounted under ("" or "/" = root mount). The served
/// SPA index gets a `<base>` tag + `window.__OXO_BASE__` injected so the
/// frontend router and relative asset URLs work from any mount point
/// (issue #79 deployment modes: "the app mounts somewhere").
static BASE_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Current deployment mode; defaults to "personal" before the router is built.
pub fn running_mode() -> &'static str {
    RUNNING_MODE.get().map(String::as_str).unwrap_or("personal")
}

/// Record the deployment mode (first call wins — a server process has one mode).
pub fn set_running_mode(mode: &str) {
    let _ = RUNNING_MODE.set(mode.to_string());
}

/// Mount path of the app ("" or "/" = root).
pub fn base_path() -> &'static str {
    BASE_PATH.get().map(String::as_str).unwrap_or("/")
}

/// Record the mount path (first call wins).
pub fn set_base_path(path: &str) {
    let _ = BASE_PATH.set(path.to_string());
}

/// Normalize a user-supplied mount path to the router contract: "" or "/"
/// for a root mount, "/name" otherwise. axum's nest() panics on paths
/// without a leading slash, so every caller must route through this.
pub fn normalize_base_path(input: &str) -> String {
    let trimmed = input.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

/// Port the server actually bound (issue #82 P0-6): share URLs must point
/// at the real listener, not a hardcoded default.
static BOUND_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

pub fn set_bound_port(port: u16) {
    let _ = BOUND_PORT.set(port);
}

/// The bound port, if the server has started binding yet.
pub fn bound_port() -> Option<u16> {
    BOUND_PORT.get().copied()
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
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();

    // Allow public endpoints without auth. The list is deliberately narrow
    // (issue #82 P0-5): system/metrics/hpc metrics and AI-config writes are
    // NOT public in team/hpc modes — anonymous callers must not see cluster
    // internals or reconfigure the shared AI provider.
    let method = request.method().clone();
    let is_public = path == "/api/auth/login"
        || path == "/api/auth/me"
        || path.starts_with("/api/auth/oauth")
        || path == "/api/health"
        || path == "/api/openapi.json"
        || path == "/api/license"
        // EventSource cannot set an Authorization header; sse_events
        // validates ?token= against the sessions table itself.
        || path == "/api/events"
        // Share landing pages are the product of a share link — the share
        // token IS the authorization (issue #82 P0-6).
        || path.starts_with("/api/share/")
        // AI config GET is public (feature discoverability); writes are
        // gated (admin-only) inside the handler.
        || (path == "/api/ai/config" && method == axum::http::Method::GET)
        || path == "/"
        || path.starts_with("/assets/")
        || path == "/favicon.svg"
        || path == "/icons.svg";
    if is_public {
        return next.run(request).await;
    }

    // API keys are first-class machine credentials (issue #82 P1-13):
    // X-API-Key resolves to the same CurrentUser the session path
    // produces, so ownership scoping applies identically.
    if let Some(api_key) = headers.get("x-api-key").and_then(|v| v.to_str().ok())
        && !api_key.is_empty()
    {
        if let Some(user) = auth::handlers::resolve_api_key(api_key).await {
            request.extensions_mut().insert(user);
            return next.run(request).await;
        }
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            axum::Json(serde_json::json!({
                "code": "INVALID_API_KEY",
                "message": "Invalid or revoked API key",
            })),
        )
            .into_response();
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
    let session = match crate::infra::db::sqlite::try_pool() {
        Ok(pool) => {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query_as::<_, crate::infra::db::models::SessionRow>(
                "SELECT * FROM sessions WHERE token = ? AND expires_at > ?",
            )
            .bind(&token)
            .bind(&now)
            .fetch_optional(pool)
            .await
            .unwrap_or(None)
        }
        Err(_) => {
            // DB not available — reject all tokens (fail-secure)
            tracing::error!("require_auth: DB pool not available, rejecting request");
            None
        }
    };

    if let Some(session) = session {
        // Resolve the canonical user id + role and hand them to handlers via
        // request extensions (issue #82 P0-4: every ownership check consumes
        // this; nothing trusts client-supplied user ids).
        //
        // sessions.user_id holds the login name — for API-created users that
        // is a username, not the UUID users.id. The users table disambiguates;
        // legacy env-password logins without a users row keep the username as
        // their identity (role: 'admin' for the admin bootstrap password,
        // 'user' otherwise).
        let (user_id, role) = match crate::infra::db::sqlite::try_pool() {
            Ok(pool) => {
                let row: Option<(String, String)> =
                    sqlx::query_as("SELECT id, role FROM users WHERE id = ? OR username = ?")
                        .bind(&session.user_id)
                        .bind(&session.user_id)
                        .fetch_optional(pool)
                        .await
                        .unwrap_or(None);
                match row {
                    Some((id, role)) => (id, role),
                    None => (
                        session.user_id.clone(),
                        if session.user_id == "admin" {
                            "admin".to_string()
                        } else {
                            "user".to_string()
                        },
                    ),
                }
            }
            // DB unavailable — reject everything (fail-secure).
            Err(_) => {
                tracing::error!("require_auth: DB pool not available, rejecting request");
                return (
                    axum::http::StatusCode::UNAUTHORIZED,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    axum::Json(serde_json::json!({
                        "code": "AUTH_REQUIRED",
                        "message": "Authentication required in team/hpc mode",
                    })),
                )
                    .into_response();
            }
        };
        request
            .extensions_mut()
            .insert(crate::domains::auth::current_user::CurrentUser { id: user_id, role });
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

#[cfg(test)]
mod normalize_tests {
    use super::*;

    #[test]
    fn normalize_base_path_contract() {
        // Root mounts collapse to "" (no nest).
        assert_eq!(normalize_base_path(""), "");
        assert_eq!(normalize_base_path("/"), "");
        // Bare names gain the leading slash axum's nest() requires.
        assert_eq!(normalize_base_path("oxoflow"), "/oxoflow");
        assert_eq!(normalize_base_path("/oxo-flow"), "/oxo-flow");
        // Trailing slashes and spaces never reach the router.
        assert_eq!(normalize_base_path("/oxo-flow/"), "/oxo-flow");
        assert_eq!(normalize_base_path("  oxo-flow/  "), "/oxo-flow");
    }
}
