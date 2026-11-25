//! `oxo-flow lock` — generate and verify a workflow-level lockfile
//! (issue #1399).
//!
//! `lock <workflow>` writes one canonical `oxo-flow.lock` beside the
//! workflow pinning its content/resolved hashes, canonical config snapshot,
//! include graph, per-rule environment identities, and declared software.
//! `lock --check` verifies an existing lock and never mutates it.
//!
//! Exit codes for `--check` (mirroring `provenance attest-verify`'s
//! convention): `0` clean, `2` drift / missing / invalid lock, `4` the
//! workflow itself could not be loaded (usage/parse error).

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::executor::checkpoint::compute_file_checksum;
use oxo_flow_core::workflow_lock::{
    LOCK_001, LOCK_007, LOCK_FORMAT, LockFinding, LockState, WorkflowLock, default_lock_path,
    load_lock, verify_lock,
};
use serde_json::json;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// `lock --check`: drift, a missing lock, or an invalid lock document.
const EXIT_DRIFT: i32 = 2;
/// `lock`: the workflow could not be loaded or expanded.
const EXIT_USAGE: i32 = 4;

/// Arguments for [`lock_command`], assembled by `main.rs`.
pub struct LockRequest {
    /// Workflow path; `None` auto-discovers in the current directory.
    pub workflow: Option<PathBuf>,
    /// Lock path override (`-o/--output`); defaults to `oxo-flow.lock`
    /// beside the workflow.
    pub output: Option<PathBuf>,
    /// Verify instead of writing. Never mutates the lock file.
    pub check: bool,
    /// Execution profile whose config keys should be pinned (same merge
    /// semantics as `run --profile`).
    pub profile: Option<String>,
    /// `KEY=VALUE` config overrides to pin (same semantics as `run --arg`).
    pub config_args: Vec<String>,
    /// Emit one machine-readable JSON document on stdout.
    pub json: bool,
}

/// Run `oxo-flow lock`.
pub fn lock_command(request: LockRequest) -> Result<()> {
    let LockRequest {
        workflow,
        output,
        check,
        profile,
        config_args,
        json,
    } = request;

    let workflow = crate::commands::resolve_workflow(workflow)?;
    let workflow_abs = crate::commands::run::absolutize(&workflow)?;
    let lock_path = match output {
        Some(path) => path,
        None => default_lock_path(&workflow_abs),
    };

    // A workflow that cannot be parsed/expanded cannot be locked or checked.
    // In `--check` mode this is a usage error (exit 4), not drift.
    let prepared = prepare_state(&workflow_abs, profile.as_deref(), config_args);
    let state = match prepared {
        Ok(state) => state,
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "command": "lock",
                        "mode": if check { "check" } else { "write" },
                        "workflow": workflow_abs.display().to_string(),
                        "lock": lock_path.display().to_string(),
                        "ok": false,
                        "error": error.to_string(),
                    }))?
                );
            }
            eprintln!(
                "{} {}",
                "Lock:".bold().red(),
                format!("cannot load workflow {}: {error}", workflow_abs.display()).red()
            );
            std::process::exit(EXIT_USAGE);
        }
    };

    if check {
        return check_lock(&workflow_abs, &lock_path, &state, json);
    }
    write_lock(&workflow_abs, &lock_path, &state, json)
}

