//! Background execution (`run --background` / `resume --background`,
//! issue #158 idea 1).
//!
//! The foreground invocation never executes the workflow: it spawns a
//! DETACHED child that re-runs the same binary with the same argv minus
//! `--background` (every other flag passes through verbatim), writes the
//! child's pid to `<workdir>/.oxo-flow/background.pid`, redirects the
//! child's stdout+stderr to the run log (the tracing tee already writes
//! there; the redirect captures anything non-tracing), prints a one-line
//! summary on stderr, and exits 0. The child then runs the normal flow —
//! checkpoint, workdir lock, and resume semantics are unchanged, so
//! monitoring works through `oxo-flow status`, the run log, and report
//! snapshots exactly as for a foreground run.

use anyhow::{Context as _, Result};
use oxo_flow_core::executor::{CheckpointState, WorkdirLock};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Build the detached child's argv from the foreground process's full
/// argument list (`std::env::args_os()`, `argv[0]` included): drop `argv[0]`
/// (the spawn replaces it with [`std::env::current_exe`]) and every exact
/// `--background` token. Exact-token removal is safe because this helper
/// only runs after clap parsed `background = true` — the flag was consumed
/// as a flag, never as another option's value — so the remainder re-parses
/// to the identical command line minus the flag.
pub fn strip_background_flag(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    args.into_iter()
        .skip(1) // argv[0]: replaced by current_exe() in spawn_detached
        .filter(|arg| arg.as_os_str() != OsStr::new("--background"))
        .collect()
}

/// Resolve the run-log path exactly like `run_command` (issue #136 fix 4):
/// an absolute `--log-file` passes through, a relative one resolves against
/// the workdir, and the default is `<workdir>/.oxo-flow/logs/oxo-flow.log`.
pub fn resolve_log_path(workdir: &Path, log_file: Option<&Path>) -> PathBuf {
    match log_file {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => workdir.join(p),
        None => workdir.join(".oxo-flow/logs/oxo-flow.log"),
    }
}

/// Effective workdir for a background `run`, mirroring `run_command`'s
/// resolution (issue #68): explicit `--workdir` wins; repository runs
/// (nextflow-style URLs) execute from the current directory; otherwise the
/// workflow's own directory. `--bundle` without `--workdir` is refused: the
/// bundle's extracted directory is created per-process, so the foreground
/// invocation cannot predict where the child will run.
pub fn background_workdir_for_run(
    workflow: Option<&Path>,
    workdir: Option<&Path>,
    bundle: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(wd) = workdir {
        return Ok(wd.to_path_buf());
    }
    if bundle.is_some() {
        anyhow::bail!(
            "run --background with --bundle requires an explicit --workdir: the bundle's \
             extracted directory is created per-process and cannot be predicted from the \
             foreground invocation.\n\
             Use: oxo-flow run --bundle <bundle> --workdir <dir> --yes --background"
        );
    }
    let wf = match workflow {
        Some(w) => w.to_path_buf(),
        None => crate::commands::resolve_workflow(None)?,
    };
    if crate::commands::pull::classify_run_source(&wf.to_string_lossy()).is_some() {
        Ok(std::env::current_dir()?)
    } else {
        Ok(oxo_flow_core::parent_dir(&wf).to_path_buf())
    }
}

/// Effective workdir for a background `resume`, mirroring `resume_command`:
/// explicit `--workdir` > the workdir the checkpoint records > the recorded
/// workflow's directory.
pub fn background_workdir_for_resume(checkpoint: &Path, workdir: Option<&Path>) -> Result<PathBuf> {
    if let Some(wd) = workdir {
        return Ok(wd.to_path_buf());
    }
    let state = CheckpointState::load_from_file(checkpoint)?;
    if let Some(wd) = state.workdir {
        return Ok(PathBuf::from(wd));
    }
    if let Some(wf) = state.workflow_path {
        return Ok(oxo_flow_core::parent_dir(Path::new(&wf)).to_path_buf());
    }
    anyhow::bail!(
        "cannot resolve a working directory for the background resume: the checkpoint \
         records neither a workdir nor a workflow path"
    )
}

