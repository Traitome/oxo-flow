use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Base workspace directory for the Web UI.
const BASE_WORKSPACE: &str = "workspace";

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
pub fn setup_run_directory(username: &str, run_id: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    validate_path_component(run_id, "run_id")?;

    let run_dir = Path::new(BASE_WORKSPACE)
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
    Ok(Path::new(BASE_WORKSPACE)
        .join("users")
        .join(username)
        .join("runs")
        .join(run_id))
}

/// Directory holding a user's uploaded inputs (issue #82 P0-2): the
/// `workspace/users/{user}/inputs/` root served by `POST /api/files`.
pub fn inputs_directory(username: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    Ok(Path::new(BASE_WORKSPACE)
        .join("users")
        .join(username)
        .join("inputs"))
}

/// Mirror the acting user's uploaded inputs into a run's working directory
/// (issue #276 L1): uploads land in `workspace/users/<u>/inputs/` but the
/// CLI executes in the run/pipeline dir, so data-referencing workflows
/// (`metadata_file`, input globs) failed pre-execution for web-only users.
///
/// The WHOLE inputs tree is mirrored (skipping files that already exist in
/// the destination, so pipeline re-runs keep newer local outputs), because
/// which files a workflow needs is a parse-time property (`metadata_file`,
/// input globs) the HTTP layer should not re-derive. Returns the number of
/// files copied.
pub fn stage_user_inputs(username: &str, run_dir: &Path) -> Result<usize> {
    let root = inputs_directory(username)?;
    if !root.is_dir() {
        return Ok(0);
    }
    let mut copied = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("failed to read inputs directory {:?}", dir))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue; // never follow symlinks out of the inputs tree
            }
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            let dest = run_dir.join(rel);
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {:?}", parent))?;
            }
            fs::copy(&path, &dest)
                .with_context(|| format!("failed to stage {:?} → {:?}", path, dest))?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Retrieve the persistent working directory of a saved pipeline (issue #69).
///
/// Runs targeting a saved pipeline execute here — the same directory across
/// runs — so `.oxo-flow/checkpoint.json` survives between web re-runs and the
/// CLI's config-change impact analysis + input manifests deliver precise
/// rebuilds (affected rules re-run, the rest are reused).
pub fn get_pipeline_directory(username: &str, pipeline_id: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    validate_path_component(pipeline_id, "pipeline_id")?;
    Ok(Path::new(BASE_WORKSPACE)
        .join("users")
        .join(username)
        .join("pipelines")
        .join(pipeline_id))
}

/// Create the pipeline working directory if it does not exist yet.
pub fn setup_pipeline_directory(username: &str, pipeline_id: &str) -> Result<PathBuf> {
    validate_path_component(username, "username")?;
    validate_path_component(pipeline_id, "pipeline_id")?;

    let dir = get_pipeline_directory(username, pipeline_id)?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create pipeline directory: {:?}", dir))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_pipeline_directory_returns_persistent_path() {
        // Issue #69: pipeline runs share one directory across runs so the
        // checkpoint (and with it precise invalidation) survives.
        let path = get_pipeline_directory("alice", "pl-123").unwrap();
        assert!(path.ends_with("workspace/users/alice/pipelines/pl-123"));
    }

    #[test]
    fn setup_pipeline_directory_rejects_traversal() {
        assert!(setup_pipeline_directory("alice", "../etc").is_err());
        assert!(setup_pipeline_directory("alice", "a/b").is_err());
        assert!(setup_pipeline_directory("", "pl-1").is_err());
    }

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
    fn stage_user_inputs_mirrors_tree_and_skips_existing() {
        // `inputs_directory` resolves `workspace/…` RELATIVE to the process
        // cwd (the same convention `setup_run_directory` follows at request
        // time), so the test runs inside a temp cwd.
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

    #[test]
    fn get_pipeline_directory_rejects_traversal_ids() {
        // Arrange / Act / Assert
        assert!(get_pipeline_directory("alice", "../p1").is_err());
        assert!(get_pipeline_directory("alice", "p1").is_ok());
    }
}
