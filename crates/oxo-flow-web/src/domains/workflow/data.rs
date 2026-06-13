//! Data discovery service — deterministic file format detection
//! and reference genome component discovery.
//!
//! Zero AI. 100% deterministic via extension matching and path traversal.

use crate::domains::observability::types::*;

/// Known bioinformatics file formats and their extensions.
static FORMAT_MAP: &[(&str, &str)] = &[
    (".fastq.gz", "FASTQ (gzipped)"),
    (".fq.gz", "FASTQ (gzipped)"),
    (".fastq", "FASTQ"),
    (".fq", "FASTQ"),
    (".bam", "BAM"),
    (".cram", "CRAM"),
    (".vcf.gz", "VCF (gzipped)"),
    (".vcf", "VCF"),
    (".bcf", "BCF"),
    (".bed", "BED"),
    (".gff", "GFF"),
    (".gtf", "GTF"),
    (".sam", "SAM"),
    (".bw", "BigWig"),
    (".bigwig", "BigWig"),
    (".tsv", "TSV"),
    (".csv", "CSV"),
    (".txt", "Text"),
];

/// Analyze files at given paths.
///
/// Detects formats via extension matching, identifies paired-end naming
/// patterns, and suggests an appropriate workflow template.
pub fn analyze_files(paths: &[String], _max_depth: Option<usize>) -> Result<DataAnalysisResponse, String> {
    let mut files = Vec::new();
    let mut formats = std::collections::HashSet::new();

    for pattern in paths {
        let path = std::path::Path::new(pattern);
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let (format_name, confidence) = FORMAT_MAP
            .iter()
            .find(|(ext, _)| filename.ends_with(ext))
            .map(|(_, name)| (name.to_string(), 0.95))
            .unwrap_or_else(|| ("unknown".to_string(), 0.0));

        formats.insert(format_name.clone());

        // Detect paired-end naming conventions
        let is_r1 = filename.contains("_R1") || filename.contains("_1.");
        let is_r2 = filename.contains("_R2") || filename.contains("_2.");

        // Extract sample name (everything before the first underscore)
        let sample_name = filename
            .split('_')
            .next()
            .map(|s| s.to_string());

        files.push(FileInfo {
            path: pattern.clone(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            format: format_name,
            format_confidence: confidence,
            paired_with: None,
            sample_name,
        });

        // Suppress unused warning
        let _ = (is_r1, is_r2);
    }

    let paired_end = files
        .iter()
        .any(|f| f.path.contains("_R1") || f.path.contains("_R2")
            || f.path.contains("_1.") || f.path.contains("_2."));

    let total_size: u64 = files.iter().map(|f| f.size).sum();

    let suggested = if formats.iter().any(|f| f.contains("FASTQ")) {
        Some(WorkflowSuggestion {
            template: if paired_end {
                "rnaseq-pe".into()
            } else {
                "qc".into()
            },
            confidence: if paired_end { 0.85 } else { 0.6 },
            reason: format!(
                "Detected FASTQ files{}",
                if paired_end {
                    " with paired-end naming"
                } else {
                    ""
                }
            ),
        })
    } else if formats.contains("BAM") {
        Some(WorkflowSuggestion {
            template: "variant-calling".into(),
            confidence: 0.7,
            reason: "BAM files detected".into(),
        })
    } else {
        None
    };

    Ok(DataAnalysisResponse {
        files,
        summary: DataSummary {
            total_size,
            formats_detected: formats.into_iter().collect(),
            paired_end_detected: paired_end,
            strand_specific: None,
        },
        suggested_workflow: suggested,
    })
}

/// Discover reference genome components for a given genome build.
///
/// Searches common reference directories and reports which components
/// are available and which are missing.
pub fn discover_reference(genome: &str, components: &[String]) -> Result<ReferenceResponse, String> {
    let search_dirs = [
        format!("/data/references/{genome}"),
        format!("/reference/{genome}"),
        format!("./reference/{genome}"),
    ];

    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut download_cmds = Vec::new();

    for comp in components {
        let basename = match comp.as_str() {
            "fasta" => format!("{genome}.fa"),
            "gtf" => format!("{genome}.gtf"),
            "star_index" => format!("{genome}_star_index"),
            "bowtie2_index" => format!("{genome}_bowtie2_index"),
            "bwa_index" => format!("{genome}_bwa_index"),
            other => other.to_string(),
        };

        let exists = search_dirs
            .iter()
            .any(|dir| std::path::Path::new(&format!("{dir}/{basename}")).exists());

        if exists {
            found.push(comp.clone());
        } else {
            missing.push(comp.clone());
            match comp.as_str() {
                "fasta" => download_cmds.push(format!(
                    "wget ftp://ftp.ensembl.org/pub/release-110/fasta/homo_sapiens/dna/Homo_sapiens.{genome}.dna.primary_assembly.fa.gz"
                )),
                "gtf" => download_cmds.push(format!(
                    "wget ftp://ftp.ensembl.org/pub/release-110/gtf/homo_sapiens/Homo_sapiens.{genome}.110.gtf.gz"
                )),
                _ => {}
            }
        }
    }

    Ok(ReferenceResponse {
        found,
        missing,
        download_commands: download_cmds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_format() {
        let result = analyze_files(&["/tmp/foo.xyz".into()], None).unwrap();
        assert_eq!(result.files[0].format, "unknown");
        assert_eq!(result.files[0].format_confidence, 0.0);
    }

    #[test]
    fn test_fastq_gz_detection() {
        let result = analyze_files(&["/data/sample_R1.fastq.gz".into()], None).unwrap();
        assert!(result.files[0].format.contains("FASTQ"));
        assert!(result.files[0].format_confidence > 0.9);
        assert!(result.summary.paired_end_detected);
    }

    #[test]
    fn test_reference_discovery() {
        let result = discover_reference(
            "hg38",
            &["fasta".into(), "gtf".into(), "star_index".into()],
        )
        .unwrap();
        // Most components will be missing in CI, but the function should not error
        assert_eq!(result.found.len() + result.missing.len(), 3);
    }
}
