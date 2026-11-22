//! CLI command: `oxo-flow export-dsl <workflow> [--format nextflow] [-o <output>] [--json]`
//!
//! The export half of the transpiler (issue #1319): emit a parsed
//! `.oxoflow` workflow back to a Nextflow DSL2 document over the same
//! documented subset the importer accepts. Anything outside the subset is a
//! hard error naming the construct — never silent loss — mirroring the
//! importer's policy.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::transpile::export_nextflow;
use serde::Serialize;
use std::fs;
use std::path::Path;

/// One export warning in the `export-dsl --json` document — the same
/// free-text shape `import --json` uses (no invented diagnostic codes).
#[derive(Serialize)]
struct JsonWarning<'a> {
    message: &'a str,
}

/// The `export-dsl --json` document. Field order is fixed by declaration
/// here (serde keeps struct field order), so two runs over the same
/// workflow emit byte-identical JSON — the `import --json` precedent.
#[derive(Serialize)]
struct ExportDslJson<'a> {
    tool: &'static str,
    workflow: String,
    format: &'static str,
    accepted: bool,
    warnings: Vec<JsonWarning<'a>>,
    dsl: &'a str,
}

/// Handle `oxo-flow export-dsl`.
///
/// The workflow is parsed and its DAG built (cycle detection + the exact
/// producer edges the wiring emitter cross-checks) before any emission.
/// With `--json`, one machine-readable document is emitted on stdout —
/// stdout stays a single JSON document (colored human text suppressed).
/// The non-JSON path prints warnings on stderr and the raw DSL on stdout
/// (or writes `-o`), byte-identical in shape to `import`.
pub fn handle_export_dsl(
    workflow_path: &Path,
    format: Option<&str>,
    output: Option<&Path>,
    json: bool,
) -> Result<()> {
    let format = match format {
        None | Some("nextflow") | Some("nf") => "nextflow",
        Some(other) => {
            anyhow::bail!(
                "export format '{other}' is not implemented — the exporter covers Nextflow \
                 DSL2 only (Snakemake/CWL/WDL export does not exist; refusing to guess)"
            )
        }
    };

    let config = oxo_flow_core::config::WorkflowConfig::from_file(workflow_path)
        .with_context(|| format!("failed to parse {}", workflow_path.display()))?;
    let dag = WorkflowDag::from_rules(&config.rules)
        .with_context(|| format!("failed to build the DAG for {}", workflow_path.display()))?;

    let (dsl, warnings) = export_nextflow(&config, &dag)
        .with_context(|| format!("failed to export {}", workflow_path.display()))?;

    if json {
        // Persist the DSL first when `-o` is set: a failure to write is a
        // hard error and must not be masked by a success-looking JSON
        // document (same reasoning as `import --json`).
        if let Some(out_path) = output {
            write_output(out_path, &dsl)?;
        }
        let document = ExportDslJson {
            tool: "oxo-flow export-dsl",
            workflow: workflow_path.display().to_string(),
            format,
            accepted: warnings.is_empty(),
            warnings: warnings
                .iter()
                .map(|message| JsonWarning { message })
                .collect(),
            dsl: &dsl,
        };
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    // Export losses that are survivable (types contracts re-derived on
    // re-import) are loud, not silent: reported on stderr before the DSL is
    // written. Hard incompatibilities already aborted above.
    for warning in &warnings {
        eprintln!("{} {}", "warning:".yellow().bold(), warning);
    }

    match output {
        Some(out_path) => {
            write_output(out_path, &dsl)?;
            eprintln!(
                "{} Exported {} (nextflow DSL2) -> {}",
                "✓".green().bold(),
                workflow_path.display(),
                out_path.display().to_string().cyan()
            );
        }
        None => {
            println!("{dsl}");
        }
    }

    Ok(())
}

/// Write the `-o` target with the same "generators never clobber"
/// convention as `import` (`init --force`, `report --init-template`):
/// silently overwriting a hand-edited file destroys work with no undo.
fn write_output(out_path: &Path, dsl: &str) -> Result<()> {
    if out_path.exists() {
        anyhow::bail!(
            "refusing to overwrite existing '{}' — move it away or pass a different -o path",
            out_path.display()
        );
    }
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(out_path, dsl)
        .with_context(|| format!("failed to write output to '{}'", out_path.display()))
}
