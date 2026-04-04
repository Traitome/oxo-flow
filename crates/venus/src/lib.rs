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

use std::collections::HashMap;

use oxo_flow_core::rule::{EnvironmentSpec, Resources, Rule};
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

    /// Known variants file (e.g., dbSNP) for BQSR.
    pub known_sites: Option<String>,

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
    /// Filter Mutect2 calls.
    FilterMutectCalls,
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
            Self::FilterMutectCalls => write!(f, "filter_mutect_calls"),
            Self::Strelka2 => write!(f, "strelka2"),
            Self::Vep => write!(f, "vep"),
            Self::ClinicalReport => write!(f, "clinical_report"),
        }
    }
}

/// Create a pipeline rule with common defaults.
fn make_rule(
    name: &str,
    input: Vec<String>,
    output: Vec<String>,
    shell: &str,
    threads: u32,
    memory: &str,
    env_conda: &str,
) -> Rule {
    Rule {
        name: name.to_string(),
        input,
        output,
        shell: Some(shell.to_string()),
        script: None,
        threads: Some(threads),
        memory: if memory.is_empty() {
            None
        } else {
            Some(memory.to_string())
        },
        resources: Resources::default(),
        environment: EnvironmentSpec {
            conda: if env_conda.is_empty() {
                None
            } else {
                Some(env_conda.to_string())
            },
            ..Default::default()
        },
        log: None,
        benchmark: None,
        params: HashMap::new(),
        priority: 0,
        target: false,
        group: None,
        description: None,
    }
}

/// Builds the Venus pipeline as a set of oxo-flow rules.
pub struct VenusPipelineBuilder {
    config: VenusConfig,
}

impl VenusPipelineBuilder {
    /// Create a new pipeline builder from a Venus configuration.
    pub fn new(config: VenusConfig) -> Self {
        Self { config }
    }

    /// Validate config, then generate all rules based on analysis mode.
    pub fn build(&self) -> anyhow::Result<Vec<Rule>> {
        self.config.validate()?;

        let mut rules = Vec::new();

        // Common preprocessing for all samples
        rules.extend(self.fastp_rules());
        rules.extend(self.bwa_mem2_rules());
        rules.extend(self.mark_duplicates_rules());
        rules.extend(self.bqsr_rules());

        // Mode-specific variant calling
        match self.config.mode {
            AnalysisMode::TumorOnly => {
                rules.extend(self.mutect2_rules());
                rules.extend(self.filter_mutect_calls_rules());
            }
            AnalysisMode::NormalOnly => {
                rules.extend(self.haplotype_caller_rules());
            }
            AnalysisMode::TumorNormal => {
                rules.extend(self.haplotype_caller_rules());
                rules.extend(self.mutect2_rules());
                rules.extend(self.filter_mutect_calls_rules());
                rules.extend(self.strelka2_rules());
            }
        }

        if self.config.annotate {
            rules.extend(self.annotation_rules());
        }

        if self.config.report {
            rules.extend(self.report_rules());
        }

        Ok(rules)
    }

    fn fastp_rules(&self) -> Vec<Rule> {
        self.config
            .all_samples()
            .into_iter()
            .map(|s| {
                make_rule(
                    &format!("fastp_{}", s.name),
                    vec![
                        format!("raw/{}_R1.fq.gz", s.name),
                        format!("raw/{}_R2.fq.gz", s.name),
                    ],
                    vec![
                        format!("trimmed/{}_R1.fq.gz", s.name),
                        format!("trimmed/{}_R2.fq.gz", s.name),
                        format!("qc/{}_fastp.json", s.name),
                    ],
                    "fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --json {output[2]} --thread {threads}",
                    8,
                    "",
                    "envs/fastp.yaml",
                )
            })
            .collect()
    }

