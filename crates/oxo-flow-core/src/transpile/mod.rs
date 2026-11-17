//! Workflow transpilation modules for Nextflow and Snakemake conversion.

pub mod nextflow;
pub mod snakemake;

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Source workflow format to transpile from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    /// Nextflow DSL2 process definitions.
    Nextflow,
    /// Snakemake rule definitions.
    Snakemake,
}

impl SourceFormat {
    /// Detect format from file path or file extension.
    #[must_use]
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy();
        if name == "Snakefile" || name.ends_with(".smk") {
            Some(Self::Snakemake)
        } else if name.ends_with(".nf") || name == "main.nf" {
            Some(Self::Nextflow)
        } else {
            None
        }
    }

    /// Detect format by inspecting content.
    #[must_use]
    pub fn detect_from_content(content: &str) -> Option<Self> {
        if content.contains("process ")
            && (content.contains("input:") || content.contains("output:"))
        {
            Some(Self::Nextflow)
        } else if content.contains("rule ")
            && (content.contains("input:") || content.contains("shell:"))
        {
            Some(Self::Snakemake)
        } else {
            None
        }
    }
}

/// Result of a transpilation: the generated workflow plus every non-fatal
/// translation warning (constructs the importer could not map faithfully).
#[derive(Debug, Clone, Default)]
pub struct TranspileOutput {
    /// Generated `.oxoflow` TOML.
    pub toml: String,
    /// Human-readable warnings to surface alongside the import.
    pub warnings: Vec<String>,
}

/// Transpile external workflow DSL source code into an oxo-flow TOML string.
pub fn transpile(source: &str, format: SourceFormat) -> Result<String> {
    transpile_with_warnings(source, format).map(|output| output.toml)
}

/// Transpile external workflow DSL source code, returning the TOML and every
/// non-fatal translation warning. `transpile` is the warning-free wrapper for
/// callers that only need the generated workflow.
///
/// The generated workflow is parsed and statically validated before it is
/// returned: an importer defect that would emit an unrunnable workflow fails
/// here instead of producing a file that only breaks at run time.
pub fn transpile_with_warnings(source: &str, format: SourceFormat) -> Result<TranspileOutput> {
    let (toml, warnings) = match format {
        SourceFormat::Nextflow => nextflow::transpile_nextflow_with_warnings(source)?,
        SourceFormat::Snakemake => snakemake::transpile_snakemake_with_warnings(source)?,
    };
    ensure_generated_workflow_is_valid(&toml, format)?;
    Ok(TranspileOutput { toml, warnings })
}

/// Parse + statically validate the generated `.oxoflow` document.
///
/// This is the last line of defence behind the per-construct checks: if the
/// emitter ever produces TOML the engine cannot parse or validate, the import
/// fails loudly instead of writing a workflow that is broken at run time.
fn ensure_generated_workflow_is_valid(toml: &str, format: SourceFormat) -> Result<()> {
    let config = crate::config::WorkflowConfig::parse(toml).map_err(|e| {
        crate::error::OxoFlowError::Config {
            message: format!(
                "the importer generated TOML that oxo-flow cannot parse ({e}); this is an \
                 importer defect, not a problem with your source workflow — please report it \
                 with the source file"
            ),
        }
    })?;
    let validation = crate::format::validate_format(&config);
    let errors: Vec<String> = validation
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == crate::format::Severity::Error)
        .map(|diagnostic| {
            let rule = diagnostic
                .rule
                .as_deref()
                .map(|rule| format!(" (rule '{rule}')"))
                .unwrap_or_default();
            format!("[{}]{rule}: {}", diagnostic.code, diagnostic.message)
        })
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    Err(crate::error::OxoFlowError::Config {
        message: format!(
            "the importer generated a workflow that fails oxo-flow validation ({} source); \
             refusing to emit an unrunnable pipeline:\n  - {}\nThis is an importer defect — \
             please report it with the source file",
            match format {
                SourceFormat::Nextflow => "Nextflow",
                SourceFormat::Snakemake => "Snakemake",
            },
            errors.join("\n  - ")
        ),
    })
}

