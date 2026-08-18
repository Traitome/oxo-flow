//! Task execution engine for oxo-flow.
//!
//! Executes workflow rules as local processes, handling concurrency,
//! status tracking, and environment activation.

use std::collections::HashMap;

use sysinfo::System;

pub mod checkpoint;
pub mod env_create_lock;
pub mod process;
pub mod rss;
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

/// Maximum substitution passes for placeholder expansion before giving up on
/// a (presumably cyclic) reference.
const MAX_EXPANSION_ITERATIONS: usize = 32;

/// Expand `{key}` placeholders in `template` to a fixed point.
///
/// Substitution repeats until the string stops changing, so values that
/// themselves contain placeholders (nested `{config.x}` references) resolve
/// deterministically regardless of `HashMap` iteration order. The `render`
/// callback normalizes each value (e.g. shell-friendly array joining); pass an
/// identity such as `|value| value.to_owned()` when no normalization is needed.
///
/// A cyclic reference (`a = "{b}"` with `b = "{a}"`) never reaches a fixed
/// point; iteration is capped at [`MAX_EXPANSION_ITERATIONS`] and the
/// best-effort result is returned.
#[must_use]
pub(crate) fn expand_to_fixed_point(
    template: &str,
    values: &HashMap<String, String>,
    render: impl Fn(&str) -> String,
) -> String {
    // Pre-render every value and pre-compute its placeholder once, so the
    // convergence loop below performs no per-iteration allocation beyond the
    // substitutions themselves.
    let pairs: Vec<(String, String)> = values
        .iter()
        .map(|(key, value)| (format!("{{{key}}}"), render(value)))
        .collect();

    let mut result = template.to_string();
    for _ in 0..MAX_EXPANSION_ITERATIONS {
        let mut changed = false;
        for (placeholder, rendered) in &pairs {
            if result.contains(placeholder.as_str()) {
                let replaced = result.replace(placeholder.as_str(), rendered);
                changed |= replaced != result;
                result = replaced;
            }
        }
        if !changed {
            return result;
        }
    }
    // Did not converge: the template contains a cyclic reference (or a
    // dependency chain deeper than MAX_EXPANSION_ITERATIONS). Leave the
    // unresolved placeholder in place but surface it for diagnosis.
    tracing::warn!(
        iterations = MAX_EXPANSION_ITERATIONS,
        "placeholder expansion did not converge (cyclic or overly deep reference); leaving unresolved placeholders"
    );
    result
}

// Re-export common items for backward compatibility and convenience
pub use checkpoint::{BenchmarkRecord, CheckpointState};
pub use process::{
    ExecutionEvent, ExecutionProvenance, ExecutionStats, ExecutorConfig, JobRecord, JobStatus,
    LocalExecutor,
};
pub use security::{sanitize_shell_command, validate_shell_safety, validate_wildcard_injection};
pub use workdir_lock::WorkdirLock;
