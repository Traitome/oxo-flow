//! Collaboration domain — pipeline sharing and forking.
//!
//! Enables fork, diff, share (link or workspace), and import of pipelines.
//! Shared pipelines are read-only; forks create independent copies in the
//! recipient's workspace. Uses the `oxo+https://` protocol for unambiguous imports.

pub mod handlers;
pub mod service;
pub mod types;
