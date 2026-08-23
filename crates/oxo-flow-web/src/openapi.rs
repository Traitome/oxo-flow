//! Code-generated OpenAPI 3.1 specification (issue #82 P1-13).
//!
//! The spec is derived from the `#[utoipa::path]` annotations on every
//! route handler in the crate (see `domains/*/handlers.rs`, `sse.rs`, and
//! `server.rs`) plus the `utoipa::ToSchema` derives on the domain types.
//! There is no hand-maintained static spec anymore — `GET /api/openapi.json`
//! serves the serialized [`ApiDoc::openapi`] document, cached in memory on
//! first request (the schema graph is static per build).
//!
//! The drift gate is `tests/openapi_gate.rs`: it asserts that every route
//! registered in [`crate::server::build_router`] is present in the
//! generated spec, so a new route without an annotation fails CI.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// Adds `bearerAuth` (JWT) and `apiKey` (`X-API-Key`) security schemes to the
/// generated OpenAPI document. Protected endpoints declare which schemes they
/// accept via `security(...)` in their `#[utoipa::path]` macro.
pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
        components.add_security_scheme(
            "apiKey",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-API-Key"))),
        );
    }
}

/// The complete generated OpenAPI document for the oxo-flow web API.
#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    info(
        title = "oxo-flow Web API",
        description = "REST API for the oxo-flow bioinformatics pipeline engine: pipeline authoring, run lifecycle, file service, AI assistant, cluster connections, collaboration, and observability.",
        version = env!("CARGO_PKG_VERSION")
    ),
    paths(
        // workflow
        crate::domains::workflow::handlers::parse_pipeline,
        crate::domains::workflow::handlers::validate_pipeline,
        crate::domains::workflow::handlers::prepare_pipeline,
        crate::domains::workflow::handlers::build_dag,
        crate::domains::workflow::handlers::format_pipeline,
        crate::domains::workflow::handlers::lint_pipeline,
        crate::domains::workflow::handlers::pipeline_stats,
        crate::domains::workflow::handlers::diff_pipelines,
        crate::domains::workflow::handlers::export_pipeline,
        crate::domains::workflow::handlers::search_pipelines,
        crate::domains::workflow::handlers::validate_plugin,
        crate::domains::workflow::handlers::save_pipeline,
        crate::domains::workflow::handlers::list_pipelines,
        crate::domains::workflow::handlers::get_pipeline,
        crate::domains::workflow::handlers::update_pipeline,
        crate::domains::workflow::handlers::delete_pipeline,
        crate::domains::workflow::handlers::list_revisions,
        crate::domains::workflow::handlers::get_revision,
        crate::domains::workflow::handlers::rollback_pipeline,
        // data
        crate::domains::workflow::handlers::analyze_data,
        crate::domains::workflow::handlers::discover_reference,
        crate::domains::workflow::handlers::perceive_data,
        crate::domains::workflow::handlers::reference_status,
        crate::domains::workflow::handlers::parse_samplesheet,
        // templates
        crate::domains::workflow::handlers::list_templates,
        crate::domains::workflow::handlers::get_template,
        crate::domains::workflow::handlers::save_template,
        crate::domains::workflow::handlers::delete_template,
        // runs
        crate::domains::execution::handlers::create_run,
        crate::domains::execution::handlers::list_runs,
        crate::domains::execution::handlers::get_run,
        crate::domains::execution::handlers::get_run_status,
        crate::domains::execution::handlers::get_dag_status,
        crate::domains::execution::handlers::get_diagnostics,
        crate::domains::execution::handlers::get_run_instances,
        crate::domains::execution::handlers::clean_run,
        crate::domains::execution::handlers::resume_checkpoint,
        crate::domains::execution::handlers::get_run_logs,
        crate::domains::execution::handlers::get_run_results,
        crate::domains::execution::handlers::retry_run,
        crate::domains::execution::handlers::cancel_run,
        crate::domains::execution::handlers::pause_run,
        crate::domains::execution::handlers::resume_run,
        crate::domains::execution::handlers::get_run_preview,
        crate::domains::execution::handlers::get_ai_status,
        crate::domains::execution::handlers::get_run_report,
        crate::domains::execution::handlers::ask_report_question,
        crate::domains::execution::handlers::visualize_report,
        // files
        crate::domains::execution::files::get_run_file,
        crate::domains::execution::files::upload_files,
        crate::domains::execution::files::list_uploaded_files,
        // auth
        crate::domains::auth::handlers::login,
        crate::domains::auth::handlers::auth_me,
        crate::domains::auth::handlers::list_users,
        crate::domains::auth::handlers::create_user,
        crate::domains::auth::handlers::delete_user,
        crate::domains::auth::handlers::oauth_authorize,
        crate::domains::auth::handlers::oauth_callback,
        crate::domains::auth::handlers::list_api_keys,
        crate::domains::auth::handlers::create_api_key,
        crate::domains::auth::handlers::revoke_api_key,
        // license
        crate::domains::auth::handlers::license_status,
        crate::domains::auth::handlers::upload_license,
        // chat
        crate::domains::chat::handlers::chat_send,
        crate::domains::chat::handlers::chat_send_json,
        crate::domains::chat::handlers::list_sessions,
        // dag edit
        crate::domains::dag::handlers::edit_command,
        crate::domains::dag::handlers::undo_command,
        crate::domains::dag::handlers::redo_command,
        // ai
        crate::domains::ai::handlers::translate,
        crate::domains::ai::handlers::translate_sse,
        crate::domains::ai::handlers::explain,
        crate::domains::ai::handlers::interpret,
        crate::domains::ai::handlers::optimize,
        crate::domains::ai::handlers::get_ai_config,
        crate::domains::ai::handlers::update_ai_config,
        crate::domains::ai::handlers::test_ai_config,
        crate::domains::ai::handlers::knowledge_tools,
        crate::domains::ai::handlers::knowledge_skills,
        crate::domains::ai::handlers::get_user_ai_config,
        crate::domains::ai::handlers::update_user_ai_config,
        crate::domains::ai::handlers::get_server_ai_config,
        crate::domains::ai::handlers::update_server_ai_config,
        crate::domains::ai::handlers::get_ai_config_effective,
        // clusters
        crate::domains::clusters::handlers::list_clusters,
        crate::domains::clusters::handlers::upsert_cluster,
        crate::domains::clusters::handlers::delete_cluster,
        crate::domains::clusters::handlers::probe_cluster,
        // collaboration
        crate::domains::collaboration::handlers::fork_pipeline,
        crate::domains::collaboration::handlers::share_pipeline,
        crate::domains::collaboration::handlers::import_pipeline,
        crate::domains::collaboration::handlers::get_share_landing,
        // observability
        crate::domains::observability::handlers::health,
        crate::domains::observability::handlers::system_info,
        crate::domains::observability::handlers::runtime_metrics,
        crate::domains::observability::handlers::quota_status,
        crate::domains::observability::handlers::get_audit_logs,
        crate::domains::observability::handlers::get_webhook_config,
        crate::domains::observability::handlers::put_webhook_config,
        crate::domains::observability::handlers::hpc_status,
        crate::sse::sse_events,
        crate::server::openapi_json,
    ),
    components(schemas(crate::domains::workflow::handlers::ApiError))
)]
pub struct ApiDoc;

/// Serialized spec, generated once per process. The schema graph is fully
/// determined by the `#[utoipa::path]` annotations compiled into the binary,
/// so regenerating per request only burns CPU.
static SPEC_JSON: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Serialize the generated spec to a JSON string (served at
/// `GET /api/openapi.json`). The serialization is cached after the first
/// call; each call clones the cached string.
pub fn spec_json() -> String {
    SPEC_JSON
        .get_or_init(|| {
            // The schema graph is static and known-good; serialization cannot fail.
            ApiDoc::openapi()
                .to_json()
                .expect("generated OpenAPI spec is always serializable")
        })
        .clone()
}