/// Escape a string for use inside a TOML basic (`"…"`) string.
///
/// Shared single point of sync: both transpilers emit source-derived text
/// (paths, commands, environment specs) into TOML, and an unescaped quote,
/// backslash, or control character would corrupt the document or silently
/// change the emitted value.
pub(crate) fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Render a `shell = …` line. Multi-line bodies use a TOML literal string so
/// shell backslashes stay verbatim; a body that itself contains `'''` falls
/// back to an escaped basic string, which is always valid.
pub(crate) fn render_shell(shell: &str) -> String {
    let shell = shell.trim();
    if shell.contains('\n') && !shell.contains("'''") {
        format!("shell = '''\n{shell}\n'''\n")
    } else {
        format!("shell = \"{}\"\n", escape_toml_basic(shell))
    }
}

/// Normalize a memory string from either DSL to the engine's `N G` / `N M` form.
///
/// Shared single point of sync: both transpilers must canonicalize `mem` the
/// same way (`GB`/`gb` → `G`, `MB`/`mb` → `M`, whitespace stripped), or the
/// emitted `[resources]` sections drift apart.
pub(crate) fn normalize_memory(raw: &str) -> String {
    let s = raw.trim().replace(" ", "");
    if s.ends_with("GB") || s.ends_with("gb") {
        format!("{}G", s[..s.len() - 2].trim())
    } else if s.ends_with("MB") || s.ends_with("mb") {
        format!("{}M", s[..s.len() - 2].trim())
    } else {
        s
    }
}

