use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// Bundle a workflow with its referenced environment files into a publishable directory.
///
/// Reads the .oxoflow workflow file, discovers referenced environment spec files,
/// and copies everything into a bundle directory with a manifest.json.
pub fn publish_command(workflow: PathBuf, output: Option<PathBuf>) -> Result<()> {
    let workflow_path =
        std::path::absolute(&workflow).context("failed to resolve workflow path")?;
    let workflow_dir = workflow_path.parent().unwrap_or(Path::new("."));

    let workflow_name = workflow_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow");

    let output_dir = if let Some(out) = output {
        std::path::absolute(&out).context("failed to resolve output path")?
    } else {
        PathBuf::from(format!("{}-bundle", workflow_name))
    };

    // Create output directory
    std::fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    // Copy the workflow file
    let dest_workflow = output_dir.join(
        workflow_path
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("workflow.oxoflow")),
    );
    std::fs::copy(&workflow_path, &dest_workflow)
        .with_context(|| format!("failed to copy workflow to {}", dest_workflow.display()))?;

    // Read workflow to discover referenced environment files
    let content =
        std::fs::read_to_string(&workflow_path).context("failed to read workflow file")?;
    let toml_value: toml::Value = content
        .parse()
        .context("failed to parse workflow as TOML")?;

    let mut referenced_files: Vec<(String, PathBuf)> = Vec::new();

    // Scan [[rules]] for environment spec file references
    if let Some(rules) = toml_value.get("rules").and_then(|v| v.as_array()) {
        for rule in rules {
            // Check for env.file (table format: [rules.env])
            if let Some(env) = rule.get("env")
                && let Some(env_file) = env.get("file").and_then(|v| v.as_str())
            {
                let abs_path = workflow_dir.join(env_file);
                if abs_path.exists() {
                    let filename = abs_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !referenced_files.iter().any(|(name, _)| name == &filename) {
                        referenced_files.push((filename, abs_path));
                    }
                }
            }

            // Check for conda_env field
            if let Some(conda_env) = rule.get("conda_env").and_then(|v| v.as_str()) {
                let abs_path = workflow_dir.join(conda_env);
                if abs_path.exists() {
                    let filename = abs_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !referenced_files.iter().any(|(name, _)| name == &filename) {
                        referenced_files.push((filename, abs_path));
                    }
                }
            }
        }
    }

    // Also check [workflow] for pairs_file / sample_groups_file
    if let Some(workflow_section) = toml_value.get("workflow") {
        for key in &["pairs_file", "sample_groups_file"] {
            if let Some(file_path) = workflow_section.get(key).and_then(|v| v.as_str()) {
                let abs_path = workflow_dir.join(file_path);
                if abs_path.exists() {
                    let filename = abs_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !referenced_files.iter().any(|(name, _)| name == &filename) {
                        referenced_files.push((filename, abs_path));
                    }
                }
            }
        }
    }

    // Copy referenced files
    for (filename, abs_path) in &referenced_files {
        let dest = output_dir.join(filename);
        std::fs::copy(abs_path, &dest).with_context(|| {
            format!(
                "failed to copy {} to {}",
                abs_path.display(),
                dest.display()
            )
        })?;
    }

    // Create manifest.json
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = serde_json::json!({
        "workflow": workflow_path.file_name().and_then(|s| s.to_str()),
        "environment_files": referenced_files.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "created_at_epoch": timestamp,
        "format": "oxoflow-bundle-v1",
    });

    let manifest_path = output_dir.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
        .context("failed to write manifest.json")?;

    // Summary
    eprintln!(
        "{} Published to {}",
        "✓".green().bold(),
        output_dir.display()
    );
    eprintln!("  workflow: {}", dest_workflow.display());
    for (filename, _) in &referenced_files {
        eprintln!("  env file: {}", filename);
    }
    eprintln!("  manifest: {}", manifest_path.display());

    Ok(())
}
