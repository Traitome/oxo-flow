//! Specialized AI agents for data perception, monitoring, and reporting.
//!
//! - `data_agent` — 4-level data perception (file scan → user input → naming → intent)
//! - `monitor_agent` — Resource prediction and alerting during execution
//! - `report_agent` — Scientific narrative generation and Q&A
//! - `orchestrator` — intent inference from free-text user messages
//! - `types` — shared agent data and status types

pub mod data_agent;
pub mod monitor_agent;
pub mod orchestrator;
pub mod report_agent;
pub mod types;
