#![forbid(unsafe_code)]
//! # oxo-flow-core
//!
//! Core library for the oxo-flow bioinformatics pipeline engine.
//!
//! oxo-flow is a Rust-native workflow engine purpose-built for
//! bioinformatics pipelines. It provides:
//!
//! - **DAG engine**: Build, validate, and execute directed acyclic graphs of tasks
//! - **Environment management**: First-class support for conda, pixi, docker, singularity, venv, and HPC modules
//! - **Wildcard expansion**: `{sample}` pattern substitution with regex constraints and automatic discovery
//! - **Resource scheduling**: CPU, memory, GPU-aware job scheduling with resource estimation hints
//! - **Report generation**: Modular, clinical-grade HTML/JSON reporting
//! - **Container packaging**: Package workflows into portable Docker/Singularity images
//! - **Checkpoint & lineage**: Persistent checkpoint state, data provenance, and output integrity verification
//! - **Multi-omics ready**: Format hints, metadata fields, and reference database tracking for any omics workflow
//!
//! # Quick Start
//!
//! ```rust
//! use oxo_flow_core::config::WorkflowConfig;
//! use oxo_flow_core::dag::WorkflowDag;
//!
//! let toml = r#"
//!     [workflow]
//!     name = "example"
//!     version = "1.0.0"
//!
//!     [[rules]]
//!     name = "step1"
//!     input = ["input.txt"]
//!     output = ["output.txt"]
//!     shell = "cat input.txt > output.txt"
//! "#;
//!
//! let config = WorkflowConfig::parse(toml).unwrap();
//! let dag = WorkflowDag::from_rules(&config.rules).unwrap();
//! let order = dag.execution_order().unwrap();
//! assert_eq!(order, vec!["step1"]);
//! ```

pub mod clinical;
#[cfg(feature = "cluster")]
pub mod cluster;
pub mod config;
#[cfg(feature = "container")]
pub mod container;
pub mod dag;
pub mod environment;
pub mod error;
pub mod executor;
pub mod format;
pub mod plugin;

#[cfg(feature = "report")]
pub mod report;
pub mod rule;
pub mod scheduler;
pub mod storage;
#[cfg(feature = "webhook")]
pub mod webhook;
pub mod wildcard;

// Re-export key types at the crate root for convenience.
pub use clinical::{
    ActionabilityAnnotation, BiomarkerResult, ClinicalReportSection, ComplianceEvent, FilterChain,
    GenePanel, QcThreshold, TumorSampleMeta, VariantClassification,
};
pub use config::WorkflowConfig;
pub use config::{
    ExperimentControlPair, Parsed, Ready, ReferenceDatabase, RuleName, SampleGroup,
    TumorNormalPair, Validated, WildcardPattern, WorkflowState,
};
pub use dag::WorkflowDag;
pub use error::{OxoFlowError, Result};
pub use executor::{
    CheckpointState, ExecutionEvent, ExecutionProvenance, ExecutionStats, JobStatus,
};
pub use rule::Rule;
pub use rule::{CombineConfig, SplitConfig, TransformConfig};
pub use rule::{EnvironmentSpec, GpuSpec, ResourceHint, Resources, RuleBuilder};
#[cfg(feature = "webhook")]
pub use webhook::{WebhookClient, WebhookConfig, WebhookData, WebhookEvent, WebhookPayload};
pub use wildcard::{wildcard_combinations_from_groups, wildcard_combinations_from_pairs};

/// Return the parent directory of `path`, falling back to `"."` when the path
/// has no parent component (e.g., a bare filename like `workflow.oxoflow`).
///
/// Rust's `Path::parent()` returns `Some(Path::new(""))` for bare filenames,
/// which causes `Command::current_dir("")` to fail with "No such file or
/// directory". This helper converts the empty-path case to `"."`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// let p = oxo_flow_core::parent_dir(Path::new("workflow.oxoflow"));
/// assert_eq!(p, Path::new("."));
///
/// let p = oxo_flow_core::parent_dir(Path::new("subdir/workflow.oxoflow"));
/// assert_eq!(p, Path::new("subdir"));
/// ```
#[must_use]
pub fn parent_dir(path: &std::path::Path) -> &std::path::Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => std::path::Path::new("."),
    }
}
