use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

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

/// The workflow gallery embedded at build time — `template` works from an
/// installed binary, not only inside a repository checkout (issue #76).
///
/// The canonical source stays `examples/gallery/*.oxoflow`; this crate
/// mirrors it under `templates/` because `cargo package`/`cargo publish`
/// can only bundle files inside the crate root (the mirror is what makes
/// the gallery work from installed crates.io releases). A unit test guards
/// stems and content against the canonical directory, so drift fails CI.
const EMBEDDED_GALLERY: &[(&str, &str)] = &[
    (
        "01_hello_world",
        include_str!("../../templates/01_hello_world.oxoflow"),
    ),
    (
        "02_file_pipeline",
        include_str!("../../templates/02_file_pipeline.oxoflow"),
    ),
    (
        "03_parallel_samples",
        include_str!("../../templates/03_parallel_samples.oxoflow"),
    ),
    (
        "04_scatter_gather",
        include_str!("../../templates/04_scatter_gather.oxoflow"),
    ),
    (
        "05_conda_environments",
        include_str!("../../templates/05_conda_environments.oxoflow"),
    ),
    (
        "06_rnaseq_quantification",
        include_str!("../../templates/06_rnaseq_quantification.oxoflow"),
    ),
    (
        "07_wgs_germline",
        include_str!("../../templates/07_wgs_germline.oxoflow"),
    ),
    (
        "08_multiomics_integration",
        include_str!("../../templates/08_multiomics_integration.oxoflow"),
    ),
    (
        "09_single_cell_rnaseq",
        include_str!("../../templates/09_single_cell_rnaseq.oxoflow"),
    ),
    (
        "10_transform_operator",
        include_str!("../../templates/10_transform_operator.oxoflow"),
    ),
    (
        "11_conditional_workflow",
        include_str!("../../templates/11_conditional_workflow.oxoflow"),
    ),
    (
        "12_cohort_analysis",
        include_str!("../../templates/12_cohort_analysis.oxoflow"),
    ),
    (
        "13_simple_variant_calling",
        include_str!("../../templates/13_simple_variant_calling.oxoflow"),
    ),
    (
        "14_paired_experiment_control",
        include_str!("../../templates/14_paired_experiment_control.oxoflow"),
    ),
    (
        "15_paired_experiment_control_pairs",
        include_str!("../../templates/15_paired_experiment_control_pairs.oxoflow"),
    ),
];

