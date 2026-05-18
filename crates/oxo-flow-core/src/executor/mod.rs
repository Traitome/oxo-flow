//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

pub mod checkpoint;
pub mod hooks;
pub mod process;
pub mod security;
pub mod timeout;

#[cfg(test)]
mod tests;

// Re-export common items for backward compatibility and convenience
pub use checkpoint::{BenchmarkRecord, CheckpointState};
pub use process::{
    ExecutionEvent, ExecutionProvenance, ExecutionStats, ExecutorConfig, JobRecord, JobStatus,
    LocalExecutor,
};
pub use security::{sanitize_shell_command, validate_shell_safety, validate_wildcard_injection};