    fn bwa_mem2_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        self.config
            .all_samples()
            .into_iter()
            .map(|s| {
                make_rule(
                    &format!("bwa_mem2_{}", s.name),
                    vec![
                        format!("trimmed/{}_R1.fq.gz", s.name),
                        format!("trimmed/{}_R2.fq.gz", s.name),
                    ],
                    vec![format!("aligned/{}.sorted.bam", s.name)],
                    &format!(
                        "bwa-mem2 mem -t {{threads}} {ref_path} {{input[0]}} {{input[1]}} | samtools sort -@ {{threads}} -o {{output[0]}}"
                    ),
                    16,
                    "32G",
                    "envs/bwa_mem2.yaml",
                )
            })
            .collect()
    }

    fn mark_duplicates_rules(&self) -> Vec<Rule> {
        self.config
            .all_samples()
            .into_iter()
            .map(|s| {
                make_rule(
                    &format!("mark_duplicates_{}", s.name),
                    vec![format!("aligned/{}.sorted.bam", s.name)],
                    vec![
                        format!("dedup/{}.dedup.bam", s.name),
                        format!("dedup/{}.metrics.txt", s.name),
                    ],
                    "gatk MarkDuplicates -I {input[0]} -O {output[0]} -M {output[1]}",
                    4,
                    "16G",
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn bqsr_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        let known_sites_flag = self
            .config
            .known_sites
            .as_deref()
            .map(|ks| format!(" --known-sites {ks}"))
            .unwrap_or_default();
        self.config
            .all_samples()
            .into_iter()
            .map(|s| {
                let name = &s.name;
                make_rule(
                    &format!("bqsr_{name}"),
                    vec![format!("dedup/{name}.dedup.bam")],
                    vec![format!("recal/{name}.recal.bam")],
                    &format!(
                        "gatk BaseRecalibrator -I {{input[0]}} -R {ref_path}{known_sites_flag} -O recal/{name}.recal.table && \
                         gatk ApplyBQSR -I {{input[0]}} -R {ref_path} --bqsr-recal-file recal/{name}.recal.table -O {{output[0]}}"
                    ),
                    4,
                    "16G",
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn haplotype_caller_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        self.config
            .normal_samples
            .iter()
            .map(|s| {
                make_rule(
                    &format!("haplotype_caller_{}", s.name),
                    vec![format!("recal/{}.recal.bam", s.name)],
                    vec![format!("variants/{}.g.vcf.gz", s.name)],
                    &format!(
                        "gatk HaplotypeCaller -I {{input[0]}} -R {ref_path} -O {{output[0]}} -ERC GVCF"
                    ),
                    4,
                    "16G",
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn mutect2_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        let normal_name = self.config.normal_samples.first().map(|s| s.name.as_str());
        self.config
            .tumor_samples
            .iter()
            .map(|s| {
                let (input, shell) = if let Some(normal) = normal_name {
                    (
                        vec![
                            format!("recal/{}.recal.bam", s.name),
                            format!("recal/{normal}.recal.bam"),
                        ],
                        format!(
                            "gatk Mutect2 -I {{input[0]}} -I {{input[1]}} -normal {normal} -R {ref_path} -O {{output[0]}}"
                        ),
                    )
                } else {
                    (
                        vec![format!("recal/{}.recal.bam", s.name)],
                        format!(
                            "gatk Mutect2 -I {{input[0]}} -R {ref_path} -O {{output[0]}}"
                        ),
                    )
                };
                make_rule(
                    &format!("mutect2_{}", s.name),
                    input,
                    vec![format!("variants/{}.mutect2.vcf.gz", s.name)],
                    &shell,
                    4,
                    "16G",
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn filter_mutect_calls_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        self.config
            .tumor_samples
            .iter()
            .map(|s| {
                make_rule(
                    &format!("filter_mutect_calls_{}", s.name),
                    vec![format!("variants/{}.mutect2.vcf.gz", s.name)],
                    vec![format!("variants/{}.mutect2.filtered.vcf.gz", s.name)],
                    &format!(
                        "gatk FilterMutectCalls -V {{input[0]}} -R {ref_path} -O {{output[0]}}"
                    ),
                    4,
                    "8G",
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn strelka2_rules(&self) -> Vec<Rule> {
        if self.config.mode != AnalysisMode::TumorNormal {
            return Vec::new();
        }
        let normal_name = match self.config.normal_samples.first() {
            Some(s) => &s.name,
            None => return Vec::new(),
        };
        let ref_path = &self.config.reference_fasta;
        self.config
            .tumor_samples
            .iter()
            .map(|s| {
                make_rule(
                    &format!("strelka2_{}", s.name),
                    vec![
                        format!("recal/{}.recal.bam", s.name),
                        format!("recal/{normal_name}.recal.bam"),
                    ],
                    vec![format!("variants/{}.strelka2.vcf.gz", s.name)],
                    &format!(
                        "configureStrelkaSomaticWorkflow.py --tumorBam {{input[0]}} --normalBam {{input[1]}} --referenceFasta {ref_path} --runDir strelka2_{tumor} && \
                         strelka2_{tumor}/runWorkflow.py -m local -j {{threads}} && \
                         cp strelka2_{tumor}/results/variants/somatic.snvs.vcf.gz {{output[0]}}",
                        tumor = s.name
                    ),
                    8,
                    "16G",
                    "envs/strelka2.yaml",
                )
            })
            .collect()
    }

    fn annotation_rules(&self) -> Vec<Rule> {
        self.called_samples()
            .into_iter()
            .map(|(sample, vcf_path)| {
                make_rule(
                    &format!("annotate_{}", sample.name),
                    vec![vcf_path],
                    vec![format!("annotated/{}.annotated.vcf.gz", sample.name)],
                    "vep --input_file {input[0]} --output_file {output[0]} --format vcf --vcf --offline --cache",
                    4,
                    "8G",
                    "envs/vep.yaml",
                )
            })
            .collect()
    }

    fn report_rules(&self) -> Vec<Rule> {
        self.called_samples()
            .into_iter()
            .map(|(sample, vcf_path)| {
                let input = if self.config.annotate {
                    format!("annotated/{}.annotated.vcf.gz", sample.name)
                } else {
                    vcf_path
                };
                let name = &sample.name;
                make_rule(
                    &format!("report_{name}"),
                    vec![input],
                    vec![format!("reports/{name}_clinical_report.html")],
                    &format!(
                        "python scripts/generate_report.py --input {{input[0]}} --output {{output[0]}} --sample {name}"
                    ),
                    1,
                    "4G",
                    "envs/report.yaml",
                )
            })
            .collect()
    }

    /// Returns samples that went through variant calling with their VCF paths.
    fn called_samples(&self) -> Vec<(&Sample, String)> {
        let mut result = Vec::new();
        match self.config.mode {
            AnalysisMode::TumorOnly => {
                for s in &self.config.tumor_samples {
                    result.push((s, format!("variants/{}.mutect2.filtered.vcf.gz", s.name)));
                }
            }
            AnalysisMode::NormalOnly => {
                for s in &self.config.normal_samples {
                    result.push((s, format!("variants/{}.g.vcf.gz", s.name)));
                }
            }
            AnalysisMode::TumorNormal => {
                for s in &self.config.tumor_samples {
                    result.push((s, format!("variants/{}.mutect2.filtered.vcf.gz", s.name)));
                }
                for s in &self.config.normal_samples {
                    result.push((s, format!("variants/{}.g.vcf.gz", s.name)));
                }
            }
        }
        result
    }
}

/// Generate a complete .oxoflow TOML string for the Venus pipeline.
pub fn generate_oxoflow(config: &VenusConfig) -> anyhow::Result<String> {
    let builder = VenusPipelineBuilder::new(config.clone());
    let rules = builder.build()?;

    let wf = oxo_flow_core::config::WorkflowConfig {
        workflow: oxo_flow_core::config::WorkflowMeta {
            name: config
                .project_name
                .clone()
                .unwrap_or_else(|| "venus-pipeline".to_string()),
            version: "0.1.0".to_string(),
            description: Some(format!(
                "Venus {} pipeline ({})",
                config.mode, config.seq_type
            )),
            author: None,
        },
        config: {
            let mut map = HashMap::new();
            map.insert(
                "reference_fasta".to_string(),
                toml::Value::String(config.reference_fasta.clone()),
            );
            map.insert(
                "genome_build".to_string(),
                toml::Value::String(config.genome_build.to_string()),
            );
            map
        },
        defaults: oxo_flow_core::config::Defaults {
            threads: Some(config.threads),
            memory: None,
            environment: None,
        },
        report: None,
        rules,
    };

    let toml_str = toml::to_string_pretty(&wf)?;
    Ok(toml_str)
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
            known_sites: None,
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
            known_sites: None,
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
            known_sites: None,
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
            known_sites: None,
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: true,
            report: true,
            project_name: None,
        };

        assert_eq!(config.all_samples().len(), 2);
    }

    fn tumor_sample(name: &str) -> Sample {
        Sample {
            name: name.to_string(),
            r1_fastq: format!("raw/{name}_R1.fq.gz"),
            r2_fastq: Some(format!("raw/{name}_R2.fq.gz")),
            is_tumor: true,
        }
    }

    fn normal_sample(name: &str) -> Sample {
        Sample {
            name: name.to_string(),
            r1_fastq: format!("raw/{name}_R1.fq.gz"),
            r2_fastq: Some(format!("raw/{name}_R2.fq.gz")),
            is_tumor: false,
        }
    }

    fn base_config(mode: AnalysisMode) -> VenusConfig {
        VenusConfig {
            mode,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            tumor_samples: vec![],
            normal_samples: vec![],
            known_sites: None,
            target_bed: None,
            threads: 8,
            output_dir: "output".to_string(),
            annotate: false,
            report: false,
            project_name: Some("TestProject".to_string()),
        }
    }

    #[test]
    fn build_tumor_only_pipeline() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];
        config.annotate = true;
        config.report = true;

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // fastp + bwa_mem2 + mark_dup + bqsr + mutect2 + filter_mutect_calls + annotate + report = 8
        assert_eq!(rules.len(), 8);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"fastp_T1"));
        assert!(names.contains(&"bwa_mem2_T1"));
        assert!(names.contains(&"mark_duplicates_T1"));
        assert!(names.contains(&"bqsr_T1"));
        assert!(names.contains(&"mutect2_T1"));
        assert!(names.contains(&"filter_mutect_calls_T1"));
        assert!(names.contains(&"annotate_T1"));
        assert!(names.contains(&"report_T1"));
        assert!(!names.iter().any(|n| n.starts_with("haplotype_caller")));
    }

    #[test]
    fn build_normal_only_pipeline() {
        let mut config = base_config(AnalysisMode::NormalOnly);
        config.normal_samples = vec![normal_sample("N1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // fastp + bwa_mem2 + mark_dup + bqsr + haplotype_caller = 5
        assert_eq!(rules.len(), 5);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"haplotype_caller_N1"));
        assert!(!names.iter().any(|n| n.starts_with("mutect2")));
    }

    #[test]
    fn build_tumor_normal_pipeline() {
        let mut config = base_config(AnalysisMode::TumorNormal);
        config.tumor_samples = vec![tumor_sample("T1")];
        config.normal_samples = vec![normal_sample("N1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // 2*(fastp + bwa_mem2 + mark_dup + bqsr) + haplotype_caller + mutect2 + filter_mutect_calls + strelka2 = 12
        assert_eq!(rules.len(), 12);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"mutect2_T1"));
        assert!(names.contains(&"filter_mutect_calls_T1"));
        assert!(names.contains(&"strelka2_T1"));
        assert!(names.contains(&"haplotype_caller_N1"));
    }

    #[test]
    fn generate_oxoflow_valid() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];

        let toml_str = generate_oxoflow(&config).unwrap();
        let wf = oxo_flow_core::config::WorkflowConfig::parse(&toml_str).unwrap();
        assert_eq!(wf.workflow.name, "TestProject");
        assert!(!wf.rules.is_empty());
    }

    #[test]
    fn pipeline_rule_dependencies() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let dag = oxo_flow_core::WorkflowDag::from_rules(&rules).unwrap();

        let order = dag.execution_order().unwrap();
        let fastp_pos = order.iter().position(|n| n == "fastp_T1").unwrap();
        let bwa_pos = order.iter().position(|n| n == "bwa_mem2_T1").unwrap();
        let markdup_pos = order
            .iter()
            .position(|n| n == "mark_duplicates_T1")
            .unwrap();
        let bqsr_pos = order.iter().position(|n| n == "bqsr_T1").unwrap();
        let mutect2_pos = order.iter().position(|n| n == "mutect2_T1").unwrap();
        let filter_pos = order
            .iter()
            .position(|n| n == "filter_mutect_calls_T1")
            .unwrap();

        assert!(fastp_pos < bwa_pos);
        assert!(bwa_pos < markdup_pos);
        assert!(markdup_pos < bqsr_pos);
        assert!(bqsr_pos < mutect2_pos);
        assert!(mutect2_pos < filter_pos);
    }

    #[test]
    fn build_tumor_normal_strelka2() {
        let mut config = base_config(AnalysisMode::TumorNormal);
        config.tumor_samples = vec![tumor_sample("T1")];
        config.normal_samples = vec![normal_sample("N1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"strelka2_T1"));

        // Strelka2 should use recal BAMs as input
        let strelka_rule = rules.iter().find(|r| r.name == "strelka2_T1").unwrap();
        assert_eq!(strelka_rule.input[0], "recal/T1.recal.bam");
        assert_eq!(strelka_rule.input[1], "recal/N1.recal.bam");
        assert_eq!(strelka_rule.output[0], "variants/T1.strelka2.vcf.gz");
    }

    #[test]
    fn strelka2_not_in_tumor_only() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("strelka2")));
    }

    #[test]
    fn known_sites_in_bqsr() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];
        config.known_sites = Some("/ref/dbsnp.vcf.gz".to_string());

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let bqsr_rule = rules.iter().find(|r| r.name == "bqsr_T1").unwrap();
        let shell = bqsr_rule.shell.as_ref().unwrap();
        assert!(shell.contains("--known-sites /ref/dbsnp.vcf.gz"));
    }

    #[test]
    fn filter_mutect_calls_uses_filtered_vcf_in_annotation() {
        let mut config = base_config(AnalysisMode::TumorOnly);
        config.tumor_samples = vec![tumor_sample("T1")];
        config.annotate = true;

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let annotate_rule = rules.iter().find(|r| r.name == "annotate_T1").unwrap();
        assert_eq!(
            annotate_rule.input[0],
            "variants/T1.mutect2.filtered.vcf.gz"
        );
    }
}
