use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Base workspace directory for the Web UI (uploaded inputs, run sandboxes).
///
/// `OXO_FLOW_WORKSPACE` overrides it — deployments that keep data on a
/// separate volume, and integration tests that must not write into the source
/// tree, both need a relocatable root.
fn base_workspace() -> PathBuf {
    std::env::var("OXO_FLOW_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("workspace"))
}

/// Validate that a path component does not contain traversal sequences.
fn validate_path_component(name: &str, field: &str) -> Result<()> {
    if name.is_empty()
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('~')
    {
        anyhow::bail!("Invalid {field}: '{name}' contains path traversal or invalid characters");
    }
    Ok(())
}

/// Validate a user-chosen username: it becomes a workspace path component,
/// so the charset is restricted to what is safe on every filesystem and in
/// logs, sessions, and URL segments.
pub fn validate_username(username: &str) -> Result<()> {
    validate_path_component(username, "username")?;
    let valid = !username.is_empty()
        && username.len() <= 64
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        anyhow::bail!(
            "Invalid username: only letters, digits, '.', '_' and '-' are allowed (max 64 chars)"
        );
    }
    Ok(())
}

/// Setup the directory structure for a specific run.
///
/// Ensures `workspace/users/<username>/runs/<run_id>` exists.
/// The acting user's workspace root — the sandbox every client-supplied
/// data/analysis path resolves against (#521). Nothing outside this tree is
/// readable through the data endpoints.
pub fn user_root(username: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    Ok(base_workspace().join("users").join(username))
}

/// Reject absolute paths, `..` components, and degenerate forms (empty,
/// `.`-only, trailing slash) in a client-supplied relative path (#521) —
/// absolute/`..` turn path-taking endpoints into arbitrary-path
/// existence/size oracles; degenerate forms resolve to directories or
/// nothing and only produce confusing downstream failures.
pub fn check_relative_path(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err("path must not be empty".to_string());
    }
    let candidate = std::path::Path::new(raw);
    if candidate.is_absolute() {
        return Err(format!(
            "absolute path '{raw}' not allowed — data paths resolve inside your workspace"
        ));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "'..' components are not allowed in data paths ('{raw}')"
        ));
    }
    if raw.ends_with('/') {
        return Err(format!("path '{raw}' must name a file, not end with '/'"));
    }
    if raw == "." || raw.starts_with("./") && raw[2..].trim_matches('/').is_empty() {
        return Err(format!("path '{raw}' does not name a file"));
    }
    // NUL is never a legal filename byte: `Path` components carry it
    // through untouched, so without this gate the rejection would only
    // surface as a confusing InvalidInput at the syscall (audit finding —
    // the old sandbox sanitizer stripped it, the canonicalizer did not).
    if raw.contains('\0') {
        return Err(format!("path '{raw}' contains a NUL byte"));
    }
    Ok(())
}

/// Canonicalize a validated client path into the workflow-relative key used
/// for admission checks and staging.
///
/// `.` components are removed and repeated separators collapse through
/// `Path::components`; `..`, absolute paths, and degenerate forms remain
/// rejected by [`check_relative_path`]. Canonicalization prevents aliases
/// such as `./workflow.oxoflow` from bypassing reserved-key checks or
/// naming a different scanner/staging target.
pub fn canonicalize_relative_path(raw: &str) -> Result<String, String> {
    check_relative_path(raw)?;
    let mut parts = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path '{raw}' cannot be canonicalized safely"));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("path '{raw}' does not name a file"));
    }
    Ok(parts.join("/"))
}

/// Resolve a client-supplied path against the user's workspace root.
pub fn resolve_user_scoped_path(username: &str, raw: &str) -> Result<PathBuf, String> {
    let root = user_root(username).map_err(|e| e.to_string())?;
    check_relative_path(raw)?;
    Ok(root.join(std::path::Path::new(raw)))
}

pub fn setup_run_directory(username: &str, run_id: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    validate_path_component(run_id, "run_id")?;

    let run_dir = base_workspace()
        .join("users")
        .join(username)
        .join("runs")
        .join(run_id);

    fs::create_dir_all(&run_dir)
        .with_context(|| format!("Failed to create run directory: {:?}", run_dir))?;

    Ok(run_dir)
}

