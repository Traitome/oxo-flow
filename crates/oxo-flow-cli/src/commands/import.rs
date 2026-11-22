/// CLI command: oxo-flow import <source> [--from nextflow|snakemake] [-o <output>] [--json]
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::transpile::{SourceFormat, transpile_with_warnings};
use serde::Serialize;
use std::fs;
use std::path::Path;

/// One translation warning in the `import --json` document.
///
/// The transpiler surfaces warnings as free-text strings with no embedded
/// diagnostic code, so the object carries only `message`. A `code` field is
/// deliberately absent rather than invented (no fabricated codes); a consumer
/// can add one when the transpiler grows stable codes.
#[derive(Serialize)]
struct JsonWarning<'a> {
    message: &'a str,
}

/// The `import --json` document: one machine-readable description of a
/// translation. Field order is fixed by declaration here (serde keeps struct
/// field order), so two runs over the same source emit byte-identical JSON.
#[derive(Serialize)]
struct ImportJson<'a> {
    tool: &'static str,
    source: String,
    format: &'static str,
    accepted: bool,
    warnings: Vec<JsonWarning<'a>>,
    toml: &'a str,
}

/// Handle `oxo-flow import` command.
///
/// With `--json`, one machine-readable document is emitted on stdout
/// describing the translation: the tool name, source path, source format,
/// whether every construct was mapped (`accepted`), the ordered list of
/// translation warnings, and the generated TOML. Colored human text is
/// suppressed and the raw TOML is not also dumped — stdout stays a single
/// JSON document. The non-JSON path is byte-identical to before (warnings on
/// stderr, raw TOML on stdout), and the exit code is unchanged in both modes.
pub fn handle_import(
    source_path: &Path,
    from: Option<SourceFormat>,
    output: Option<&Path>,
    json: bool,
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

    let transpiled = transpile_with_warnings(&source_content, format)
        .with_context(|| format!("failed to transpile '{}'", source_path.display()))?;

    // `--json` (issue #1332): emit ONE machine-readable document on stdout so
    // CI and tooling can consume a structured "what was accepted / what was
    // lost" verdict instead of regexing colored stderr. The human path is
    // skipped entirely (no colored warnings, no raw TOML echo) so stdout stays
    // pure JSON; the exit code semantics are unchanged.
    if json {
        return emit_import_json(source_path, format, &transpiled, output);
    }

    // Translation losses are loud, not silent: a construct the importer could
    // not map is reported on stderr before the workflow is written.
    for warning in &transpiled.warnings {
        eprintln!("{} {}", "warning:".yellow().bold(), warning);
    }

    // Validate generated workflow TOML with oxo-flow parser
    oxo_flow_core::config::WorkflowConfig::parse(&transpiled.toml)
        .with_context(|| "generated workflow failed oxo-flow parse validation")?;

    match output {
        Some(out_path) => {
            // Same "generators never clobber" convention as init
            // (--force) and `report --init-template`: silently overwriting
            // a hand-edited pipeline destroys work with no undo.
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
            fs::write(out_path, &transpiled.toml)
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
            println!("{}", transpiled.toml);
        }
    }

    Ok(())
}

/// Emit the machine-readable import document (`import --json`, issue #1332).
///
/// Shape (stable, struct-ordered):
///
/// ```json
/// {
///   "tool": "oxo-flow import",
///   "source": "main.nf",
///   "format": "nextflow",
///   "accepted": true,
///   "warnings": [ { "message": "..." } ],
///   "toml": "[[rules]]\n..."
/// }
/// ```
///
/// `accepted` is `warnings.is_empty()`: a warning means a source construct was
/// not mapped faithfully. A consumer that later wants codes can extend the
/// warning object; today the transpiler emits free-text only, so no code is
/// fabricated. When `-o` was given, the file is written exactly as in the
/// human path (same refusal to clobber, same directory creation) — the document
/// is still emitted, so machine consumers need not read the filesystem.
fn emit_import_json(
    source_path: &Path,
    format: SourceFormat,
    transpiled: &oxo_flow_core::transpile::TranspileOutput,
    output: Option<&Path>,
) -> Result<()> {
    // Persist the workflow first when `-o` is set: a failure to write is a hard
    // error and must not be masked by a success-looking JSON document.
    if let Some(out_path) = output {
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
        fs::write(out_path, &transpiled.toml)
            .with_context(|| format!("failed to write output to '{}'", out_path.display()))?;
    }

    let document = ImportJson {
        tool: "oxo-flow import",
        source: source_path.display().to_string(),
        format: match format {
            SourceFormat::Nextflow => "nextflow",
            SourceFormat::Snakemake => "snakemake",
        },
        accepted: transpiled.warnings.is_empty(),
        warnings: transpiled
            .warnings
            .iter()
            .map(|message| JsonWarning { message })
            .collect(),
        toml: &transpiled.toml,
    };
    println!("{}", serde_json::to_string_pretty(&document)?);
    Ok(())
}
