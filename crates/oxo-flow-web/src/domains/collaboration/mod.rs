//! Collaboration domain — pipeline sharing and forking.
//!
//! Enables fork, diff, share (link or workspace), and import of pipelines.
//! Shared pipelines are read-only; forks create independent copies in the
//! recipient's workspace. Uses the `oxo+https://` / `oxo+http://` protocols
//! for unambiguous imports (the share handler mints `oxo+http://` by default
//! — the server has no native TLS — and `oxo+https://` only when
//! OXO_FLOW_PUBLIC_URL declares a public HTTPS origin; issue #907 R16).

pub mod handlers;
pub mod service;
pub mod types;