/// Create a sandbox for the user by copying the workflow TOML into the run directory.
pub fn initialize_sandbox(username: &str, run_id: &str, toml_content: &str) -> Result<PathBuf> {
    let run_dir = setup_run_directory(username, run_id)?;
    let workflow_file = run_dir.join("workflow.oxoflow");

    fs::write(&workflow_file, toml_content)
        .with_context(|| format!("Failed to write workflow file to {:?}", workflow_file))?;

    Ok(run_dir)
}

/// Retrieve the run directory path.
///
/// The components are user-controlled (session username, client-provided
/// ids), so they are validated before joining — a traversal username must
/// fail loudly instead of escaping the workspace/users tree.
pub fn get_run_directory(username: &str, run_id: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    validate_path_component(run_id, "run_id")?;
    Ok(base_workspace()
        .join("users")
        .join(username)
        .join("runs")
        .join(run_id))
}

/// Directory holding a user's uploaded inputs (issue #82 P0-2): the
/// `workspace/users/{user}/inputs/` root served by `POST /api/files`.
pub fn inputs_directory(username: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    Ok(base_workspace().join("users").join(username).join("inputs"))
}

/// Mirror the acting user's uploaded inputs into a run's working directory
/// (issue #276 L1): uploads land in `workspace/users/<u>/inputs/` but the
/// CLI executes in the run/pipeline dir, so data-referencing workflows
/// (`metadata_file`, input globs) failed pre-execution for web-only users.
///
/// The WHOLE inputs tree is mirrored, because which files a workflow needs
/// is a parse-time property (`metadata_file`, input globs) the HTTP layer
/// should not re-derive. A destination file is skipped only while it is at
/// least as new as the staged source — so a re-uploaded input (source
/// newer than the stale copy) is refreshed instead of being shadowed by it
/// forever (issue #297 item 1), while a newer LOCAL file (a pipeline output
/// sharing the staged path, written after staging) still survives
/// re-staging. Returns the number of files copied.
pub fn stage_user_inputs(username: &str, run_dir: &Path) -> Result<usize> {
    let root = inputs_directory(username)?;

    match fs::symlink_metadata(&root) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("uploaded-inputs root {:?} is a symlink", root);
        }
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to inspect inputs root {:?}", root));
        }
    }

    let root = fs::canonicalize(&root)
        .with_context(|| format!("failed to canonicalize inputs root {:?}", root))?;
    reject_symlink_components(&root)?;

    let run_root = fs::canonicalize(run_dir)
        .with_context(|| format!("failed to canonicalize run directory {:?}", run_dir))?;
    if !run_root.is_dir() {
        anyhow::bail!("run staging destination {:?} is not a directory", run_root);
    }
    reject_symlink_components(&run_root)?;

    let mut copied = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        reject_symlink_components(&dir)?;
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("failed to read inputs directory {:?}", dir))?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("failed to read an entry in inputs directory {:?}", dir)
            })?;
            let path = entry.path();
            reject_symlink_components(&path)?;

            let meta = fs::symlink_metadata(&path)
                .with_context(|| format!("failed to inspect uploaded input {:?}", path))?;
            if meta.file_type().is_symlink() {
                anyhow::bail!("uploaded input {:?} is a symlink", path);
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o444 == 0 {
                    anyhow::bail!("uploaded input {:?} is unreadable (mode 000)", path);
                }
            }

            // A staged-upload temp a killed process left behind is engine
            // scratch, not an input; mirroring it would hand every run a
            // phantom file (issue #1161).
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(crate::domains::execution::files::is_staged_upload_tmp)
            {
                continue;
            }

            let rel = path.strip_prefix(&root).with_context(|| {
                format!(
                    "uploaded input {:?} escaped canonical inputs root {:?}",
                    path, root
                )
            })?;
            let dest = run_root.join(rel);
            if dest_is_current(&dest, &meta)? {
                continue;
            }

            if let Some(parent) = dest.parent() {
                reject_symlink_components(parent)?;
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {:?}", parent))?;
                reject_symlink_components(parent)?;
                let canonical_parent = fs::canonicalize(parent)
                    .with_context(|| format!("failed to canonicalize destination {:?}", parent))?;
                if !canonical_parent.starts_with(&run_root) {
                    anyhow::bail!(
                        "destination {:?} escaped canonical run root {:?}",
                        canonical_parent,
                        run_root
                    );
                }
            }

            reject_symlink_components(&dest)?;
            if fs::symlink_metadata(&dest)
                .ok()
                .is_some_and(|m| m.file_type().is_symlink())
            {
                anyhow::bail!("destination {:?} is a symlink", dest);
            }

            // Open the source without following symlinks, then re-check the
            // path and inode immediately around the copy. This closes the
            // common symlink swap while keeping ordinary regular files intact.
            let mut source = open_regular_nofollow(&path)?;
            recheck_regular_source(&path, &root, &mut source)?;

            let Some(parent) = dest.parent() else {
                anyhow::bail!("destination {:?} has no parent", dest);
            };
            let mut temp = tempfile::NamedTempFile::new_in(parent)
                .with_context(|| format!("failed to create staged temp in {:?}", parent))?;
            io::copy(&mut source, temp.as_file_mut())
                .with_context(|| format!("failed to read uploaded input {:?}", path))?;

            // Re-check source and destination immediately before the atomic
            // rename. Persist replaces a symlink at the destination name itself
            // rather than following it.
            recheck_regular_source(&path, &root, &mut source)?;
            reject_symlink_components(&dest)?;
            temp.persist(&dest).map_err(|e| {
                anyhow::Error::new(e.error).context(format!(
                    "failed to atomically stage {:?} → {:?}",
                    path, dest
                ))
            })?;

            reject_symlink_components(&dest)?;
            let dest_meta = fs::symlink_metadata(&dest)
                .with_context(|| format!("failed to inspect staged destination {:?}", dest))?;
            if dest_meta.file_type().is_symlink() || !dest_meta.is_file() {
                anyhow::bail!("staged destination {:?} is not a regular file", dest);
            }
            copied += 1;
        }
    }
    Ok(copied)
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(component.as_os_str()),
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                anyhow::bail!("path {:?} contains a parent traversal", path);
            }
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        anyhow::bail!("path {:?} contains symlink {:?}", path, current);
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e).with_context(|| {
                            format!("failed to inspect path component {:?}", current)
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn open_regular_nofollow(path: &Path) -> Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.read(true).custom_flags(nix::libc::O_NOFOLLOW);
        options
            .open(path)
            .with_context(|| format!("failed to open uploaded input {:?}", path))
    }
    #[cfg(not(unix))]
    {
        fs::File::open(path).with_context(|| format!("failed to open uploaded input {:?}", path))
    }
}

