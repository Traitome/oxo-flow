#![forbid(unsafe_code)]
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
#[serde(rename_all = "kebab-case")]
pub enum AnalysisMode {
    /// Experimental sample only (no matched control).
    #[serde(alias = "ExperimentOnly")]
    ExperimentOnly,
    /// Control/germline sample only.
    #[serde(alias = "ControlOnly")]
    ControlOnly,
    /// Paired experiment-control analysis.
    #[serde(alias = "ExperimentControl")]
    ExperimentControl,
}

impl std::fmt::Display for AnalysisMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExperimentOnly => write!(f, "experiment-only"),
            Self::ControlOnly => write!(f, "control-only"),
            Self::ExperimentControl => write!(f, "experiment-control"),
        }
    }
}

/// Sequencing type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SeqType {
    /// Whole genome sequencing.
    #[serde(alias = "WGS")]
    WGS,
    /// Whole exome sequencing.
    #[serde(alias = "WES")]
    WES,
    /// Targeted panel sequencing.
    #[serde(alias = "Panel")]
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

    /// Whether this is an experimental sample.
    pub is_experiment: bool,
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

    /// Experimental samples.
    #[serde(default)]
    pub experiment_samples: Vec<Sample>,

    /// Control samples.
    #[serde(default)]
    pub control_samples: Vec<Sample>,

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
            AnalysisMode::ExperimentOnly => {
                if self.experiment_samples.is_empty() {
                    anyhow::bail!("experiment-only mode requires at least one experimental sample");
                }
            }
            AnalysisMode::ControlOnly => {
                if self.control_samples.is_empty() {
                    anyhow::bail!("control-only mode requires at least one control sample");
                }
            }
            AnalysisMode::ExperimentControl => {
                if self.experiment_samples.is_empty() || self.control_samples.is_empty() {
                    anyhow::bail!(
                        "experiment-control mode requires both experimental and control samples"
                    );
                }
            }
        }

        // Validate target BED for WES/Panel
        if matches!(self.seq_type, SeqType::WES | SeqType::Panel) && self.target_bed.is_none() {
            anyhow::bail!("WES/Panel sequencing requires a target BED file");
        }

        Ok(())
    }

    /// Returns all samples (experiment + control).
    pub fn all_samples(&self) -> Vec<&Sample> {
        self.experiment_samples
            .iter()
            .chain(self.control_samples.iter())
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
    /// Copy number variation calling.
    CnvKit,
    /// Microsatellite instability detection.
    MsiSensor,
    /// Tumor mutation burden calculation.
    TmbCalc,
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
            Self::CnvKit => write!(f, "cnvkit"),
            Self::MsiSensor => write!(f, "msi_sensor"),
            Self::TmbCalc => write!(f, "tmb_calc"),
        }
    }
}

/// Builds the Venus pipeline as a set of oxo-flow rules.
pub struct VenusPipelineBuilder {
    config: VenusConfig,
}

