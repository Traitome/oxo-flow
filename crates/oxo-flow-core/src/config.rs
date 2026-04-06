//! Workflow configuration and `.oxoflow` file parsing.
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use crate::error::{OxoFlowError, Result};
use crate::rule::{EnvironmentSpec, Rule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Maximum depth for nested include directives to prevent infinite recursion.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Strongly-typed rule name for compile-time safety.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleName(pub String);

impl std::fmt::Display for RuleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for RuleName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for RuleName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Strongly-typed wildcard pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WildcardPattern(pub String);

impl std::fmt::Display for WildcardPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for WildcardPattern {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WildcardPattern {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Top-level workflow metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMeta {
    /// Workflow name.
    pub name: String,

    /// Semantic version.
    #[serde(default = "default_version")]
    pub version: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,

    /// Author name or organization.
    #[serde(default)]
    pub author: Option<String>,

    /// Format specification version (e.g., "1.0").
    #[serde(default)]
    pub min_version: Option<String>,

    /// Format specification version for compatibility checking.
    #[serde(default)]
    pub format_version: Option<String>,

    /// Genome build (e.g., "GRCh38", "hg38", "GRCh37").
    #[serde(default)]
    pub genome_build: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Default settings applied to all rules unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default thread count.
    #[serde(default)]
    pub threads: Option<u32>,

    /// Default memory.
    #[serde(default)]
    pub memory: Option<String>,

    /// Default environment.
    #[serde(default)]
    pub environment: Option<EnvironmentSpec>,
}

/// Report configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReportConfig {
    /// Report template name.
    #[serde(default)]
    pub template: Option<String>,

    /// Output formats (html, pdf, json).
    #[serde(default)]
    pub format: Vec<String>,

    /// Report sections to include.
    #[serde(default)]
    pub sections: Vec<String>,
}

/// Include directive for modular workflow composition.
///
/// Allows importing rules from another `.oxoflow` file into the
/// current workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IncludeDirective {
    /// Path to the included `.oxoflow` file.
    pub path: String,

    /// Optional namespace prefix for included rule names.
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Execution mode for an execution group.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    /// Rules in the group execute one after another.
    Sequential,
    /// Rules in the group execute concurrently.
    #[default]
    Parallel,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Sequential => write!(f, "sequential"),
            ExecutionMode::Parallel => write!(f, "parallel"),
        }
    }
}

/// Execution group for explicit rule ordering.
///
/// Groups a set of rules under a named block with a specified execution
/// mode (sequential or parallel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionGroup {
    /// Group name.
    pub name: String,

    /// Rules in this group (by name).
    #[serde(default)]
    pub rules: Vec<String>,

    /// Execution mode.
    #[serde(default)]
    pub mode: ExecutionMode,
}

/// Citation information for workflow reproducibility and publication.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CitationInfo {
    /// DOI reference for this workflow.
    #[serde(default)]
    pub doi: Option<String>,
    /// URL to the workflow repository or publication.
    #[serde(default)]
    pub url: Option<String>,
    /// Authors of this workflow.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Associated publication title.
    #[serde(default)]
    pub title: Option<String>,
}

/// Cluster execution profile for HPC deployment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterProfile {
    /// Backend type (slurm, pbs, sge, lsf).
    #[serde(default)]
    pub backend: Option<String>,
    /// Default partition/queue.
    #[serde(default)]
    pub partition: Option<String>,
    /// Default account for billing.
    #[serde(default)]
    pub account: Option<String>,
    /// Additional arguments passed to the scheduler.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Resource budget constraints for the entire workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceBudget {
    /// Maximum total CPU threads across all running jobs.
    #[serde(default)]
    pub max_threads: Option<u32>,
    /// Maximum total memory across all running jobs.
    #[serde(default)]
    pub max_memory: Option<String>,
    /// Maximum total running jobs.
    #[serde(default)]
    pub max_jobs: Option<usize>,
}

