//! Shared structured JSON failure output for `--json` commands.
//!
//! `validate --json` and `lint --json` already emit a machine-readable
//! document when the workflow cannot be parsed. `dry-run --json` and
//! `status --json` used to exit 1 with EMPTY stdout on exactly those failure
//! paths, forcing every consumer to special-case them. This module is the
//! single envelope those commands reuse:
//!
//! ```json
//! {
//!   "command": "<command>",
//!   "errors": [ { "code": "<CODE>", "message": "<detail>" } ]
//! }
//! ```
//!
//! Command-specific context (workflow/checkpoint paths) is merged in as
//! additional top-level fields. The command still exits non-zero — the
//! dispatcher propagates the original error.

use serde_json::{Value, json};

/// Emit the failure envelope on stdout.
///
/// `context` must be a JSON object (or `Value::Null`); its fields are merged
/// into the envelope. The document is always printed — a JSON consumer must
/// never receive an empty stdout.
pub fn emit(command: &str, code: &str, message: &str, context: Value) {
    let mut document = json!({
        "command": command,
        "errors": [{"code": code, "message": message}],
    });
    if let (Some(object), Value::Object(extra)) = (document.as_object_mut(), context) {
        for (key, value) in extra {
            object.insert(key, value);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&document).expect("JSON document is serializable")
    );
}

/// Best-effort machine code for a CLI failure.
///
/// Workflow parse/deserialization failures use `PARSE` — the code
/// `validate --json` and `lint --json` already report — so a consumer can
/// branch on one spelling across commands. Everything else falls back to the
/// caller's code.
pub fn code_for(error: &anyhow::Error, fallback: &'static str) -> &'static str {
    for cause in error.chain() {
        if let Some(core) = cause.downcast_ref::<oxo_flow_core::error::OxoFlowError>() {
            return match core {
                oxo_flow_core::error::OxoFlowError::Parse { .. }
                | oxo_flow_core::error::OxoFlowError::Toml(_)
                | oxo_flow_core::error::OxoFlowError::Config { .. } => "PARSE",
                _ => fallback,
            };
        }
    }
    fallback
}