impl VenusPipelineBuilder {
    /// Create a pipeline rule with common defaults, respecting output_dir and scaling threads.
    #[allow(clippy::too_many_arguments)]
    fn make_rule(
        &self,
        name: &str,
        input: Vec<String>,
        output: Vec<String>,
        shell: &str,
        threads_ratio: f64, // e.g. 1.0 for max threads, 0.5 for half
        memory_gb: u32,
        env_conda: &str,
    ) -> Rule {
        let output_dir = &self.config.output_dir;

        let prepend_dir = |p: &str| -> String {
            if p.starts_with("raw/") || p.starts_with('/') {
                p.to_string() // Assume raw data is external
            } else {
                format!("{}/{}", output_dir, p)
            }
        };

        let scaled_threads = (self.config.threads as f64 * threads_ratio).max(1.0) as u32;
        let mem_string = if memory_gb > 0 {
            Some(format!("{}G", memory_gb))
        } else {
            None
        };

        #[allow(deprecated)]
        Rule {
            name: name.to_string(),
            input: input.into_iter().map(|p| prepend_dir(&p)).collect(),
            output: output.into_iter().map(|p| prepend_dir(&p)).collect(),
            shell: Some(shell.to_string()),
            script: None,
            threads: Some(scaled_threads),
            memory: mem_string.clone(),
            resources: Resources {
                threads: scaled_threads,
                memory: mem_string,
                ..Default::default()
            },
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
            ..Default::default()
        }
    }

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
            AnalysisMode::ExperimentOnly => {
                rules.extend(self.mutect2_rules());
                rules.extend(self.filter_mutect_calls_rules());
            }
            AnalysisMode::ControlOnly => {
                rules.extend(self.haplotype_caller_rules());
            }
            AnalysisMode::ExperimentControl => {
                rules.extend(self.haplotype_caller_rules());
                rules.extend(self.mutect2_rules());
                rules.extend(self.filter_mutect_calls_rules());
                rules.extend(self.strelka2_rules());
            }
        }

        // CNV calling for experimental samples
        if matches!(
            self.config.mode,
            AnalysisMode::ExperimentOnly | AnalysisMode::ExperimentControl
        ) {
            rules.extend(self.cnvkit_rules());
        }

        // MSI detection for paired experiment-control
        if self.config.mode == AnalysisMode::ExperimentControl {
            rules.extend(self.msi_rules());
        }

        // TMB calculation
        rules.extend(self.tmb_rules());

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
                self.make_rule(
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
                    0.5,
                    0,
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
                self.make_rule(
                    &format!("bwa_mem2_{}", s.name),
                    vec![
                        format!("trimmed/{}_R1.fq.gz", s.name),
                        format!("trimmed/{}_R2.fq.gz", s.name),
                    ],
                    vec![format!("aligned/{}.sorted.bam", s.name)],
                    &format!(
                        "bwa-mem2 mem -t {{threads}} {ref_path} {{input[0]}} {{input[1]}} | samtools sort -@ {{threads}} -o {{output[0]}}"
                    ),
                    1.0,
                    32,
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
                self.make_rule(
                    &format!("mark_duplicates_{}", s.name),
                    vec![format!("aligned/{}.sorted.bam", s.name)],
                    vec![
                        format!("dedup/{}.dedup.bam", s.name),
                        format!("dedup/{}.metrics.txt", s.name),
                    ],
                    "gatk MarkDuplicates -I {input[0]} -O {output[0]} -M {output[1]}",
                    0.25,
                    16,
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
                self.make_rule(
                    &format!("bqsr_{name}"),
                    vec![format!("dedup/{name}.dedup.bam")],
                    vec![format!("recal/{name}.recal.bam")],
                    &format!(
                        "gatk BaseRecalibrator -I {{input[0]}} -R {ref_path}{known_sites_flag} -O recal/{name}.recal.table && \
                         gatk ApplyBQSR -I {{input[0]}} -R {ref_path} --bqsr-recal-file recal/{name}.recal.table -O {{output[0]}}"
                    ),
                    0.25,
                    16,
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn haplotype_caller_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        self.config
            .control_samples
            .iter()
            .map(|s| {
                self.make_rule(
                    &format!("haplotype_caller_{}", s.name),
                    vec![format!("recal/{}.recal.bam", s.name)],
                    vec![format!("variants/{}.g.vcf.gz", s.name)],
                    &format!(
                        "gatk HaplotypeCaller -I {{input[0]}} -R {ref_path} -O {{output[0]}} -ERC GVCF"
                    ),
                    0.25,
                    16,
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn mutect2_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        let control_name = self.config.control_samples.first().map(|s| s.name.as_str());
        self.config
            .experiment_samples
            .iter()
            .map(|s| {
                let (input, shell) = if let Some(control) = control_name {
                    (
                        vec![
                            format!("recal/{}.recal.bam", s.name),
                            format!("recal/{control}.recal.bam"),
                        ],
                        format!(
                            "gatk Mutect2 -I {{input[0]}} -I {{input[1]}} -normal {control} -R {ref_path} -O {{output[0]}}"
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
                self.make_rule(
                    &format!("mutect2_{}", s.name),
                    input,
                    vec![format!("variants/{}.mutect2.vcf.gz", s.name)],
                    &shell,
                    0.25,
                    16,
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn filter_mutect_calls_rules(&self) -> Vec<Rule> {
        let ref_path = &self.config.reference_fasta;
        self.config
            .experiment_samples
            .iter()
            .map(|s| {
                self.make_rule(
                    &format!("filter_mutect_calls_{}", s.name),
                    vec![format!("variants/{}.mutect2.vcf.gz", s.name)],
                    vec![format!("variants/{}.mutect2.filtered.vcf.gz", s.name)],
                    &format!(
                        "gatk FilterMutectCalls -V {{input[0]}} -R {ref_path} -O {{output[0]}}"
                    ),
                    0.25,
                    8,
                    "envs/gatk.yaml",
                )
            })
            .collect()
    }

    fn strelka2_rules(&self) -> Vec<Rule> {
        if self.config.mode != AnalysisMode::ExperimentControl {
            return Vec::new();
        }
        let control_name = match self.config.control_samples.first() {
            Some(s) => &s.name,
            None => return Vec::new(),
        };
        let ref_path = &self.config.reference_fasta;
        self.config
            .experiment_samples
            .iter()
            .map(|s| {
                self.make_rule(
                    &format!("strelka2_{}", s.name),
                    vec![
                        format!("recal/{}.recal.bam", s.name),
                        format!("recal/{control_name}.recal.bam"),
                    ],
                    vec![format!("variants/{}.strelka2.vcf.gz", s.name)],
                    &format!(
                        "configureStrelkaSomaticWorkflow.py --tumorBam {{input[0]}} --normalBam {{input[1]}} --referenceFasta {ref_path} --runDir strelka2_{exp} && \
                         strelka2_{exp}/runWorkflow.py -m local -j {{threads}} && \
                         cp strelka2_{exp}/results/variants/somatic.snvs.vcf.gz {{output[0]}}",
                        exp = s.name
                    ),
                    0.5,
                    16,
                    "envs/strelka2.yaml",
                )
            })
            .collect()
    }

    fn cnvkit_rules(&self) -> Vec<Rule> {
        self.config
            .experiment_samples
            .iter()
            .map(|s| {
                let control_bam = self
                    .config
                    .control_samples
                    .first()
                    .map(|n| format!("recal/{}.recal.bam", n.name));
                let mut input = vec![format!("recal/{}.recal.bam", s.name)];
                if let Some(ref nb) = control_bam {
                    input.push(nb.clone());
                }
                self.make_rule(
                    &format!("cnvkit_{}", s.name),
                    input,
                    vec![format!("cnv/{}.cnr", s.name), format!("cnv/{}.cns", s.name)],
                    &format!(
                        "cnvkit.py batch {{input[0]}} {} -d cnv/",
                        control_bam
                            .as_ref()
                            .map(|n| format!("--normal {n}"))
                            .unwrap_or_default()
                    ),
                    0.25,
                    16,
                    "envs/cnvkit.yaml",
                )
            })
            .collect()
    }

    fn msi_rules(&self) -> Vec<Rule> {
        self.config
            .experiment_samples
            .iter()
            .filter_map(|s| {
                let control = self.config.control_samples.first()?;
                Some(self.make_rule(
                    &format!("msi_sensor_{}", s.name),
                    vec![
                        format!("recal/{}.recal.bam", s.name),
                        format!("recal/{}.recal.bam", control.name),
                    ],
                    vec![format!("msi/{}.msi.txt", s.name)],
                    "msisensor2 msi -t {input[0]} -n {input[1]} -o {output[0]}",
                    0.25,
                    8,
                    "envs/msi.yaml",
                ))
            })
            .collect()
    }

    fn tmb_rules(&self) -> Vec<Rule> {
        self.called_samples()
            .into_iter()
            .map(|(sample, vcf_path)| {
                let input = if self.config.annotate {
                    format!("annotated/{}.annotated.vcf.gz", sample.name)
                } else {
                    vcf_path
                };
                self.make_rule(
                    &format!("tmb_{}", sample.name),
                    vec![input],
                    vec![format!("tmb/{}.tmb.txt", sample.name)],
                    "python scripts/calc_tmb.py --input {input[0]} --output {output[0]}",
                    0.125,
                    4,
                    "envs/report.yaml",
                )
            })
            .collect()
    }

    fn annotation_rules(&self) -> Vec<Rule> {
        self.called_samples()
            .into_iter()
            .map(|(sample, vcf_path)| {
                self.make_rule(
                    &format!("annotate_{}", sample.name),
                    vec![vcf_path],
                    vec![format!("annotated/{}.annotated.vcf.gz", sample.name)],
                    "vep --input_file {input[0]} --output_file {output[0]} --format vcf --vcf --offline --cache",
                    0.25,
                    8,
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
                self.make_rule(
                    &format!("report_{name}"),
                    vec![input],
                    vec![format!("reports/{name}_clinical_report.html")],
                    &format!(
                        "python scripts/generate_report.py --input {{input[0]}} --output {{output[0]}} --sample {name}"
                    ),
                    0.125,
                    4,
                    "envs/report.yaml",
                )
            })
            .collect()
    }

    /// Returns samples that went through variant calling with their VCF paths.
    fn called_samples(&self) -> Vec<(&Sample, String)> {
        let mut result = Vec::new();
        match self.config.mode {
            AnalysisMode::ExperimentOnly => {
                for s in &self.config.experiment_samples {
                    result.push((s, format!("variants/{}.mutect2.filtered.vcf.gz", s.name)));
                }
            }
            AnalysisMode::ControlOnly => {
                for s in &self.config.control_samples {
                    result.push((s, format!("variants/{}.g.vcf.gz", s.name)));
                }
            }
            AnalysisMode::ExperimentControl => {
                for s in &self.config.experiment_samples {
                    result.push((s, format!("variants/{}.mutect2.filtered.vcf.gz", s.name)));
                }
                for s in &self.config.control_samples {
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
            min_version: None,
            format_version: None,
            genome_build: None,
            interpreter_map: HashMap::new(),
            pairs_file: None,
            sample_groups_file: None,
            pairs_pattern: None,
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
        includes: Vec::new(),
        execution_groups: Vec::new(),
        citation: None,
        cluster: None,
        resource_budget: None,
        resource_groups: std::collections::HashMap::new(),
        reference_databases: Vec::new(),
        wildcard_constraints: std::collections::HashMap::new(),
        pairs: Vec::new(),
        sample_groups: Vec::new(),
    };

    let toml_str = toml::to_string_pretty(&wf)?;
    Ok(toml_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_mode_display() {
        assert_eq!(AnalysisMode::ExperimentOnly.to_string(), "experiment-only");
        assert_eq!(AnalysisMode::ControlOnly.to_string(), "control-only");
        assert_eq!(
            AnalysisMode::ExperimentControl.to_string(),
            "experiment-control"
        );
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
    fn validate_experiment_only() {
        let config = VenusConfig {
            mode: AnalysisMode::ExperimentOnly,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![Sample {
                name: "EXP_01".to_string(),
                r1_fastq: "exp_R1.fq.gz".to_string(),
                r2_fastq: Some("exp_R2.fq.gz".to_string()),
                is_experiment: true,
            }],
            control_samples: vec![],
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
    fn validate_experiment_only_no_samples() {
        let config = VenusConfig {
            mode: AnalysisMode::ExperimentOnly,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![],
            control_samples: vec![],
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
            mode: AnalysisMode::ExperimentOnly,
            seq_type: SeqType::WES,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![Sample {
                name: "EXP".to_string(),
                r1_fastq: "exp_R1.fq.gz".to_string(),
                r2_fastq: None,
                is_experiment: true,
            }],
            control_samples: vec![],
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
            mode: AnalysisMode::ExperimentControl,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![Sample {
                name: "EXP".to_string(),
                r1_fastq: "exp.fq.gz".to_string(),
                r2_fastq: None,
                is_experiment: true,
            }],
            control_samples: vec![Sample {
                name: "CTRL".to_string(),
                r1_fastq: "ctrl.fq.gz".to_string(),
                r2_fastq: None,
                is_experiment: false,
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

    fn experiment_sample(name: &str) -> Sample {
        Sample {
            name: name.to_string(),
            r1_fastq: format!("raw/{name}_R1.fq.gz"),
            r2_fastq: Some(format!("raw/{name}_R2.fq.gz")),
            is_experiment: true,
        }
    }

    fn control_sample(name: &str) -> Sample {
        Sample {
            name: name.to_string(),
            r1_fastq: format!("raw/{name}_R1.fq.gz"),
            r2_fastq: Some(format!("raw/{name}_R2.fq.gz")),
            is_experiment: false,
        }
    }

    fn base_config(mode: AnalysisMode) -> VenusConfig {
        VenusConfig {
            mode,
            seq_type: SeqType::WGS,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![],
            control_samples: vec![],
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
    fn build_experiment_only_pipeline() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.annotate = true;
        config.report = true;

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // fastp + bwa_mem2 + mark_dup + bqsr + mutect2 + filter_mutect_calls + cnvkit + tmb + annotate + report = 10
        assert_eq!(rules.len(), 10);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"fastp_EXP1"));
        assert!(names.contains(&"bwa_mem2_EXP1"));
        assert!(names.contains(&"mark_duplicates_EXP1"));
        assert!(names.contains(&"bqsr_EXP1"));
        assert!(names.contains(&"mutect2_EXP1"));
        assert!(names.contains(&"filter_mutect_calls_EXP1"));
        assert!(names.contains(&"annotate_EXP1"));
        assert!(names.contains(&"report_EXP1"));
        assert!(!names.iter().any(|n| n.starts_with("haplotype_caller")));
    }

    #[test]
    fn build_control_only_pipeline() {
        let mut config = base_config(AnalysisMode::ControlOnly);
        config.control_samples = vec![control_sample("CTRL1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // fastp + bwa_mem2 + mark_dup + bqsr + haplotype_caller + tmb = 6
        assert_eq!(rules.len(), 6);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"haplotype_caller_CTRL1"));
        assert!(!names.iter().any(|n| n.starts_with("mutect2")));
    }

    #[test]
    fn build_experiment_control_pipeline() {
        let mut config = base_config(AnalysisMode::ExperimentControl);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.control_samples = vec![control_sample("CTRL1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        // 2*(fastp + bwa_mem2 + mark_dup + bqsr) + haplotype_caller + mutect2 + filter_mutect_calls + strelka2 + cnvkit + msi + 2*tmb = 16
        assert_eq!(rules.len(), 16);
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"mutect2_EXP1"));
        assert!(names.contains(&"filter_mutect_calls_EXP1"));
        assert!(names.contains(&"strelka2_EXP1"));
        assert!(names.contains(&"haplotype_caller_CTRL1"));
    }

    #[test]
    fn generate_oxoflow_valid() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];

        let toml_str = generate_oxoflow(&config).unwrap();
        let wf = oxo_flow_core::config::WorkflowConfig::parse(&toml_str).unwrap();
        assert_eq!(wf.workflow.name, "TestProject");
        assert!(!wf.rules.is_empty());
    }

    #[test]
    fn pipeline_rule_dependencies() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let dag = oxo_flow_core::WorkflowDag::from_rules(&rules).unwrap();

        let order = dag.execution_order().unwrap();
        let fastp_pos = order.iter().position(|n| n == "fastp_EXP1").unwrap();
        let bwa_pos = order.iter().position(|n| n == "bwa_mem2_EXP1").unwrap();
        let markdup_pos = order
            .iter()
            .position(|n| n == "mark_duplicates_EXP1")
            .unwrap();
        let bqsr_pos = order.iter().position(|n| n == "bqsr_EXP1").unwrap();
        let mutect2_pos = order.iter().position(|n| n == "mutect2_EXP1").unwrap();
        let filter_pos = order
            .iter()
            .position(|n| n == "filter_mutect_calls_EXP1")
            .unwrap();

        assert!(fastp_pos < bwa_pos);
        assert!(bwa_pos < markdup_pos);
        assert!(markdup_pos < bqsr_pos);
        assert!(bqsr_pos < mutect2_pos);
        assert!(mutect2_pos < filter_pos);
    }

    #[test]
    fn build_experiment_control_strelka2() {
        let mut config = base_config(AnalysisMode::ExperimentControl);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.control_samples = vec![control_sample("CTRL1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"strelka2_EXP1"));

        // Strelka2 should use recal BAMs as input
        let strelka_rule = rules.iter().find(|r| r.name == "strelka2_EXP1").unwrap();
        assert_eq!(
            strelka_rule.input.get_index(0).unwrap(),
            "output/recal/EXP1.recal.bam"
        );
        assert_eq!(
            strelka_rule.input.get_index(1).unwrap(),
            "output/recal/CTRL1.recal.bam"
        );
        assert_eq!(
            strelka_rule.output.get_index(0).unwrap(),
            "output/variants/EXP1.strelka2.vcf.gz"
        );
    }

    #[test]
    fn strelka2_not_in_experiment_only() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("strelka2")));
    }

    #[test]
    fn known_sites_in_bqsr() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.known_sites = Some("/ref/dbsnp.vcf.gz".to_string());

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let bqsr_rule = rules.iter().find(|r| r.name == "bqsr_EXP1").unwrap();
        let shell = bqsr_rule.shell.as_ref().unwrap();
        assert!(shell.contains("--known-sites /ref/dbsnp.vcf.gz"));
    }

    #[test]
    fn filter_mutect_calls_uses_filtered_vcf_in_annotation() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.annotate = true;

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let annotate_rule = rules.iter().find(|r| r.name == "annotate_EXP1").unwrap();
        assert_eq!(
            annotate_rule.input.get_index(0).unwrap(),
            "output/variants/EXP1.mutect2.filtered.vcf.gz"
        );
    }

    #[test]
    fn validate_experiment_control_needs_both_samples() {
        let mut config = base_config(AnalysisMode::ExperimentControl);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        // control_samples left empty — ExperimentControl requires both
        assert!(config.validate().is_err());
        let err = config.validate().unwrap_err().to_string();
        assert!(
            err.contains("both"),
            "expected 'both' in error message, got: {err}"
        );
    }

    #[test]
    fn pipeline_rule_environments() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        for rule in &rules {
            assert!(
                rule.environment.conda.is_some(),
                "rule '{}' should have a conda environment spec",
                rule.name
            );
            let conda = rule.environment.conda.as_ref().unwrap();
            assert!(
                !conda.is_empty(),
                "rule '{}' has empty conda spec",
                rule.name
            );
        }
    }

    #[test]
    fn generate_oxoflow_round_trip() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.annotate = true;
        config.report = true;

        // Generate TOML
        let toml_str = generate_oxoflow(&config).unwrap();

        // Parse it back
        let wf = oxo_flow_core::config::WorkflowConfig::parse(&toml_str).unwrap();
        assert_eq!(wf.workflow.name, "TestProject");
        assert!(!wf.rules.is_empty());

        // Build DAG from parsed rules and verify validity
        let dag = oxo_flow_core::WorkflowDag::from_rules(&wf.rules).unwrap();
        assert!(dag.validate().is_ok());
        assert!(dag.node_count() > 0);
        assert_eq!(dag.node_count(), wf.rules.len());
    }

    #[test]
    fn panel_sequencing_with_bed() {
        let config = VenusConfig {
            mode: AnalysisMode::ExperimentOnly,
            seq_type: SeqType::Panel,
            genome_build: GenomeBuild::GRCh38,
            reference_fasta: "/ref/hg38.fa".to_string(),
            experiment_samples: vec![experiment_sample("EXP1")],
            control_samples: vec![],
            known_sites: None,
            target_bed: Some("targets/panel.bed".to_string()),
            threads: 8,
            output_dir: "output".to_string(),
            annotate: false,
            report: false,
            project_name: Some("PanelTest".to_string()),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn venus_step_display_cnv_msi_tmb() {
        assert_eq!(VenusStep::CnvKit.to_string(), "cnvkit");
        assert_eq!(VenusStep::MsiSensor.to_string(), "msi_sensor");
        assert_eq!(VenusStep::TmbCalc.to_string(), "tmb_calc");
    }

    #[test]
    fn build_experiment_control_has_msi() {
        let mut config = base_config(AnalysisMode::ExperimentControl);
        config.experiment_samples = vec![experiment_sample("EXP1")];
        config.control_samples = vec![control_sample("CTRL1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"msi_sensor_EXP1"),
            "ExperimentControl pipeline should include MSI rules"
        );
    }

    #[test]
    fn build_experiment_only_no_msi() {
        let mut config = base_config(AnalysisMode::ExperimentOnly);
        config.experiment_samples = vec![experiment_sample("EXP1")];

        let builder = VenusPipelineBuilder::new(config);
        let rules = builder.build().unwrap();

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(
            !names.iter().any(|n| n.starts_with("msi_sensor")),
            "ExperimentOnly pipeline should NOT include MSI rules"
        );
    }
}