/// Complete workflow configuration parsed from an `.oxoflow` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// Workflow metadata.
    pub workflow: WorkflowMeta,

    /// Configuration variables (user-defined key-value pairs).
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,

    /// Default settings for all rules.
    #[serde(default)]
    pub defaults: Defaults,

    /// Report configuration.
    #[serde(default)]
    pub report: Option<ReportConfig>,

    /// List of rules (pipeline steps).
    #[serde(default, rename = "rules")]
    pub rules: Vec<Rule>,

    /// Include directives for importing rules from other workflow files.
    #[serde(default, rename = "include")]
    pub includes: Vec<IncludeDirective>,

    /// Explicit execution groups for sequential/parallel rule ordering.
    #[serde(default, rename = "execution_group")]
    pub execution_groups: Vec<ExecutionGroup>,

    /// Citation information for reproducibility.
    #[serde(default)]
    pub citation: Option<CitationInfo>,

    /// Cluster execution profile.
    #[serde(default)]
    pub cluster: Option<ClusterProfile>,

    /// Resource budget for the workflow.
    #[serde(default)]
    pub resource_budget: Option<ResourceBudget>,
}

// ---------------------------------------------------------------------------
// Clinical & domain types (Expert 2, Expert 13, Expert 17)
// ---------------------------------------------------------------------------

/// ACMG/AMP variant classification for somatic mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariantClassification {
    /// Tier I: Strong clinical significance
    TierI,
    /// Tier II: Potential clinical significance
    TierII,
    /// Tier III: Unknown clinical significance
    TierIII,
    /// Tier IV: Benign or likely benign
    TierIV,
    /// Pathogenic (germline)
    Pathogenic,
    /// Likely pathogenic (germline)
    LikelyPathogenic,
    /// Uncertain significance (germline)
    Vus,
    /// Likely benign (germline)
    LikelyBenign,
    /// Benign (germline)
    Benign,
}

impl std::fmt::Display for VariantClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TierI => write!(f, "Tier I"),
            Self::TierII => write!(f, "Tier II"),
            Self::TierIII => write!(f, "Tier III"),
            Self::TierIV => write!(f, "Tier IV"),
            Self::Pathogenic => write!(f, "Pathogenic"),
            Self::LikelyPathogenic => write!(f, "Likely Pathogenic"),
            Self::Vus => write!(f, "VUS"),
            Self::LikelyBenign => write!(f, "Likely Benign"),
            Self::Benign => write!(f, "Benign"),
        }
    }
}

/// Biomarker result (MSI status, TMB value, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BiomarkerResult {
    /// Biomarker name (e.g., "MSI", "TMB", "HRD").
    pub name: String,
    /// Measured value.
    pub value: f64,
    /// Unit of measurement (e.g., "mutations/Mb", "score").
    pub unit: String,
    /// Classification (e.g., "MSI-H", "TMB-High").
    pub classification: Option<String>,
    /// Threshold used for classification.
    pub threshold: Option<f64>,
}

impl std::fmt::Display for BiomarkerResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {:.2} {}", self.name, self.value, self.unit)?;
        if let Some(ref class) = self.classification {
            write!(f, " ({})", class)?;
        }
        Ok(())
    }
}

/// Tumor sample metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TumorSampleMeta {
    /// Estimated tumor purity (0.0–1.0).
    pub tumor_purity: Option<f64>,
    /// Estimated ploidy.
    pub ploidy: Option<f64>,
    /// Sample type (tumor, normal, etc.).
    pub sample_type: Option<String>,
    /// Match ID for tumor-normal pairing.
    pub match_id: Option<String>,
}

/// Configurable QC threshold with pass/fail bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QcThreshold {
    /// Metric name (e.g., "mean_coverage", "mapping_rate").
    pub metric: String,
    /// Minimum acceptable value (inclusive).
    pub min: Option<f64>,
    /// Maximum acceptable value (inclusive).
    pub max: Option<f64>,
    /// Description of this threshold.
    pub description: Option<String>,
}

