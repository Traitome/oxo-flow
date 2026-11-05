//! Run-log startup phase: per-run archived log with rotation, masked
//! header, and the background-redirect tee skip (#194 A3, #115 pillar 1).

use super::*;

/// Everything the rest of the run needs from the startup phase: the shared
/// log handle, the absolutized workflow path, and the workflow repo's HEAD
/// SHA (provenance, issue #115 pillar 1).
pub(crate) struct RunLogStart {
    pub run_log: SharedRunLog,
    pub workflow_abs: PathBuf,
    pub workflow_git_sha: Option<String>,
}

/// Relative `--log-file` paths resolve against the workdir, matching the
/// default log location (issue #136 fix 4).
pub(crate) fn resolve_run_log_path(workdir: &Path, log_file: Option<&Path>) -> PathBuf {
    match log_file {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => workdir.join(p),
        None => workdir.join(".oxo-flow/logs/oxo-flow.log"),
    }
}

/// The archived log's header names the exact workflow version (name,
/// version, git HEAD) that produced the record. `command_line` must
/// already be masked by the caller — this formats it verbatim.
pub(crate) fn run_log_header(
    command_line: &str,
    workflow_abs: &Path,
    config: &WorkflowConfig,
    workflow_git_sha: Option<&str>,
    workdir: &Path,
) -> String {
    format!(
        "oxo-flow run log\nstarted_at: {}\noxo-flow: v{}\ncommand: {}\nworkflow: {}\nworkflow_name: {}\nworkflow_version: {}\ngit_sha: {}\nworkdir: {}\n\n",
        chrono::Local::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        command_line,
        workflow_abs.display(),
        config.workflow.name,
        config.workflow.version,
        workflow_git_sha.unwrap_or("(not inside a git repository)"),
        workdir.display(),
    )
}

/// The run-log startup phase: mask the raw command line (sensitive values
/// are computed by the caller BEFORE this runs — `--arg KEY=secret` must
/// never reach the log or its rotated backups, issue #99 B1 / #136 fix 3),
/// build the header, and arm the file log. Best-effort: a logging failure
/// never fails the run.
pub(crate) fn start_run_log(
    workflow: &Path,
    config: &WorkflowConfig,
    workdir: &Path,
    log_file: Option<&Path>,
    sensitive_values: &[String],
) -> anyhow::Result<RunLogStart> {
    let workflow_abs = absolutize(workflow)?;
    let workflow_git_sha =
        oxo_flow_core::executor::checkpoint::CheckpointState::workflow_git_sha(&workflow_abs);
    let run_log_path = resolve_run_log_path(workdir, log_file);
    let command_line = oxo_flow_core::executor::process::mask_sensitive(
        &std::env::args().collect::<Vec<_>>().join(" "),
        sensitive_values,
    );
    let run_log_header = run_log_header(
        &command_line,
        &workflow_abs,
        config,
        workflow_git_sha.as_deref(),
        workdir,
    );
    // Background runs (issue #194 A3): `spawn_detached` already redirected
    // this process's stdout/stderr onto the run log, so arming the tracing
    // tee here would write every event TWICE into the same file. In that
    // mode the header is printed to stderr (→ the redirect) and the tee is
    // skipped entirely; foreground runs arm the tee as before.
    let run_log: SharedRunLog = Arc::new(std::sync::Mutex::new(
        if std::env::var_os("OXO_FLOW_STDERR_ALREADY_REDIRECTED").is_some() {
            eprintln!("{run_log_header}");
            None
        } else {
            match crate::logging::activate_run_log(&run_log_path, &run_log_header) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    tracing::warn!(error = %e, path = %run_log_path.display(), "failed to open run log; continuing without file logging");
                    None
                }
            }
        },
    ));
    tracing::info!(
        workflow = %config.workflow.name,
        version = %config.workflow.version,
        git_sha = workflow_git_sha.as_deref().unwrap_or("(none)"),
        workdir = %workdir.display(),
        "workflow run started (log: {})",
        run_log_path.display()
    );
    Ok(RunLogStart {
        run_log,
        workflow_abs,
        workflow_git_sha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_resolves_relative_against_the_workdir() {
        // issue #136 fix 4: a relative --log-file must not silently land in
        // the invocation cwd when the run uses a separate --workdir.
        let workdir = std::path::Path::new("/data/run");
        assert_eq!(
            resolve_run_log_path(workdir, None),
            workdir.join(".oxo-flow/logs/oxo-flow.log")
        );
        assert_eq!(
            resolve_run_log_path(workdir, Some(std::path::Path::new("logs/custom.log"))),
            workdir.join("logs/custom.log")
        );
        assert_eq!(
            resolve_run_log_path(workdir, Some(std::path::Path::new("/abs/custom.log"))),
            std::path::PathBuf::from("/abs/custom.log")
        );
    }

    #[test]
    fn header_names_workflow_identity_and_keeps_the_masked_line() {
        let config = WorkflowConfig::parse(
            r#"
[workflow]
name = "wes"
version = "2.0"
"#,
        )
        .unwrap();
        let header = run_log_header(
            "oxo-flow run wf.oxoflow --arg token=****",
            std::path::Path::new("/repo/wf.oxoflow"),
            &config,
            Some("abc123"),
            std::path::Path::new("/data/run"),
        );
        assert!(
            header.starts_with("oxo-flow run log\nstarted_at: "),
            "{header}"
        );
        assert!(
            header.contains("command: oxo-flow run wf.oxoflow --arg token=****"),
            "{header}"
        );
        assert!(header.contains("workflow: /repo/wf.oxoflow"), "{header}");
        assert!(header.contains("workflow_name: wes"), "{header}");
        assert!(header.contains("workflow_version: 2.0"), "{header}");
        assert!(header.contains("git_sha: abc123"), "{header}");
        assert!(header.contains("workdir: /data/run"), "{header}");
        // No git repo — the placeholder, not an empty field.
        let no_sha = run_log_header(
            "cmd",
            std::path::Path::new("/repo/wf.oxoflow"),
            &config,
            None,
            std::path::Path::new("/data/run"),
        );
        assert!(
            no_sha.contains("git_sha: (not inside a git repository)"),
            "{no_sha}"
        );
    }
}
