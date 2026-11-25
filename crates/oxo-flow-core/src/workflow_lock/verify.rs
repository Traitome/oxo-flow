//! Lock-vs-reality comparison and the stable `LOCK-*` drift codes (issue #1399).

use std::collections::BTreeSet;

use serde::Serialize;

use crate::env_lock::ENV_LOCK_UNRESOLVED;

use super::document::WorkflowLock;
use super::state::LockState;
use super::{LOCK_002, LOCK_003, LOCK_004, LOCK_005, LOCK_006, LOCK_007, LOCK_FORMAT};

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
    /// Drift findings, sorted by `(code, subject)`.
    pub findings: Vec<LockFinding>,
}

impl LockVerification {
    /// Whether the lock matches the observed state.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
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