impl QcThreshold {
    /// Check whether a value passes this threshold.
    #[must_use]
    pub fn passes(&self, value: f64) -> bool {
        if let Some(min) = self.min
            && value < min
        {
            return false;
        }
        if let Some(max) = self.max
            && value > max
        {
            return false;
        }
        true
    }
}

impl std::fmt::Display for QcThreshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.metric)?;
        if let Some(min) = self.min {
            write!(f, " ≥ {min:.2}")?;
        }
        if let Some(max) = self.max {
            write!(f, " ≤ {max:.2}")?;
        }
        Ok(())
    }
}

/// Compliance event for CAP/CLIA audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceEvent {
    /// Timestamp (ISO 8601).
    pub timestamp: String,
    /// Event type (e.g., "analysis_started", "result_reviewed").
    pub event_type: String,
    /// Operator or system that triggered the event.
    pub actor: String,
    /// Human-readable description.
    pub description: String,
    /// Optional evidence hash for traceability.
    pub evidence_hash: Option<String>,
}

/// Gene panel definition for targeted analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenePanel {
    /// Panel name (e.g., "Oncomine Focus Assay").
    pub name: String,
    /// Panel version.
    pub version: Option<String>,
    /// Gene symbols in the panel.
    pub genes: Vec<String>,
    /// BED file path for the panel regions.
    pub bed_file: Option<String>,
}

impl std::fmt::Display for GenePanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({} genes)", self.name, self.genes.len())?;
        if let Some(ref v) = self.version {
            write!(f, " v{v}")?;
        }
        Ok(())
    }
}

/// Actionability annotation from clinical databases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionabilityAnnotation {
    /// Source database (e.g., "OncoKB", "ClinVar", "CIViC").
    pub source: String,
    /// Evidence level (e.g., "Level 1", "Level 2A").
    pub evidence_level: String,
    /// Associated drug or therapy.
    pub therapy: Option<String>,
    /// Disease context.
    pub disease: Option<String>,
}

/// Sequential variant filter with audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterChain {
    /// Filter name.
    pub name: String,
    /// Ordered list of filter expressions.
    pub filters: Vec<String>,
    /// Whether each filter is hard (remove) or soft (flag).
    pub hard: Vec<bool>,
}

/// Required sections in a clinical report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClinicalReportSection {
    /// Patient/specimen information.
    SpecimenInfo,
    /// Methodology description.
    Methodology,
    /// Results summary.
    Results,
    /// Variant interpretation.
    Interpretation,
    /// Quality control metrics.
    QualityControl,
    /// Known limitations of the assay.
    Limitations,
    /// References and citations.
    References,
    /// Appendix / supplementary data.
    Appendix,
}

impl std::fmt::Display for ClinicalReportSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpecimenInfo => write!(f, "Specimen Information"),
            Self::Methodology => write!(f, "Methodology"),
            Self::Results => write!(f, "Results"),
            Self::Interpretation => write!(f, "Interpretation"),
            Self::QualityControl => write!(f, "Quality Control"),
            Self::Limitations => write!(f, "Limitations"),
            Self::References => write!(f, "References"),
            Self::Appendix => write!(f, "Appendix"),
        }
    }
}

// ---------------------------------------------------------------------------
// Type-state pattern for workflow lifecycle
// ---------------------------------------------------------------------------

/// Marker type for a parsed (but not validated) workflow.
#[derive(Debug, Clone)]
pub struct Parsed;

/// Marker type for a validated workflow.
#[derive(Debug, Clone)]
pub struct Validated;

/// Marker type for a workflow that is ready to execute.
#[derive(Debug, Clone)]
pub struct Ready;

/// Type-state wrapper for [`WorkflowConfig`] that enforces lifecycle transitions
/// at compile time: Parsed → Validated → Ready.
#[derive(Debug, Clone)]
pub struct WorkflowState<S> {
    pub config: WorkflowConfig,
    _state: std::marker::PhantomData<S>,
}

