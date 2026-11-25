//! The canonical lock document and its on-disk envelope (issue #1399).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::LOCK_FORMAT;
use super::state::LockState;
use crate::config::ResolvedIncludeSource;

/// Tool/version that generated the document. Informational — verification
/// never requires it to match, so a lock remains checkable by a newer
/// engine. It is inside `lock_digest`, so a hand edit is still detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockGenerator {
    /// Always `"oxo-flow"`.
    pub tool: String,
    /// `CARGO_PKG_VERSION` of the generating binary.
    pub engine_version: String,
}

/// The workflow identity a lock pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedWorkflow {
    /// Workflow name (`[workflow] name`).
    pub name: String,
    /// Workflow version (`[workflow] version`).
    pub version: String,
    /// `sha256:<hex>` of the workflow file's bytes.
    pub content_sha256: String,
    /// Override-aware identity of the workflow + resolved config (computed
    /// WITHOUT the lock digest — the lock cannot contain itself).
    pub resolved_hash: String,
    /// Canonical metadata-contract digest, when the workflow declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_digest: Option<String>,
}

/// One pinned include source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInclude {
    /// `"local"` or `"repo"`.
    pub source: String,
    /// Git repository URL (repo includes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Declared git ref (repo includes only).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Checkout HEAD SHA recorded at resolution time (repo includes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_sha: Option<String>,
    /// Content digest of the include file, or `unresolved`.
    pub content_sha256: String,
}

impl LockedInclude {
    pub(crate) fn from_source(source: &ResolvedIncludeSource) -> Self {
        Self {
            source: if source.repo.is_some() {
                "repo".to_string()
            } else {
                "local".to_string()
            },
            repo: source.repo.clone(),
            git_ref: source.git_ref.clone(),
            resolved_sha: source.resolved_sha.clone(),
            content_sha256: source.content_sha256.clone(),
        }
    }
}

/// One rule's pinned environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedRule {
    /// The `"<kind>|<value>"` environment-lock identity from
    /// [`crate::env_lock::compute_env_lock_identity`].
    pub environment: String,
    /// Declared software specs in stable key order: image tags, bare env
    /// names, env-file paths, and the declared module list. These are
    /// declarations, not resolved package versions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub declared_software: BTreeMap<String, String>,
}

/// The canonical lock document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowLock {
    /// Format tag, always [`LOCK_FORMAT`] for a supported document.
    pub format: String,
    /// Generating tool/version (informational).
    pub generated_by: LockGenerator,
    /// Pinned workflow identity.
    pub workflow: LockedWorkflow,
    /// Canonical `[config]` snapshot.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, String>,
    /// Pinned include graph, keyed by canonical source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub includes: BTreeMap<String, LockedInclude>,
    /// Pinned per-rule environments.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, LockedRule>,
    /// `sha256:<hex>` over the canonical document excluding this field.
    pub lock_digest: String,
}

impl WorkflowLock {
    /// Build a lock from an observation. The digest is computed over the
    /// document without the digest field, so the returned document is
    /// self-consistent.
    #[must_use]
    pub fn from_state(state: &LockState) -> Self {
        let document = Self {
            format: LOCK_FORMAT.to_string(),
            generated_by: LockGenerator {
                tool: "oxo-flow".to_string(),
                engine_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            workflow: LockedWorkflow {
                name: state.workflow_name.clone(),
                version: state.workflow_version.clone(),
                content_sha256: state.workflow_content_sha256.clone(),
                resolved_hash: state.workflow_resolved_hash.clone(),
                metadata_digest: state.metadata_digest.clone(),
            },
            config: state.config.clone(),
            includes: state.includes.clone(),
            rules: state.rules.clone(),
            lock_digest: String::new(),
        };
        let digest = document.compute_digest();
        Self {
            lock_digest: digest,
            ..document
        }
    }

    /// `sha256:<hex>` over the canonical (key-sorted) JSON of this document
    /// with the `lock_digest` field removed. Deterministic by construction:
    /// `serde_json` maps are `BTreeMap`-backed, so field order never leaks
    /// in.
    #[must_use]
    pub fn compute_digest(&self) -> String {
        let mut value = serde_json::to_value(self)
            .expect("serializing a workflow lock for hashing cannot fail");
        if let Some(object) = value.as_object_mut() {
            object.remove("lock_digest");
        }
        let bytes = serde_json::to_vec(&value)
            .expect("canonicalizing a workflow lock for hashing cannot fail");
        format!("sha256:{}", hex::encode(Sha256::digest(&bytes)))
    }

    /// Whether the recorded digest matches the document's current content.
    #[must_use]
    pub fn digest_matches(&self) -> bool {
        self.lock_digest == self.compute_digest()
    }

    /// Canonical on-disk form: pretty-printed, key-sorted JSON with a
    /// trailing newline. Byte-identical for the same logical lock on every
    /// host and working directory.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        let value = serde_json::to_value(self).expect("serializing a workflow lock cannot fail");
        format!(
            "{}\n",
            serde_json::to_string_pretty(&value)
                .expect("pretty-printing a workflow lock cannot fail")
        )
    }

    /// Parse and validate a lock document's envelope: valid JSON and a
    /// supported format tag. Field-level drift is [`verify_lock`]'s job.
    pub fn parse(text: &str) -> Result<Self, LockParseError> {
        let lock: Self =
            serde_json::from_str(text).map_err(|e| LockParseError::Malformed(e.to_string()))?;
        if lock.format != LOCK_FORMAT {
            return Err(LockParseError::UnsupportedFormat { found: lock.format });
        }
        Ok(lock)
    }
}

/// Why a lock document could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LockParseError {
    /// The document is not a valid lock JSON document.
    #[error("lock document is not valid JSON: {0}")]
    Malformed(String),
    /// The document carries an unknown format tag.
    #[error("unsupported lock format '{found}' (this engine writes '{LOCK_FORMAT}')")]
    UnsupportedFormat {
        /// The tag found in the document.
        found: String,
    },
}

/// Why a lock file could not be loaded from disk.
#[derive(Debug, thiserror::Error)]
pub enum LockLoadError {
    /// The path does not exist.
    #[error("lock file not found: {}", .0.display())]
    Missing(PathBuf),
    /// The file exists but could not be read.
    #[error("failed to read lock file {}: {message}", path.display())]
    Io {
        /// The lock path.
        path: PathBuf,
        /// The I/O error message.
        message: String,
    },
    /// The file exists but is not a supported lock document.
    #[error("invalid lock file {}: {source}", path.display())]
    Parse {
        /// The lock path.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: LockParseError,
    },
}

/// Load and validate a lock file, distinguishing "missing" from "invalid"
/// so the caller can emit `LOCK-001` or `LOCK-007` faithfully.
pub fn load_lock(path: &Path) -> Result<WorkflowLock, LockLoadError> {
    if !path.exists() {
        return Err(LockLoadError::Missing(path.to_path_buf()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| LockLoadError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    WorkflowLock::parse(&text).map_err(|source| LockLoadError::Parse {
        path: path.to_path_buf(),
        source,
    })
}
