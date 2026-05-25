use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::commands::print_banner;

pub fn init_command(name: String, dir: Option<PathBuf>) -> Result<()> {
    print_banner();

    // Validate project name: must be non-empty and a valid identifier
    if name.trim().is_empty() {
        anyhow::bail!(
            "project name must not be empty. Provide a name, e.g.:\n  oxo-flow init my-pipeline"
        );
    }
    // Reject names that are only whitespace or contain path separators
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!(
            "project name '{}' must not contain path separators. Use a simple name, e.g.: my-pipeline",
            name
        );
    }

    let project_dir = dir.unwrap_or_else(|| PathBuf::from(&name));

    // Warn if project directory already exists
    if project_dir.exists() {
        eprintln!(
            "{} Directory '{}' already exists. Files may be overwritten.",
            "Warning:".bold().yellow(),
            project_dir.display()
        );
    }

    std::fs::create_dir_all(&project_dir)?;

    let workflow_content = format!(
        r#"[workflow]
name = "{name}"
version = "0.1.0"
description = "A new oxo-flow pipeline"
author = ""

[config]
# Variables defined here are used in shell commands as {{config.key}}
sample_name = "example"
greeting = "Hello from oxo-flow!"

[defaults]
threads = 1
memory = "1G"

# ── Rules ──────────────────────────────────────────────────────────────────
# Each rule is a single processing step with inputs, outputs, and a shell command.
#
# Shell template reference:
#   {{input[0]}}    — first input file    {{input}}  — all inputs (space-joined)
#   {{output[0]}}   — first output file   {{output}} — all outputs
#   {{threads}}     — CPU thread count    {{memory}} — memory limit
#   {{config.key}}  — config variable     {{sample}} — wildcard value

[[rules]]
name = "hello_world"
description = "A minimal rule that writes a greeting"
output = ["results/{{config.sample_name}}_output.txt"]
shell = "echo '{{config.greeting}}' > {{output[0]}}"

# ── Adding a second rule with a dependency ─────────────────────────────────
# Uncomment the block below to create a two-step pipeline:
#
# [[rules]]
# name = "process_results"
# description = "Transform the output from hello_world"
# input = ["results/{{config.sample_name}}_output.txt"]
# output = ["results/final_report.txt"]
# shell = "wc -l {{input[0]}} > {{output[0]}}"
# [rules.environment]
# conda = "envs/example.yaml"
"#
    );

    let workflow_path = project_dir.join(format!("{name}.oxoflow"));
    std::fs::write(&workflow_path, workflow_content)?;

    // Create additional directories
    let envs_dir = project_dir.join("envs");
    let scripts_dir = project_dir.join("scripts");
    let data_dir = project_dir.join("data");
    let results_dir = project_dir.join("results");
    std::fs::create_dir_all(&envs_dir)?;
    std::fs::create_dir_all(&scripts_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&results_dir)?;

    // Create initial input file
    std::fs::write(
        data_dir.join("input.txt"),
        "This is your starting input data.\n",
    )?;

    // Create starter environment file with China mirror channels
    let env_content = "\
# Example Conda environment specification
# For users in China, uncomment the mirror channels below for faster downloads
name: example-env
channels:
  # - https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/bioconda
  # - https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge
  - bioconda
  - conda-forge
  - defaults
dependencies:
  - fastp=0.24.0
  - samtools=1.20
";
    std::fs::write(envs_dir.join("example.yaml"), env_content)?;

    // Create starter script
    let script_content = "\
#!/bin/bash
# Example helper script
echo \"Running helper script for $1\"
";
    std::fs::write(scripts_dir.join("example.sh"), script_content)?;

    // Create a .gitignore with common bioinformatics patterns
    let gitignore_content = "\
# Alignment files
*.bam
*.bam.bai
*.cram
*.cram.crai
*.sam

# Variant files
*.vcf.gz
*.vcf.gz.tbi
*.bcf

# Index files
*.fai
*.dict

# Workflow outputs
logs/
results/
benchmarks/

# oxo-flow internals
.oxo-flow/
.oxo-flow-cache/

# OS files
.DS_Store
Thumbs.db
";
    let gitignore_path = project_dir.join(".gitignore");
    std::fs::write(&gitignore_path, gitignore_content)?;

    eprintln!(
        "{} Created new project at {}",
        "✓".green().bold(),
        project_dir.display()
    );
    eprintln!("  {}", workflow_path.display());
    eprintln!("  {}/example.yaml", envs_dir.display());
    eprintln!("  {}/example.sh", scripts_dir.display());
    eprintln!("  {}", gitignore_path.display());
    eprintln!(
        "\n  {} To run your first workflow:",
        "Next steps:".bold().cyan()
    );
    eprintln!("    cd {}", project_dir.display());
    eprintln!(
        "    oxo-flow run {}",
        workflow_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workflow.oxoflow")
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Gallery / template helpers
// ---------------------------------------------------------------------------

/// Walk upward from `start` looking for an `examples/gallery/` directory.
fn walk_up_for_gallery(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let gallery = dir.join("examples").join("gallery");
        if gallery.is_dir() {
            return Some(gallery);
        }
        current = dir.parent();
    }
    None
}

/// Locate the `examples/gallery/` directory using several strategies.
fn find_gallery_directory() -> Result<PathBuf> {
    // Strategy 1 – walk up from CWD
    if let Ok(cwd) = std::env::current_dir()
        && let Some(gallery) = walk_up_for_gallery(&cwd)
    {
        return Ok(gallery);
    }

    // Strategy 2 – walk up from the binary path
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && let Some(gallery) = walk_up_for_gallery(parent)
    {
        return Ok(gallery);
    }

    // Strategy 3 – compile-time manifest dir (works in development)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(gallery) = walk_up_for_gallery(&manifest_dir) {
        return Ok(gallery);
    }

    anyhow::bail!(
        "could not find examples/gallery/ directory.\n\
         Make sure you are inside the oxo-flow repository."
    )
}