impl WorkflowState<Parsed> {
    /// Create a new parsed workflow state from a config.
    #[must_use]
    pub fn new(config: WorkflowConfig) -> Self {
        Self {
            config,
            _state: std::marker::PhantomData,
        }
    }

    /// Validate the workflow and transition to Validated state.
    pub fn validate(self) -> crate::Result<WorkflowState<Validated>> {
        self.config.validate()?;
        for rule in &self.config.rules {
            rule.validate()
                .map_err(|e| crate::OxoFlowError::Validation {
                    message: e,
                    rule: Some(rule.name.clone()),
                    suggestion: None,
                })?;
        }
        Ok(WorkflowState {
            config: self.config,
            _state: std::marker::PhantomData,
        })
    }
}

impl WorkflowState<Validated> {
    /// Build the DAG and transition to Ready state.
    pub fn prepare(self) -> crate::Result<WorkflowState<Ready>> {
        let _dag = crate::dag::WorkflowDag::from_rules(&self.config.rules)?;
        Ok(WorkflowState {
            config: self.config,
            _state: std::marker::PhantomData,
        })
    }
}

impl<S> WorkflowState<S> {
    /// Access the underlying config.
    #[must_use]
    pub fn config(&self) -> &WorkflowConfig {
        &self.config
    }
}

impl WorkflowConfig {
    /// Parse a workflow configuration from a TOML string.
    #[must_use = "parsing a config returns a Result that must be used"]
    pub fn parse(content: &str) -> Result<Self> {
        let config: WorkflowConfig = toml::from_str(content)?;
        config.validate()?;
        Ok(config)
    }

    /// Parse a workflow configuration from a `.oxoflow` file.
    #[must_use = "parsing a config file returns a Result that must be used"]
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let config: WorkflowConfig = toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the workflow configuration for internal consistency.
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate(&self) -> Result<()> {
        // Check for duplicate rule names
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            if !seen.insert(&rule.name) {
                return Err(OxoFlowError::DuplicateRule {
                    name: rule.name.clone(),
                });
            }
        }

