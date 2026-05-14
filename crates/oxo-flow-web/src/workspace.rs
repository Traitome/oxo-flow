use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use std::fs;

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