/// Match an embedded template by exact stem or `_<name>` suffix (the same
/// rules the filesystem scan used); exact matches win.
fn find_embedded_template<'a>(
    gallery: &'a [(&'a str, &'a str)],
    template_name: &str,
) -> Option<(&'a str, &'a str)> {
    gallery
        .iter()
        .copied()
        .find(|(stem, _)| *stem == template_name)
        .or_else(|| {
            gallery
                .iter()
                .copied()
                .find(|(stem, _)| stem.ends_with(&format!("_{template_name}")))
        })
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

fn list_templates() -> Result<()> {
    let mut entries: Vec<(&str, String, String)> = EMBEDDED_GALLERY
        .iter()
        .map(|(stem, content)| {
            let (title, description) = parse_template_header(content);
            (*stem, title, description)
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(b.0));

    eprintln!();
    eprintln!("{}", "Available templates:".bold().cyan());
    eprintln!();

    for (stem, title, description) in &entries {
        if !title.is_empty() {
            eprintln!("  {}  {}", stem.bold(), title.dimmed());
        } else {
            eprintln!("  {}", stem.bold());
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

fn apply_template(template_name: &str, output: Option<PathBuf>) -> Result<()> {
    let (template_stem, content) = match find_embedded_template(EMBEDDED_GALLERY, template_name) {
        Some(found) => found,
        None => anyhow::bail!(
            "template '{}' not found.\n  \
             Use 'oxo-flow template' to list available templates.",
            template_name
        ),
    };

    // Derive the new workflow name from the file stem (strip number prefix)
    let new_name = descriptive_name_from_stem(template_stem);

    // Substitute the `name` field
    let new_content = substitute_workflow_name(content, &new_name);

    // Write to specified output path, or current directory with template name
    let output_path = match output {
        Some(p) => {
            if p.is_dir() {
                p.join(format!("{}.oxoflow", new_name))
            } else {
                p
            }
        }
        None => std::env::current_dir()
            .context("cannot determine current directory")?
            .join(format!("{}.oxoflow", new_name)),
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
        template_stem
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

pub async fn template_command(
    name: Option<String>,
    output: Option<PathBuf>,
    ai: bool,
    from_url: Vec<String>,
    from_file: Vec<PathBuf>,
    ai_max_retries: Option<u32>,
) -> Result<()> {
    print_banner();

    // AI-powered generation
    if ai {
        let intent = name.ok_or_else(|| {
            anyhow::anyhow!(
                "AI template generation requires a description.\n\
                 Example: oxo-flow template \"RNA-seq with STAR\" --ai"
            )
        })?;

        // Initialize AI provider
        let provider = oxo_flow_ai::provider::create_provider_from_env();
        if matches!(provider, oxo_flow_ai::provider::AiProvider::Noop) {
            anyhow::bail!(
                "AI provider not configured.\n\
                 Set OXO_FLOW_AI_PROVIDER=deepseek and DEEPSEEK_API_KEY=sk-...\n\
                 Or configure via ~/.oxo-flow/ai_config.json"
            );
        }

        crate::commands::ai_template::generate_workflow(
            &intent,
            &from_url,
            &from_file,
            output,
            ai_max_retries,
        )
        .await?;
        return Ok(());
    }

    match name {
        None => list_templates(),
        Some(template_name) => apply_template(&template_name, output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Vec<(String, String)> {
        EMBEDDED_GALLERY
            .iter()
            .map(|(stem, content)| (stem.to_string(), content.to_string()))
            .collect()
    }

    #[test]
    fn embedded_gallery_is_non_empty_and_valid() {
        assert!(
            !EMBEDDED_GALLERY.is_empty(),
            "the gallery must ship inside the binary"
        );
        for (stem, content) in EMBEDDED_GALLERY {
            assert!(
                content.contains("[workflow]"),
                "embedded template {stem} has no [workflow] section"
            );
            assert!(
                !parse_template_header(content).0.is_empty(),
                "embedded template {stem} has no title comment"
            );
        }
    }

    #[test]
    fn embedded_gallery_matches_disk_gallery() {
        // Drift guard: the embedded gallery must stay in sync with
        // examples/gallery/ (the canonical source the docs and tests use).
        let disk_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("examples")
            .join("gallery");
        let mut disk_stems: Vec<String> = std::fs::read_dir(&disk_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "oxoflow"))
            .map(|p| {
                p.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        disk_stems.sort();

        let mut embedded_stems: Vec<String> = EMBEDDED_GALLERY
            .iter()
            .map(|(s, _)| s.to_string())
            .collect();
        embedded_stems.sort();

        assert_eq!(
            embedded_stems, disk_stems,
            "embedded gallery diverged from examples/gallery/ — add or remove \
             include_str! entries in EMBEDDED_GALLERY"
        );

        // Same content check: a rebuilt binary serves the same files.
        for (stem, content) in embedded() {
            let disk = std::fs::read_to_string(disk_dir.join(format!("{stem}.oxoflow"))).unwrap();
            assert_eq!(content, disk, "embedded content for {stem} diverged");
        }
    }

    #[test]
    fn apply_template_embedded_matches_by_stem_and_suffix() {
        // The same matching rules as the old filesystem scan: exact stem or
        // `_<name>` suffix.
        let gallery: Vec<(&str, &str)> = EMBEDDED_GALLERY.iter().map(|(s, c)| (*s, *c)).collect();
        let exact = find_embedded_template(&gallery, "03_parallel_samples");
        assert_eq!(exact.unwrap().0, "03_parallel_samples");
        let by_suffix = find_embedded_template(&gallery, "parallel_samples");
        assert_eq!(by_suffix.unwrap().0, "03_parallel_samples");
        assert!(find_embedded_template(&gallery, "no_such_template").is_none());
    }
}