fn recheck_regular_source(path: &Path, root: &Path, source: &mut fs::File) -> Result<()> {
    reject_symlink_components(path)?;
    let lmeta = fs::symlink_metadata(path)
        .with_context(|| format!("uploaded input disappeared before copy: {:?}", path))?;
    if lmeta.file_type().is_symlink() || !lmeta.is_file() {
        anyhow::bail!("uploaded input {:?} changed into a non-regular file", path);
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to canonicalize uploaded input {:?}", path))?;
    if !canonical.starts_with(root) {
        anyhow::bail!(
            "uploaded input {:?} escaped canonical inputs root {:?}",
            canonical,
            root
        );
    }

    let opened = source
        .metadata()
        .with_context(|| format!("failed to inspect opened uploaded input {:?}", path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != lmeta.dev() || opened.ino() != lmeta.ino() {
            anyhow::bail!("uploaded input {:?} changed during staging", path);
        }
    }
    Ok(())
}

/// True when `dest` already carries content at least as new as the staged
/// source (whose metadata is `src_meta`): destination mtime same-or-newer
/// than the source's. Compares instead of merely checking existence so a
/// re-uploaded input (source newer than the stale copy) replaces it (issue
/// #297 item 1) while newer local outputs are left alone. Size is
/// deliberately NOT part of the check — freshness is an mtime property,
/// and a same-size stale copy must still be refreshed.
fn dest_is_current(dest: &Path, src_meta: &fs::Metadata) -> Result<bool> {
    reject_symlink_components(dest)?;
    let dest_meta = match fs::symlink_metadata(dest) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to inspect destination {:?}", dest));
        }
    };
    if dest_meta.file_type().is_symlink() {
        anyhow::bail!("destination {:?} is a symlink", dest);
    }
    if !dest_meta.is_file() {
        return Ok(false); // directory/special in the way → staging reports copy failure
    }
    match (src_meta.modified().ok(), dest_meta.modified().ok()) {
        (Some(src_m), Some(dest_m)) => Ok(src_m <= dest_m),
        // Filesystems without mtime: cannot compare — refresh.
        _ => Ok(false),
    }
}