/// Spawn the detached child: same binary, `args`, stdout+stderr redirected
/// to the run log. Unix: the child gets its own process group, so it
/// survives the terminal's SIGHUP (and Ctrl-C's SIGINT targets the
/// foreground group, not this child). Windows: CREATE_NEW_PROCESS_GROUP |
/// DETACHED_PROCESS. The log is opened in append mode — the child's
/// `activate_run_log` truncates it and the tracing tee writes the header;
/// the inherited descriptors capture anything the tee does not.
fn spawn_detached(args: &[OsString], log_path: &Path) -> io::Result<Child> {
    // The child's `activate_run_log` creates parent directories for the log;
    // the parent must do the same before it can open the redirect target.
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(args)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    command.spawn()
}

/// Write the child's pid to `<workdir>/.oxo-flow/background.pid`. The
/// `.oxo-flow` directory is created here so the pid file's placement never
/// depends on the child acquiring the workdir lock first.
fn write_pid_file(workdir: &Path, pid: u32) -> io::Result<PathBuf> {
    let dir = workdir.join(".oxo-flow");
    fs::create_dir_all(&dir)?;
    let path = dir.join("background.pid");
    fs::write(&path, format!("{pid}\n"))?;
    Ok(path)
}

/// Spawn the detached child, record its pid, print the one-line summary on
/// stderr, and return. The caller exits 0; the child runs the normal flow.
pub fn launch_in_background(args: &[OsString], workdir: &Path, log_path: &Path) -> Result<()> {
    // Fail fast instead of reporting "started" for a child that would
    // immediately die on the workdir lock (issue #70). Advisory: the child
    // still enforces the lock itself.
    if WorkdirLock::is_locked(workdir) {
        anyhow::bail!(
            "another run is already active in {} — the background run would fail on the \
             workdir lock ({}); wait for it to finish or stop it first",
            workdir.display(),
            workdir.join(".oxo-flow/lock").display()
        );
    }
    let child = spawn_detached(args, log_path).with_context(|| {
        format!(
            "failed to spawn the background process (log: {})",
            log_path.display()
        )
    })?;
    let pid = child.id();
    let pid_file = write_pid_file(workdir, pid)?;
    let checkpoint = workdir.join(".oxo-flow/checkpoint.json");
    eprintln!(
        "started in background (pid {pid}) · log: {} · monitor: oxo-flow status {} · stop: kill {pid}",
        log_path.display(),
        checkpoint.display()
    );
    tracing::info!(
        pid,
        pid_file = %pid_file.display(),
        log = %log_path.display(),
        "run detached into background"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn strip_background_flag_drops_argv0_and_flag_keeps_rest() {
        // Full args_os() shape: argv[0] first, the flag anywhere after.
        let args = os(&[
            "/usr/bin/oxo-flow",
            "run",
            "--background",
            "wf.oxoflow",
            "-j",
            "4",
        ]);
        assert_eq!(
            strip_background_flag(args),
            os(&["run", "wf.oxoflow", "-j", "4"])
        );
    }

    #[test]
    fn strip_background_flag_handles_leading_global_flags() {
        let args = os(&[
            "/usr/bin/oxo-flow",
            "--json",
            "run",
            "wf.oxoflow",
            "--background",
        ]);
        assert_eq!(
            strip_background_flag(args),
            os(&["--json", "run", "wf.oxoflow"])
        );
    }

    #[test]
    fn strip_background_flag_keeps_values_that_merely_contain_the_token() {
        // A config override value that contains "--background" is NOT the
        // flag and must survive (the flag itself was already consumed by
        // clap as a flag for the helper to be called at all).
        let args = os(&[
            "/usr/bin/oxo-flow",
            "run",
            "wf.oxoflow",
            "TAG=--background",
            "--background",
            "NAME=x",
        ]);
        assert_eq!(
            strip_background_flag(args),
            os(&["run", "wf.oxoflow", "TAG=--background", "NAME=x"])
        );
    }

    #[test]
    fn resolve_log_path_defaults_under_workdir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_log_path(dir.path(), None),
            dir.path().join(".oxo-flow/logs/oxo-flow.log")
        );
    }

    #[test]
    fn resolve_log_path_joins_relative_log_file_against_workdir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_log_path(dir.path(), Some(Path::new("logs/custom.log"))),
            dir.path().join("logs/custom.log")
        );
    }

    #[test]
    fn resolve_log_path_passes_absolute_log_file_through() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_log_path(dir.path(), Some(Path::new("/tmp/abs.log"))),
            PathBuf::from("/tmp/abs.log")
        );
    }

    #[test]
    fn background_workdir_for_run_prefers_explicit_workdir() {
        let wf = Path::new("/data/wf.oxoflow");
        let wd = Path::new("/analysis");
        assert_eq!(
            background_workdir_for_run(Some(wf), Some(wd), None).unwrap(),
            PathBuf::from("/analysis")
        );
    }

    #[test]
    fn background_workdir_for_run_defaults_to_workflow_parent() {
        let wf = Path::new("/data/nested/wf.oxoflow");
        assert_eq!(
            background_workdir_for_run(Some(wf), None, None).unwrap(),
            PathBuf::from("/data/nested")
        );
    }

    #[test]
    fn background_workdir_for_run_uses_cwd_for_repository_runs() {
        let wf = Path::new("gh:owner/pipeline@v1.2.3");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            background_workdir_for_run(Some(wf), None, None).unwrap(),
            cwd
        );
    }

    #[test]
    fn background_workdir_for_run_refuses_bundle_without_workdir() {
        let err = background_workdir_for_run(None, None, Some(Path::new("b.tgz")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--workdir"),
            "bundle+background without --workdir must name the fix: {err}"
        );
    }

    #[test]
    fn background_workdir_for_run_accepts_bundle_with_workdir() {
        let wd = Path::new("/analysis");
        assert_eq!(
            background_workdir_for_run(None, Some(wd), Some(Path::new("b.tgz"))).unwrap(),
            PathBuf::from("/analysis")
        );
    }

    /// `completed_rules`/`failed_rules`/`benchmarks` are required checkpoint
    /// fields (no serde defaults), so every fixture carries them.
    fn checkpoint_with(dir: &Path, extra: &str) -> PathBuf {
        let path = dir.join("checkpoint.json");
        fs::write(
            &path,
            format!(
                r#"{{"completed_rules": [], "failed_rules": [], "benchmarks": {{}}, {extra}}}"#
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn background_workdir_for_resume_prefers_explicit_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint_with(dir.path(), r#""workdir": "/wrong""#);
        assert_eq!(
            background_workdir_for_resume(&ck, Some(Path::new("/right"))).unwrap(),
            PathBuf::from("/right")
        );
    }

    #[test]
    fn background_workdir_for_resume_uses_checkpoint_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint_with(dir.path(), r#""workdir": "/recorded/wd""#);
        assert_eq!(
            background_workdir_for_resume(&ck, None).unwrap(),
            PathBuf::from("/recorded/wd")
        );
    }

    #[test]
    fn background_workdir_for_resume_falls_back_to_workflow_parent() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint_with(dir.path(), r#""workflow_path": "/repo/nested/wf.oxoflow""#);
        assert_eq!(
            background_workdir_for_resume(&ck, None).unwrap(),
            PathBuf::from("/repo/nested")
        );
    }

    #[test]
    fn background_workdir_for_resume_errors_without_recorded_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ck = checkpoint_with(dir.path(), r#""workflow_name": "x""#);
        assert!(
            background_workdir_for_resume(&ck, None).is_err(),
            "a checkpoint with no workdir and no workflow path cannot resolve a workdir"
        );
    }
}
