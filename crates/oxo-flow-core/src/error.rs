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

    /// Resource exhaustion - rule requires more resources than available.
    #[error(
        "resource exhausted: rule '{rule}' requires {required_threads} threads (available: {available_threads}) and {required_memory_mb}MB memory (available: {available_memory_mb}MB)"
    )]
    ResourceExhausted {
        rule: String,
        required_threads: u32,
        available_threads: u32,
        required_memory_mb: u64,
        available_memory_mb: u64,
    },
}

impl OxoFlowError {
    /// Returns an actionable suggestion for how to fix or investigate this error.
    ///
    /// Provides context-specific advice based on the error variant to help
    /// users quickly resolve common problems.
    pub fn suggestion(&self) -> Option<String> {
        match self {
            OxoFlowError::Config { message } => {
                if message.contains("missing") {
                    Some("check your .oxoflow file for missing required fields".to_string())
                } else if message.contains("type") {
                    Some("verify that field types match the expected format".to_string())
                } else {
                    Some("run 'oxo-flow validate' to check your configuration".to_string())
                }
            }
            OxoFlowError::Parse { path, .. } => {
                Some(format!("check the TOML syntax in '{}'", path.display()))
            }
            OxoFlowError::CycleDetected { .. } => Some(
                "review rule input/output patterns and depends_on fields to break the circular dependency".to_string(),
            ),
            OxoFlowError::MissingInput { rule, path } => Some(format!(
                "ensure '{}' is produced by another rule or exists as a source file. \
                 Check rule '{}' inputs for typos",
                path, rule
            )),
            OxoFlowError::DuplicateRule { name } => Some(format!(
                "rename one of the rules named '{}' to a unique name",
                name
            )),
            OxoFlowError::RuleNotFound { name } => Some(format!(
                "check for typos in '{}' or run 'oxo-flow show' to list available rules",
                name
            )),
            OxoFlowError::Execution { rule, message } => {
                if message.contains("permission") {
                    Some(format!("check file permissions for rule '{}'", rule))
                } else if message.contains("not found") {
                    Some(format!(
                        "ensure the required tools are installed for rule '{}'",
                        rule
                    ))
                } else {
                    Some(format!("check stderr output and logs for rule '{}'", rule))
                }
            }
            OxoFlowError::TaskFailed { rule, code } => Some(format!(
                "rule '{}' exited with code {}. Check the rule's log file or run with -v for verbose output",
                rule, code
            )),
            OxoFlowError::Environment { kind, .. } => match kind.as_str() {
                "conda" => Some(
                    "ensure conda is installed and the environment YAML is valid".to_string(),
                ),
                "docker" => Some(
                    "ensure Docker is running and the image reference is correct".to_string(),
                ),
                "singularity" => Some(
                    "ensure Singularity/Apptainer is installed and the image is accessible"
                        .to_string(),
                ),
                "pixi" => Some(
                    "ensure pixi is installed and the project manifest is valid".to_string(),
                ),
                "venv" => Some(
                    "ensure Python is installed and the requirements file exists".to_string(),
                ),
                _ => Some(format!("check your {} environment configuration", kind)),
            },
            OxoFlowError::Wildcard { rule, .. } => Some(format!(
                "check wildcard patterns in rule '{}'. Ensure input files exist for pattern discovery",
                rule
            )),
            OxoFlowError::Validation { suggestion, .. } => suggestion.clone(),
            OxoFlowError::Checkpoint { path, .. } => {
                if let Some(p) = path {
                    Some(format!(
                        "try deleting the checkpoint file '{}' and re-running",
                        p.display()
                    ))
                } else {
                    Some("try running with --force to ignore checkpoint state".to_string())
                }
            }
            OxoFlowError::Integrity { failed_files, .. } => {
                if failed_files.is_empty() {
                    Some("re-run the workflow to regenerate outputs".to_string())
                } else {
                    Some(format!(
                        "re-run the rules that produce: {}",
                        failed_files.join(", ")
                    ))
                }
            }
            _ => None,
        }
    }
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

    #[test]
    fn error_suggestion_config() {
        let err = OxoFlowError::Config {
            message: "missing field 'name'".to_string(),
        };
        let suggestion = err.suggestion();
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("missing"));
    }

    #[test]
    fn error_suggestion_cycle() {
        let err = OxoFlowError::CycleDetected {
            details: "a → b → a".to_string(),
        };
        let suggestion = err.suggestion();
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("circular dependency"));
    }

    #[test]
    fn error_suggestion_task_failed() {
        let err = OxoFlowError::TaskFailed {
            rule: "align".to_string(),
            code: 1,
        };
        let suggestion = err.suggestion();
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("verbose"));
    }

    #[test]
    fn error_suggestion_environment() {
        let err = OxoFlowError::Environment {
            kind: "conda".to_string(),
            message: "env not found".to_string(),
        };
        let suggestion = err.suggestion();
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("conda"));
    }
}
