//! AI domain — natural language translation layer.
//!
//! Translates user intent into validated pipelines, explains failures,
//! interprets results, and optimizes parameters. All AI functions call the
//! deterministic core APIs — they have zero write access to the database,
//! filesystem, or process management.

pub mod agents;
pub mod copilot;
pub mod handlers;
pub mod service;
pub mod types;
