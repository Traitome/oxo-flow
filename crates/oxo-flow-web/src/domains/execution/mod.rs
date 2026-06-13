//! Execution domain — run lifecycle and diagnostics.
//!
//! Manages run creation, status tracking, DAG-level live status, deterministic
//! diagnostics (30+ error patterns), smart retry (DAG dependency analysis),
//! sandbox workspace management, and background process execution.

pub mod diagnostics;
pub mod handlers;
pub mod runner;
pub mod sandbox;
pub mod service;
pub mod types;
