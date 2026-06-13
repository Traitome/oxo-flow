//! Observability domain — health, metrics, logging, and audit.
//!
//! Implements the three-layer logging strategy (structured JSON, human-readable
//! execution logs, compliance audit trail), health checks with component status,
//! runtime metrics collection, and Server-Sent Events for real-time updates.

pub mod handlers;
pub mod logging;
pub mod service;
pub mod types;
