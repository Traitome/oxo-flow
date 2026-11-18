//! Workflow domain — pipeline lifecycle management.
//!
//! Handles parsing, validation, preparation, DAG construction, formatting,
//! linting, statistics, diffing, export, search, data discovery, and plugin
//! validation. All logic is in `service.rs` with zero HTTP dependency.

pub mod data;
pub mod handlers;
pub mod service;
pub mod types;
pub mod visibility;

// The ONE pipeline visibility policy (WEB-13 / issue #1138) is exported at
// the domain root so every consumer — list/read/write handlers, fork/share,
// run creation, AI optimize — evaluates the same predicate.
pub use visibility::{can_read_pipeline, can_write_pipeline};
