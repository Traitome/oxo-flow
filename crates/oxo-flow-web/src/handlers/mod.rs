//! Handler modules for oxo-flow-web API endpoints.
//!
//! Each module groups related handlers by domain:
//! - `workflow`: Workflow parsing, validation, DAG, and execution
//! - `runs`: Workflow run management and monitoring
//! - `system`: System health, version, metrics, and environments
//! - `reports`: Workflow report generation
//! - `auth`: Authentication and license status
//! - `saved`: Saved workflow CRUD operations
//! - `scheduled`: Scheduled workflow runs
//! - `partials`: HTMX partial handlers (placeholder)

pub mod auth;
pub mod partials;
pub mod reports;
pub mod runs;
pub mod saved;
pub mod scheduled;
pub mod system;
pub mod workflow;

// Re-export all handlers for convenient access from lib.rs
pub use auth::{auth_me, license_status, login};
pub use reports::generate_report;
pub use runs::{cancel_run, get_run_detail, get_run_logs, list_runs};
pub use saved::{delete_saved_workflow, get_saved_workflow, list_saved_workflows, save_workflow};
pub use scheduled::{
    cancel_scheduled_run, create_scheduled_run, get_scheduled_run, list_scheduled_runs,
};
pub use system::{
    get_audit_logs, health, list_environments, runtime_metrics, sse_events, system_info, version,
};
pub use workflow::{
    build_dag, clean_workflow, diff_workflows_endpoint, dry_run, export_workflow,
    format_workflow_endpoint, lint_workflow, lint_workflow_paginated, parse_workflow, run_workflow,
    validate_workflow, workflow_stats_endpoint,
};
