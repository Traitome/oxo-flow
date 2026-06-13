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

/// Serve the React SPA index.html with license footer injected.
async fn spa_index() -> impl IntoResponse {
    let html = include_str!("../static/index.html");
    let footer = crate::infra::license::license_footer_html();
    let html_with_footer = if let Some(pos) = html.rfind("</body>") {
        format!(
            "{}<div class=\"oxo-flow-license-footer\" style=\"position:fixed;bottom:0;left:0;right:0;text-align:center;padding:4px 0;font-size:11px;color:#94A3B8;background:#F8FAFC;border-top:1px solid #E2E8F0;z-index:999\">{}</div>\n</body>",
            &html[..pos],
            footer
        )
    } else {
        format!(
            "{}\n<div class=\"oxo-flow-license-footer\">{}</div>",
            html, footer
        )
    };
    (
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-cache"),
        ],
        html_with_footer,
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

/// Serve embedded JS bundle
async fn app_js() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/javascript; charset=utf-8")],
        include_str!("../static/assets/index-Hf5dXr86.js"),
    )
}

/// Serve embedded CSS
async fn app_css() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/css; charset=utf-8")],
        include_str!("../static/assets/index-akIlVKkc.css"),
    )
}

/// SPA fallback: serve index.html for any non-API route.
async fn spa_fallback() -> impl IntoResponse {
    spa_index().await
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
        .route("/assets/index-Hf5dXr86.js", get(app_js))
        .route("/assets/index-akIlVKkc.css", get(app_css))
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
        );

    // ---- Data routes ----
    let data_routes = Router::new()
        .route("/api/data/analyze", post(workflow::handlers::analyze_data))
        .route(
            "/api/data/reference",
            post(workflow::handlers::discover_reference),
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

    // ---- Chat routes (v0.9 AI Companion) ----
    let chat_routes = Router::new()
        .route("/api/chat/send", post(chat::handlers::chat_send))
        .route("/api/chat/send/json", post(chat::handlers::chat_send_json))
        .route("/api/chat/sessions", get(chat::handlers::list_sessions));

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
        .route("/api/ai/test", post(ai::handlers::test_ai_config));

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

    // ---- SPA fallback: any unknown route serves index.html ----
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

    router
        .merge(spa_fallback)
        .layer(LicenseHeaderLayer)
        .layer(tower_http::cors::CorsLayer::permissive())
}

/// Middleware: require authentication for team/hpc mode.
/// Allows unauthenticated access to auth endpoints (login, oauth, health).
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

    // Check for Bearer token
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    if token.is_some() {
        return next.run(request).await;
    }

    // No valid token — return 401
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::Json(serde_json::json!({
            "code": "AUTH_REQUIRED",
            "message": "Authentication required in team/hpc mode",
            "suggestion": "Login at POST /api/auth/login to obtain a session token"
        })),
    )
        .into_response()
}
