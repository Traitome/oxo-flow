//! Workflow-level lockfile (issue #1399).
//!
//! ## What this is
//!
//! A run already records evidence in its checkpoint (workflow content and
//! resolved hashes, metadata digest, per-rule environment-lock identity,
//! include resolutions). Those records are *after the fact*. This module
//! produces the **before the fact** counterpart: one canonical
//! `oxo-flow.lock` document that pins what a run is allowed to consume, so
//! `oxo-flow lock --check` (a CI gate) and `run --locked` /
//! `[engine] locked = true` can refuse to execute when the inputs moved.
//!
//! ## What is pinned
//!
//! - the workflow file's content digest, its override-aware
//!   `workflow_resolved_hash`, and the canonical metadata-contract digest;
//! - the canonical `[config]` snapshot (the same map the checkpoint stores:
//!   engine-injected keys excluded, sensitive values as SHA-256 digests);
//! - the include graph: workflow-root-relative path + content digest for
//!   local includes, `repo@ref` + resolved HEAD SHA + content digest for git
//!   modules;
//! - per-rule environment identity from
//!   [`crate::env_lock::compute_env_lock_identity`] (resolved container
//!   digest, env lock hash, or an explicit sentinel) and the declared
//!   software specs/versions the workflow writes down (image tags, bare env
//!   names, modules, env-file content digests);
//! - `lock_digest`, a SHA-256 over the canonical document excluding the
//!   digest field itself.
//!
//! ## What is NOT pinned
//!
//! A lock is **evidence of declared inputs, not bitwise reproducibility**.
//! It does not solve packages, pull images, or contact a registry; it does
//! not prove a tool behaves identically on another host. Every container
//! digest is whatever the local runtime could report offline — an absent
//! image is recorded as [`crate::env_lock::ENV_LOCK_UNRESOLVED`], never
//! fabricated. `lock_digest` proves the document's fields were the ones
//! hashed together; it is not a signature and not an external notarization.
//!
//! ## Unknown is never a match
//!
//! Two sentinels exist, inherited from the environment-lock identity:
//!
//! - `unresolved` — real content exists but could not be resolved offline.
//!   It never compares equal, **including to itself** ([`LOCK_006`]).
//! - `unavailable` — the backend supplies no lockable content (`system`).
//!   It compares equal only to `unavailable`.
//!
//! ## Drift codes
//!
//! | Code | Meaning |
//! |---|---|
//! | [`LOCK_001`] | lock missing under enforcement |
//! | [`LOCK_002`] | workflow content / resolved hash / metadata digest changed |
//! | [`LOCK_003`] | config value added, removed, or changed |
//! | [`LOCK_004`] | environment identity or declared software changed / rule not pinned |
//! | [`LOCK_005`] | include changed, added, removed, or unresolvable |
//! | [`LOCK_006`] | environment identity unresolved (unknown never matches) |
//! | [`LOCK_007`] | malformed or unsupported lock document |
//!
//! ## Backward compatibility
//!
//! Nothing here runs unless a lock is enforced. A workflow without
//! `--locked` / `[engine] locked = true` loads, executes, and records exactly
//! the bytes it recorded before this module existed.

use std::path::{Path, PathBuf};

mod document;
mod state;
mod verify;

#[cfg(test)]
mod tests;

pub use document::{
    LockGenerator, LockLoadError, LockParseError, LockedInclude, LockedRule, LockedWorkflow,
    WorkflowLock, load_lock,
};
pub use state::LockState;
pub use verify::{LockFinding, LockVerification, verify_lock};

/// Format tag carried by every lock document.
pub const LOCK_FORMAT: &str = "oxo-flow.lock/v1";

/// Canonical lock filename written next to the workflow by default.
pub const DEFAULT_LOCK_FILENAME: &str = "oxo-flow.lock";

/// `LOCK-001`: no lock file exists but enforcement is on.
pub const LOCK_001: &str = "LOCK-001";
/// `LOCK-002`: workflow content hash, resolved hash, or metadata digest
/// changed.
pub const LOCK_002: &str = "LOCK-002";
/// `LOCK-003`: a config value was added, removed, or changed.
pub const LOCK_003: &str = "LOCK-003";
/// `LOCK-004`: an environment identity or declared software changed, or a
/// current rule is not pinned.
pub const LOCK_004: &str = "LOCK-004";
/// `LOCK-005`: an include was changed, added, removed, or is unresolvable.
pub const LOCK_005: &str = "LOCK-005";
/// `LOCK-006`: an environment identity is unresolved — unknown never
/// matches, including to itself.
pub const LOCK_006: &str = "LOCK-006";
/// `LOCK-007`: the lock document is malformed, unsupported, or its own
/// digest does not match.
pub const LOCK_007: &str = "LOCK-007";

/// The default lock path for a workflow: `oxo-flow.lock` beside it.
#[must_use]
pub fn default_lock_path(workflow: &Path) -> PathBuf {
    crate::parent_dir(workflow).join(DEFAULT_LOCK_FILENAME)
}
