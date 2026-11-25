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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{ResolvedIncludeSource, WorkflowConfig};
use crate::env_lock::{ENV_LOCK_UNRESOLVED, compute_env_lock_identity};
use crate::executor::checkpoint::compute_workflow_resolved_hash;

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
    fn from_source(source: &ResolvedIncludeSource) -> Self {
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

/// The current on-disk state a lock is verified against: exactly the fields
/// a lock pins, derived once from the freshly parsed (and resolved) config.
///
/// Generation and verification both build this struct from the same
/// observation function, so a lock can never pin a quantity the verifier
/// computes differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockState {
    pub workflow_name: String,
    pub workflow_version: String,
    pub workflow_content_sha256: String,
    pub workflow_resolved_hash: String,
    pub metadata_digest: Option<String>,
    pub config: BTreeMap<String, String>,
    pub includes: BTreeMap<String, LockedInclude>,
    pub rules: BTreeMap<String, LockedRule>,
}

impl LockState {
    /// Observe a resolved workflow: `config` must already be post-defaults,
    /// post-override/profile, and post-wildcard-expansion — the state a run
    /// executes. `workflow_content_sha256` is the digest of the workflow
    /// file's raw bytes. Entirely offline: environment identities are what
    /// the local runtime can report without pulling anything.
    #[must_use]
    pub fn observe(
        config: &WorkflowConfig,
        workflow_path: Option<&Path>,
        workflow_content_sha256: &str,
    ) -> Self {
        let config_snapshot = crate::config_impact::canonical_config_snapshot(config);
        let resolved_hash = compute_workflow_resolved_hash(
            workflow_content_sha256,
            &config_snapshot,
            config.metadata_digest.as_deref(),
        );
        let env_identity = compute_env_lock_identity(config, workflow_path);
        let software =
            crate::software_versions::collect_software_versions(config, None, workflow_path);
        // Declared software per rule, shaped for canonical comparison. The
        // environment map is the authoritative rule set (it is built from
        // the same config), so the two halves can never disagree about
        // which rules exist.
        let declared_software: BTreeMap<String, BTreeMap<String, String>> = software
            .entries
            .iter()
            .map(|entry| {
                let mut declared: BTreeMap<String, String> = entry.specs.iter().cloned().collect();
                if !entry.modules.is_empty() {
                    let modules: Vec<String> = entry
                        .modules
                        .iter()
                        .map(|(name, version)| match version {
                            Some(version) => format!("{name}/{version}"),
                            None => name.clone(),
                        })
                        .collect();
                    declared.insert("modules".to_string(), modules.join(","));
                }
                (entry.rule.clone(), declared)
            })
            .collect();

        let rules: BTreeMap<String, LockedRule> = env_identity
            .into_iter()
            .map(|(rule, environment)| {
                let declared = declared_software.get(&rule).cloned().unwrap_or_default();
                (
                    rule,
                    LockedRule {
                        environment,
                        declared_software: declared,
                    },
                )
            })
            .collect();

        let includes: BTreeMap<String, LockedInclude> = config
            .include_sources
            .iter()
            .map(|source| (source.lock_key(), LockedInclude::from_source(source)))
            .collect();

        Self {
            workflow_name: config.workflow.name.clone(),
            workflow_version: config.workflow.version.clone(),
            workflow_content_sha256: workflow_content_sha256.to_string(),
            workflow_resolved_hash: resolved_hash,
            metadata_digest: config.metadata_digest.clone(),
            config: config_snapshot,
            includes,
            rules,
        }
    }
}

/// One drift finding. `code` is one of the stable `LOCK-*` constants;
/// `subject` names the rule/field and `expected`/`actual` carry the values
/// being compared (either may be absent when the subject is missing on one
/// side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LockFinding {
    /// Stable drift code (`LOCK-001` … `LOCK-007`).
    pub code: &'static str,
    /// The rule name, config key, include key, or workflow field.
    pub subject: String,
    /// The locked value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// The current value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Human-readable explanation, including which side is missing.
    pub message: String,
}

/// The full verification result: findings sorted by `(code, subject)` so
/// the same drift always produces byte-identical output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LockVerification {
    pub findings: Vec<LockFinding>,
}

