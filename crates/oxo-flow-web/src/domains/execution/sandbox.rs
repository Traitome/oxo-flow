//! Workspace directory management for pipeline execution.
//!
//! Creates isolated work directories per run, manages cleanup, and ensures
//! path safety. Each run gets a unique workspace under the configured
//! workspace root directory.
//!
//! Migrated from `workspace.rs`.

use std::path::{Path, PathBuf};

/// Configuration for workspace management.
#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    /// Root directory for all workspaces.
    pub root: PathBuf,
    /// Whether to auto-cleanup workspaces for completed runs.
    pub auto_cleanup: bool,
    /// Maximum workspace age before cleanup (days).
    pub max_age_days: u32,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("workspace"),
            auto_cleanup: false,
            max_age_days: 30,
        }
    }
}

/// Create a new workspace directory for a run.
///
/// Returns the path to the run's workspace.
pub fn create_run_workspace(config: &WorkspaceConfig, run_id: &str) -> Result<PathBuf, String> {
    let workspace = resolve_workspace_root(&config.root)?;

    // Sanitize run_id to prevent path traversal
    let safe_id = sanitize_path_component(run_id);
    let run_dir = workspace.join(&safe_id);

    std::fs::create_dir_all(&run_dir)
        .map_err(|e| format!("Failed to create workspace {}: {e}", run_dir.display()))?;

    // Create standard subdirectories
    std::fs::create_dir_all(run_dir.join("input")).ok();
    std::fs::create_dir_all(run_dir.join("output")).ok();
    std::fs::create_dir_all(run_dir.join("logs")).ok();
    std::fs::create_dir_all(run_dir.join("tmp")).ok();

    tracing::info!("Created workspace: {}", run_dir.display());
    Ok(run_dir)
}

/// Resolve the workspace root, creating it if needed.
fn resolve_workspace_root(root: &Path) -> Result<PathBuf, String> {
    let resolved = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current dir: {e}"))?
            .join(root)
    };

    std::fs::create_dir_all(&resolved).map_err(|e| {
        format!(
            "Failed to create workspace root {}: {e}",
            resolved.display()
        )
    })?;

    Ok(resolved)
}

/// Sanitize a path component to prevent directory traversal.
///
/// Replaces `/`, `\`, and `..` with safe alternatives.
fn sanitize_path_component(component: &str) -> String {
    component.replace("..", "_").replace(['/', '\\', '\0'], "_")
}

/// List all runs currently in the workspace.
pub fn list_workspace_runs(config: &WorkspaceConfig) -> Result<Vec<String>, String> {
    let workspace = resolve_workspace_root(&config.root)?;
    let mut runs = Vec::new();

    let entries =
        std::fs::read_dir(&workspace).map_err(|e| format!("Failed to read workspace: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && let Some(name) = entry.file_name().to_str()
        {
            runs.push(name.to_string());
        }
    }

    Ok(runs)
}

/// Clean up a specific run's workspace directory.
pub fn cleanup_run_workspace(config: &WorkspaceConfig, run_id: &str) -> Result<(), String> {
    let workspace = resolve_workspace_root(&config.root)?;
    let safe_id = sanitize_path_component(run_id);
    let run_dir = workspace.join(&safe_id);

    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)
            .map_err(|e| format!("Failed to cleanup workspace {}: {e}", run_dir.display()))?;
        tracing::info!("Cleaned up workspace: {}", run_dir.display());
    }

    Ok(())
}

/// Get the log directory for a run.
pub fn run_log_dir(config: &WorkspaceConfig, run_id: &str) -> Result<PathBuf, String> {
    let workspace = resolve_workspace_root(&config.root)?;
    let safe_id = sanitize_path_component(run_id);
    let log_dir = workspace.join(&safe_id).join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    Ok(log_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_component() {
        assert_eq!(sanitize_path_component("abc"), "abc");
        assert_eq!(sanitize_path_component("a/b"), "a_b");
        assert_eq!(sanitize_path_component(".."), "_");
        assert_eq!(sanitize_path_component("a\\b"), "a_b");
        assert_eq!(sanitize_path_component("a\0b"), "a_b");
    }

    #[test]
    fn test_create_and_cleanup_workspace() {
        let config = WorkspaceConfig {
            root: std::env::temp_dir().join("oxo-test-workspace"),
            auto_cleanup: true,
            max_age_days: 1,
        };

        let run_id = "test-run-001";
        let ws = create_run_workspace(&config, run_id).expect("should create workspace");
        assert!(ws.exists());
        assert!(ws.join("logs").exists());
        assert!(ws.join("output").exists());

        // Cleanup
        cleanup_run_workspace(&config, run_id).expect("should cleanup");
        assert!(!ws.exists());

        // Cleanup workspace root
        let root = resolve_workspace_root(&config.root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_list_workspace_runs() {
        let config = WorkspaceConfig {
            root: std::env::temp_dir().join("oxo-test-list-workspace"),
            auto_cleanup: false,
            max_age_days: 30,
        };

        create_run_workspace(&config, "run-a").unwrap();
        create_run_workspace(&config, "run-b").unwrap();

        let runs = list_workspace_runs(&config).unwrap();
        assert!(runs.contains(&"run-a".to_string()));
        assert!(runs.contains(&"run-b".to_string()));

        // Cleanup
        cleanup_run_workspace(&config, "run-a").unwrap();
        cleanup_run_workspace(&config, "run-b").unwrap();
        let root = resolve_workspace_root(&config.root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
