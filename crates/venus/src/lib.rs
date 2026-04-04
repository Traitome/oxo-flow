//! Venus — Clinical-grade tumor variant detection pipeline.
//!
//! Venus (启明星) is an end-to-end tumor mutation detection, annotation,
//! and clinical reporting pipeline built on the oxo-flow engine.
//!
//! ## Supported Scenarios
//!
//! 1. **Tumor-only**: Somatic variant calling without a matched normal
//! 2. **Normal-only**: Germline variant calling
//! 3. **Tumor-Normal paired**: Somatic variant calling with matched normal control
//!
//! ## Pipeline Steps
//!
//! - FASTQ quality control (fastp/FastQC)
//! - Read alignment (BWA-MEM2)
//! - Duplicate marking (GATK MarkDuplicates)
//! - Base quality recalibration (GATK BQSR)
//! - Variant calling (GATK HaplotypeCaller, Mutect2, Strelka2, VarDict)
//! - Variant annotation (VEP, SnpEff, ClinVar, COSMIC)
//! - Clinical report generation

use serde::{Deserialize, Serialize};

/// Analysis mode for the Venus pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisMode {
    /// Tumor sample only (no matched normal).
    TumorOnly,
    /// Normal/germline sample only.
    NormalOnly,
    /// Paired tumor-normal analysis.
    TumorNormal,
}

impl std::fmt::Display for AnalysisMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TumorOnly => write!(f, "tumor-only"),
            Self::NormalOnly => write!(f, "normal-only"),
            Self::TumorNormal => write!(f, "tumor-normal"),
        }
    }
}

/// Sequencing type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeqType {
    /// Whole genome sequencing.
    WGS,
    /// Whole exome sequencing.
    WES,
    /// Targeted panel sequencing.
    Panel,
}

impl std::fmt::Display for SeqType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WGS => write!(f, "WGS"),
            Self::WES => write!(f, "WES"),
            Self::Panel => write!(f, "Panel"),
        }
    }
}

/// Reference genome build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenomeBuild {
    GRCh37,
    GRCh38,
}

impl std::fmt::Display for GenomeBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GRCh37 => write!(f, "GRCh37"),
            Self::GRCh38 => write!(f, "GRCh38"),
        }
    }
}

/// Sample information for the Venus pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Sample name/identifier.
    pub name: String,

    /// Path to R1 FASTQ file.
    pub r1_fastq: String,

    /// Path to R2 FASTQ file (optional for single-end).
    pub r2_fastq: Option<String>,

    /// Whether this is a tumor sample.
    pub is_tumor: bool,
}

/// Venus pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenusConfig {
    /// Analysis mode.
    pub mode: AnalysisMode,

    /// Sequencing type.
    pub seq_type: SeqType,

    /// Reference genome build.
    pub genome_build: GenomeBuild,

    /// Path to reference genome FASTA.
    pub reference_fasta: String,

    /// Tumor samples.
    #[serde(default)]
    pub tumor_samples: Vec<Sample>,

    /// Normal samples.
    #[serde(default)]
    pub normal_samples: Vec<Sample>,

    /// Target BED file (for WES/Panel).
    pub target_bed: Option<String>,

    /// Maximum threads per job.
    #[serde(default = "default_threads")]
    pub threads: u32,

    /// Output directory.
    #[serde(default = "default_output_dir")]
    pub output_dir: String,

    /// Enable variant annotation.
    #[serde(default = "default_true")]
    pub annotate: bool,

    /// Generate clinical report.
    #[serde(default = "default_true")]
    pub report: bool,

    /// Project name for report headers.
    pub project_name: Option<String>,
}

fn default_threads() -> u32 {
    8
}

fn default_output_dir() -> String {
    "venus_output".to_string()
}

fn default_true() -> bool {
    true
}

impl VenusConfig {
    /// Validate the Venus configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate analysis mode vs sample availability
        match self.mode {
            AnalysisMode::TumorOnly => {
                if self.tumor_samples.is_empty() {
                    anyhow::bail!("tumor-only mode requires at least one tumor sample");
                }
            }
            AnalysisMode::NormalOnly => {
                if self.normal_samples.is_empty() {
                    anyhow::bail!("normal-only mode requires at least one normal sample");
                }
            }
            AnalysisMode::TumorNormal => {
                if self.tumor_samples.is_empty() || self.normal_samples.is_empty() {
                    anyhow::bail!("tumor-normal mode requires both tumor and normal samples");
                }
            }
        }

