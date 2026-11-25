//! Workflow-lock enforcement for `run` (issue #1399).
//!
//! `--locked` (the explicit per-invocation form) or `[engine] locked = true`
//! (the persistent form) makes a run verify `oxo-flow.lock` BEFORE any rule
//! executes and before outputs are created. Drift fails closed with the
//! stable `LOCK-*` codes. Web-spawned/queued and remote-cluster runs inherit
//! the config key; they must stage `oxo-flow.lock` beside the workflow (see
//! `docs/guide/src/reference/web-api.md` and `commands/lock.md`).
//!
//! Without either switch this module returns immediately and never reads
//! the lock file — legacy runs are byte-identical.

use std::collections::BTreeMap;

use super::*;
use oxo_flow_core::workflow_lock::{
    LockLoadError, LockState, default_lock_path, load_lock, verify_lock,
};

/// A lock that verified for the current workflow bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedLock {
    /// The lock document's `lock_digest` — recorded in the checkpoint and
    /// folded into the resolved-workflow identity.
    pub digest: String,
    /// Per-rule environment identity observed while verifying. Threaded to
    /// the checkpoint so a locked run does not recompute it (and does not
    /// re-run the bounded offline docker probe a second time — issue #1399
    /// review M8).
    pub env_identity: BTreeMap<String, String>,
}

/// Verify the enforced workflow lock.
///
/// `Ok(None)` means no lock is in effect (neither the flag nor the config
/// key is set) — nothing was read or verified. `Ok(Some(_))` means the lock
/// verified and carries the digest plus observed environment identities.
/// Any error means enforcement failed and the run must not proceed.
pub(crate) fn enforce_workflow_lock(
    workflow: &Path,
    config: &WorkflowConfig,
    locked_flag: bool,
) -> Result<Option<VerifiedLock>> {
    let config_enforced = config.engine.as_ref().and_then(|engine| engine.locked) == Some(true);
    if !locked_flag && !config_enforced {
        return Ok(None);
    }

    let workflow_abs = absolutize(workflow)?;
    let lock_path = default_lock_path(&workflow_abs);
    let lock = match load_lock(&lock_path) {
        Ok(lock) => lock,
        Err(LockLoadError::Missing(_)) => {
            anyhow::bail!(
                "workflow lock is enforced (--locked or [engine] locked = true) but no lock \
                 file exists at {}.\n  LOCK-001: the run is refused before any rule executes.\n  \
                 hint: run `oxo-flow lock {}` and commit the result.",
                lock_path.display(),
                workflow.display()
            );
        }
        Err(error) => {
            anyhow::bail!(
                "workflow lock is enforced but the lock file cannot be used.\n  LOCK-007: {}\n  \
                 hint: regenerate it with `oxo-flow lock {}`.",
                error,
                workflow.display()
            );
        }
    };

    let content_hash = oxo_flow_core::executor::checkpoint::compute_file_checksum(&workflow_abs)
        .with_context(|| {
            format!(
                "workflow lock is enforced but {} could not be hashed",
                workflow_abs.display()
            )
        })?;
    let state = LockState::observe(config, Some(&workflow_abs), &content_hash);
    let verification = verify_lock(&lock, &state);
    if verification.ok() {
        diagnostic_narrate(
            format_args!(
                "  {} workflow lock verified: {}",
                "Lock:".bold().green(),
                lock.lock_digest.dimmed()
            ),
            None,
        );
        let env_identity = state
            .rules
            .iter()
            .map(|(rule, pinned)| (rule.clone(), pinned.environment.clone()))
            .collect();
        return Ok(Some(VerifiedLock {
            digest: lock.lock_digest,
            env_identity,
        }));
    }

    for finding in &verification.findings {
        eprintln!(
            "  {} [{}] {}: {}",
            "✗".red().bold(),
            finding.code,
            finding.subject,
            finding.message
        );
        if let Some(expected) = &finding.expected {
            eprintln!("      locked:  {}", expected.dimmed());
        }
        if let Some(actual) = &finding.actual {
            eprintln!("      current: {}", actual.dimmed());
        }
    }
    let mut codes: Vec<&str> = verification
        .findings
        .iter()
        .map(|finding| finding.code)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    anyhow::bail!(
        "workflow lock drift detected under --locked [{}]: {} against {}.\n  \
         The run is refused before any rule executes. Regenerate the lock with `oxo-flow lock {}` \
         only after reviewing the change.",
        codes.join(", "),
        oxo_flow_core::plural(verification.findings.len(), "finding", "findings"),
        lock_path.display(),
        workflow.display()
    )
}