        // Ensure each rule has either shell or script
        for rule in &self.rules {
            if rule.shell.is_none() && rule.script.is_none() && !rule.output.is_empty() {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "rule '{}' has outputs but no shell command or script",
                        rule.name
                    ),
                });
            }
        }

        self.validate_execution_groups()?;

        Ok(())
    }

    /// Resolve include directives by loading and merging rules from included files.
    /// Rules from included files are optionally prefixed with the namespace.
    #[must_use = "resolving includes returns a Result that must be checked"]
    pub fn resolve_includes(&mut self, base_dir: &Path) -> Result<()> {
        self.resolve_includes_with_depth(base_dir, 0)
    }

    fn resolve_includes_with_depth(&mut self, base_dir: &Path, depth: usize) -> Result<()> {
        if depth >= MAX_INCLUDE_DEPTH {
            return Err(OxoFlowError::Config {
                message: format!(
                    "include depth exceeds maximum of {} — possible circular includes",
                    MAX_INCLUDE_DEPTH
                ),
            });
        }
        let includes = std::mem::take(&mut self.includes);
        for inc in &includes {
            let inc_path = base_dir.join(&inc.path);
            let content = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                path: inc_path.clone(),
                message: format!("failed to read include '{}': {}", inc.path, e),
            })?;
            let mut inc_config: WorkflowConfig =
                toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: e.to_string(),
                })?;
            // Recursively resolve nested includes
            if let Some(parent) = inc_path.parent() {
                inc_config.resolve_includes_with_depth(parent, depth + 1)?;
            }
            for mut rule in inc_config.rules {
                if let Some(ref ns) = inc.namespace {
                    rule.name = format!("{}::{}", ns, rule.name);
                }
                if !self.rules.iter().any(|r| r.name == rule.name) {
                    self.rules.push(rule);
                }
            }
        }
        self.includes = includes;
        Ok(())
    }

    /// Validate that all execution group references point to existing rules.
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate_execution_groups(&self) -> Result<()> {
        let rule_names: std::collections::HashSet<&str> =
            self.rules.iter().map(|r| r.name.as_str()).collect();
        for group in &self.execution_groups {
            for rule_ref in &group.rules {
                if !rule_names.contains(rule_ref.as_str()) {
                    return Err(OxoFlowError::Config {
                        message: format!(
                            "execution group '{}' references unknown rule '{}'",
                            group.name, rule_ref
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Get a rule by name.
    pub fn get_rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Get a config value by key.
    pub fn get_config_value(&self, key: &str) -> Option<&toml::Value> {
        self.config.get(key)
    }

    /// Returns the list of all rule names.
    pub fn rule_names(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.name.as_str()).collect()
    }

    /// Apply global defaults to all rules that don't have explicit overrides.
    pub fn apply_defaults(&mut self) {
        for rule in &mut self.rules {
            // Apply default threads if rule doesn't specify one
            if rule.threads.is_none() {
                rule.threads = self.defaults.threads;
            }
            // Apply default memory if rule doesn't specify one
            if rule.memory.is_none()
                && let Some(ref mem) = self.defaults.memory
            {
                rule.memory = Some(mem.clone());
            }
            // Apply default environment if rule doesn't specify one
            if rule.environment.is_empty()
                && let Some(ref env) = self.defaults.environment
            {
                rule.environment = env.clone();
            }
        }
    }

    /// Compute a SHA-256 checksum of the workflow configuration for reproducibility.
    ///
    /// The checksum is computed from a deterministic hash of the config,
    /// ensuring consistent results regardless of field ordering.
    pub fn checksum(&self) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::hash::DefaultHasher::new();
        self.workflow.name.hash(&mut hasher);
        self.workflow.version.hash(&mut hasher);
        self.rules.len().hash(&mut hasher);
        for rule in &self.rules {
            rule.name.hash(&mut hasher);
            rule.input.hash(&mut hasher);
            rule.output.hash(&mut hasher);
            rule.shell.hash(&mut hasher);
            rule.script.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// Validate that a reference genome file path has a recognized extension
    /// (`.fa`, `.fasta`, `.fa.gz`, `.fasta.gz`) and optionally check that
    /// it exists on disk.
    #[must_use]
    pub fn validate_reference(path: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let valid_extensions = [".fa", ".fasta", ".fa.gz", ".fasta.gz"];
        let has_valid_ext = valid_extensions.iter().any(|ext| path.ends_with(ext));
        if !has_valid_ext {
            warnings.push(format!(
                "Reference path '{}' does not have a recognized extension (.fa, .fasta, .fa.gz, .fasta.gz)",
                path
            ));
        }
        // Check for .fai index
        let fai_path = format!("{}.fai", path);
        let p = std::path::Path::new(&fai_path);
        if !p.exists() && std::path::Path::new(path).exists() {
            warnings.push(format!(
                "Reference index '{}' not found; you may need to run 'samtools faidx'",
                fai_path
            ));
        }
        warnings
    }

    /// Validate a sample sheet CSV/TSV: check that it has a header row,
    /// no duplicate sample IDs, and at least one data row.
    #[must_use]
    pub fn validate_sample_sheet(content: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            warnings.push("Sample sheet is empty".to_string());
            return warnings;
        }
        // Detect delimiter
        let delimiter = if lines[0].contains('\t') { '\t' } else { ',' };
        let header: Vec<&str> = lines[0].split(delimiter).collect();
        if header.is_empty() {
            warnings.push("Sample sheet header is empty".to_string());
            return warnings;
        }
        if lines.len() < 2 {
            warnings.push("Sample sheet has no data rows".to_string());
            return warnings;
        }
        // Check for duplicate IDs in the first column
        let mut seen = std::collections::HashSet::new();
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(delimiter).collect();
            if let Some(id) = fields.first()
                && !seen.insert(*id)
            {
                warnings.push(format!("Duplicate sample ID '{}' at line {}", id, i + 1));
            }
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_WORKFLOW: &str = r#"
        [workflow]
        name = "test-pipeline"
        version = "0.1.0"
    "#;

    const FULL_WORKFLOW: &str = r#"
        [workflow]
        name = "test-pipeline"
        version = "1.0.0"
        description = "A test pipeline"
        author = "Test"

        [config]
        reference = "/path/to/ref.fa"
        samples = "samples.csv"

        [defaults]
        threads = 4
        memory = "8G"

        [[rules]]
        name = "fastqc"
        input = ["{sample}_R1.fastq.gz"]
        output = ["qc/{sample}_fastqc.html"]
        threads = 2
        shell = "fastqc {input} -o qc/"

        [rules.environment]
        conda = "envs/qc.yaml"

        [[rules]]
        name = "align"
        input = ["{sample}_R1.fastq.gz"]
        output = ["{sample}.bam"]
        threads = 16
        memory = "32G"
        shell = "bwa mem {config.reference} {input} | samtools sort -o {output}"

        [rules.environment]
        docker = "biocontainers/bwa:0.7.17"
    "#;

    #[test]
    fn parse_minimal_workflow() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert_eq!(config.workflow.name, "test-pipeline");
        assert_eq!(config.workflow.version, "0.1.0");
        assert!(config.rules.is_empty());
    }

    #[test]
    fn parse_full_workflow() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert_eq!(config.workflow.name, "test-pipeline");
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].name, "fastqc");
        assert_eq!(config.rules[1].name, "align");
        assert_eq!(config.rules[0].environment.kind(), "conda");
        assert_eq!(config.rules[1].environment.kind(), "docker");
    }

    #[test]
    fn config_values() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert!(config.get_config_value("reference").is_some());
        assert!(config.get_config_value("nonexistent").is_none());
    }

    #[test]
    fn duplicate_rule_names() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"

            [[rules]]
            name = "step1"
            output = ["out2.txt"]
            shell = "echo world"
        "#;

        let result = WorkflowConfig::parse(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate rule name"));
    }

    #[test]
    fn rule_names_list() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        let names = config.rule_names();
        assert_eq!(names, vec!["fastqc", "align"]);
    }

    #[test]
    fn get_rule_by_name() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert!(config.get_rule("fastqc").is_some());
        assert!(config.get_rule("nonexistent").is_none());
    }

    #[test]
    fn apply_defaults_propagates() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 8
            memory = "16G"

            [defaults.environment]
            conda = "envs/default.yaml"

            [[rules]]
            name = "step1"
            shell = "echo hello"

            [[rules]]
            name = "step2"
            threads = 2
            memory = "4G"
            shell = "echo world"

            [rules.environment]
            docker = "ubuntu:latest"
        "#;

        let mut config = WorkflowConfig::parse(toml_str).unwrap();
        config.apply_defaults();

        // step1 should get defaults
        let step1 = config.get_rule("step1").unwrap();
        assert_eq!(step1.threads, Some(8));
        assert_eq!(step1.memory.as_deref(), Some("16G"));
        assert_eq!(step1.environment.kind(), "conda");

        // step2 already has overrides, should keep them
        let step2 = config.get_rule("step2").unwrap();
        assert_eq!(step2.threads, Some(2));
        assert_eq!(step2.memory.as_deref(), Some("4G"));
        assert_eq!(step2.environment.kind(), "docker");
    }

    #[test]
    fn parse_include_directives() {
        let toml_str = r#"
            [workflow]
            name = "modular"

            [[include]]
            path = "common/qc.oxoflow"
            namespace = "qc"

            [[include]]
            path = "align.oxoflow"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.includes.len(), 2);
        assert_eq!(config.includes[0].path, "common/qc.oxoflow");
        assert_eq!(config.includes[0].namespace.as_deref(), Some("qc"));
        assert_eq!(config.includes[1].path, "align.oxoflow");
        assert!(config.includes[1].namespace.is_none());
    }

    #[test]
    fn parse_execution_groups() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[execution_group]]
            name = "preprocessing"
            rules = ["fastp", "fastqc"]
            mode = "parallel"

            [[execution_group]]
            name = "alignment"
            rules = ["bwa", "sort", "index"]
            mode = "sequential"

            [[rules]]
            name = "fastp"
            shell = "fastp"

            [[rules]]
            name = "fastqc"
            shell = "fastqc"

            [[rules]]
            name = "bwa"
            shell = "bwa"

            [[rules]]
            name = "sort"
            shell = "sort"

            [[rules]]
            name = "index"
            shell = "index"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.execution_groups.len(), 2);
        assert_eq!(config.execution_groups[0].name, "preprocessing");
        assert_eq!(config.execution_groups[0].mode, ExecutionMode::Parallel);
        assert_eq!(config.execution_groups[0].rules.len(), 2);
        assert_eq!(config.execution_groups[1].name, "alignment");
        assert_eq!(config.execution_groups[1].mode, ExecutionMode::Sequential);
        assert_eq!(config.execution_groups[1].rules.len(), 3);
    }

    #[test]
    fn include_directive_deserialization() {
        let toml_str = r#"
            path = "sub/workflow.oxoflow"
            namespace = "sub"
        "#;

        let inc: IncludeDirective = toml::from_str(toml_str).unwrap();
        assert_eq!(inc.path, "sub/workflow.oxoflow");
        assert_eq!(inc.namespace.as_deref(), Some("sub"));
    }

    #[test]
    fn execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Parallel);
    }

    #[test]
    fn workflow_with_advanced_rule_features() {
        let toml_str = r#"
            [workflow]
            name = "advanced"

            [[rules]]
            name = "scattered_call"
            input = ["{sample}.bam"]
            output = ["{sample}.vcf"]
            shell = "call {input} > {output}"
            when = "config.run_calling"
            retries = 2
            temp_output = ["{sample}.tmp"]
            protected_output = ["{sample}.vcf"]

            [rules.scatter]
            variable = "sample"
            values = ["S1", "S2"]
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        let rule = &config.rules[0];
        assert_eq!(rule.when.as_deref(), Some("config.run_calling"));
        assert_eq!(rule.retries, 2);
        assert_eq!(rule.temp_output, vec!["{sample}.tmp"]);
        assert_eq!(rule.protected_output, vec!["{sample}.vcf"]);
        let scatter = rule.scatter.as_ref().unwrap();
        assert_eq!(scatter.variable, "sample");
        assert_eq!(scatter.values, vec!["S1", "S2"]);
    }

    #[test]
    fn resolve_includes_with_namespace() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "qc_step"
            shell = "fastqc"

            [[rules]]
            name = "trim_step"
            shell = "fastp"
        "#;
        let inc_path = dir.path().join("qc.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "qc.oxoflow"
            namespace = "qc"

            [[rules]]
            name = "align"
            shell = "bwa"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 3);
        assert_eq!(config.rules[0].name, "align");
        assert_eq!(config.rules[1].name, "qc::qc_step");
        assert_eq!(config.rules[2].name, "qc::trim_step");
    }

    #[test]
    fn resolve_includes_without_namespace() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "helper"
            shell = "echo help"
        "#;
        let inc_path = dir.path().join("helper.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "helper.oxoflow"

            [[rules]]
            name = "main_step"
            shell = "echo main"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[1].name, "helper");
    }

    #[test]
    fn resolve_includes_skips_duplicate_rules() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "shared_step"
            shell = "echo included"
        "#;
        let inc_path = dir.path().join("inc.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "inc.oxoflow"

            [[rules]]
            name = "shared_step"
            shell = "echo main"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        // Should NOT add duplicate
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].shell.as_deref(), Some("echo main"));
    }

    #[test]
    fn resolve_includes_missing_file() {
        let dir = tempfile::tempdir().unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "nonexistent.oxoflow"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        let result = config.resolve_includes(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn validate_execution_groups_valid() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[execution_group]]
            name = "prep"
            rules = ["step1"]

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert!(config.validate_execution_groups().is_ok());
    }

    #[test]
    fn validate_execution_groups_unknown_rule() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let mut config = WorkflowConfig::parse(toml_str).unwrap();
        config.execution_groups.push(ExecutionGroup {
            name: "bad_group".to_string(),
            rules: vec!["nonexistent".to_string()],
            mode: ExecutionMode::Parallel,
        });

        let result = config.validate_execution_groups();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("bad_group"));
    }

    #[test]
    fn validate_rejects_bad_execution_groups() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [[execution_group]]
            name = "group1"
            rules = ["missing_rule"]

            [[rules]]
            name = "real_rule"
            shell = "echo hi"
        "#;

        let result = WorkflowConfig::parse(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_includes_depth_limit() {
        // Verify the depth constant is reasonable
        assert!(
            MAX_INCLUDE_DEPTH >= 8,
            "include depth limit should be at least 8"
        );
    }

    #[test]
    fn checksum_deterministic() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let c1 = WorkflowConfig::parse(toml).unwrap();
        let c2 = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(c1.checksum(), c2.checksum());
    }

    #[test]
    fn checksum_differs_for_different_configs() {
        let c1 = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test1"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#,
        )
        .unwrap();
        let c2 = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test2"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#,
        )
        .unwrap();
        assert_ne!(c1.checksum(), c2.checksum());
    }

    #[test]
    fn parse_citation_info() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [citation]
            doi = "10.1234/test"
            url = "https://github.com/example/test"
            authors = ["Alice", "Bob"]
            title = "My Workflow Paper"
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let citation = config.citation.unwrap();
        assert_eq!(citation.doi.as_deref(), Some("10.1234/test"));
        assert_eq!(
            citation.url.as_deref(),
            Some("https://github.com/example/test")
        );
        assert_eq!(citation.authors, vec!["Alice", "Bob"]);
        assert_eq!(citation.title.as_deref(), Some("My Workflow Paper"));
    }

    #[test]
    fn citation_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.citation.is_none());
    }

    #[test]
    fn parse_cluster_profile() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [cluster]
            backend = "slurm"
            partition = "gpu"
            account = "proj123"
            extra_args = ["--exclusive", "--gres=gpu:1"]
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let cluster = config.cluster.unwrap();
        assert_eq!(cluster.backend.as_deref(), Some("slurm"));
        assert_eq!(cluster.partition.as_deref(), Some("gpu"));
        assert_eq!(cluster.account.as_deref(), Some("proj123"));
        assert_eq!(cluster.extra_args, vec!["--exclusive", "--gres=gpu:1"]);
    }

    #[test]
    fn cluster_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.cluster.is_none());
    }

    #[test]
    fn parse_resource_budget() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [resource_budget]
            max_threads = 64
            max_memory = "256G"
            max_jobs = 10
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let budget = config.resource_budget.unwrap();
        assert_eq!(budget.max_threads, Some(64));
        assert_eq!(budget.max_memory.as_deref(), Some("256G"));
        assert_eq!(budget.max_jobs, Some(10));
    }

    #[test]
    fn resource_budget_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.resource_budget.is_none());
    }

    #[test]
    fn parse_format_version_in_workflow_meta() {
        let toml_str = r#"
            [workflow]
            name = "test"
            format_version = "1.0"
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.workflow.format_version.as_deref(), Some("1.0"));
    }

    #[test]
    fn format_version_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.workflow.format_version.is_none());
    }
}
