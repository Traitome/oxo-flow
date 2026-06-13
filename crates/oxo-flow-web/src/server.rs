//! Domain-driven server router assembly.
//!
//! This module assembles the full application router from domain handler
//! modules.  Each domain (workflow, execution, auth, observability)
//! contributes its own route group, keeping the router definition close to
//! the domain code it serves.
//!
//! This is the v0.8 forward-looking router.  The existing `build_router()`
//! in `lib.rs` remains the current active router used by `main.rs`.

use axum::{
    Router,
    routing::{delete, get, post, put},
};

use crate::domains::*;
use crate::infra::license::LicenseHeaderLayer;

/// Build the full application router for the given serve mode.
pub fn build_router(_mode: &str) -> Router {
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
        .route("/api/users/{id}", delete(auth::handlers::delete_user));

    // ---- License routes ----
    let license_routes = Router::new()
        .route("/api/license", get(auth::handlers::license_status))
        .route("/api/license/upload", post(auth::handlers::upload_license));

    // ---- AI routes ----
    let ai_routes = Router::new()
        .route("/api/ai/translate", post(ai::handlers::translate))
        .route("/api/ai/explain", post(ai::handlers::explain))
        .route("/api/ai/interpret", post(ai::handlers::interpret))
        .route("/api/ai/optimize", post(ai::handlers::optimize))
        .route("/api/ai/config", get(ai::handlers::get_ai_config))
        .route("/api/ai/config", post(ai::handlers::update_ai_config))
        .route("/api/ai/test", post(ai::handlers::test_ai_config));

    // ---- Observability routes ----
    let obs_routes = Router::new()
        .route("/api/health", get(observability::handlers::health))
        .route("/api/system", get(observability::handlers::system_info))
        .route(
            "/api/metrics",
            get(observability::handlers::runtime_metrics),
        )
        .route("/api/events", get(observability::handlers::sse_events))
        .route("/api/audit", get(observability::handlers::get_audit_logs));

    // ---- Assemble ----
    Router::new()
        .merge(workflow_routes)
        .merge(run_routes)
        .merge(data_routes)
        .merge(template_routes)
        .merge(auth_routes)
        .merge(license_routes)
        .merge(ai_routes)
        .merge(obs_routes)
        .layer(LicenseHeaderLayer)
        .layer(tower_http::cors::CorsLayer::permissive())
}