/// Re-verify the lock after a post-gate rewrite of the workflow file
/// (issue #1399 review I1).
///
/// The in-process type-admission repair and the verified-repair loop both
/// write a patched workflow AFTER the initial gate. An enforced lock pins
/// the pre-patch bytes, so continuing without a second verification would
/// execute rules from a workflow the lock does not cover and record a
/// `lock_digest` against a content hash it never described. This function is
/// the single re-entry point both paths use; with enforcement off it is a
/// no-op, and with enforcement on the patched file fails closed (`LOCK-002`).
pub(crate) fn revalidate_lock_after_mutation(
    workflow: &Path,
    config: &WorkflowConfig,
    locked_flag: bool,
) -> Result<Option<VerifiedLock>> {
    enforce_workflow_lock(workflow, config, locked_flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_workflow(dir: &Path, text: &str) -> PathBuf {
        let workflow = dir.join("wf.oxoflow");
        std::fs::write(&workflow, text).unwrap();
        workflow
    }

    fn generate_lock(workflow: &Path) {
        let mut config = WorkflowConfig::from_file(workflow).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let content_hash =
            oxo_flow_core::executor::checkpoint::compute_file_checksum(workflow).unwrap();
        let state = LockState::observe(&config, Some(workflow), &content_hash);
        let lock = oxo_flow_core::workflow_lock::WorkflowLock::from_state(&state);
        std::fs::write(
            oxo_flow_core::workflow_lock::default_lock_path(workflow),
            lock.canonical_json(),
        )
        .unwrap();
    }

    const WORKFLOW: &str = "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"a\"\nshell = \"true\"\n";

    #[test]
    fn enforcement_is_off_without_flag_or_config_key() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), WORKFLOW);
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        let result = enforce_workflow_lock(&workflow, &config, false).unwrap();
        assert!(
            result.is_none(),
            "without --locked or [engine] locked no lock is read"
        );
    }

    #[test]
    fn config_key_enforces_and_flag_wins() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(
            dir.path(),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n\n\
             [[rules]]\nname = \"a\"\nshell = \"true\"\n",
        );
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        // `[engine] locked = true` enforces even without the flag: no lock
        // file exists, so the gate fails closed with LOCK-001.
        let error = enforce_workflow_lock(&workflow, &config, false).unwrap_err();
        assert!(error.to_string().contains("LOCK-001"), "{error}");
        // The flag wins over an explicit `locked = false`.
        write_workflow(
            dir.path(),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[engine]\nlocked = false\n\n\
             [[rules]]\nname = \"a\"\nshell = \"true\"\n",
        );
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        assert!(
            enforce_workflow_lock(&workflow, &config, false)
                .unwrap()
                .is_none()
        );
        let error = enforce_workflow_lock(&workflow, &config, true).unwrap_err();
        assert!(error.to_string().contains("LOCK-001"), "{error}");
    }

    #[test]
    fn verified_lock_carries_the_environment_identity() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), WORKFLOW);
        generate_lock(&workflow);
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        let verified = enforce_workflow_lock(&workflow, &config, true)
            .unwrap()
            .expect("a fresh lock verifies");
        assert_eq!(
            verified.env_identity.get("a").map(String::as_str),
            Some("system|unavailable")
        );
        assert!(verified.digest.starts_with("sha256:"));
    }

    // Review I1: the shared re-validation seam refuses a workflow that was
    // rewritten after the gate, and is a no-op with enforcement off.
    #[test]
    fn mutation_after_the_gate_fails_closed_under_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = write_workflow(dir.path(), WORKFLOW);
        generate_lock(&workflow);
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        assert!(revalidate_lock_after_mutation(&workflow, &config, true).is_ok());

        // The repair paths rewrite the workflow, then re-enter here.
        std::fs::write(&workflow, WORKFLOW.replace("true", "false")).unwrap();
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        let error = revalidate_lock_after_mutation(&workflow, &config, true).unwrap_err();
        assert!(error.to_string().contains("LOCK-002"), "{error}");
        assert!(
            revalidate_lock_after_mutation(&workflow, &config, false)
                .unwrap()
                .is_none(),
            "without enforcement the re-validation is a no-op"
        );
    }
}