/// Infer BioType annotations based on tool name and file extensions.
///
/// Shared single point of sync for both transpilers: the tool-name branches
/// and extension fallbacks must classify identically in the Nextflow and
/// Snakemake emitters, or the two DSLs produce divergent `types` contracts
/// for the same pipeline. The returned names must match the `[types.*]`
/// catalog tables each backend emits (`fastq` / `bam_coord` / `vcf_indexed`).
pub(crate) fn infer_bio_types(
    name: &str,
    inputs: &[String],
    outputs: &[String],
) -> (Option<&'static str>, Option<&'static str>) {
    let lower = name.to_lowercase();
    let has_fastq_in = inputs
        .iter()
        .any(|i| i.contains(".fastq") || i.contains(".fq"));
    let has_bam_out = outputs.iter().any(|o| o.contains(".bam"));
    let has_vcf_out = outputs.iter().any(|o| o.contains(".vcf"));

    if lower.contains("bwa")
        || lower.contains("bowtie")
        || lower.contains("hisat")
        || lower.contains("star")
        || lower.contains("minimap")
    {
        (Some("fastq"), Some("bam_coord"))
    } else if lower.contains("fastqc") {
        (Some("fastq"), None)
    } else if lower.contains("sort") {
        (Some("bam_coord"), Some("bam_coord"))
    } else if lower.contains("call")
        || lower.contains("mutect")
        || lower.contains("haplotype")
        || lower.contains("deepvariant")
        || lower.contains("strelka")
    {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else if has_fastq_in && has_bam_out {
        (Some("fastq"), Some("bam_coord"))
    } else if has_bam_out && has_vcf_out {
        (Some("bam_coord"), Some("vcf_indexed"))
    } else {
        (None, None)
    }
}

/// Emit a rule-level `types = { inputs = {...}, outputs = {...} }` contract,
/// rendering each side independently. Single-sided contracts (e.g. a QC rule
/// that only types its fastq input) are valid `.oxoflow`
/// ([`crate::types::RuleTypesSpec`] skips empty maps) and round-trip through
/// the parser, so a side is dropped only when the type is unknown or the
/// channel list is empty — never defaulted to an empty channel name. Shared
/// single point of sync for both transpilers (DRY).
pub(crate) fn push_types_contract(
    toml: &mut String,
    inputs: Option<(&str, &str)>,
    outputs: Option<(&str, &str)>,
) {
    let mut parts = Vec::new();
    if let Some((channel, ty)) = inputs {
        parts.push(format!("inputs = {{ \"{channel}\" = \"{ty}\" }}"));
    }
    if let Some((channel, ty)) = outputs {
        parts.push(format!("outputs = {{ \"{channel}\" = \"{ty}\" }}"));
    }
    if parts.is_empty() {
        return;
    }
    toml.push_str(&format!("types = {{ {} }}\n", parts.join(", ")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_detection_from_path() {
        assert_eq!(
            SourceFormat::detect_from_path(Path::new("main.nf")),
            Some(SourceFormat::Nextflow)
        );
        assert_eq!(
            SourceFormat::detect_from_path(Path::new("modules/bwa.nf")),
            Some(SourceFormat::Nextflow)
        );
        assert_eq!(
            SourceFormat::detect_from_path(Path::new("Snakefile")),
            Some(SourceFormat::Snakemake)
        );
        assert_eq!(
            SourceFormat::detect_from_path(Path::new("rules/align.smk")),
            Some(SourceFormat::Snakemake)
        );
        assert_eq!(
            SourceFormat::detect_from_path(Path::new("workflow.oxoflow")),
            None
        );
    }

    #[test]
    fn test_format_detection_from_content() {
        let nf_snippet =
            "process FASTQC {\n  input:\n  path reads\n  output:\n  path \"*.html\"\n}";
        assert_eq!(
            SourceFormat::detect_from_content(nf_snippet),
            Some(SourceFormat::Nextflow)
        );

        let smk_snippet = "rule fastqc:\n    input:\n        \"data/{sample}.fastq\"\n    shell:\n        \"fastqc {input}\"";
        assert_eq!(
            SourceFormat::detect_from_content(smk_snippet),
            Some(SourceFormat::Snakemake)
        );
    }

    #[test]
    fn test_transpile_nextflow_simple() {
        let nf = r#"
            process BWA_MEM {
                cpus 8
                memory '16 GB'
                conda 'bioconda::bwa=0.7.17'

                input:
                tuple val(meta), path(reads)

                output:
                tuple val(meta), path("${reads}.bam"), emit: bam

                script:
                """
                bwa mem -t $task.cpus $reads > ${meta.id}.bam
                """
            }
            workflow { BWA_MEM(Channel.fromPath("data/*.fastq.gz")) }
        "#;
        let toml_res = transpile(nf, SourceFormat::Nextflow);
        assert!(toml_res.is_ok());
        let toml_str = toml_res.unwrap();
        assert!(toml_str.contains("[[rules]]"));
        assert!(toml_str.contains("name = \"bwa_mem\""));
        assert!(
            toml_str.contains("resources = { threads = 8, memory = \"16G\" }"),
            "threads/memory must use the canonical resources form (W025): {toml_str}"
        );
        assert!(toml_str.contains("bwa mem -t {threads} {input} > {sample}.bam"));
        assert!(toml_str.contains("types = {"));
    }

    #[test]
    fn test_transpile_snakemake_simple() {
        let smk = r#"
            rule bwa_mem:
                input:
                    "data/{sample}.fastq.gz"
                output:
                    "aligned/{sample}.bam"
                threads: 4
                resources:
                    mem_mb=8000
                conda:
                    "envs/bwa.yaml"
                shell:
                    "bwa mem -t {threads} {input} > {output}"
        "#;
        let toml_str = transpile(smk, SourceFormat::Snakemake).expect("snakemake transpile");
        assert!(toml_str.contains("[[rules]]"));
        assert!(toml_str.contains("name = \"bwa_mem\""));
        assert!(
            toml_str.contains("resources = { threads = 4, memory = \"8000M\" }"),
            "threads/memory must use the canonical resources form (W025): {toml_str}"
        );
        assert!(toml_str.contains("types = {"));
    }

    #[test]
    fn test_push_types_contract_single_sided() {
        // Both sides present.
        let mut toml = String::new();
        push_types_contract(
            &mut toml,
            Some(("reads", "fastq")),
            Some(("bam", "bam_coord")),
        );
        assert_eq!(
            toml,
            "types = { inputs = { \"reads\" = \"fastq\" }, outputs = { \"bam\" = \"bam_coord\" } }\n"
        );

        // Input-only contract (fastqc arm): outputs side omitted, no empty keys.
        let mut toml = String::new();
        push_types_contract(&mut toml, Some(("reads", "fastq")), None);
        assert_eq!(toml, "types = { inputs = { \"reads\" = \"fastq\" } }\n");

        // No inferable side: no line at all.
        let mut toml = String::new();
        push_types_contract(&mut toml, None, None);
        assert!(toml.is_empty());
    }
}
