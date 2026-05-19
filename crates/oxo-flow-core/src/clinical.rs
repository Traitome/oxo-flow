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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- VariantClassification ---

    #[test]
    fn variant_classification_display_tiers() {
        assert_eq!(VariantClassification::TierI.to_string(), "Tier I");
        assert_eq!(VariantClassification::TierII.to_string(), "Tier II");
        assert_eq!(VariantClassification::TierIII.to_string(), "Tier III");
        assert_eq!(VariantClassification::TierIV.to_string(), "Tier IV");
    }

    #[test]
    fn variant_classification_display_acmg() {
        assert_eq!(VariantClassification::Pathogenic.to_string(), "Pathogenic");
        assert_eq!(
            VariantClassification::LikelyPathogenic.to_string(),
            "Likely Pathogenic"
        );
        assert_eq!(VariantClassification::Vus.to_string(), "VUS");
        assert_eq!(
            VariantClassification::LikelyBenign.to_string(),
            "Likely Benign"
        );
        assert_eq!(VariantClassification::Benign.to_string(), "Benign");
    }

    #[test]
    fn variant_classification_json_roundtrip() {
        let classifications = vec![
            VariantClassification::TierI,
            VariantClassification::Pathogenic,
            VariantClassification::Vus,
            VariantClassification::Benign,
        ];
        for vc in classifications {
            let json = serde_json::to_string(&vc).unwrap();
            let restored: VariantClassification = serde_json::from_str(&json).unwrap();
            assert_eq!(vc, restored);
        }
    }

    // --- BiomarkerResult ---

    #[test]
    fn biomarker_result_display() {
        let bm = BiomarkerResult {
            name: "TMB".to_string(),
            value: 12.5,
            unit: "mutations/Mb".to_string(),
            classification: Some("TMB-High".to_string()),
            threshold: Some(10.0),
        };
        let display = bm.to_string();
        assert!(display.contains("TMB: 12.50"));
        assert!(display.contains("mutations/Mb"));
        assert!(display.contains("TMB-High"));
    }

    #[test]
    fn biomarker_result_no_classification() {
        let bm = BiomarkerResult {
            name: "HRD".to_string(),
            value: 42.0,
            unit: "score".to_string(),
            classification: None,
            threshold: None,
        };
        assert_eq!(bm.to_string(), "HRD: 42.00 score");
    }

    #[test]
    fn biomarker_result_json_roundtrip() {
        let bm = BiomarkerResult {
            name: "MSI".to_string(),
            value: 0.15,
            unit: "score".to_string(),
            classification: Some("MSI-H".to_string()),
            threshold: Some(0.2),
        };
        let json = serde_json::to_string(&bm).unwrap();
        let restored: BiomarkerResult = serde_json::from_str(&json).unwrap();
        assert_eq!(bm, restored);
    }

    // --- TumorSampleMeta ---

    #[test]
    fn tumor_sample_meta_default() {
        let meta = TumorSampleMeta::default();
        assert!(meta.tumor_purity.is_none());
        assert!(meta.ploidy.is_none());
        assert!(meta.sample_type.is_none());
        assert!(meta.match_id.is_none());
    }

    #[test]
    fn tumor_sample_meta_with_fields() {
        let meta = TumorSampleMeta {
            tumor_purity: Some(0.8),
            ploidy: Some(2.0),
            sample_type: Some("tumor".to_string()),
            match_id: Some("PAIR_01".to_string()),
        };
        assert_eq!(meta.tumor_purity, Some(0.8));
        assert_eq!(meta.sample_type.as_deref(), Some("tumor"));
    }

    // --- QcThreshold ---

    #[test]
    fn qc_threshold_passes_within_bounds() {
        let threshold = QcThreshold {
            metric: "mean_coverage".to_string(),
            min: Some(30.0),
            max: Some(100.0),
            description: None,
        };
        assert!(threshold.passes(50.0));
    }

    #[test]
    fn qc_threshold_fails_below_min() {
        let threshold = QcThreshold {
            metric: "mean_coverage".to_string(),
            min: Some(30.0),
            max: None,
            description: None,
        };
        assert!(!threshold.passes(20.0));
    }

    #[test]
    fn qc_threshold_fails_above_max() {
        let threshold = QcThreshold {
            metric: "duplicate_rate".to_string(),
            min: None,
            max: Some(0.3),
            description: None,
        };
        assert!(!threshold.passes(0.5));
    }

    #[test]
    fn qc_threshold_passes_at_boundary() {
        let threshold = QcThreshold {
            metric: "mean_coverage".to_string(),
            min: Some(30.0),
            max: Some(100.0),
            description: None,
        };
        assert!(threshold.passes(30.0));
        assert!(threshold.passes(100.0));
    }

    #[test]
    fn qc_threshold_display() {
        let threshold = QcThreshold {
            metric: "mean_coverage".to_string(),
            min: Some(30.0),
            max: Some(100.0),
            description: None,
        };
        let display = threshold.to_string();
        assert!(display.contains("mean_coverage"));
        assert!(display.contains("30.00"));
        assert!(display.contains("100.00"));
    }

    // --- ComplianceEvent ---

    #[test]
    fn compliance_event_fields() {
        let event = ComplianceEvent {
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            event_type: "analysis_started".to_string(),
            actor: "operator@lab".to_string(),
            description: "Tumor-normal pipeline started for CASE_001".to_string(),
            evidence_hash: Some("sha256:abc123".to_string()),
        };
        assert_eq!(event.event_type, "analysis_started");
        assert_eq!(event.actor, "operator@lab");
        assert_eq!(event.evidence_hash, Some("sha256:abc123".to_string()));
    }

    #[test]
    fn compliance_event_json_roundtrip() {
        let event = ComplianceEvent {
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            event_type: "result_reviewed".to_string(),
            actor: "reviewer@lab".to_string(),
            description: "Results reviewed and approved".to_string(),
            evidence_hash: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: ComplianceEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, restored);
    }

    // --- GenePanel ---

    #[test]
    fn gene_panel_display() {
        let panel = GenePanel {
            name: "Oncomine Focus".to_string(),
            version: Some("2.0".to_string()),
            genes: vec!["BRCA1".to_string(), "TP53".to_string(), "EGFR".to_string()],
            bed_file: Some("panels/oncomine_focus.bed".to_string()),
        };
        let display = panel.to_string();
        assert!(display.contains("Oncomine Focus"));
        assert!(display.contains("3 genes"));
        assert!(display.contains("v2.0"));
    }

    #[test]
    fn gene_panel_no_version() {
        let panel = GenePanel {
            name: "Custom Panel".to_string(),
            version: None,
            genes: vec!["BRCA1".to_string()],
            bed_file: None,
        };
        assert_eq!(panel.to_string(), "Custom Panel (1 genes)");
    }

    #[test]
    fn gene_panel_json_roundtrip() {
        let panel = GenePanel {
            name: "Test Panel".to_string(),
            version: Some("1.0".to_string()),
            genes: vec!["GENE1".to_string(), "GENE2".to_string()],
            bed_file: None,
        };
        let json = serde_json::to_string(&panel).unwrap();
        let restored: GenePanel = serde_json::from_str(&json).unwrap();
        assert_eq!(panel, restored);
    }

    // --- ActionabilityAnnotation ---

    #[test]
    fn actionability_annotation_fields() {
        let annot = ActionabilityAnnotation {
            source: "OncoKB".to_string(),
            evidence_level: "Level 1".to_string(),
            therapy: Some("Osimertinib".to_string()),
            disease: Some("NSCLC".to_string()),
        };
        assert_eq!(annot.source, "OncoKB");
        assert_eq!(annot.evidence_level, "Level 1");
        assert_eq!(annot.therapy.as_deref(), Some("Osimertinib"));
    }

    #[test]
    fn actionability_annotation_json_roundtrip() {
        let annot = ActionabilityAnnotation {
            source: "ClinVar".to_string(),
            evidence_level: "Level 2A".to_string(),
            therapy: None,
            disease: Some("Breast Cancer".to_string()),
        };
        let json = serde_json::to_string(&annot).unwrap();
        let restored: ActionabilityAnnotation = serde_json::from_str(&json).unwrap();
        assert_eq!(annot, restored);
    }

    // --- FilterChain ---

    #[test]
    fn filter_chain_fields() {
        let fc = FilterChain {
            name: "Tumor-Normal Filter".to_string(),
            filters: vec![
                "AF >= 0.05".to_string(),
                "DP >= 20".to_string(),
                "POP_MAX_AF < 0.01".to_string(),
            ],
            hard: vec![true, true, false],
        };
        assert_eq!(fc.name, "Tumor-Normal Filter");
        assert_eq!(fc.filters.len(), 3);
        assert_eq!(fc.hard.len(), 3);
    }

    #[test]
    fn filter_chain_json_roundtrip() {
        let fc = FilterChain {
            name: "QC Filter".to_string(),
            filters: vec!["DP >= 10".to_string()],
            hard: vec![true],
        };
        let json = serde_json::to_string(&fc).unwrap();
        let restored: FilterChain = serde_json::from_str(&json).unwrap();
        assert_eq!(fc, restored);
    }

    // --- ClinicalReportSection ---

    #[test]
    fn clinical_report_section_display() {
        assert_eq!(
            ClinicalReportSection::SpecimenInfo.to_string(),
            "Specimen Information"
        );
        assert_eq!(
            ClinicalReportSection::Methodology.to_string(),
            "Methodology"
        );
        assert_eq!(ClinicalReportSection::Results.to_string(), "Results");
        assert_eq!(
            ClinicalReportSection::QualityControl.to_string(),
            "Quality Control"
        );
    }

    #[test]
    fn clinical_report_section_json_roundtrip() {
        let sections = vec![
            ClinicalReportSection::Interpretation,
            ClinicalReportSection::Limitations,
            ClinicalReportSection::Appendix,
        ];
        for section in sections {
            let json = serde_json::to_string(&section).unwrap();
            let restored: ClinicalReportSection = serde_json::from_str(&json).unwrap();
            assert_eq!(section, restored);
        }
    }
}
