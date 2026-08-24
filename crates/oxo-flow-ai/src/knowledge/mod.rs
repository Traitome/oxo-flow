//! Knowledge system — domain knowledge for AI agents.
//!
//! The knowledge system provides:
//! - Built-in bioinformatics tool reference tables
//! - Error pattern matching for diagnosis
//! - Best practice rules for workflow validation
//! - Context assembly for different agent scenarios

pub mod assembler;
pub mod bioconda;
pub mod builtin;
pub mod meta;
pub mod pipeline_graph;
pub mod registry;
pub mod skills;
