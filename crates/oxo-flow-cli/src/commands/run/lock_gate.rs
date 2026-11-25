//! Workflow-lock enforcement for `run` (issue #1399).
//!
//! `--locked` (the explicit per-invocation form) or `[engine] locked = true`
//! (the persistent form that also covers web-spawned/queued runs) makes a
//! run verify `oxo-flow.lock` BEFORE any rule executes and before outputs
//! are created. Drift fails closed with the stable `LOCK-*` codes.
//!
//! Without either switch this module returns immediately and never reads
//! the lock file — legacy runs are byte-identical.

use super::*;
use oxo_flow_core::workflow_lock::{
    LockLoadError, LockState, default_lock_path, load_lock, verify_lock,
};

/// Verify the enforced workflow lock and return its digest.
///
/// `Ok(None)` means no lock is in effect (neither the flag nor the config
/// key is set) — nothing was read or verified. `Ok(Some(digest))` means the
/// lock verified and the digest must be recorded in the checkpoint and
/// folded into the resolved-workflow identity. Any error means enforcement
/// failed and the run must not proceed.
pub(crate) fn enforce_workflow_lock(
    workflow: &Path,
    config: &WorkflowConfig,
    locked_flag: bool,
) -> Result<Option<String>> {
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
        return Ok(Some(lock.lock_digest));
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
    anyhow::bail!(
        "workflow lock drift detected under --locked: {} against {}.\n  \
         The run is refused before any rule executes. Regenerate the lock with `oxo-flow lock {}` \
         only after reviewing the change.",
        oxo_flow_core::plural(verification.findings.len(), "finding", "findings"),
        lock_path.display(),
        workflow.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforcement_is_off_without_flag_or_config_key() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = dir.path().join("wf.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"a\"\nshell = \"true\"\n",
        )
        .unwrap();
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
        let workflow = dir.path().join("wf.oxoflow");
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n\n\
             [[rules]]\nname = \"a\"\nshell = \"true\"\n",
        )
        .unwrap();
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        // `[engine] locked = true` enforces even without the flag: no lock
        // file exists, so the gate fails closed with LOCK-001.
        let error = enforce_workflow_lock(&workflow, &config, false).unwrap_err();
        assert!(error.to_string().contains("LOCK-001"), "{error}");
        // The flag wins over an explicit `locked = false`.
        std::fs::write(
            &workflow,
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[engine]\nlocked = false\n\n\
             [[rules]]\nname = \"a\"\nshell = \"true\"\n",
        )
        .unwrap();
        let config = WorkflowConfig::from_file(&workflow).unwrap();
        assert!(
            enforce_workflow_lock(&workflow, &config, false)
                .unwrap()
                .is_none()
        );
        let error = enforce_workflow_lock(&workflow, &config, true).unwrap_err();
        assert!(error.to_string().contains("LOCK-001"), "{error}");
    }
}