// Saved pipelines no longer own a shared on-disk workdir: every create-run
// is isolated under `users/<u>/runs/<run_id>/` (audit RB-02; retry/resume
// continue a predecessor lineage), so the old `pipelines/<pipeline_id>/`
// helpers were removed. `pipeline_id` is a DB association only.

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that `set_current_dir` into a temp cwd —
    /// `stage_user_inputs` resolves `workspace/…` relative to the process
    /// cwd, so two concurrent cwd-dancing tests would stomp each other.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn setup_run_directory_creates_path() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("workspace/users/testuser/runs/run-001");
        fs::create_dir_all(&run_dir).unwrap();
        assert!(run_dir.exists());
    }

    #[test]
    fn initialize_sandbox_writes_workflow() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("workspace/users/testuser/runs/run-002");
        fs::create_dir_all(&run_dir).unwrap();
        let wf = run_dir.join("workflow.oxoflow");
        fs::write(&wf, "[workflow]\nname = \"test\"\nversion = \"1.0\"\n").unwrap();
        assert!(wf.exists());
        let content = fs::read_to_string(&wf).unwrap();
        assert!(content.contains("test"));
    }

    #[test]
    fn get_run_directory_returns_correct_path() {
        let path = get_run_directory("alice", "run-abc").unwrap();
        assert!(path.ends_with("workspace/users/alice/runs/run-abc"));
    }

    #[test]
    fn get_run_directory_rejects_traversal_usernames() {
        // Arrange — usernames are the user-controlled component; each must
        // fail loudly instead of escaping the workspace/users tree.
        for name in ["..", "../alice", "/tmp", "a/b", "~root"] {
            // Act / Assert
            assert!(
                get_run_directory(name, "run-abc").is_err(),
                "username {name:?} must be rejected"
            );
        }
    }

    #[test]
    fn inputs_directory_rejects_traversal_and_absolute_usernames() {
        // Arrange / Act / Assert
        assert!(inputs_directory("..").is_err());
        assert!(inputs_directory("/etc").is_err());
        assert!(inputs_directory("alice").is_ok());
    }

    #[test]
    fn check_relative_path_rejects_degenerate_forms() {
        // Empty, `.`-only, and trailing-slash keys resolve to directories
        // or nothing — reject them at the gate instead of failing later
        // with a confusing 500.
        assert!(check_relative_path("").is_err());
        assert!(check_relative_path(".").is_err());
        assert!(check_relative_path("./").is_err());
        assert!(check_relative_path("dir/").is_err());
        assert!(check_relative_path("./").is_err());
        // Real relative file paths, including nested ones, stay accepted.
        assert!(check_relative_path("envs/pixi.toml").is_ok());
        assert!(check_relative_path("a.txt").is_ok());
        // Pre-existing guarantees keep holding.
        assert!(check_relative_path("/etc/passwd").is_err());
        assert!(check_relative_path("../secret").is_err());
        assert!(check_relative_path("a/../b").is_err());
    }

    #[test]
    fn canonicalize_relative_path_removes_dot_and_repeated_separator_aliases() {
        assert_eq!(
            canonicalize_relative_path("./workflow.oxoflow").unwrap(),
            "workflow.oxoflow"
        );
        assert_eq!(
            canonicalize_relative_path("modules//typed.oxoflow").unwrap(),
            "modules/typed.oxoflow"
        );
        assert_eq!(
            canonicalize_relative_path("./modules/./typed.oxoflow").unwrap(),
            "modules/typed.oxoflow"
        );
        assert!(canonicalize_relative_path("../workflow.oxoflow").is_err());
        assert!(canonicalize_relative_path("/workflow.oxoflow").is_err());
    }

    #[test]
    fn stage_user_inputs_mirrors_tree_and_skips_existing() {
        // `inputs_directory` resolves `workspace/…` RELATIVE to the process
        // cwd (the same convention `setup_run_directory` follows at request
        // time), so the test runs inside a temp cwd.
        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs/data").unwrap();
        std::fs::write(
            "workspace/users/alice/inputs/meta.tsv",
            "sample\tlevel\nS1\thigh\n",
        )
        .unwrap();
        std::fs::write("workspace/users/alice/inputs/data/raw.txt", "x\n").unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();

        // Act — stage twice.
        let first = stage_user_inputs("alice", &run_dir).unwrap();
        let second = stage_user_inputs("alice", &run_dir).unwrap();

        // Restore cwd before asserting (same pattern as audit.rs tests):
        // a panic with the cwd inside tmp would leave every other test
        // resolving `workspace/…` inside a deleted temp dir.
        std::env::set_current_dir(prev).unwrap();

        // Assert — both files staged, paths preserved, idempotent.
        assert_eq!(first, 2, "both files staged on the first pass");
        assert_eq!(second, 0, "existing files are not re-copied");
        assert!(run_dir.join("meta.tsv").is_file());
        assert!(run_dir.join("data/raw.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn stage_user_inputs_rejects_source_and_destination_symlinks() {
        use std::os::unix::fs::symlink;

        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::create_dir_all("workspace/users/alice/runs/r1").unwrap();
        std::fs::write("external-secret.txt", "do not stage\n").unwrap();
        symlink(
            tmp.path().join("external-secret.txt"),
            "workspace/users/alice/inputs/leak.txt",
        )
        .unwrap();

        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        let source_err = stage_user_inputs("alice", &run_dir).unwrap_err();
        assert!(
            source_err.to_string().contains("symlink"),
            "source symlink must be rejected: {source_err:#}"
        );

        std::fs::remove_file("workspace/users/alice/inputs/leak.txt").unwrap();
        std::fs::write("workspace/users/alice/inputs/safe.txt", "safe\n").unwrap();
        symlink("../../../../external-secret.txt", run_dir.join("safe.txt")).unwrap();

        let dest_err = stage_user_inputs("alice", &run_dir).unwrap_err();
        assert!(
            dest_err.to_string().contains("symlink"),
            "destination symlink must be rejected: {dest_err:#}"
        );

        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    fn stage_user_inputs_skips_staged_upload_temps() {
        // Issue #1161: a process killed between creating its staging temp and
        // renaming it over the destination leaves
        // `<name>.oxoupload-<seq>-<pid>.tmp` beside the upload. Mirroring it
        // would give every run a phantom input whose name no user ever chose.
        // The bare numeric shape (`meta.tsv.7.999999.tmp`) is a REAL file —
        // the collision case the marker redesign exists to protect.
        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::write("workspace/users/alice/inputs/meta.tsv", "a\n").unwrap();
        // The orphan shape the upload handler produces, a versioned real file
        // the old numeric shape would have swallowed, and a genuine upload
        // that merely happens to end in `.tmp` — the latter two must survive.
        std::fs::write(
            "workspace/users/alice/inputs/meta.tsv.oxoupload-7-999999.tmp",
            "partial",
        )
        .unwrap();
        std::fs::write(
            "workspace/users/alice/inputs/notes.tmp.1.999999.tmp",
            "keep",
        )
        .unwrap();
        std::fs::write("workspace/users/alice/inputs/notes.tmp", "keep").unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();

        let copied = stage_user_inputs("alice", &run_dir).unwrap();

        std::env::set_current_dir(prev).unwrap();

        assert_eq!(copied, 3, "only the real uploads are staged");
        assert!(run_dir.join("meta.tsv").is_file());
        assert!(
            run_dir.join("notes.tmp").is_file(),
            "a real upload ending in .tmp is not engine scratch"
        );
        assert!(
            run_dir.join("notes.tmp.1.999999.tmp").is_file(),
            "a versioned real file matching the old numeric shape is not \
             engine scratch"
        );
        assert!(
            !run_dir.join("meta.tsv.oxoupload-7-999999.tmp").exists(),
            "the orphaned staging temp must not be mirrored"
        );
    }

    #[test]
    fn stage_user_inputs_refreshes_stale_copy_from_newer_upload() {
        // Issue #297 item 1: `dest.exists()` alone kept a stale run-dir copy
        // serving forever after the user re-uploaded the input. A newer
        // upload must replace it; a newer LOCAL file (pipeline output on the
        // staged path) must survive re-staging.
        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::write("workspace/users/alice/inputs/meta.tsv", "v1\n").unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        assert_eq!(stage_user_inputs("alice", &run_dir).unwrap(), 1);

        // Second run: the pipeline overwrote the staged copy with a local
        // result (older than nothing), and the user re-uploaded meta.tsv.
        let staged = run_dir.join("meta.tsv");
        std::fs::write(&staged, "stale local copy\n").unwrap();
        set_mtime(&staged, older());
        std::fs::write("workspace/users/alice/inputs/meta.tsv", "v2 re-upload\n").unwrap();

        let second = stage_user_inputs("alice", &run_dir).unwrap();

        // A local file newer than its staged source is NOT clobbered.
        let kept = run_dir.join("kept.txt");
        std::fs::write(&kept, "pipeline output\n").unwrap();
        std::fs::write("workspace/users/alice/inputs/kept.txt", "older upload\n").unwrap();
        set_mtime(
            std::path::Path::new("workspace/users/alice/inputs/kept.txt"),
            older(),
        );
        let third = stage_user_inputs("alice", &run_dir).unwrap();

        std::env::set_current_dir(prev).unwrap();

        assert_eq!(second, 1, "the newer upload replaces the stale copy");
        assert_eq!(std::fs::read_to_string(&staged).unwrap(), "v2 re-upload\n");
        assert_eq!(third, 0, "both files current — nothing left to stage");
        assert_eq!(
            std::fs::read_to_string(&kept).unwrap(),
            "pipeline output\n",
            "a newer local file survives re-staging"
        );
    }

    /// Set a file's mtime into the recent past (deterministically older than
    /// anything written by the test body itself).
    fn older() -> std::time::SystemTime {
        std::time::SystemTime::now() - std::time::Duration::from_secs(120)
    }

    fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    #[test]
    fn stage_user_inputs_without_uploads_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("runs/r2");
        std::fs::create_dir_all(&run_dir).unwrap();
        assert_eq!(stage_user_inputs("nobody", &run_dir).unwrap(), 0);
    }

    #[test]
    fn stage_user_inputs_rejects_traversal_username() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("runs/r3");
        std::fs::create_dir_all(&run_dir).unwrap();
        assert!(stage_user_inputs("../alice", &run_dir).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stage_user_inputs_rejects_source_symlinks() {
        use std::os::unix::fs::symlink;

        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::write("outside.txt", "outside secret").unwrap();
        symlink(
            tmp.path().join("outside.txt"),
            "workspace/users/alice/inputs/leak.txt",
        )
        .unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = stage_user_inputs("alice", &run_dir);
        std::env::set_current_dir(prev).unwrap();

        assert!(result.is_err(), "source symlinks must fail staging");
        assert!(
            !run_dir.join("leak.txt").exists(),
            "symlinked upload must not be copied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stage_user_inputs_rejects_symlinked_source_parent() {
        use std::os::unix::fs::symlink;

        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let outside = tmp.path().join("outside");
        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("data.txt"), "outside").unwrap();
        symlink(&outside, "workspace/users/alice/inputs/external").unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = stage_user_inputs("alice", &run_dir);
        std::env::set_current_dir(prev).unwrap();

        assert!(
            result.is_err(),
            "symlinked source parents must fail staging"
        );
        assert!(
            !run_dir.join("external/data.txt").exists(),
            "files below a symlinked source parent must not be copied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stage_user_inputs_rejects_symlinked_destination_parent() {
        use std::os::unix::fs::symlink;

        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        std::fs::write("workspace/users/alice/inputs/data.txt", "upload").unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, run_dir.join("redirect")).unwrap();

        // Make the upload resolve below the symlinked destination parent.
        std::fs::create_dir_all("workspace/users/alice/inputs/redirect").unwrap();
        std::fs::write("workspace/users/alice/inputs/redirect/data.txt", "upload").unwrap();

        let result = stage_user_inputs("alice", &run_dir);
        std::env::set_current_dir(prev).unwrap();

        assert!(
            result.is_err(),
            "symlinked destination parents must fail staging"
        );
        assert!(
            !outside.join("redirect/data.txt").exists() && !outside.join("data.txt").exists(),
            "a destination symlink must not redirect staged data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stage_user_inputs_rejects_unreadable_regular_file() {
        use std::os::unix::fs::PermissionsExt;

        let _cwd = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::create_dir_all("workspace/users/alice/inputs").unwrap();
        let source = std::path::Path::new("workspace/users/alice/inputs/private.txt");
        std::fs::write(source, "private").unwrap();
        std::fs::set_permissions(source, std::fs::Permissions::from_mode(0o0)).unwrap();
        let run_dir = tmp.path().join("workspace/users/alice/runs/r1");
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = stage_user_inputs("alice", &run_dir);
        std::fs::set_permissions(source, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::env::set_current_dir(prev).unwrap();

        assert!(
            result.is_err(),
            "mode-000 uploaded input must be reported as unreadable"
        );
    }
}
