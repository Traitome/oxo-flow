/// CLI command: oxo-flow import <source> [--from nextflow|snakemake] [-o <output>]
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::transpile::{SourceFormat, transpile};
use std::fs;
use std::path::Path;

/// Handle `oxo-flow import` command.
pub fn handle_import(
    source_path: &Path,
    from: Option<SourceFormat>,
    output: Option<&Path>,
) -> Result<()> {
    let source_content = fs::read_to_string(source_path)
        .with_context(|| format!("failed to read source file '{}'", source_path.display()))?;

    let format = match from {
        Some(f) => f,
        None => SourceFormat::detect_from_path(source_path)
            .or_else(|| SourceFormat::detect_from_content(&source_content))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "could not auto-detect workflow format for '{}'; please specify --from nextflow or --from snakemake",
                    source_path.display()
                )
            })?,
    };

    let transpiled = transpile(&source_content, format)
        .with_context(|| format!("failed to transpile '{}'", source_path.display()))?;

    // Validate generated workflow TOML with oxo-flow parser
    oxo_flow_core::config::WorkflowConfig::parse(&transpiled)
        .with_context(|| "generated workflow failed oxo-flow parse validation")?;

    match output {
        Some(out_path) => {
            if let Some(parent) = out_path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)?;
            }
            fs::write(out_path, &transpiled)
                .with_context(|| format!("failed to write output to '{}'", out_path.display()))?;
            eprintln!(
                "{} Transpiled {} ({:?}) -> {}",
                "✓".green().bold(),
                source_path.display(),
                format,
                out_path.display().to_string().cyan()
            );
        }
        None => {
            println!("{transpiled}");
        }
    }

    Ok(())
}
