//! # oxo-flow-core
//!
//! Core library for the oxo-flow bioinformatics pipeline engine.
//!
//! oxo-flow is a Rust-native workflow engine purpose-built for
//! bioinformatics pipelines. It provides:
//!
//! - **DAG engine**: Build, validate, and execute directed acyclic graphs of tasks
//! - **Environment management**: First-class support for conda, pixi, docker, singularity, venv
//! - **Wildcard expansion**: `{sample}` pattern substitution with automatic discovery
//! - **Resource scheduling**: CPU, memory, GPU-aware job scheduling
//! - **Report generation**: Modular, clinical-grade HTML/JSON reporting
//! - **Container packaging**: Package workflows into portable Docker/Singularity images
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

pub mod cluster;
pub mod config;
pub mod container;
pub mod dag;
pub mod environment;
pub mod error;
pub mod executor;
pub mod format;
pub mod report;
pub mod rule;
pub mod scheduler;
pub mod wildcard;

// Re-export key types at the crate root for convenience.
pub use config::WorkflowConfig;
pub use dag::WorkflowDag;
pub use error::{OxoFlowError, Result};
pub use rule::Rule;
