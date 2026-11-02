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

/// Transpile external workflow DSL source code into an oxo-flow TOML string.
pub fn transpile(source: &str, format: SourceFormat) -> Result<String> {
    match format {
        SourceFormat::Nextflow => nextflow::transpile_nextflow(source),
        SourceFormat::Snakemake => snakemake::transpile_snakemake(source),
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
                tuple val(meta), path("*.bam"), emit: bam

                script:
                """
                bwa mem -t $task.cpus $reads > ${meta.id}.bam
                """
            }
        "#;
        let toml_res = transpile(nf, SourceFormat::Nextflow);
        assert!(toml_res.is_ok());
        let toml_str = toml_res.unwrap();
        assert!(toml_str.contains("[[rules]]"));
        assert!(toml_str.contains("name = \"bwa_mem\""));
        assert!(toml_str.contains("threads = 8"));
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
        let toml_res = transpile(smk, SourceFormat::Snakemake);
        assert!(toml_res.is_ok());
        let toml_str = toml_res.unwrap();
        assert!(toml_str.contains("[[rules]]"));
        assert!(toml_str.contains("name = \"bwa_mem\""));
        assert!(toml_str.contains("threads = 4"));
        assert!(toml_str.contains("resources = { memory = \"8G\" }"));
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