impl LockVerification {
    /// Whether the lock matches the observed state.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
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

/// Compare a lock against the current observation. Pure and offline.
///
/// Direction: every rule the current config declares must be pinned. An
/// entry present only in the lock is NOT a finding — partial execution
/// (`--samples`, `--target`, `--module`, `when` pruning) legitimately runs a
/// subset of the expanded workflow, and a genuinely removed rule is already
/// caught by the workflow content hash (`LOCK-002`).
#[must_use]
pub fn verify_lock(lock: &WorkflowLock, state: &LockState) -> LockVerification {
    let mut findings = Vec::new();
    if lock.format != LOCK_FORMAT {
        findings.push(finding(
            LOCK_007,
            "format",
            Some(LOCK_FORMAT.to_string()),
            Some(lock.format.clone()),
            "the lock document uses an unsupported format tag",
        ));
    }
    if !lock.digest_matches() {
        findings.push(finding(
            LOCK_007,
            "lock_digest",
            Some(lock.compute_digest()),
            Some(lock.lock_digest.clone()),
            "the lock document's own digest does not match its content — the file was edited or corrupted",
        ));
    }
    verify_workflow_identity(lock, state, &mut findings);
    verify_config(lock, state, &mut findings);
    verify_includes(lock, state, &mut findings);
    verify_rules(lock, state, &mut findings);
    findings.sort_by(|left, right| {
        (left.code, left.subject.as_str()).cmp(&(right.code, right.subject.as_str()))
    });
    LockVerification { findings }
}

fn verify_workflow_identity(
    lock: &WorkflowLock,
    state: &LockState,
    findings: &mut Vec<LockFinding>,
) {
    let checks = [
        (
            "workflow.content_sha256",
            lock.workflow.content_sha256.clone(),
            state.workflow_content_sha256.clone(),
            "the workflow file's bytes changed since the lock was generated",
        ),
        (
            "workflow.resolved_hash",
            lock.workflow.resolved_hash.clone(),
            state.workflow_resolved_hash.clone(),
            "the workflow's resolved identity changed since the lock was generated",
        ),
    ];
    for (subject, expected, actual, message) in checks {
        if expected != actual {
            findings.push(finding(
                LOCK_002,
                subject,
                Some(expected),
                Some(actual),
                message,
            ));
        }
    }
    if lock.workflow.metadata_digest != state.metadata_digest {
        findings.push(finding(
            LOCK_002,
            "workflow.metadata_digest",
            lock.workflow.metadata_digest.clone(),
            state.metadata_digest.clone(),
            "the metadata contract (schema or rows) changed since the lock was generated",
        ));
    }
}

fn verify_config(lock: &WorkflowLock, state: &LockState, findings: &mut Vec<LockFinding>) {
    let keys: BTreeSet<&String> = lock.config.keys().chain(state.config.keys()).collect();
    for key in keys {
        let expected = lock.config.get(key);
        let actual = state.config.get(key);
        if expected == actual {
            continue;
        }
        let message = match (expected, actual) {
            (None, Some(_)) => "the config key is not pinned by the lock (added since generation)",
            (Some(_), None) => "the config key pinned by the lock is no longer declared",
            _ => "the config value differs from the locked value",
        };
        findings.push(finding(
            LOCK_003,
            format!("config.{key}"),
            expected.cloned(),
            actual.cloned(),
            message,
        ));
    }
}

fn verify_includes(lock: &WorkflowLock, state: &LockState, findings: &mut Vec<LockFinding>) {
    let keys: BTreeSet<&String> = lock.includes.keys().chain(state.includes.keys()).collect();
    for key in keys {
        let expected = lock.includes.get(key);
        let actual = state.includes.get(key);
        if expected == actual {
            continue;
        }
        let message = if expected.is_none() {
            "an include is not pinned by the lock (added since generation)"
        } else if actual.is_none() {
            "an include pinned by the lock is no longer in the workflow's include graph"
        } else if expected.is_some_and(|include| is_unresolved(&include.content_sha256))
            || actual.is_some_and(|include| is_unresolved(&include.content_sha256))
        {
            "an include's content digest is unresolved — unknown never matches"
        } else {
            "the include's source or content changed since generation"
        };
        findings.push(finding(
            LOCK_005,
            key.clone(),
            expected.map(|include| include.content_sha256.clone()),
            actual.map(|include| include.content_sha256.clone()),
            message,
        ));
    }
}

fn verify_rules(lock: &WorkflowLock, state: &LockState, findings: &mut Vec<LockFinding>) {
    for (rule, current) in &state.rules {
        let Some(locked) = lock.rules.get(rule) else {
            findings.push(finding(
                LOCK_004,
                rule.clone(),
                None,
                Some(current.environment.clone()),
                "the rule is not pinned by the lock",
            ));
            continue;
        };
        compare_environment(rule, &locked.environment, &current.environment, findings);
        let fields: BTreeSet<&String> = locked
            .declared_software
            .keys()
            .chain(current.declared_software.keys())
            .collect();
        for field in fields {
            let expected = locked.declared_software.get(field);
            let actual = current.declared_software.get(field);
            if expected == actual {
                continue;
            }
            findings.push(finding(
                LOCK_004,
                format!("{rule}.{field}"),
                expected.cloned(),
                actual.cloned(),
                "the rule's declared software/version differs from the lock",
            ));
        }
    }
}

/// Environment-identity comparison with the two sentinels:
/// `unresolved` never matches (including itself); `unavailable` matches only
/// `unavailable`.
fn compare_environment(rule: &str, expected: &str, actual: &str, findings: &mut Vec<LockFinding>) {
    if is_unresolved(expected) || is_unresolved(actual) {
        findings.push(finding(
            LOCK_006,
            rule.to_string(),
            Some(expected.to_string()),
            Some(actual.to_string()),
            "the environment identity is unresolved — a missing digest is unknown, never a match",
        ));
        return;
    }
    if expected == actual {
        return;
    }
    findings.push(finding(
        LOCK_004,
        rule.to_string(),
        Some(expected.to_string()),
        Some(actual.to_string()),
        "the rule's environment identity differs from the lock",
    ));
}

fn is_unresolved(value: &str) -> bool {
    value.ends_with(ENV_LOCK_UNRESOLVED)
}

fn finding(
    code: &'static str,
    subject: impl Into<String>,
    expected: Option<String>,
    actual: Option<String>,
    message: impl Into<String>,
) -> LockFinding {
    LockFinding {
        code,
        subject: subject.into(),
        expected,
        actual,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::checkpoint::compute_file_checksum;
    use std::path::PathBuf;

    const BASE_WORKFLOW: &str = r#"
[workflow]
name = "lockdemo"
version = "1.0"

[[rules]]
name = "hello"
shell = "echo hi"
"#;

    /// Parse a workflow file the way the CLI does before observing it:
    /// includes resolved, defaults applied, wildcards expanded.
    fn loaded_config(workflow: &Path) -> WorkflowConfig {
        let mut config = WorkflowConfig::from_file(workflow).expect("workflow parses");
        config.apply_defaults();
        config.expand_wildcards().expect("wildcards expand");
        config
    }

    fn write_workflow(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("wf.oxoflow");
        std::fs::write(&path, content).expect("workflow written");
        path
    }

    /// The CLI-equivalent generation pair: (lock, observation).
    fn generate(workflow: &Path) -> (WorkflowLock, LockState) {
        let config = loaded_config(workflow);
        let content_hash = compute_file_checksum(workflow).expect("workflow hashes");
        let state = LockState::observe(&config, Some(workflow), &content_hash);
        let lock = WorkflowLock::from_state(&state);
        (lock, state)
    }

    fn findings_for(lock: &WorkflowLock, workflow: &Path) -> Vec<LockFinding> {
        let config = loaded_config(workflow);
        let content_hash = compute_file_checksum(workflow).expect("workflow hashes");
        let state = LockState::observe(&config, Some(workflow), &content_hash);
        verify_lock(lock, &state).findings
    }

    fn codes(findings: &[LockFinding]) -> Vec<&'static str> {
        findings.iter().map(|finding| finding.code).collect()
    }

    fn state_with_rules(environments: &[(&str, &str)]) -> LockState {
        LockState {
            workflow_name: "t".to_string(),
            workflow_version: "1.0".to_string(),
            workflow_content_sha256: "sha256:aa".to_string(),
            workflow_resolved_hash: "sha256:bb".to_string(),
            metadata_digest: None,
            config: BTreeMap::new(),
            includes: BTreeMap::new(),
            rules: environments
                .iter()
                .map(|(rule, environment)| {
                    (
                        (*rule).to_string(),
                        LockedRule {
                            environment: (*environment).to_string(),
                            declared_software: BTreeMap::new(),
                        },
                    )
                })
                .collect(),
        }
    }

    // (a) Generation determinism: the same resolved input produces the same
    // bytes, twice.
    #[test]
    fn generation_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
        let (first, _) = generate(&workflow);
        let (second, _) = generate(&workflow);
        assert_eq!(
            first.canonical_json(),
            second.canonical_json(),
            "the lock document must be byte-identical across runs"
        );
        assert!(first.digest_matches(), "generation finalizes the digest");
    }

