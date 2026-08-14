//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use sysinfo::System;

pub mod checkpoint;
pub mod process;
pub mod security;
pub mod staging;
pub mod timeout;
pub mod workdir_lock;

#[cfg(test)]
mod tests;

/// Get available CPU threads for auto-scaling.
#[must_use]
pub fn available_threads() -> u32 {
    num_cpus::get() as u32
}

/// Get available memory in GB for auto-scaling.
#[must_use]
pub fn available_memory_gb() -> u64 {
    // Only memory is needed; `System::new_all()` would also walk every process,
    // disk, and network interface in /proc for no reason.
    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory() / (1024 * 1024 * 1024) // Convert bytes to GB
}

// Re-export common items for backward compatibility and convenience
pub use checkpoint::{BenchmarkRecord, CheckpointState};
pub use process::{
    ExecutionEvent, ExecutionProvenance, ExecutionStats, ExecutorConfig, JobRecord, JobStatus,
    LocalExecutor,
};
pub use security::{sanitize_shell_command, validate_shell_safety, validate_wildcard_injection};
pub use workdir_lock::WorkdirLock;
