//! Shared helpers and utilities for CLI commands.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// Auto-discover workflow file in current directory.
/// Priority: main.oxoflow > *.oxoflow (alphabetically first)
pub fn discover_workflow_file() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;

    // Priority 1: main.oxoflow
    let main_workflow = cwd.join("main.oxoflow");
    if main_workflow.exists() {
        return Ok(main_workflow);
    }

    // Priority 2: any *.oxoflow file (alphabetically first)
    let mut oxoflow_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&cwd)
        .with_context(|| format!("cannot read directory {}", cwd.display()))?
    {
        let entry = entry.context("cannot read directory entry")?;
        let path = entry.path();
        if let Some(ext) = path.extension()
            && ext == "oxoflow"
        {
            oxoflow_files.push(path);
        }
    }

    if oxoflow_files.is_empty() {
        return Err(anyhow::anyhow!(
            "no .oxoflow file found in current directory.\n\
            Specify a workflow file or run 'oxo-flow init' to create one."
        ));
    }

    // Sort alphabetically and pick first
    oxoflow_files.sort();
    Ok(oxoflow_files.into_iter().next().unwrap())
}

/// Resolve workflow path: use provided path or auto-discover.
pub fn resolve_workflow(provided: Option<PathBuf>) -> Result<PathBuf> {
    match provided {
        Some(path) => Ok(path),
        None => discover_workflow_file(),
    }
}

pub fn print_banner() {
    eprintln!(
        "{} {} — {}",
        "oxo-flow".bold().cyan(),
        env!("CARGO_PKG_VERSION"),
        "Bioinformatics Pipeline Engine".dimmed()
    );
}

/// Expand placeholders in a command template.
pub fn expand_batch_template(template: &str, item: &str, nr: usize) -> String {
    let path = std::path::Path::new(item);
    let basename = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy())
        .unwrap_or_default();
    let dir = path
        .parent()
        .map(|p| p.to_string_lossy())
        .unwrap_or_else(|| ".".into());

    template
        .replace("{}", item)
        .replace("{item}", item)
        .replace("{nr}", &nr.to_string())
        .replace("{basename}", &basename)
        .replace("{stem}", &stem)
        .replace("{ext}", &ext)
        .replace("{dir}", &dir)
}

/// Parse item lines, skipping blank lines and # comments.
pub fn parse_item_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// Collect items from arguments, file, or stdin.
pub fn collect_batch_items(items: &[String], file: Option<&PathBuf>) -> Result<Vec<String>> {
    // Priority: file > stdin > arguments
    if let Some(path) = file {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read items from {}", path.display()))?;
        return Ok(parse_item_lines(&content));
    }

    // Check stdin if no items provided
    if items.is_empty() {
        use std::io::{self, BufRead};
        let stdin = io::stdin();
        let lines: Vec<String> = stdin.lock().lines().map_while(Result::ok).collect();
        if !lines.is_empty() {
            return Ok(parse_item_lines(&lines.join("\n")));
        }
        return Err(anyhow::anyhow!(
            "no items provided (use -f FILE, stdin, or arguments)"
        ));
    }

    // Expand globs in arguments
    let expanded: Vec<String> = items
        .iter()
        .flat_map(|item| {
            if item.contains('*') || item.contains('?') || item.contains('[') {
                glob::glob(item)
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
            } else {
                vec![item.clone()]
            }
        })
        .collect();

    Ok(expanded)
}

/// Wrap command with environment activation.
pub fn wrap_batch_command(cmd: &str, env_spec: &str) -> String {
    // Parse environment spec: "conda: env.yaml" or "docker: image"
    if let Some((type_, spec)) = env_spec.split_once(':') {
        match type_.trim() {
            "conda" => {
                let env_name = std::path::Path::new(spec.trim())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(spec.trim());
                format!("conda run --no-banner -n {} {}", env_name, cmd)
            }
            "docker" => format!("docker run --rm {} sh -c '{}'", spec.trim(), cmd),
            "singularity" => format!("singularity exec {} sh -c '{}'", spec.trim(), cmd),
            _ => cmd.to_string(),
        }
    } else {
        // Assume conda environment name
        format!("conda run --no-banner -n {} {}", env_spec.trim(), cmd)
    }
}

/// Run a single batch command synchronously.
pub fn run_batch_command(cmd: &str, workdir: &Path) -> Result<i32> {
    use std::process::Command;

    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .output()
        .with_context(|| format!("failed to execute: {}", cmd))?;

    Ok(output.status.code().unwrap_or(-1))
}

pub mod batch;
pub mod clean;
pub mod cluster;
pub mod completions;
pub mod infra;
pub mod output;
pub mod project;
pub mod quality;
pub mod run;
pub mod web;
