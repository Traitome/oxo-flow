//! oxo-flow error types.
//!
//! Provides a unified error enum for all core library operations.
//! Each variant carries context-rich information to help users diagnose
//! and fix problems quickly.

use std::path::PathBuf;

/// Unified error type for oxo-flow core operations.
///
/// All public API functions in `oxo-flow-core` return `Result<T, OxoFlowError>`.
/// Error variants are designed to carry enough context (rule names, file paths,
/// suggestions) for actionable error messages.
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

    /// Validation error with diagnostic details.
    #[error("validation error: {message}")]
    Validation {
        message: String,
        /// Rule that triggered the error, if applicable.
        rule: Option<String>,
        /// Suggested fix, if available.
        suggestion: Option<String>,
    },

    /// Checkpoint persistence error (save/load/corrupt).
    #[error("checkpoint error: {message}")]
    Checkpoint {
        message: String,
        /// Path to the checkpoint file, if applicable.
        path: Option<PathBuf>,
    },

    /// Output integrity verification failure.
    #[error("integrity error: {message}")]
    Integrity {
        message: String,
        /// Files that failed verification.
        failed_files: Vec<String>,
    },
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

    #[test]
    fn error_display_validation_with_suggestion() {
        let err = OxoFlowError::Validation {
            message: "empty rule name".to_string(),
            rule: Some("step1".to_string()),
            suggestion: Some("provide a non-empty name".to_string()),
        };
        assert!(err.to_string().contains("empty rule name"));
    }

    #[test]
    fn error_display_checkpoint() {
        let err = OxoFlowError::Checkpoint {
            message: "corrupt checkpoint file".to_string(),
            path: Some(PathBuf::from("/work/.oxo-flow/checkpoint.json")),
        };
        assert!(err.to_string().contains("checkpoint error"));
        assert!(err.to_string().contains("corrupt"));
    }

    #[test]
    fn error_display_integrity() {
        let err = OxoFlowError::Integrity {
            message: "output checksums do not match".to_string(),
            failed_files: vec!["output.bam".to_string()],
        };
        assert!(err.to_string().contains("integrity error"));
    }
}
