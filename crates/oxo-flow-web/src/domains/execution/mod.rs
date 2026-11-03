//! Execution domain — run lifecycle and diagnostics.
//!
//! Manages run creation, status tracking, DAG-level live status, deterministic
//! diagnostics (30+ error patterns), smart retry (DAG dependency analysis),
//! and background process execution.

pub mod checkpoint_status;
pub mod diagnostics;
pub mod files;
pub mod handlers;
pub mod service;
pub mod types;
