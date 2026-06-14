//! Specialized AI agents for pipeline creation, monitoring, and reporting.
//!
//! Agents follow the uniform AgentMessage protocol and are dispatched
//! by the Orchestrator. Each agent handles one concern:
//!
//! - `data_agent` — 4-level data perception (file scan → user input → naming → intent)
//! - `tool_expert` — Tool recommendations with resource hints
//! - `validator_agent` — DAG and parameter validation
//! - `monitor_agent` — Resource prediction and alerting during execution
//! - `report_agent` — Scientific narrative generation and Q&A
//! - `search_agent` — Web search with quality scoring
//! - `orchestrator` — Coordinates agents for conversational pipeline creation

pub mod data_agent;
pub mod monitor_agent;
pub mod orchestrator;
pub mod report_agent;
pub mod search_agent;
pub mod tool_expert;
pub mod types;
pub mod validator_agent;
