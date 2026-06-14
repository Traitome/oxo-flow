//! Workflow domain — pipeline lifecycle management.
//!
//! Handles parsing, validation, preparation, DAG construction, formatting,
//! linting, statistics, diffing, export, search, data discovery, and plugin
//! validation. All logic is in `service.rs` with zero HTTP dependency.

pub mod data;
pub mod handlers;
pub mod service;
pub mod types;
