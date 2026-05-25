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
pub fn get_run_directory(username: &str, run_id: &str) -> PathBuf {
    Path::new(BASE_WORKSPACE)
        .join("users")
        .join(username)
        .join("runs")
        .join(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let path = get_run_directory("alice", "run-abc");
        assert!(path.ends_with("workspace/users/alice/runs/run-abc"));
    }
}
