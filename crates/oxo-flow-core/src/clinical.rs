//! Clinical and domain-specific types for bioinformatics workflows.
//!
//! Provides strongly-typed structures for variant classification,
//! biomarker results, QC thresholds, and clinical reporting sections.

use serde::{Deserialize, Serialize};

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
    /// Match ID for experiment-control pairing.
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
