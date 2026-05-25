//! Maud HTML templates for oxo-flow-web.
//!
//! This module provides server-side rendered HTML templates using the maud macro
//! system. All templates follow the GitHub dark theme and include HTMX attributes
//! for real-time updates.
//!
//! # Module Structure
//!
//! - `partials` - Shared components (header, nav, cards, badges, theme colors)
//! - `dashboard` - Dashboard page with metrics and recent runs
//! - `workflow` - Workflow editor and list templates
//! - `runs` - Run history and detail pages
//! - `auth` - Login page and authentication partials
//!
//! # Usage
//!
//! ```rust,ignore
//! use oxo_flow_web::templates::{dashboard_page, workflow_editor_page};
//!
//! // Render dashboard with metrics
//! let metrics_json = "{}";
//! let html = dashboard_page("admin", metrics_json);
//!
//! // Render workflow editor
//! let html = workflow_editor_page("admin", None);
//! ```

pub mod auth;
pub mod dashboard;
pub mod partials;
pub mod runs;
pub mod workflow;

// Re-export main template functions for convenience
pub use auth::{login_page, login_result_partial, logout_partial, user_info_partial};
pub use dashboard::{
    dashboard_page, metrics_panel_partial, recent_runs_partial, run_status_partial,
};
pub use runs::{log_modal, run_detail_modal, run_detail_page, runs_page, runs_table_partial};
pub use workflow::{
    WorkflowDetail, WorkflowListItem, WorkflowStats, workflow_detail_page, workflow_editor_page,
    workflow_list_page, workflow_stats_partial,
};

// Re-export data types from runs for use in dashboard
pub use runs::{RunDetail, RunSummary};