    // (b) The acceptance test: the same logical workflow in two different
    // working directories generates byte-identical lock bytes. Paths in the
    // document are workflow-relative — no absolute or host-specific path may
    // leak in.
    #[test]
    fn lock_bytes_are_identical_across_working_directories() {
        let workflow_text = r#"
[workflow]
name = "portable"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
"#;
        let extra = "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n";
        let env = "dependencies:\n  - bwa=0.7.17\n";

        let mut locks = Vec::new();
        for _ in 0..2 {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("rules")).unwrap();
            std::fs::create_dir_all(dir.path().join("envs")).unwrap();
            std::fs::write(dir.path().join("rules/extra.oxoflow"), extra).unwrap();
            std::fs::write(dir.path().join("envs/env.yaml"), env).unwrap();
            let workflow = write_workflow(dir.path(), workflow_text);
            let (lock, _) = generate(&workflow);
            let json = lock.canonical_json();
            assert!(
                !json.contains(&dir.path().display().to_string()),
                "no absolute path may leak into the lock: {json}"
            );
            locks.push(json);
        }
        assert_eq!(
            locks[0], locks[1],
            "the same logical workflow in different cwds must produce identical lock bytes"
        );
    }

    // (c) Input row order must not leak into the lock: reordering metadata
    // rows leaves the metadata digest (and therefore the lock) unchanged.
    #[test]
    fn lock_bytes_are_identical_across_metadata_row_order() {
        let workflow_text = r#"
[workflow]
name = "meta-lock"
version = "1.0"
metadata_file = "meta.tsv"

[workflow.metadata_schema]
required_columns = ["sample"]

[[rules]]
name = "hello"
shell = "echo hi"
"#;
        let header = "sample\tvalue\n";
        let rows_a = "S1\t10\nS2\t20\n";
        let rows_b = "S2\t20\nS1\t10\n";

        let mut locks = Vec::new();
        for rows in [rows_a, rows_b] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("meta.tsv"), format!("{header}{rows}")).unwrap();
            let workflow = write_workflow(dir.path(), workflow_text);
            let (lock, _) = generate(&workflow);
            assert!(
                lock.workflow.metadata_digest.is_some(),
                "a metadata contract must be pinned"
            );
            locks.push(lock.canonical_json());
        }
        assert_eq!(
            locks[0], locks[1],
            "metadata row order must not change lock bytes"
        );
    }

    // (d) Config declaration order must not leak into the lock either.
    #[test]
    fn config_declaration_order_does_not_change_lock_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_workflow(
            dir.path(),
            r#"
[workflow]
name = "cfg"
version = "1.0"

[config]
b = 2
a = 1

[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let config_a = loaded_config(&first);
        let text_b = std::fs::read_to_string(&first)
            .unwrap()
            .replace("b = 2\na = 1", "a = 1\nb = 2");
        let second = dir.path().join("wf2.oxoflow");
        std::fs::write(&second, text_b).unwrap();
        let config_b = loaded_config(&second);
        // The two files differ in bytes, so hold the content digest constant
        // to isolate config ordering.
        let state_a = LockState::observe(&config_a, Some(&first), "sha256:same");
        let state_b = LockState::observe(&config_b, Some(&second), "sha256:same");
        assert_eq!(state_a.config, state_b.config);
        assert_eq!(
            WorkflowLock::from_state(&state_a).canonical_json(),
            WorkflowLock::from_state(&state_b).canonical_json(),
            "config declaration order must not change lock bytes"
        );
    }

    // (e) Canonical on-disk form is key-ordered JSON with a trailing newline.
    #[test]
    fn canonical_json_is_key_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(
            dir.path(),
            &BASE_WORKFLOW.replace("[[rules]]", "[config]\nthreshold = 1\n\n[[rules]]"),
        );
        let (lock, _) = generate(&workflow);
        let json = lock.canonical_json();
        assert!(json.ends_with('\n'), "the file ends with a newline");
        let config = json.find("\n  \"config\"").expect("config key");
        let format = json.find("\n  \"format\"").expect("format key");
        let workflow_key = json.find("\n  \"workflow\"").expect("workflow key");
        assert!(
            config < format && format < workflow_key,
            "top-level keys must be sorted: {json}"
        );
    }

    // (f) Clean verification: a freshly generated lock verifies against the
    // same workflow.
    #[test]
    fn fresh_lock_verifies_clean() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
        let (lock, state) = generate(&workflow);
        let verification = verify_lock(&lock, &state);
        assert!(
            verification.ok(),
            "a fresh lock must verify: {:?}",
            verification.findings
        );
    }

    // (g) Workflow content drift.
    #[test]
    fn workflow_content_change_reports_lock_002() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
        let (lock, _) = generate(&workflow);
        std::fs::write(&workflow, BASE_WORKFLOW.replace("echo hi", "echo bye")).unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_002), "{findings:?}");
        assert!(
            findings
                .iter()
                .any(|finding| finding.subject == "workflow.content_sha256")
        );
    }

    // (h) Metadata drift: an edited metadata row is LOCK-002 even when the
    // workflow file's bytes and config are unchanged.
    #[test]
    fn metadata_change_reports_lock_002() {
        let workflow_text = r#"
[workflow]
name = "meta-lock"
version = "1.0"
metadata_file = "meta.tsv"

[workflow.metadata_schema]
required_columns = ["sample"]

[[rules]]
name = "hello"
shell = "echo hi"
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("meta.tsv"), "sample\tvalue\nS1\t10\n").unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        std::fs::write(dir.path().join("meta.tsv"), "sample\tvalue\nS1\t11\n").unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_002), "{findings:?}");
        assert!(
            findings
                .iter()
                .any(|finding| finding.subject == "workflow.metadata_digest")
        );
    }

    // (i) Config drift.
    #[test]
    fn config_change_reports_lock_003() {
        let workflow_text = r#"
[workflow]
name = "cfg"
version = "1.0"

[config]
threshold = 10

[[rules]]
name = "hello"
shell = "echo {config.threshold}"
"#;
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        let changed = workflow_text.replace("threshold = 10", "threshold = 20");
        std::fs::write(&workflow, changed).unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_003), "{findings:?}");
        let config_finding = findings
            .iter()
            .find(|finding| finding.subject == "config.threshold")
            .expect("per-key finding");
        assert_eq!(config_finding.expected.as_deref(), Some("10"));
        assert_eq!(config_finding.actual.as_deref(), Some("20"));
    }

    // (j) Environment spec drift: editing the spec file changes the
    // environment identity, with no network or solver involved.
    #[test]
    fn environment_spec_change_reports_lock_004() {
        let workflow_text = r#"
[workflow]
name = "env-lock"
version = "1.0"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("envs")).unwrap();
        std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.17\n").unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        assert!(
            crate::env_lock::env_lock_is_resolved(&lock.rules["hello"].environment),
            "the conda file fixture must resolve: {:?}",
            lock.rules
        );
        std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.18\n").unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_004), "{findings:?}");
    }

    // (k) Include drift: editing an included module is LOCK-005.
    #[test]
    fn include_content_change_reports_lock_005() {
        let workflow_text = r#"
[workflow]
name = "inc-lock"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::write(
            dir.path().join("rules/extra.oxoflow"),
            "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
        )
        .unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        assert!(lock.includes.contains_key("rules/extra.oxoflow"));
        std::fs::write(
            dir.path().join("rules/extra.oxoflow"),
            "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo changed\"\n",
        )
        .unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_005), "{findings:?}");
        assert!(
            findings
                .iter()
                .any(|finding| finding.subject == "rules/extra.oxoflow")
        );
    }

    // (l) A rule that disappears from the include graph is LOCK-005.
    #[test]
    fn include_removed_reports_lock_005() {
        let workflow_text = r#"
[workflow]
name = "inc-lock"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::write(
            dir.path().join("rules/extra.oxoflow"),
            "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
        )
        .unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        let removed = workflow_text.replace("[[include]]\npath = \"rules/extra.oxoflow\"\n", "");
        std::fs::write(&workflow, removed).unwrap();
        let findings = findings_for(&lock, &workflow);
        assert!(codes(&findings).contains(&LOCK_005), "{findings:?}");
    }

    // (m) Unknown is never a match: an unresolved environment identity does
    // not verify even against itself.
    #[test]
    fn unresolved_environment_never_matches_itself() {
        let state = state_with_rules(&[("align", "docker|unresolved")]);
        let lock = WorkflowLock::from_state(&state);
        let findings = verify_lock(&lock, &state).findings;
        assert_eq!(codes(&findings), vec![LOCK_006], "{findings:?}");
    }

    // (n) `unavailable` compares equal only to `unavailable`.
    #[test]
    fn unavailable_matches_only_unavailable() {
        let state = state_with_rules(&[("plain", "system|unavailable")]);
        let lock = WorkflowLock::from_state(&state);
        assert!(verify_lock(&lock, &state).ok());

        let changed = state_with_rules(&[("plain", "docker|repo@sha256:abc")]);
        let findings = verify_lock(&lock, &changed).findings;
        assert_eq!(codes(&findings), vec![LOCK_004], "{findings:?}");
    }

    // (o) A tampered lock document is LOCK-007, and the field-level drift it
    // introduced is still reported.
    #[test]
    fn tampered_lock_digest_reports_lock_007() {
        let state = state_with_rules(&[("plain", "system|unavailable")]);
        let mut lock = WorkflowLock::from_state(&state);
        lock.lock_digest = "sha256:0000".to_string();
        let findings = verify_lock(&lock, &state).findings;
        assert!(codes(&findings).contains(&LOCK_007), "{findings:?}");
    }

    // (p) Envelope errors: malformed JSON and unsupported format tags.
    #[test]
    fn malformed_and_unsupported_locks_are_rejected() {
        assert!(matches!(
            WorkflowLock::parse("{not json"),
            Err(LockParseError::Malformed(_))
        ));
        let state = state_with_rules(&[("plain", "system|unavailable")]);
        let lock = WorkflowLock::from_state(&state);
        let renamed = lock
            .canonical_json()
            .replace(LOCK_FORMAT, "oxo-flow.lock/v99");
        assert!(matches!(
            WorkflowLock::parse(&renamed),
            Err(LockParseError::UnsupportedFormat { .. })
        ));
    }

    // (q) Round trip: the canonical on-disk bytes parse back to an equal
    // document whose digest still matches.
    #[test]
    fn canonical_json_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
        let (lock, _) = generate(&workflow);
        let parsed = WorkflowLock::parse(&lock.canonical_json()).unwrap();
        assert_eq!(parsed, lock);
        assert!(parsed.digest_matches());
    }

    // (r) Include sources are recorded workflow-relative with `/`
    // separators, and declared software is pinned.
    #[test]
    fn include_sources_and_declared_software_are_recorded() {
        let workflow_text = r#"
[workflow]
name = "record"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
modules = ["gcc/11.2"]
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::create_dir_all(dir.path().join("envs")).unwrap();
        std::fs::write(
            dir.path().join("rules/extra.oxoflow"),
            "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.17\n").unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let config = loaded_config(&workflow);
        assert_eq!(config.include_sources.len(), 1);
        assert_eq!(config.include_sources[0].path, "rules/extra.oxoflow");
        assert!(
            config.include_sources[0]
                .content_sha256
                .starts_with("sha256:")
        );

        let (lock, _) = generate(&workflow);
        let rule = &lock.rules["hello"];
        assert_eq!(
            rule.declared_software.get("conda_file").map(String::as_str),
            Some("envs/env.yaml")
        );
        assert_eq!(
            rule.declared_software.get("modules").map(String::as_str),
            Some("gcc/11.2")
        );
    }

    // (s) The resolved-hash fold is additive: no lock digest keeps the
    // legacy value, and a lock digest changes the identity.
    #[test]
    fn resolved_hash_fold_is_additive() {
        let snapshot: BTreeMap<String, String> =
            BTreeMap::from([("k".to_string(), "v".to_string())]);
        let legacy = compute_workflow_resolved_hash("sha256:abc", &snapshot, None);
        assert_eq!(
            legacy,
            crate::executor::checkpoint::compute_workflow_resolved_hash_with_lock(
                "sha256:abc",
                &snapshot,
                None,
                None
            ),
            "the three-argument form must stay byte-identical"
        );
        let locked = crate::executor::checkpoint::compute_workflow_resolved_hash_with_lock(
            "sha256:abc",
            &snapshot,
            None,
            Some("sha256:lock"),
        );
        assert_ne!(legacy, locked);
    }

    // (t) The checkpoint's lock_digest is additive: absent by default,
    // round-trips when set.
    #[test]
    fn checkpoint_lock_digest_is_legacy_safe() {
        let mut state = crate::executor::CheckpointState::new();
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("lock_digest"),
            "a legacy checkpoint must not grow a lock field: {json}"
        );
        state.set_lock_digest(Some("sha256:abc".to_string()));
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("lock_digest"));
        let loaded: crate::executor::CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.lock_digest.as_deref(), Some("sha256:abc"));
        // A legacy checkpoint without the field loads with None.
        let legacy: crate::executor::CheckpointState =
            serde_json::from_str(r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{}}"#)
                .unwrap();
        assert!(legacy.lock_digest.is_none());
    }
}
