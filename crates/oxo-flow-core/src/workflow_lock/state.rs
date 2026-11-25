//! Observation of a resolved workflow — the fields a lock pins (issue #1399).

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::WorkflowConfig;
use crate::env_lock::compute_env_lock_identity;
use crate::executor::checkpoint::compute_workflow_resolved_hash;

use super::document::{LockedInclude, LockedRule};

/// The current on-disk state a lock is verified against: exactly the fields
/// a lock pins, derived once from the freshly parsed (and resolved) config.
///
/// Generation and verification both build this struct from the same
/// observation function, so a lock can never pin a quantity the verifier
/// computes differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockState {
    /// `[workflow] name` of the observed workflow.
    pub workflow_name: String,
    /// `[workflow] version` of the observed workflow.
    pub workflow_version: String,
    /// `sha256:<hex>` of the workflow file's raw bytes.
    pub workflow_content_sha256: String,
    /// Override-aware workflow identity (computed WITHOUT the lock digest).
    pub workflow_resolved_hash: String,
    /// Canonical metadata-contract digest, when the workflow declares one.
    pub metadata_digest: Option<String>,
    /// Canonical `[config]` snapshot (sensitive values already digested).
    pub config: BTreeMap<String, String>,
    /// The current include graph, keyed like the lock document.
    pub includes: BTreeMap<String, LockedInclude>,
    /// The current per-rule environment identities and declared software.
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
        // The lock is a committed artifact, so a credential-shaped key is
        // stored as a SHA-256 digest even when the workflow failed to declare
        // it `sensitive = true` (issue #1399 review I3). Declared sensitive
        // keys are already digested by `canonical_config_snapshot`; this only
        // adds the conservative name heuristic. Verification observes the
        // same way, so equality semantics are unchanged — only the stored
        // form is.
        let sensitive = config.sensitive_config_keys();
        let config_snapshot: BTreeMap<String, String> = config_snapshot
            .into_iter()
            .map(|(key, value)| {
                if sensitive.contains(&key) || !is_credential_key(&key) {
                    (key, value)
                } else {
                    (key, crate::config_impact::digest_config_value(&value))
                }
            })
            .collect();
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

/// Conservative credential-name heuristic for the lock's config snapshot
/// (issue #1399 review I3): a key whose `_`/`-`/camel-case segments name a
/// credential is stored as a digest even without an explicit
/// `sensitive = true`. Segment-based equality keeps ordinary keys such as
/// `hockey` or `monkey` verbatim; `api_token`, `API_KEY`, `dbPassword`,
/// `aws_secret` and `secret_sauce` match.
#[must_use]
pub(crate) fn is_credential_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "CREDENTIALS",
    ];
    const SUFFIXES: &[&str] = &[
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "CREDENTIALS",
    ];
    key.split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .any(|segment| {
            EXACT.contains(&segment.as_str())
                || SUFFIXES
                    .iter()
                    .any(|suffix| segment.len() > suffix.len() && segment.ends_with(suffix))
        })
}