        // Validate target BED for WES/Panel
        if matches!(self.seq_type, SeqType::WES | SeqType::Panel) && self.target_bed.is_none() {
            anyhow::bail!("WES/Panel sequencing requires a target BED file");
        }

        Ok(())
    }

    /// Returns all samples (tumor + normal).
    pub fn all_samples(&self) -> Vec<&Sample> {
        self.tumor_samples
            .iter()
            .chain(self.normal_samples.iter())
            .collect()
    }
}

/// Pipeline step identifiers for Venus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenusStep {
    /// FASTQ quality control.
    Fastp,
    /// Read alignment.
    BwaMem2,
    /// Duplicate marking.
    MarkDuplicates,
    /// Base quality recalibration.
    Bqsr,
    /// Germline variant calling.
    HaplotypeCaller,
    /// Somatic variant calling (Mutect2).
    Mutect2,
    /// Somatic variant calling (Strelka2).
    Strelka2,
    /// Variant annotation (VEP).
    Vep,
    /// Clinical report generation.
    ClinicalReport,
}

impl std::fmt::Display for VenusStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fastp => write!(f, "fastp"),
            Self::BwaMem2 => write!(f, "bwa_mem2"),
            Self::MarkDuplicates => write!(f, "mark_duplicates"),
            Self::Bqsr => write!(f, "bqsr"),
            Self::HaplotypeCaller => write!(f, "haplotype_caller"),
            Self::Mutect2 => write!(f, "mutect2"),
            Self::Strelka2 => write!(f, "strelka2"),
            Self::Vep => write!(f, "vep"),
            Self::ClinicalReport => write!(f, "clinical_report"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_mode_display() {
        assert_eq!(AnalysisMode::TumorOnly.to_string(), "tumor-only");
        assert_eq!(AnalysisMode::NormalOnly.to_string(), "normal-only");
        assert_eq!(AnalysisMode::TumorNormal.to_string(), "tumor-normal");
    }

    #[test]
    fn seq_type_display() {
        assert_eq!(SeqType::WGS.to_string(), "WGS");
        assert_eq!(SeqType::WES.to_string(), "WES");
        assert_eq!(SeqType::Panel.to_string(), "Panel");
    }

    #[test]
    fn genome_build_display() {
        assert_eq!(GenomeBuild::GRCh37.to_string(), "GRCh37");
        assert_eq!(GenomeBuild::GRCh38.to_string(), "GRCh38");
    }

    #[test]
    fn venus_step_display() {
        assert_eq!(VenusStep::Fastp.to_string(), "fastp");
        assert_eq!(VenusStep::Mutect2.to_string(), "mutect2");
        assert_eq!(VenusStep::ClinicalReport.to_string(), "clinical_report");
    }

    #[test]
    fn validate_tumor_only() {
        let config = VenusConfig {
            mode: AnalysisMode::TumorOnly,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            tumor_samples: vec![Sample {
                name: "TUMOR_01".to_string(),
                r1_fastq: "tumor_R1.fq.gz".to_string(),
                r2_fastq: Some("tumor_R2.fq.gz".to_string()),
                is_tumor: true,
            }],
            normal_samples: vec![],
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: true,
            report: true,
            project_name: Some("Test".to_string()),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_tumor_only_no_samples() {
        let config = VenusConfig {
            mode: AnalysisMode::TumorOnly,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            tumor_samples: vec![],
            normal_samples: vec![],
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: true,
            report: true,
            project_name: None,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_wes_needs_bed() {
        let config = VenusConfig {
            mode: AnalysisMode::TumorOnly,
            seq_type: SeqType::WES,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            tumor_samples: vec![Sample {
                name: "T".to_string(),
                r1_fastq: "t_R1.fq.gz".to_string(),
                r2_fastq: None,
                is_tumor: true,
            }],
            normal_samples: vec![],
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: true,
            report: true,
            project_name: None,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn all_samples() {
        let config = VenusConfig {
            mode: AnalysisMode::TumorNormal,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            tumor_samples: vec![Sample {
                name: "T".to_string(),
                r1_fastq: "t.fq.gz".to_string(),
                r2_fastq: None,
                is_tumor: true,
            }],
            normal_samples: vec![Sample {
                name: "N".to_string(),
                r1_fastq: "n.fq.gz".to_string(),
                r2_fastq: None,
                is_tumor: false,
            }],
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: true,
            report: true,
            project_name: None,
        };

        assert_eq!(config.all_samples().len(), 2);
    }
}
