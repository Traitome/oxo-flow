use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Base workspace directory for the Web UI.
const BASE_WORKSPACE: &str = "workspace";

/// Setup the directory structure for a specific run.
///
/// Ensures `workspace/users/<username>/runs/<run_id>` exists.
pub fn setup_run_directory(username: &str, run_id: &str) -> Result<PathBuf> {
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
        let dir = setup_run_directory("testuser", "run-001").unwrap();
        assert!(dir.exists());
        assert!(dir.ends_with("workspace/users/testuser/runs/run-001"));
        // Cleanup
        let _ = fs::remove_dir_all("workspace");
    }

    #[test]
    fn initialize_sandbox_writes_workflow() {
        let toml = "[workflow]\nname = \"test\"\nversion = \"1.0\"\n";
        let dir = initialize_sandbox("testuser", "run-002", toml).unwrap();
        let wf = dir.join("workflow.oxoflow");
        assert!(wf.exists());
        let content = fs::read_to_string(&wf).unwrap();
        assert!(content.contains("test"));
        // Cleanup
        let _ = fs::remove_dir_all("workspace");
    }

    #[test]
    fn get_run_directory_returns_correct_path() {
        let path = get_run_directory("alice", "run-abc");
        assert!(path.ends_with("workspace/users/alice/runs/run-abc"));
    }
}
