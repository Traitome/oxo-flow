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
}