/// Parse the workflow and build the state a lock pins. Mirrors the CLI run
/// path's config phases that affect the pinned identity (overrides, profile,
/// defaults, wildcard expansion) — the one difference is that `lock` has no
/// sample filter, so the lock covers the workflow as authored.
fn prepare_state(
    workflow: &Path,
    profile: Option<&str>,
    config_args: Vec<String>,
) -> Result<LockState> {
    let workflow_dir = oxo_flow_core::parent_dir(workflow);
    let mut config = oxo_flow_core::config::WorkflowConfig::from_file(workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    if !config_args.is_empty() {
        crate::commands::run::apply_cli_override_phase(&mut config, workflow, config_args, "lock")?;
    }
    if let Some(profile_name) = profile {
        crate::commands::run_preview::merge_profile(&mut config, profile_name, workflow_dir)?;
    }
    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;
    let content_hash = compute_file_checksum(workflow)
        .with_context(|| format!("failed to hash {}", workflow.display()))?;
    Ok(LockState::observe(&config, Some(workflow), &content_hash))
}

/// Verify an existing lock, print the report, and exit with the documented
/// code. Clean verification returns `Ok(())`.
fn check_lock(workflow: &Path, lock_path: &Path, state: &LockState, json: bool) -> Result<()> {
    let loaded = load_lock(lock_path);
    let (lock, findings) = match loaded {
        Ok(lock) => {
            let verification = verify_lock(&lock, state);
            (Some(lock), verification.findings)
        }
        Err(oxo_flow_core::workflow_lock::LockLoadError::Missing(_)) => (
            None,
            vec![LockFinding {
                code: LOCK_001,
                subject: "lock".to_string(),
                expected: Some(lock_path.display().to_string()),
                actual: None,
                message: "no lock file exists for this workflow — run `oxo-flow lock <workflow>` and commit the result".to_string(),
            }],
        ),
        Err(error) => (
            None,
            vec![LockFinding {
                code: LOCK_007,
                subject: "lock".to_string(),
                expected: Some(format!("a {LOCK_FORMAT} document")),
                actual: Some(error.to_string()),
                message: "the lock file is unreadable or malformed".to_string(),
            }],
        ),
    };

    let clean = findings.is_empty();
    if json {
        let mut document = json!({
            "command": "lock",
            "mode": "check",
            "workflow": workflow.display().to_string(),
            "lock": lock_path.display().to_string(),
            "ok": clean,
            "findings": findings,
        });
        if let Some(lock) = &lock {
            document["lock_digest"] = json!(lock.lock_digest);
        }
        println!("{}", serde_json::to_string_pretty(&document)?);
        std::io::stdout().flush().ok();
    }

    eprintln!("{} {}", "Lock check:".bold().cyan(), lock_path.display());
    if let Some(lock) = &lock {
        eprintln!("  lock digest: {}", lock.lock_digest.dimmed());
        eprintln!(
            "  workflow content: {}",
            lock.workflow.content_sha256.dimmed()
        );
    }
    eprintln!(
        "  rules: {} pinned, {} include(s)",
        state.rules.len().to_string().cyan(),
        state.includes.len().to_string().cyan()
    );
    for finding in &findings {
        eprintln!(
            "  {} [{}] {}: {}",
            "✗".red().bold(),
            finding.code,
            finding.subject,
            finding.message
        );
        if let Some(expected) = &finding.expected {
            eprintln!("      expected: {}", expected.dimmed());
        }
        if let Some(actual) = &finding.actual {
            eprintln!("      actual:   {}", actual.dimmed());
        }
    }
    if clean {
        eprintln!("{} lock matches the workflow", "OK:".bold().green());
        return Ok(());
    }
    eprintln!(
        "{} {} — regenerate with `oxo-flow lock {}`",
        "Lock check FAILED:".bold().red(),
        oxo_flow_core::plural(findings.len(), "finding", "findings"),
        workflow.display()
    );
    std::process::exit(EXIT_DRIFT);
}

/// Write the lock atomically (staged sibling + fsync + rename) and report
/// what it pins. Unresolved environments are recorded with their explicit
/// sentinel and surfaced honestly — `--check` will report `LOCK-006` for
/// them until they resolve, because unknown is never a match.
fn write_lock(workflow: &Path, lock_path: &Path, state: &LockState, json: bool) -> Result<()> {
    let lock = WorkflowLock::from_state(state);
    let tmp_path = oxo_flow_core::atomic_file::staged_tmp_path(lock_path);
    oxo_flow_core::atomic_file::publish_staged(&lock.canonical_json(), lock_path, &tmp_path)
        .with_context(|| format!("failed to write lock file {}", lock_path.display()))?;

    let unresolved: Vec<&String> = lock
        .rules
        .iter()
        .filter(|(_, rule)| {
            rule.environment
                .ends_with(oxo_flow_core::env_lock::ENV_LOCK_UNRESOLVED)
        })
        .map(|(name, _)| name)
        .collect();
    let unavailable = lock
        .rules
        .values()
        .filter(|rule| {
            rule.environment
                .ends_with(oxo_flow_core::env_lock::ENV_LOCK_UNAVAILABLE)
        })
        .count();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "command": "lock",
                "mode": "write",
                "workflow": workflow.display().to_string(),
                "lock": lock_path.display().to_string(),
                "ok": true,
                "lock_digest": lock.lock_digest,
                "rules": lock.rules.len(),
                "includes": lock.includes.len(),
                "unavailable_rules": unavailable,
                "unresolved_rules": unresolved,
            }))?
        );
        std::io::stdout().flush().ok();
    }

    eprintln!("{} {}", "Lock:".bold().cyan(), lock_path.display());
    eprintln!(
        "  workflow content: {}",
        lock.workflow.content_sha256.dimmed()
    );
    eprintln!(
        "  rules: {} pinned ({} unavailable, {} unresolved), {} include(s)",
        lock.rules.len().to_string().cyan(),
        unavailable.to_string().cyan(),
        unresolved.len().to_string().cyan(),
        lock.includes.len().to_string().cyan()
    );
    eprintln!("  lock digest: {}", lock.lock_digest.dimmed());
    if !unresolved.is_empty() {
        eprintln!(
            "  {} {} rule(s) have an unresolved environment digest ({}): the lock records the \
             explicit sentinel, and `lock --check` will report LOCK-006 until the digest resolves \
             offline — unknown is never treated as a match",
            "Warning:".bold().yellow(),
            unresolved.len(),
            unresolved
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    eprintln!(
        "{} Commit {} with the workflow; verify in CI with `oxo-flow lock --check {}`",
        "Written.".bold().green(),
        lock_path.display(),
        workflow.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_the_documented_ones() {
        assert_eq!(EXIT_DRIFT, 2);
        assert_eq!(EXIT_USAGE, 4);
    }
}
