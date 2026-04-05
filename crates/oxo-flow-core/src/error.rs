//! oxo-flow error types.
//!
//! Provides a unified error enum for all core library operations.

use std::path::PathBuf;

/// Unified error type for oxo-flow core operations.
#[derive(Debug, thiserror::Error)]
pub enum OxoFlowError {
    /// Error parsing a workflow configuration file.
    #[error("config error: {message}")]
    Config { message: String },

    /// Error parsing an `.oxoflow` file.
    #[error("parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },

    /// A cycle was detected in the workflow DAG.
    #[error("cycle detected in workflow DAG: {details}")]
    CycleDetected { details: String },

    /// A rule references an input that no other rule produces and no source file exists.
    #[error("missing input for rule '{rule}': {path}")]
    MissingInput { rule: String, path: String },

    /// Duplicate rule names in a workflow.
    #[error("duplicate rule name: '{name}'")]
    DuplicateRule { name: String },

    /// A requested rule was not found.
    #[error("rule not found: '{name}'")]
    RuleNotFound { name: String },

    /// Error executing a task.
    #[error("execution error in rule '{rule}': {message}")]
    Execution { rule: String, message: String },

    /// A task exited with a non-zero exit code.
    #[error("rule '{rule}' failed with exit code {code}")]
    TaskFailed { rule: String, code: i32 },

    /// Environment-related error (conda, docker, singularity, etc.).
    #[error("environment error ({kind}): {message}")]
    Environment { kind: String, message: String },

    /// Error during report generation.
    #[error("report error: {message}")]
    Report { message: String },

    /// Wildcard expansion error.
    #[error("wildcard error in rule '{rule}': {message}")]
    Wildcard { rule: String, message: String },

    /// I/O error wrapper.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// TOML deserialization error.
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Serialization error.
    #[error("serialization error: {0}")]
    SerdeJson(#[from] serde_json::Error),

    /// Template rendering error.
    #[error("template error: {0}")]
    Template(#[from] tera::Error),

    /// Scheduler-related error.
    #[error("scheduler error: {message}")]
    Scheduler { message: String },

    /// Container packaging error.
    #[error("container error: {message}")]
    Container { message: String },
}

/// Convenience alias for `Result<T, OxoFlowError>`.
pub type Result<T> = std::result::Result<T, OxoFlowError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_config() {
        let err = OxoFlowError::Config {
            message: "missing field".to_string(),
        };
        assert_eq!(err.to_string(), "config error: missing field");
    }

    #[test]
    fn error_display_cycle() {
        let err = OxoFlowError::CycleDetected {
            details: "A -> B -> A".to_string(),
        };
        assert!(err.to_string().contains("cycle detected"));
    }

    #[test]
    fn error_display_task_failed() {
        let err = OxoFlowError::TaskFailed {
            rule: "bwa_align".to_string(),
            code: 1,
        };
        assert!(err.to_string().contains("exit code 1"));
    }

    #[test]
    fn error_display_scheduler() {
        let err = OxoFlowError::Scheduler {
            message: "no available slots".to_string(),
        };
        assert_eq!(err.to_string(), "scheduler error: no available slots");
    }

    #[test]
    fn error_display_container() {
        let err = OxoFlowError::Container {
            message: "build failed".to_string(),
        };
        assert_eq!(err.to_string(), "container error: build failed");
    }
}