/// Extract a display title and one-line description from the leading comments
/// of a `.oxoflow` template file.
fn parse_template_header(content: &str) -> (String, String) {
    let mut title = String::new();
    let mut description = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('#') {
            break;
        }
        let comment = trimmed.trim_start_matches('#').trim();
        if comment.is_empty() {
            continue;
        }
        if title.is_empty() {
            title = comment.to_string();
        } else if description.is_empty() {
            description = comment.to_string();
        } else {
            break; // only need first two meaningful comment lines
        }
    }

    (title, description)
}

/// Replace the first `name = "..."` (the workflow name field) with `new_name`.
fn substitute_workflow_name(content: &str, new_name: &str) -> String {
    let marker = "name = \"";
    if let Some(start) = content.find(marker) {
        let after_equals = start + marker.len();
        if let Some(end) = content[after_equals..].find('"') {
            let mut result = content[..start].to_string();
            result.push_str(&format!("name = \"{}\"", new_name));
            result.push_str(&content[after_equals + end + 1..]);
            return result;
        }
    }
    content.to_string()
}

/// Derive a "descriptive name" from the file stem by stripping a leading
/// `XX_` number prefix (e.g. `01_hello_world` -> `hello_world`).
fn descriptive_name_from_stem(stem: &str) -> String {
    stem.split_once('_')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| stem.to_string())
}

// ---------------------------------------------------------------------------
// List all available templates
// ---------------------------------------------------------------------------

fn list_templates(gallery_dir: &Path) -> Result<()> {
    let mut entries: Vec<(String, String, String)> = Vec::new();

    for entry in std::fs::read_dir(gallery_dir)
        .with_context(|| format!("cannot read gallery directory {}", gallery_dir.display()))?
    {
        let entry = entry.context("cannot read directory entry")?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "oxoflow") {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let (title, description) = parse_template_header(&content);
            let filename = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            entries.push((filename, title, description));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!();
    eprintln!("{}", "Available templates:".bold().cyan());
    eprintln!();

    for (filename, title, description) in &entries {
        if !title.is_empty() {
            eprintln!("  {}  {}", filename.bold(), title.dimmed());
        } else {
            eprintln!("  {}", filename.bold());
        }
        if !description.is_empty() {
            eprintln!("      {}", description.dimmed());
        }
        eprintln!();
    }

    eprintln!(
        "{}  {} <NAME>  to generate a workflow from a template.",
        "Usage:".bold(),
        "oxo-flow template".bold().cyan()
    );
    eprintln!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Apply a single template (copy + name substitution)
// ---------------------------------------------------------------------------

fn apply_template(gallery_dir: &Path, template_name: &str, output: Option<PathBuf>) -> Result<()> {
    // Collect candidate files matching by full stem or descriptive suffix.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(gallery_dir)
        .with_context(|| format!("cannot read gallery directory {}", gallery_dir.display()))?
    {
        let entry = entry.context("cannot read directory entry")?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "oxoflow") {
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if stem == template_name || stem.ends_with(&format!("_{}", template_name)) {
                candidates.push(path);
            }
        }
    }

    let template_path = match candidates.len() {
        0 => anyhow::bail!(
            "template '{}' not found.\n  \
             Use 'oxo-flow template' to list available templates.",
            template_name
        ),
        1 => candidates.into_iter().next().unwrap(),
        _ => {
            // Prefer an exact stem match
            let exact: Vec<&PathBuf> = candidates
                .iter()
                .filter(|p| p.file_stem().is_some_and(|s| s == template_name))
                .collect();
            if exact.len() == 1 {
                exact.into_iter().next().unwrap().clone()
            } else {
                candidates.into_iter().next().unwrap()
            }
        }
    };

    let content = std::fs::read_to_string(&template_path)
        .with_context(|| format!("cannot read {}", template_path.display()))?;

    // Derive the new workflow name from the file stem (strip number prefix)
    let template_stem = template_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let new_name = descriptive_name_from_stem(&template_stem);

    // Substitute the `name` field
    let new_content = substitute_workflow_name(&content, &new_name);

    // Write to specified output path, or current directory with template name
    let output_path = match output {
        Some(p) => {
            if p.is_dir() {
                p.join(format!("{}.oxoflow", &new_name))
            } else {
                p
            }
        }
        None => std::env::current_dir()
            .context("cannot determine current directory")?
            .join(format!("{}.oxoflow", &new_name)),
    };

    if output_path.exists() {
        anyhow::bail!(
            "{} already exists.\n  \
             Remove it first or choose a different name.",
            output_path.display()
        );
    }

    std::fs::write(&output_path, new_content)
        .with_context(|| format!("cannot write {}", output_path.display()))?;

    eprintln!();
    eprintln!(
        "{} Created workflow from template: {}",
        "\u{2713}".green().bold(),
        template_path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or(std::borrow::Cow::Borrowed("workflow.oxoflow"))
    );
    eprintln!("  {}", output_path.display());
    eprintln!();
    eprintln!("{}  To run this workflow:", "Next steps:".bold().cyan());
    eprintln!("    oxo-flow run {}", output_path.display());
    eprintln!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn template_command(name: Option<String>, output: Option<PathBuf>) -> Result<()> {
    print_banner();

    let gallery_dir = find_gallery_directory()?;

    match name {
        None => list_templates(&gallery_dir),
        Some(template_name) => apply_template(&gallery_dir, &template_name, output),
    }
}
