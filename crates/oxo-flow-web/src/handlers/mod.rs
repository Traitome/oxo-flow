//! Legacy handler modules (deprecated in v0.8).
//!
//! These endpoints are preserved for backward compatibility.
//! New code should use the domain-driven modules in `crate::domains::*`.
//! Removed no earlier than v0.10.0.

#[deprecated(since = "0.8.0", note = "use `crate::domains::ai`")]
pub mod ai;
#[deprecated(since = "0.8.0", note = "use `crate::domains::auth`")]
pub mod auth;
#[deprecated(since = "0.8.0", note = "use `crate::domains::observability`")]
pub mod reports;
pub mod runs;
pub mod saved;
pub mod scheduled;
pub mod system;
pub mod templates;
pub mod users;
pub mod workflow;

// Re-export all handlers for convenient access from lib.rs
pub use ai::{
    debug_run, get_ai_config, search_workflows, suggest_pipeline, test_ai_config, try_ai_generate,
    update_ai_config,
};
pub use auth::{auth_me, license_status, login, upload_license};
pub use reports::generate_report;
pub use runs::{
    cancel_run, get_run_detail, get_run_logs, get_run_results, hpc_submit_run, list_runs,
};
pub use saved::{delete_saved_workflow, get_saved_workflow, list_saved_workflows, save_workflow};
pub use scheduled::{
    cancel_scheduled_run, create_scheduled_run, get_scheduled_run, list_scheduled_runs,
};
pub use system::{
    get_audit_logs, health, hpc_status, list_environments, runtime_metrics, sse_events,
    system_info, version,
};
pub use templates::{delete_template, get_template, list_templates, save_template};
pub use users::{create_user, delete_user, list_users};
pub use workflow::{
    build_dag, build_dag_json, clean_workflow, diff_workflows_endpoint, dry_run, export_workflow,
    format_workflow_endpoint, lint_workflow, lint_workflow_paginated, parse_workflow, run_workflow,
    validate_workflow, workflow_stats_endpoint,
};
