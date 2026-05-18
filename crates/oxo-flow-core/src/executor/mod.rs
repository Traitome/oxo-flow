//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use sysinfo::System;

pub mod checkpoint;
pub mod hooks;
pub mod process;
pub mod security;
pub mod timeout;

#[cfg(test)]
mod tests;

/// Get available CPU threads for auto-scaling.
#[must_use]
pub fn available_threads() -> u32 {
    System::new_all().cpus().len() as u32
}

/// Get available memory in GB for auto-scaling.
#[must_use]
pub fn available_memory_gb() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.available_memory() / (1024 * 1024 * 1024) // Convert bytes to GB
}

/// Check if optional rule inputs exist.
///
/// Returns true if all non-wildcard input paths exist.
#[must_use]
pub fn optional_inputs_exist(rule: &crate::rule::Rule) -> bool {
    use std::path::Path;
    for input in rule.input.to_vec() {
        // Skip wildcard patterns - they'll be expanded at runtime
        if !input.contains('{') && !Path::new(&input).exists() {
            return false;
        }
    }
    true
}

// Re-export common items for backward compatibility and convenience
pub use checkpoint::{BenchmarkRecord, CheckpointState};
pub use process::{
    ExecutionEvent, ExecutionProvenance, ExecutionStats, ExecutorConfig, JobRecord, JobStatus,
    LocalExecutor,
};
pub use security::{sanitize_shell_command, validate_shell_safety, validate_wildcard_injection};
