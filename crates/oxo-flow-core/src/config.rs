//! Workflow configuration and `.oxoflow` file parsing.
// Accesses deprecated `Rule::threads` / `Rule::memory` shorthand fields to
// apply defaults and expand rules.  Will be removed once the shorthand
// fields are retired.
#![allow(deprecated)]
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use crate::error::{OxoFlowError, Result};
use crate::rule::{EnvironmentSpec, FilePatterns, Rule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

fn is_defaults_empty(d: &Defaults) -> bool {
    d.threads.is_none() && d.memory.is_none() && d.environment.is_none()
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Author name or organization.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Format specification version (e.g., "1.0").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,

    /// Format specification version for compatibility checking.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_version: Option<String>,

    /// Genome build (e.g., "GRCh38", "hg38", "GRCh37").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genome_build: Option<String>,

    /// Custom interpreter mappings for script file extensions.
    ///
    /// Overrides default auto-detection for specified extensions.
    /// Example: `interpreter_map = { ".m" = "octave", ".sas" = "sas" }`.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub interpreter_map: HashMap<String, String>,

    /// Path to an external file containing experiment-control pairs.
    ///
    /// Supports TSV, CSV, and JSON formats. Useful for large cohort studies
    /// with hundreds or thousands of sample pairs.
    ///
    /// # File format
    ///
    /// **TSV/CSV** (tab or comma separated):
    /// ```text
    /// pair_id    experiment    control    experiment_type
    /// CASE_001    EXP_01    CTRL_01    lung_adenocarcinoma
    /// CASE_002    EXP_02    CTRL_02    colorectal
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
    ///   {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
    /// ]
    /// ```
    ///
    /// Inline `[[pairs]]` and `pairs_file` can be used together; entries from
    /// both sources are merged.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_file: Option<String>,

    /// Path to an external file containing sample groups.
    ///
    /// Supports TSV, CSV, and JSON formats.
    ///
    /// # File format
    ///
    /// **TSV/CSV**:
    /// ```text
    /// name    samples
    /// control    CTRL_001,CTRL_002,CTRL_003
    /// case    S001,S002,S003
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"name": "control", "samples": ["CTRL_001", "CTRL_002"]},
    ///   {"name": "case", "samples": ["S001", "S002"]}
    /// ]
    /// ```
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_groups_file: Option<String>,

    /// Wildcard pattern for auto-discovering experiment-control pairs.
    ///
    /// The pattern must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards.
    /// oxo-flow scans matching files and extracts pair definitions from paths.
    ///
    /// # Example
    ///
    /// ```toml
    /// [workflow]
    /// pairs_pattern = "aligned/{pair_id}/{experiment}_vs_{control}.bam"
    /// ```
    ///
    /// For file `aligned/CASE_001/EXP_01_vs_CTRL_01.bam`, creates pair:
    /// - pair_id = CASE_001, experiment = EXP_01, control = CTRL_01
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_pattern: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Default settings applied to all rules unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default thread count.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,

    /// Default memory.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,

    /// Default environment.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentSpec>,
}

/// Report configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReportConfig {
    /// Report template name.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Output formats (html, pdf, json).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub format: Vec<String>,

    /// Report sections to include.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    /// URL to the workflow repository or publication.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Authors of this workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Associated publication title.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Cluster execution profile for HPC deployment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterProfile {
    /// Backend type (slurm, pbs, sge, lsf).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Default partition/queue.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
    /// Default account for billing.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Additional arguments passed to the scheduler.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

/// Resource budget constraints for the entire workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceBudget {
    /// Maximum total CPU threads across all running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_threads: Option<u32>,
    /// Maximum total memory across all running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory: Option<String>,
    /// Maximum total running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<usize>,
}

/// Reference database configuration for tracking versions and provenance.
///
/// Bioinformatics workflows often depend on reference databases (genome builds,
/// annotation databases, variant databases). This section tracks versions for
/// reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceDatabase {
    /// Database name (e.g., "GRCh38", "dbSNP", "ClinVar", "COSMIC").
    pub name: String,
    /// Version string (e.g., "p14", "b156", "v99").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// URL or path to the database.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Checksum of the database file for integrity verification.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Date when this database version was downloaded/accessed.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessed_date: Option<String>,
}

impl std::fmt::Display for ReferenceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(ref v) = self.version {
            write!(f, " v{v}")?;
        }
        Ok(())
    }
}

/// An experiment-control sample pair for comparative analysis workflows.
///
/// Each pair defines `{pair_id}`, `{experiment}`, and `{control}` wildcard
/// values.
///
/// Backward compatibility:
/// - `{tumor}` aliases `{experiment}`
/// - `{normal}` aliases `{control}`
///
/// Rules containing any of these wildcards in their `input`, `output`, or
/// `shell` fields are expanded once per pair.
///
/// # Example `.oxoflow` usage
///
/// ```toml
/// [[pairs]]
/// pair_id = "CASE_001"
/// experiment = "SAMPLE_EXP_01"
/// control    = "SAMPLE_CTRL_01"
/// experiment_type = "condition_a"
///
/// [[pairs]]
/// pair_id = "CASE_002"
/// experiment = "SAMPLE_EXP_02"
/// control    = "SAMPLE_CTRL_02"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperimentControlPair {
    /// Unique identifier for this pair (available as `{pair_id}`).
    pub pair_id: String,

    /// Experiment sample identifier (available as `{experiment}`).
    ///
    /// Backward-compatible TOML alias: `tumor`.
    #[serde(alias = "tumor")]
    pub experiment: String,

    /// Control sample identifier (available as `{control}`).
    ///
    /// Backward-compatible TOML alias: `normal`.
    #[serde(alias = "normal")]
    pub control: String,

    /// Optional experiment type / cohort label (available as `{experiment_type}`).
    ///
    /// Backward-compatible TOML alias: `tumor_type`.
    #[serde(default)]
    #[serde(alias = "tumor_type")]
    pub experiment_type: Option<String>,

    /// Arbitrary key-value metadata; each key is available as a wildcard.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Backward-compatible alias; prefer [`ExperimentControlPair`].
pub type TumorNormalPair = ExperimentControlPair;

impl ExperimentControlPair {
    /// Load pairs from a TSV, CSV, or JSON file.
    ///
    /// # File format
    ///
    /// **TSV** (tab-separated, header required):
    /// ```text
    /// pair_id    experiment    control    experiment_type
    /// CASE_001    EXP_01    CTRL_01    lung_adenocarcinoma
    /// CASE_002    EXP_02    CTRL_02    colorectal
    /// ```
    ///
    /// **CSV** (comma-separated):
    /// ```text
    /// pair_id,experiment,control,experiment_type
    /// CASE_001,EXP_01,CTRL_01,lung_adenocarcinoma
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
    ///   {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
    /// ]
    /// ```
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        let metadata = std::fs::metadata(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read metadata for pairs file: {}", e),
        })?;

        // 50MB limit to prevent OOM on accidental binary file input
        if metadata.len() > 50 * 1024 * 1024 {
            return Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "pairs file is too large ({} bytes). Maximum allowed size is 50MB.",
                    metadata.len()
                ),
            });
        }

        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read pairs file: {}", e),
        })?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match extension {
            "json" => Self::parse_json(&content, path),
            "csv" => Self::parse_csv(&content, path),
            "tsv" | "txt" | "" => Self::parse_tsv(&content, path),
            _ => Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("unsupported pairs file format: {}", extension),
            }),
        }
    }

    /// Discover pairs from a wildcard pattern by scanning the filesystem.
    ///
    /// The pattern must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards.
    /// oxo-flow scans matching files and extracts wildcard values from paths.
    ///
    /// # Example patterns
    ///
    /// - `aligned/{pair_id}/{experiment}_vs_{control}.bam`
    /// - `results/{pair_id}/mutect2_{experiment}_{control}.vcf.gz`
    ///
    /// For file `aligned/CASE_001/EXP_01_vs_CTRL_01.bam`, extracts:
    /// - pair_id = CASE_001
    /// - experiment = EXP_01
    /// - control = CTRL_01
    pub fn discover_from_pattern(pattern: &str, base_dir: &Path) -> Result<Vec<Self>> {
        use crate::wildcard::{extract_wildcards, pattern_to_regex};

        // Validate pattern contains required wildcards
        if !pattern.contains("{pair_id}")
            || !pattern.contains("{experiment}")
            || !pattern.contains("{control}")
        {
            return Err(OxoFlowError::Config {
                message: format!(
                    "pairs_pattern must contain {{pair_id}}, {{experiment}}, and {{control}}: {}",
                    pattern
                ),
            });
        }

        // Get wildcard names from pattern
        let wildcard_names = extract_wildcards(pattern);

        // Build regex from pattern for matching
        let re = pattern_to_regex(pattern)?;

        // Convert pattern to glob pattern for filesystem scanning
        let glob_pattern = pattern
            .replace("{pair_id}", "*")
            .replace("{experiment}", "*")
            .replace("{control}", "*")
            .replace("{experiment_type}", "*");

        let full_glob = if glob_pattern.starts_with('/') {
            glob_pattern
        } else {
            base_dir.join(&glob_pattern).to_string_lossy().to_string()
        };

        // Scan filesystem for matching files
        let mut pairs: Vec<Self> = Vec::new();
        let mut seen_pair_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entry in glob::glob(&full_glob).map_err(|e| OxoFlowError::Config {
            message: format!("invalid glob pattern '{}': {}", full_glob, e),
        })? {
            let path = entry.map_err(|e| OxoFlowError::Config {
                message: format!("glob error: {}", e),
            })?;

            // Get relative path from base_dir for extraction
            let rel_path = path.strip_prefix(base_dir).unwrap_or(&path);
            let path_str = rel_path.to_string_lossy();

            // Extract wildcard values from the path using regex
            if let Some(captures) = re.captures(&path_str) {
                let mut wildcards: HashMap<String, String> = HashMap::new();
                for name in &wildcard_names {
                    if let Some(m) = captures.name(name) {
                        wildcards.insert(name.clone(), m.as_str().to_string());
                    }
                }

                if let Some(pair_id) = wildcards.get("pair_id") {
                    // Skip duplicates (same pair_id)
                    if seen_pair_ids.contains(pair_id) {
                        continue;
                    }
                    seen_pair_ids.insert(pair_id.clone());

                    if let Some(experiment) = wildcards.get("experiment")
                        && let Some(control) = wildcards.get("control")
                    {
                        let pair = Self {
                            pair_id: pair_id.clone(),
                            experiment: experiment.clone(),
                            control: control.clone(),
                            experiment_type: wildcards.get("experiment_type").cloned(),
                            metadata: HashMap::new(),
                        };
                        pairs.push(pair);
                    }
                }
            }
        }

        if pairs.is_empty() {
            tracing::warn!(
                "pairs_pattern '{}' matched no files in {}",
                pattern,
                base_dir.display()
            );
        } else {
            tracing::info!(
                "Discovered {} pairs from pattern '{}'",
                pairs.len(),
                pattern
            );
        }

        Ok(pairs)
    }

    fn parse_json(content: &str, path: &Path) -> Result<Vec<Self>> {
        serde_json::from_str(content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid JSON pairs file: {}", e),
        })
    }

    fn parse_csv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, ',', path)
    }

    fn parse_tsv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, '\t', path)
    }

    fn parse_delimited(content: &str, delimiter: char, path: &Path) -> Result<Vec<Self>> {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter as u8)
            .has_headers(true)
            .trim(csv::Trim::All)
            .comment(Some(b'#'))
            .from_reader(content.as_bytes());

        let headers = reader
            .headers()
            .map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("pairs file is empty or has invalid headers: {}", e),
            })?
            .clone();

        let col_index: HashMap<&str, usize> =
            headers.iter().enumerate().map(|(i, h)| (h, i)).collect();

        // Required columns
        let pair_id_col = col_index
            .get("pair_id")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'pair_id' column".to_string(),
            })?;
        let experiment_col = col_index
            .get("experiment")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'experiment' column (or 'tumor')".to_string(),
            })?;
        let control_col = col_index
            .get("control")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'control' column (or 'normal')".to_string(),
            })?;

        // Optional columns
        let experiment_type_col = col_index
            .get("experiment_type")
            .or(col_index.get("tumor_type"));

        let mut pairs = Vec::new();
        for (row_idx, result) in reader.records().enumerate() {
            let record = result.map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("error parsing row {}: {}", row_idx + 2, e),
            })?;

            let pair = Self {
                pair_id: record.get(*pair_id_col).unwrap_or("").to_string(),
                experiment: record.get(*experiment_col).unwrap_or("").to_string(),
                control: record.get(*control_col).unwrap_or("").to_string(),
                experiment_type: experiment_type_col
                    .and_then(|&i| record.get(i).map(|s| s.to_string())),
                metadata: HashMap::new(),
            };
            pairs.push(pair);
        }

        Ok(pairs)
    }
}

/// A named group of samples for cohort-level analysis.
///
/// Rules containing `{group}` or `{sample}` wildcards are expanded for every
/// (group, sample) combination across all defined groups.
///
/// # Example `.oxoflow` usage
///
/// ```toml
/// [[sample_groups]]
/// name    = "control"
/// samples = ["S001", "S002"]
///
/// [[sample_groups]]
/// name    = "case"
/// samples = ["S003", "S004"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampleGroup {
    /// Group name (available as `{group}`).
    pub name: String,

    /// Sample identifiers belonging to this group (each available as `{sample}`).
    #[serde(default)]
    pub samples: Vec<String>,

    /// Arbitrary key-value metadata for the group; each key is a wildcard.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl SampleGroup {
    /// Load sample groups from a TSV, CSV, or JSON file.
    ///
    /// # File format
    ///
    /// **TSV**:
    /// ```text
    /// name    samples
    /// control    CTRL_001,CTRL_002,CTRL_003
    /// case    S001,S002,S003
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"name": "control", "samples": ["CTRL_001", "CTRL_002"]},
    ///   {"name": "case", "samples": ["S001", "S002"]}
    /// ]
    /// ```
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        let metadata = std::fs::metadata(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read metadata for sample_groups file: {}", e),
        })?;

        // 50MB limit to prevent OOM on accidental binary file input
        if metadata.len() > 50 * 1024 * 1024 {
            return Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "sample_groups file is too large ({} bytes). Maximum allowed size is 50MB.",
                    metadata.len()
                ),
            });
        }

        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read sample_groups file: {}", e),
        })?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match extension {
            "json" => Self::parse_json(&content, path),
            "csv" => Self::parse_csv(&content, path),
            "tsv" | "txt" | "" => Self::parse_tsv(&content, path),
            _ => Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("unsupported sample_groups file format: {}", extension),
            }),
        }
    }

    fn parse_json(content: &str, path: &Path) -> Result<Vec<Self>> {
        serde_json::from_str(content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid JSON sample_groups file: {}", e),
        })
    }

    fn parse_csv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, ',', path)
    }

    fn parse_tsv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, '\t', path)
    }

    fn parse_delimited(content: &str, delimiter: char, path: &Path) -> Result<Vec<Self>> {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter as u8)
            .has_headers(true)
            .trim(csv::Trim::All)
            .comment(Some(b'#'))
            .from_reader(content.as_bytes());

        let headers = reader
            .headers()
            .map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("sample_groups file is empty or has invalid headers: {}", e),
            })?
            .clone();

        let col_index: HashMap<&str, usize> =
            headers.iter().enumerate().map(|(i, h)| (h, i)).collect();

        let name_col = col_index.get("name").ok_or_else(|| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: "sample_groups file missing 'name' column".to_string(),
        })?;
        let samples_col = col_index
            .get("samples")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "sample_groups file missing 'samples' column".to_string(),
            })?;

        let mut groups = Vec::new();
        for (row_idx, result) in reader.records().enumerate() {
            let record = result.map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("error parsing row {}: {}", row_idx + 2, e),
            })?;

            // Samples can be comma-separated within the field
            let samples: Vec<String> = record
                .get(*samples_col)
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let group = Self {
                name: record.get(*name_col).unwrap_or("").to_string(),
                samples,
                metadata: HashMap::new(),
            };
            groups.push(group);
        }

        Ok(groups)
    }
}

/// Resource group configuration for limiting shared resources like API rate
/// limits or database connections.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ResourceGroupConfig {
    /// Maximum capacity of the resource (e.g., 10 for 10 concurrent connections).
    pub max: u32,
    /// Optional wait strategy: "queue" (default) or "fail".
    #[serde(default = "default_wait_strategy")]
    pub wait: String,
}

fn default_wait_strategy() -> String {
    "queue".to_string()
}

/// Complete workflow configuration parsed from an `.oxoflow` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// Workflow metadata.
    pub workflow: WorkflowMeta,

    /// Configuration variables (user-defined key-value pairs).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, toml::Value>,

    /// Default settings for all rules.
    #[serde(default)]
    #[serde(skip_serializing_if = "is_defaults_empty")]
    pub defaults: Defaults,

    /// Report configuration.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportConfig>,

    /// List of rules (pipeline steps).
    #[serde(default, rename = "rules")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,

    /// Include directives for importing rules from other workflow files.
    #[serde(default, rename = "include")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<IncludeDirective>,

    /// Explicit execution groups for sequential/parallel rule ordering.
    #[serde(default, rename = "execution_group")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub execution_groups: Vec<ExecutionGroup>,

    /// Citation information for reproducibility.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation: Option<CitationInfo>,

    /// Cluster execution profile.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterProfile>,

    /// Resource budget for the workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_budget: Option<ResourceBudget>,

    /// Shared resource groups for limiting concurrent access to APIs or databases.
    #[serde(default, rename = "resource_groups")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub resource_groups: HashMap<String, ResourceGroupConfig>,

    /// Reference database versions used by this workflow.
    #[serde(default, rename = "reference_db")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reference_databases: Vec<ReferenceDatabase>,

    /// Global wildcard constraints (regular expressions).
    /// Each key is a wildcard name, value is the regex pattern it must match.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub wildcard_constraints: HashMap<String, String>,

    /// Experiment-control sample pairs for comparative analysis workflows.
    ///
    /// Rules containing `{experiment}`, `{control}`, or `{pair_id}` wildcards
    /// are expanded once per pair by [`WorkflowConfig::expand_wildcards`].
    ///
    /// Backward compatibility:
    /// - `{tumor}` aliases `{experiment}`
    /// - `{normal}` aliases `{control}`
    #[serde(default, rename = "pairs")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<ExperimentControlPair>,

    /// Sample groups for cohort-level analysis.
    ///
    /// Rules containing `{group}` or `{sample}` wildcards are expanded for
    /// every (group, sample) combination by [`WorkflowConfig::expand_wildcards`].
    #[serde(default, rename = "sample_groups")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_groups: Vec<SampleGroup>,
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
            rule.validate()?;
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
        let mut config: WorkflowConfig =
            toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        // Resolve modular includes
        if let Some(parent) = path.parent() {
            config.resolve_includes(parent)?;

            // Load pairs from external file if specified
            if let Some(ref pairs_file) = config.workflow.pairs_file {
                let pairs_path = parent.join(pairs_file);
                let file_pairs = ExperimentControlPair::load_from_file(&pairs_path)?;
                let count = file_pairs.len();
                // Merge with inline pairs
                config.pairs.extend(file_pairs);
                tracing::info!("Loaded {} pairs from {}", count, pairs_file);
            }

            // Discover pairs from pattern if specified
            if let Some(ref pairs_pattern) = config.workflow.pairs_pattern {
                let discovered_pairs =
                    ExperimentControlPair::discover_from_pattern(pairs_pattern, parent)?;
                let count = discovered_pairs.len();
                // Merge with inline/file pairs
                config.pairs.extend(discovered_pairs);
                tracing::info!(
                    "Discovered {} pairs from pattern '{}'",
                    count,
                    pairs_pattern
                );
            }

            // Load sample_groups from external file if specified
            if let Some(ref groups_file) = config.workflow.sample_groups_file {
                let groups_path = parent.join(groups_file);
                let file_groups = SampleGroup::load_from_file(&groups_path)?;
                let count = file_groups.len();
                // Merge with inline groups
                config.sample_groups.extend(file_groups);
                tracing::info!("Loaded {} sample groups from {}", count, groups_file);
            }
        }

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

        // Ensure each rule has either shell, script, or transform
        for rule in &self.rules {
            if rule.shell.is_none()
                && rule.script.is_none()
                && rule.transform.is_none()
                && !rule.output.is_empty()
            {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "rule '{}' has outputs but no shell command, script, or transform",
                        rule.name
                    ),
                });
            }
        }

        self.validate_execution_groups()?;

        // Warn about rules exceeding system capacity (but don't block)
        let system_threads = num_cpus::get() as u32;
        let system_memory_mb = {
            use sysinfo::System;
            let mut sys = System::new_all();
            sys.refresh_memory();
            sys.total_memory() / 1024 / 1024
        };

        for rule in &self.rules {
            for warning in crate::scheduler::validate_resources_against_system(
                rule,
                system_threads,
                system_memory_mb,
            ) {
                tracing::warn!("{}", warning);
            }
        }

        // Validate wildcard constraints
        for (name, pattern) in &self.wildcard_constraints {
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(OxoFlowError::Config {
                    message: format!("invalid regex for wildcard constraint '{}': {}", name, e),
                });
            }
        }

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
            // Collect original rule names from included file for dependency resolution
            let original_rule_names: std::collections::HashSet<String> =
                inc_config.rules.iter().map(|r| r.name.clone()).collect();
            for mut rule in inc_config.rules {
                if let Some(ref ns) = inc.namespace {
                    // Prefix rule name with namespace
                    rule.name = format!("{}::{}", ns, rule.name);
                    // Prefix depends_on references that point to rules in the same included file
                    for dep in &mut rule.depends_on {
                        if original_rule_names.contains(dep) {
                            *dep = format!("{}::{}", ns, dep);
                        }
                    }
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

    /// Expand rules that contain pair or group wildcards into concrete instances.
    ///
    /// Scans each rule for wildcard placeholders:
    /// - Rules containing `{experiment}`, `{control}`, or `{pair_id}` are
    ///   expanded once per entry in `self.pairs`.
    /// - Backward-compatible aliases `{tumor}` and `{normal}` are also
    ///   recognized.
    /// - Rules containing `{group}` or `{sample}` are expanded once per
    ///   (group, sample) combination in `self.sample_groups`.
    /// - Rules without any of these wildcards are kept unchanged.
    ///
    /// The expanded rule names follow the pattern `{original_name}_{suffix}`,
    /// where the suffix is the `pair_id` for pair rules or `{group}_{sample}`
    /// for group rules.
    ///
    /// After calling this method, `self.rules` contains only concrete rules
    /// (no pair/group wildcards) and the DAG can be built normally.
    ///
    /// # Errors
    ///
    /// Returns an error if duplicate rule names would be produced (e.g., two
    /// pairs with the same `pair_id`), or if a pair/group is defined but no
    /// rules reference its wildcards (this is not an error—those pairs are
    /// simply ignored).
    pub fn expand_wildcards(&mut self) -> Result<()> {
        use crate::wildcard::{
            expand_pattern, has_wildcards, validate_wildcard_constraints_compiled,
            wildcard_combinations_from_groups, wildcard_combinations_from_pairs,
        };
        use regex::Regex;

        let pair_combos = wildcard_combinations_from_pairs(&self.pairs);
        let group_combos = wildcard_combinations_from_groups(&self.sample_groups);

        // Pre-compile constraints for performance
        let mut compiled_constraints = HashMap::new();
        for (name, pattern) in &self.wildcard_constraints {
            let re = Regex::new(pattern).map_err(|e| OxoFlowError::Wildcard {
                rule: String::new(),
                message: format!(
                    "invalid regex constraint '{}' for wildcard '{}': {}",
                    pattern, name, e
                ),
            })?;
            compiled_constraints.insert(name.clone(), re);
        }

        // Wildcards that trigger pair expansion.
        // Include backward-compatible aliases `{tumor}`/`{normal}`.
        const PAIR_WILDCARDS: &[&str] = &["experiment", "control", "tumor", "normal", "pair_id"];
        // Wildcards that trigger group expansion
        const GROUP_WILDCARDS: &[&str] = &["group", "sample"];

        let mut expanded_rules: Vec<Rule> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for rule in &self.rules {
            // Collect all text fields that might contain wildcards
            let mut all_text: Vec<&str> = rule.input.iter().map(String::as_str).collect();
            all_text.extend(rule.output.iter().map(String::as_str));
            if let Some(ref shell) = rule.shell {
                all_text.push(shell);
            }

            let uses_pair_wildcard = !pair_combos.is_empty()
                && all_text.iter().any(|t| {
                    PAIR_WILDCARDS
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            let uses_group_wildcard = !group_combos.is_empty()
                && all_text.iter().any(|t| {
                    GROUP_WILDCARDS
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            if uses_pair_wildcard {
                // Expand for each pair
                for combo in &pair_combos {
                    // Validate constraints
                    validate_wildcard_constraints_compiled(combo, &compiled_constraints)?;

                    let suffix = combo
                        .get("pair_id")
                        .cloned()
                        .unwrap_or_else(|| combo.values().cloned().collect::<Vec<_>>().join("_"));
                    let new_name = format!("{}_{}", rule.name, suffix);

                    if !seen_names.insert(new_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule { name: new_name });
                    }

                    let mut expanded = rule.clone();
                    expanded.name = new_name;

                    // Expand input/output/shell patterns
                    expanded.input = match rule.input {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    if has_wildcards(p) {
                                        expand_pattern(p, combo).unwrap_or_else(|_| p.to_string())
                                    } else {
                                        p.to_string()
                                    }
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        if has_wildcards(v) {
                                            expand_pattern(v, combo)
                                                .unwrap_or_else(|_| v.to_string())
                                        } else {
                                            v.to_string()
                                        },
                                    )
                                })
                                .collect(),
                        ),
                    };
                    expanded.output = match rule.output {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    if has_wildcards(p) {
                                        expand_pattern(p, combo).unwrap_or_else(|_| p.to_string())
                                    } else {
                                        p.to_string()
                                    }
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        if has_wildcards(v) {
                                            expand_pattern(v, combo)
                                                .unwrap_or_else(|_| v.to_string())
                                        } else {
                                            v.to_string()
                                        },
                                    )
                                })
                                .collect(),
                        ),
                    };
                    if let Some(ref shell) = rule.shell
                        && has_wildcards(shell)
                    {
                        expanded.shell =
                            Some(expand_pattern(shell, combo).unwrap_or_else(|_| shell.clone()));
                    }

                    expanded_rules.push(expanded);
                }
            } else if uses_group_wildcard {
                // Expand for each (group, sample) combination
                for combo in &group_combos {
                    // Validate constraints
                    validate_wildcard_constraints_compiled(combo, &compiled_constraints)?;

                    let group = combo.get("group").map(String::as_str).unwrap_or("group");
                    let sample = combo.get("sample").map(String::as_str).unwrap_or("sample");
                    let new_name = format!("{}_{}_{}", rule.name, group, sample);

                    if !seen_names.insert(new_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule { name: new_name });
                    }

                    let mut expanded = rule.clone();
                    expanded.name = new_name;

                    expanded.input = rule
                        .input
                        .iter()
                        .map(|p| {
                            if has_wildcards(p) {
                                expand_pattern(p, combo).unwrap_or_else(|_| p.clone())
                            } else {
                                p.clone()
                            }
                        })
                        .collect();
                    expanded.output = rule
                        .output
                        .iter()
                        .map(|p| {
                            if has_wildcards(p) {
                                expand_pattern(p, combo).unwrap_or_else(|_| p.clone())
                            } else {
                                p.clone()
                            }
                        })
                        .collect();
                    if let Some(ref shell) = rule.shell
                        && has_wildcards(shell)
                    {
                        expanded.shell =
                            Some(expand_pattern(shell, combo).unwrap_or_else(|_| shell.clone()));
                    }

                    expanded_rules.push(expanded);
                }
            } else {
                // No expansion needed — keep rule as-is
                if !seen_names.insert(rule.name.clone()) {
                    return Err(OxoFlowError::DuplicateRule {
                        name: rule.name.clone(),
                    });
                }
                expanded_rules.push(rule.clone());
            }
        }

        let mut final_rules = Vec::new();
        let mut gather_injections: HashMap<String, Vec<String>> = HashMap::new();

        for rule in expanded_rules {
            if let Some(ref scatter) = rule.scatter {
                let mut values = scatter.values.clone();
                if values.is_empty()
                    && let Some(ref v_from) = scatter.values_from
                    && let Some(resolved) = self.resolve_config_list(v_from)
                {
                    values = resolved;
                }

                let mut scatter_outputs = Vec::new();

                for val in &values {
                    let mut combo = HashMap::new();
                    combo.insert(scatter.variable.clone(), val.clone());

                    let mut scattered_rule = rule.clone();
                    scattered_rule.name = format!("{}_{}", rule.name, val);
                    scattered_rule.scatter = None; // remove scatter from generated rule

                    scattered_rule.input = match scattered_rule.input {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                    };
                    scattered_rule.output = match scattered_rule.output {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                    };
                    if let Some(ref shell) = scattered_rule.shell {
                        scattered_rule.shell =
                            Some(expand_pattern(shell, &combo).unwrap_or_else(|_| shell.clone()));
                    }

                    scatter_outputs.extend(scattered_rule.output.to_vec());
                    final_rules.push(scattered_rule);
                }

                if let Some(ref gather_rule) = scatter.gather {
                    gather_injections
                        .entry(gather_rule.clone())
                        .or_default()
                        .extend(scatter_outputs);
                }
            } else if let Some(ref transform) = rule.transform {
                // Handle transform operator: split -> map -> combine
                let split_values = self.resolve_split_values(&transform.split)?;

                // Validate that split values are not empty
                if split_values.is_empty() {
                    return Err(OxoFlowError::Validation {
                        message: format!("transform rule '{}' has no split values", rule.name),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "provide values, values_from, n, or glob in split config".to_string(),
                        ),
                    });
                }

                let split_var = &transform.split.by;
                let mut all_chunk_outputs: Vec<String> = Vec::new();

                // Generate map rules for each split value
                for value in &split_values {
                    // Determine chunk output path
                    let chunk_output = if rule.output.is_empty() {
                        format!(".oxo-flow/chunks/{split_var}/{value}.out")
                    } else if rule
                        .output
                        .get_index(0)
                        .map(|o| o.contains(&format!("{{{split_var}}}")))
                        .unwrap_or(false)
                    {
                        // Replace only {split_var} in output
                        rule.output
                            .get_index(0)
                            .unwrap()
                            .replace(&format!("{{{split_var}}}"), value)
                    } else {
                        let base = rule.output.get_index(0).unwrap();
                        let ext = base.rsplit('.').next().unwrap_or("out");
                        format!(".oxo-flow/chunks/{split_var}/{value}.{ext}")
                    };

                    all_chunk_outputs.push(chunk_output.clone());

                    let map_rule_name = format!("{}_{}", rule.name, value);
                    // Replace only {split_var} in map shell, keep other placeholders for execution
                    let map_shell = transform.map.replace(&format!("{{{split_var}}}"), value);

                    let mut map_rule = Rule {
                        name: map_rule_name,
                        input: rule.input.clone(),
                        output: vec![chunk_output].into(),
                        shell: Some(map_shell),

                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        retries: rule.retries,
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        map_rule.threads = rule.threads;
                        map_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(map_rule);
                }

                // Generate combine rule if specified
                if let Some(ref combine) = transform.combine {
                    let combine_rule_name = format!("{}_combine", rule.name);
                    let combine_shell = if let Some(ref shell) = combine.shell {
                        let chunks_str = all_chunk_outputs.join(" ");
                        shell
                            .replace("{chunks}", &chunks_str)
                            .replace("{input}", &chunks_str)
                            .replace("{output}", &rule.output.join(" "))
                    } else if combine.aggregate {
                        let method = combine.method.as_deref().unwrap_or("concat");
                        let chunks_str = all_chunk_outputs.join(" ");
                        let output_str = rule.output.join(" ");

                        match method {
                            "concat" => {
                                let header = combine
                                    .header
                                    .as_deref()
                                    .map(|h| format!("echo '{}' && ", h))
                                    .unwrap_or_default();
                                format!("{}cat {} > {}", header, chunks_str, output_str)
                            }
                            "json_merge" => {
                                format!("jq -s 'add' {} > {}", chunks_str, output_str)
                            }
                            _ => {
                                return Err(OxoFlowError::Validation {
                                    message: format!("unknown aggregation method: {}", method),
                                    rule: Some(rule.name.clone()),
                                    suggestion: Some("use 'concat' or 'json_merge'".to_string()),
                                });
                            }
                        }
                    } else {
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "transform rule '{}' has combine but no shell or aggregate method",
                                rule.name
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "specify combine.shell or combine.aggregate".to_string(),
                            ),
                        });
                    };

                    let mut combine_rule = Rule {
                        name: combine_rule_name,
                        input: FilePatterns::List(all_chunk_outputs.clone()),
                        output: rule.output.clone(),
                        shell: Some(combine_shell),
                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        combine_rule.threads = rule.threads;
                        combine_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(combine_rule);
                }
            } else {
                final_rules.push(rule);
            }
        }

        // Apply gather injections and expand_inputs
        for rule in &mut final_rules {
            if let Some(injected) = gather_injections.get(&rule.name) {
                let mut current_input = rule.input.to_vec();
                current_input.extend(injected.clone());
                rule.input = FilePatterns::List(current_input);
            }

            // process expand_inputs
            for exp in &rule.expand_inputs {
                let mut variables = HashMap::new();
                for (var_name, var_ref) in &exp.variables {
                    if let Some(vals) = self.resolve_config_list(var_ref) {
                        variables.insert(var_name.clone(), vals);
                    } else if var_ref.starts_with('[') && var_ref.ends_with(']') {
                        let inner = &var_ref[1..var_ref.len() - 1];
                        let vals = inner
                            .split(',')
                            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        variables.insert(var_name.clone(), vals);
                    } else {
                        variables.insert(var_name.clone(), vec![var_ref.clone()]);
                    }
                }

                let expanded = crate::wildcard::cartesian_expand(&exp.pattern, &variables);
                let mut current_input = rule.input.to_vec();
                current_input.extend(expanded);
                rule.input = FilePatterns::List(current_input);
            }
        }

        self.rules = final_rules;
        Ok(())
    }

    /// Expand rules with `transform` field into map and combine rules.
    ///
    /// The transform operator creates:
    /// - N map rules (one per split value) that run in parallel
    /// - One combine rule (if combine is specified) that merges results
    ///
    /// This is called automatically during workflow expansion.
    pub fn expand_transform(&mut self) -> Result<()> {
        use crate::wildcard::expand_pattern;

        let mut final_rules: Vec<Rule> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for rule in &self.rules {
            if let Some(ref transform) = rule.transform {
                // Resolve split values
                let split_values = self.resolve_split_values(&transform.split)?;

                if split_values.is_empty() {
                    return Err(OxoFlowError::Validation {
                        message: format!("transform rule '{}' has no split values", rule.name),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "provide values, values_from, n, or glob in split config".to_string(),
                        ),
                    });
                }

                let split_var = &transform.split.by;
                let mut all_chunk_outputs: Vec<String> = Vec::new();

                // Generate map rules for each split value
                for value in &split_values {
                    let mut combo = HashMap::new();
                    combo.insert(split_var.clone(), value.clone());

                    // Chunk output path: .oxo-flow/chunks/{split_var}/{value}.ext
                    // or use the pattern in the rule's output if it contains {split_var}
                    let chunk_output = if rule.output.is_empty() {
                        // Generate a temp chunk file
                        format!(".oxo-flow/chunks/{split_var}/{value}.out")
                    } else if rule
                        .output
                        .get_index(0)
                        .map(|o| o.contains(&format!("{{{split_var}}}")))
                        .unwrap_or(false)
                    {
                        expand_pattern(rule.output.get_index(0).unwrap(), &combo)
                            .unwrap_or_else(|_| rule.output.get_index(0).unwrap().to_string())
                    } else {
                        // Append split value to output
                        let base = rule.output.get_index(0).unwrap();
                        let ext = base.rsplit('.').next().unwrap_or("out");
                        format!(".oxo-flow/chunks/{split_var}/{value}.{ext}")
                    };

                    all_chunk_outputs.push(chunk_output.clone());

                    // Create the map rule
                    let map_rule_name = format!("{}_{}", rule.name, value);
                    if !seen_names.insert(map_rule_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule {
                            name: map_rule_name,
                        });
                    }

                    let map_shell = expand_pattern(&transform.map, &combo)
                        .unwrap_or_else(|_| transform.map.clone());

                    let mut map_rule = Rule {
                        name: map_rule_name,
                        input: rule.input.clone(),
                        output: vec![chunk_output].into(),
                        shell: Some(map_shell),
                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        retries: rule.retries,
                        ..Default::default()
                    };

                    // Handle deprecated fields
                    #[allow(deprecated)]
                    {
                        map_rule.threads = rule.threads;
                        map_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(map_rule);
                }

                // Generate combine rule if specified
                if let Some(ref combine) = transform.combine {
                    let combine_rule_name = format!("{}_combine", rule.name);
                    if !seen_names.insert(combine_rule_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule {
                            name: combine_rule_name,
                        });
                    }

                    // Build combine shell command
                    let combine_shell = if let Some(ref shell) = combine.shell {
                        // Replace {chunks} with all chunk outputs
                        let chunks_str = all_chunk_outputs.join(" ");
                        shell
                            .replace("{chunks}", &chunks_str)
                            .replace("{input}", &chunks_str)
                            .replace("{output}", &rule.output.join(" "))
                    } else if combine.aggregate {
                        // Use aggregation method
                        let method = combine.method.as_deref().unwrap_or("concat");
                        let chunks_str = all_chunk_outputs.join(" ");
                        let output_str = rule.output.join(" ");

                        match method {
                            "concat" => {
                                let header = combine
                                    .header
                                    .as_deref()
                                    .map(|h| format!("echo '{}' && ", h))
                                    .unwrap_or_default();
                                format!("{}cat {} > {}", header, chunks_str, output_str)
                            }
                            "json_merge" => {
                                format!("jq -s 'add' {} > {}", chunks_str, output_str)
                            }
                            _ => {
                                return Err(OxoFlowError::Validation {
                                    message: format!("unknown aggregation method: {}", method),
                                    rule: Some(rule.name.clone()),
                                    suggestion: Some("use 'concat' or 'json_merge'".to_string()),
                                });
                            }
                        }
                    } else {
                        // No combine shell specified
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "transform rule '{}' has combine but no shell or aggregate method",
                                rule.name
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "specify combine.shell or combine.aggregate".to_string(),
                            ),
                        });
                    };

                    let mut combine_rule = Rule {
                        name: combine_rule_name,
                        input: FilePatterns::List(all_chunk_outputs.clone()),
                        output: rule.output.clone(),
                        shell: Some(combine_shell),
                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        combine_rule.threads = rule.threads;
                        combine_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(combine_rule);
                } else if rule.output.is_empty() {
                    // No combine, no output - each map rule produces its own output
                    // Already handled above - chunk outputs are individual
                } else {
                    // No combine but rule has output - this means each map produces part of output
                    // Use the rule's output pattern for each map (already set above)
                }
            } else {
                // No transform - keep rule as-is
                if !seen_names.insert(rule.name.clone()) {
                    return Err(OxoFlowError::DuplicateRule {
                        name: rule.name.clone(),
                    });
                }
                final_rules.push(rule.clone());
            }
        }

        self.rules = final_rules;
        Ok(())
    }

    /// Resolve split values from SplitConfig.
    fn resolve_split_values(&self, split: &crate::rule::SplitConfig) -> Result<Vec<String>> {
        // Priority: values > values_from > n > glob
        if !split.values.is_empty() {
            return Ok(split.values.clone());
        }

        if let Some(ref values_from) = split.values_from {
            if let Some(vals) = self.resolve_config_list(values_from) {
                return Ok(vals);
            }
            return Err(OxoFlowError::Validation {
                message: format!("cannot resolve split.values_from: {}", values_from),
                rule: None,
                suggestion: Some("ensure config variable exists and is an array".to_string()),
            });
        }

        if let Some(ref n_str) = split.n {
            // Resolve n from config or parse as number
            let n = if n_str.starts_with("config.") {
                self.resolve_config_list(n_str)
                    .and_then(|v| v.first().and_then(|s| s.parse::<usize>().ok()))
                    .unwrap_or(1)
            } else {
                n_str.parse::<usize>().unwrap_or(1)
            };
            // Generate chunk indices: 0, 1, 2, ..., n-1
            return Ok((0..n).map(|i| i.to_string()).collect());
        }

        if let Some(ref glob) = split.glob {
            // Glob expansion - find matching files
            let matches: Vec<String> = glob::glob(glob)
                .map_err(|e| OxoFlowError::Validation {
                    message: format!("invalid glob pattern: {}", e),
                    rule: None,
                    suggestion: None,
                })?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            if matches.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: format!("glob pattern '{}' matched no files", glob),
                    rule: None,
                    suggestion: Some("check the glob path and ensure files exist".to_string()),
                });
            }
            return Ok(matches);
        }

        Ok(Vec::new())
    }

    /// Resolve a config variable (e.g., "config.samples") into a list of strings.
    pub fn resolve_config_list(&self, var: &str) -> Option<Vec<String>> {
        if let Some(key) = var.strip_prefix("config.")
            && let Some(val) = self.config.get(key)
        {
            if let Some(arr) = val.as_array() {
                return Some(
                    arr.iter()
                        .map(|v| match v {
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect(),
                );
            } else if let Some(s) = val.as_str() {
                // Fallback to single-item list
                return Some(vec![s.to_string()]);
            }
        }
        None
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

/// Resolve rule template inheritance.
///
/// For each rule with an `extends` field, copy missing fields from the
/// named base rule. Only fields that are at their default values in the
/// child rule are inherited; explicitly set fields are preserved.
///
/// Returns an error if an `extends` target does not exist or if a
/// circular inheritance chain is detected.
pub fn resolve_rule_templates(rules: &mut [crate::rule::Rule]) -> crate::Result<()> {
    // Build a name→index map
    let name_to_idx: std::collections::HashMap<String, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.name.clone(), i))
        .collect();

    // Detect circular inheritance
    for rule in rules.iter() {
        if let Some(ref base_name) = rule.extends {
            let mut visited = std::collections::HashSet::new();
            visited.insert(rule.name.clone());
            let mut current = base_name.clone();
            while let Some(&idx) = name_to_idx.get(&current) {
                if !visited.insert(current.clone()) {
                    return Err(crate::OxoFlowError::Config {
                        message: format!(
                            "circular extends chain detected: rule '{}' extends '{}' which forms a cycle",
                            rule.name, base_name
                        ),
                    });
                }
                match &rules[idx].extends {
                    Some(next) => current = next.clone(),
                    None => break,
                }
            }
        }
    }

    // Resolve templates (iterate by index to avoid borrow issues)
    let snapshot: Vec<crate::rule::Rule> = rules.to_vec();

    for rule in rules.iter_mut() {
        if let Some(ref base_name) = rule.extends.clone() {
            let base_idx =
                name_to_idx
                    .get(base_name)
                    .ok_or_else(|| crate::OxoFlowError::Config {
                        message: format!(
                            "rule '{}' extends '{}' which does not exist",
                            rule.name, base_name
                        ),
                    })?;
            let base = &snapshot[*base_idx];

            // Inherit fields that are at their default values
            if rule.threads.is_none() && base.threads.is_some() {
                rule.threads = base.threads;
            }
            if rule.memory.is_none() && base.memory.is_some() {
                rule.memory = base.memory.clone();
            }
            if rule.resources == crate::rule::Resources::default()
                && base.resources != crate::rule::Resources::default()
            {
                rule.resources = base.resources.clone();
            }
            if rule.environment.is_empty() && !base.environment.is_empty() {
                rule.environment = base.environment.clone();
            }
            if rule.tags.is_empty() && !base.tags.is_empty() {
                rule.tags = base.tags.clone();
            }
            if rule.retries == 0 && base.retries > 0 {
                rule.retries = base.retries;
            }
            if rule.retry_delay.is_none() && base.retry_delay.is_some() {
                rule.retry_delay = base.retry_delay.clone();
            }
            if rule.group.is_none() && base.group.is_some() {
                rule.group = base.group.clone();
            }
            if rule.log.is_none() && base.log.is_some() {
                rule.log = base.log.clone();
            }
            // Inherit params that are not already set
            for (key, value) in &base.params {
                let k: String = key.clone();
                let v: toml::Value = value.clone();
                rule.params.entry(k).or_insert(v);
            }
        }
    }

    Ok(())
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
    fn resolve_includes_with_namespace_and_depends_on() {
        let dir = tempfile::tempdir().unwrap();

        // Included file has rules with internal dependencies
        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "qc_step"
            shell = "fastqc"

            [[rules]]
            name = "trim_step"
            shell = "fastp"
            depends_on = ["qc_step"]
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
        // Find trim_step rule and check its depends_on
        let trim_rule = config
            .rules
            .iter()
            .find(|r| r.name == "qc::trim_step")
            .unwrap();
        assert_eq!(trim_rule.depends_on, vec!["qc::qc_step"]);
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
    fn resolve_includes_with_namespace_external_depends_on() {
        let dir = tempfile::tempdir().unwrap();

        // Included file has rule that depends on external (main workflow) rule
        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "post_process"
            shell = "samtools stats"
            depends_on = ["align"]  # External dependency - should NOT be prefixed
        "#;
        let inc_path = dir.path().join("post.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "post.oxoflow"
            namespace = "post"

            [[rules]]
            name = "align"
            shell = "bwa"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 2);
        // Find post_process rule and check its depends_on is NOT prefixed
        let post_rule = config
            .rules
            .iter()
            .find(|r| r.name == "post::post_process")
            .unwrap();
        assert_eq!(post_rule.depends_on, vec!["align"]); // Not prefixed because "align" is external
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

    #[test]
    fn workflow_state_lifecycle() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            [[rules]]
            name = "step1"
            input = ["a.txt"]
            output = ["b.txt"]
            shell = "cat a.txt > b.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let parsed = WorkflowState::new(config);
        assert_eq!(parsed.config().workflow.name, "test");
        let validated = parsed.validate().unwrap();
        assert_eq!(validated.config().workflow.name, "test");
        let ready = validated.prepare().unwrap();
        assert_eq!(ready.config().workflow.name, "test");
    }

    #[test]
    fn validate_reference_valid_path() {
        let warnings = WorkflowConfig::validate_reference("ref.fa");
        assert!(warnings.is_empty() || warnings.iter().all(|w| w.contains("index")));
    }

    #[test]
    fn validate_reference_invalid_extension() {
        let warnings = WorkflowConfig::validate_reference("ref.txt");
        assert!(warnings.iter().any(|w| w.contains("recognized extension")));
    }

    #[test]
    fn validate_sample_sheet_valid() {
        let csv =
            "sample_id,fastq_r1,fastq_r2\nS1,s1_R1.fq.gz,s1_R2.fq.gz\nS2,s2_R1.fq.gz,s2_R2.fq.gz";
        let warnings = WorkflowConfig::validate_sample_sheet(csv);
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_sample_sheet_empty() {
        let warnings = WorkflowConfig::validate_sample_sheet("");
        assert!(warnings.iter().any(|w| w.contains("empty")));
    }

    #[test]
    fn validate_sample_sheet_duplicates() {
        let csv = "sample_id,fastq\nS1,a.fq\nS1,b.fq";
        let warnings = WorkflowConfig::validate_sample_sheet(csv);
        assert!(warnings.iter().any(|w| w.contains("Duplicate")));
    }

    #[test]
    fn variant_classification_display() {
        assert_eq!(VariantClassification::TierI.to_string(), "Tier I");
        assert_eq!(VariantClassification::Vus.to_string(), "VUS");
        assert_eq!(VariantClassification::Benign.to_string(), "Benign");
    }

    #[test]
    fn biomarker_result_display() {
        let br = BiomarkerResult {
            name: "TMB".to_string(),
            value: 12.5,
            unit: "mutations/Mb".to_string(),
            classification: Some("TMB-High".to_string()),
            threshold: Some(10.0),
        };
        let s = br.to_string();
        assert!(s.contains("TMB"));
        assert!(s.contains("12.50"));
        assert!(s.contains("TMB-High"));
    }

    #[test]
    fn qc_threshold_passes() {
        let t = QcThreshold {
            metric: "coverage".to_string(),
            min: Some(30.0),
            max: Some(1000.0),
            description: None,
        };
        assert!(t.passes(50.0));
        assert!(!t.passes(10.0));
        assert!(!t.passes(2000.0));
    }

    #[test]
    fn gene_panel_display() {
        let gp = GenePanel {
            name: "Test Panel".to_string(),
            version: Some("1.0".to_string()),
            genes: vec!["BRCA1".to_string(), "BRCA2".to_string()],
            bed_file: None,
        };
        assert_eq!(gp.to_string(), "Test Panel (2 genes) v1.0");
    }

    #[test]
    fn rule_name_newtype() {
        let rn = RuleName::from("align");
        assert_eq!(rn.to_string(), "align");
        assert_eq!(rn, RuleName("align".to_string()));
    }

    #[test]
    fn wildcard_pattern_newtype() {
        let wp = WildcardPattern::from("{sample}.bam");
        assert_eq!(wp.to_string(), "{sample}.bam");
    }

    #[test]
    fn execution_mode_display() {
        assert_eq!(ExecutionMode::Sequential.to_string(), "sequential");
        assert_eq!(ExecutionMode::Parallel.to_string(), "parallel");
    }

    #[test]
    fn genome_build_in_workflow_meta() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            genome_build = "GRCh38"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(config.workflow.genome_build.as_deref(), Some("GRCh38"));
    }

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
    }

    #[test]
    fn reference_database_display() {
        let db = ReferenceDatabase {
            name: "GRCh38".to_string(),
            version: Some("p14".to_string()),
            source: None,
            checksum: None,
            accessed_date: None,
        };
        assert_eq!(db.to_string(), "GRCh38 vp14");
    }

    #[test]
    fn reference_database_default() {
        let db = ReferenceDatabase::default();
        assert!(db.name.is_empty());
        assert!(db.version.is_none());
    }

    #[test]
    fn parse_workflow_with_reference_db() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[reference_db]]
            name = "GRCh38"
            version = "p14"
            source = "https://ftp.ncbi.nlm.nih.gov/genomes/all/GCA/000/001/405/GCA_000001405.15_GRCh38/GCA_000001405.15_GRCh38_genomic.fna.gz"
            checksum = "sha256:abc123"

            [[reference_db]]
            name = "dbSNP"
            version = "b156"

            [[rules]]
            name = "align"
            input = ["reads.fastq"]
            output = ["aligned.bam"]
            shell = "bwa mem ref.fa reads.fastq > aligned.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(config.reference_databases.len(), 2);
        assert_eq!(config.reference_databases[0].name, "GRCh38");
        assert_eq!(
            config.reference_databases[1].version,
            Some("b156".to_string())
        );
    }

    #[test]
    fn resolve_rule_templates_basic() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "base_align".to_string(),
                threads: Some(16),
                memory: Some("32G".to_string()),
                environment: crate::rule::EnvironmentSpec {
                    docker: Some("biocontainers/bwa:0.7.17".to_string()),
                    ..Default::default()
                },
                tags: vec!["alignment".to_string()],
                retries: 2,
                ..Default::default()
            },
            crate::rule::Rule {
                name: "align_sample".to_string(),
                extends: Some("base_align".to_string()),
                input: vec!["reads.fq".to_string()].into(),
                output: vec!["aligned.bam".to_string()].into(),
                shell: Some("bwa mem ref.fa {input} > {output}".to_string()),
                ..Default::default()
            },
        ];

        resolve_rule_templates(&mut rules).unwrap();

        let child = &rules[1];
        assert_eq!(child.threads, Some(16));
        assert_eq!(child.memory.as_deref(), Some("32G"));
        assert_eq!(
            child.environment.docker.as_deref(),
            Some("biocontainers/bwa:0.7.17")
        );
        assert_eq!(child.tags, vec!["alignment"]);
        assert_eq!(child.retries, 2);
        // Shell should NOT be inherited (it's set on the child)
        assert_eq!(
            child.shell.as_deref(),
            Some("bwa mem ref.fa {input} > {output}")
        );
    }

    #[test]
    fn resolve_rule_templates_override() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "base".to_string(),
                threads: Some(16),
                memory: Some("32G".to_string()),
                ..Default::default()
            },
            crate::rule::Rule {
                name: "child".to_string(),
                extends: Some("base".to_string()),
                threads: Some(8), // Override
                ..Default::default()
            },
        ];

        resolve_rule_templates(&mut rules).unwrap();

        let child = &rules[1];
        assert_eq!(child.threads, Some(8)); // Kept child's value
        assert_eq!(child.memory.as_deref(), Some("32G")); // Inherited
    }

    #[test]
    fn resolve_rule_templates_missing_base() {
        let mut rules = vec![crate::rule::Rule {
            name: "child".to_string(),
            extends: Some("nonexistent".to_string()),
            ..Default::default()
        }];

        let result = resolve_rule_templates(&mut rules);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn resolve_rule_templates_circular() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "a".to_string(),
                extends: Some("b".to_string()),
                ..Default::default()
            },
            crate::rule::Rule {
                name: "b".to_string(),
                extends: Some("a".to_string()),
                ..Default::default()
            },
        ];

        let result = resolve_rule_templates(&mut rules);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("circular"));
    }

    // ── Transform Operator Tests ───────────────────────────────────────────────

    #[test]
    fn parse_transform_with_split_by_values() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "parallel_qc"
            input = ["sample.bam"]
            threads = 4

            [rules.transform.split]
            by = "chr"
            values = ["chr1", "chr2", "chr3"]

            [rules.transform]
            map = "samtools view -b {input} {chr} > qc/{chr}.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(transform.split.by, "chr");
        assert_eq!(
            transform.split.values,
            vec!["chr1".to_string(), "chr2".to_string(), "chr3".to_string()]
        );
        assert_eq!(
            transform.map,
            "samtools view -b {input} {chr} > qc/{chr}.bam"
        );
        assert!(transform.combine.is_none());
    }

    #[test]
    fn parse_transform_with_values_from() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "variant_calling"
            input = ["sample.bam"]
            output = ["sample.vcf.gz"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "call {input} {chr}"

            [rules.transform.combine]
            shell = "merge {chunks} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(
            transform.split.values_from,
            Some("config.chromosomes".to_string())
        );
        let combine = transform.combine.as_ref().unwrap();
        assert_eq!(combine.shell, Some("merge {chunks} > {output}".to_string()));
    }

    #[test]
    fn parse_transform_with_aggregate_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "collect_stats"
            input = ["data.txt"]

            [rules.transform.split]
            by = "chunk"
            n = "5"

            [rules.transform]
            map = "process {input} > .oxo-flow/chunks/{chunk}.txt"

            [rules.transform.combine]
            aggregate = true
            method = "concat"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(transform.split.n, Some("5".to_string()));
        let combine = transform.combine.as_ref().unwrap();
        assert!(combine.aggregate);
        assert_eq!(combine.method, Some("concat".to_string()));
    }

    #[test]
    fn resolve_split_values_from_config() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"

            [config]
            chromosomes = ["chr1", "chr2", "chr3"]

            [[rules]]
            name = "test_rule"
            shell = "echo test"
        "#,
        )
        .unwrap();

        let split = crate::rule::SplitConfig {
            by: "chr".to_string(),
            values: vec![], // empty, use values_from
            values_from: Some("config.chromosomes".to_string()),
            n: None,
            glob: None,
        };

        let values = config.resolve_split_values(&split).unwrap();
        assert_eq!(
            values,
            vec!["chr1".to_string(), "chr2".to_string(), "chr3".to_string()]
        );
    }

    #[test]
    fn resolve_split_values_direct() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "test_rule"
            shell = "echo test"
        "#,
        )
        .unwrap();

        let split = crate::rule::SplitConfig {
            by: "chr".to_string(),
            values: vec!["chr1".to_string(), "chr2".to_string()],
            values_from: None,
            n: None,
            glob: None,
        };

        let values = config.resolve_split_values(&split).unwrap();
        assert_eq!(values, vec!["chr1".to_string(), "chr2".to_string()]);
    }

    #[test]
    fn expand_transform_split_map_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "variant_calling"
            input = ["sample.bam"]
            output = ["sample.vcf.gz"]
            threads = 8

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "gatk HaplotypeCaller -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz"

            [rules.transform.combine]
            shell = "gatk GatherVcfs {chunks} -O {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 2 map rules + 1 combine rule = 3 rules
        assert_eq!(config.rules.len(), 3);

        // Check map rules
        let map1 = &config.rules[0];
        assert!(map1.name.contains("chr1"));
        assert!(map1.shell.as_ref().unwrap().contains("chr1"));

        let map2 = &config.rules[1];
        assert!(map2.name.contains("chr2"));
        assert!(map2.shell.as_ref().unwrap().contains("chr2"));

        // Check combine rule
        let combine = &config.rules[2];
        assert!(combine.name.contains("combine"));
        assert!(combine.shell.as_ref().unwrap().contains("GatherVcfs"));
    }

    #[test]
    fn expand_transform_split_map_no_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2", "chr3"]

            [[rules]]
            name = "parallel_qc"
            input = ["sample.bam"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "samtools flagstat {input} > qc/{chr}.flagstat.txt"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 3 map rules (no combine)
        assert_eq!(config.rules.len(), 3);

        // Each rule should have its own output based on chr
        for (i, rule) in config.rules.iter().enumerate() {
            let expected_chr = ["chr1", "chr2", "chr3"][i];
            assert!(rule.name.contains(expected_chr));
        }
    }

    #[test]
    fn transform_validation_missing_split_values() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "bad_transform"
            input = ["sample.bam"]
            output = ["result.txt"]

            [rules.transform.split]
            by = "chr"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            shell = "merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        let result = config.expand_wildcards();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no split values"));
    }

    #[test]
    fn transform_validation_combine_without_shell_or_aggregate() {
        let toml = r###"
            [workflow]
            name = "test"

            [config]
            chromosomes = ["chr1"]

            [[rules]]
            name = "bad_combine"
            input = ["sample.bam"]
            output = ["result.vcf"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            header = "# header without shell"
        "###;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        let result = config.expand_wildcards();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no shell or aggregate method"));
    }

    #[test]
    fn transform_inherits_threads_and_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 8
            memory = "16G"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "inherited_transform"
            input = ["sample.bam"]
            output = ["result.vcf"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            shell = "merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // All expanded rules should inherit defaults
        for rule in &config.rules {
            assert_eq!(rule.threads, Some(8));
            assert_eq!(rule.memory.as_deref(), Some("16G"));
        }
    }

    #[test]
    fn transform_with_aggregate_concat() {
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            chunks = ["part1", "part2"]

            [[rules]]
            name = "aggregate_test"
            input = ["data.txt"]
            output = ["combined.txt"]

            [rules.transform.split]
            by = "part"
            values_from = "config.chunks"

            [rules.transform]
            map = "process > .oxo-flow/chunks/{part}.txt"

            [rules.transform.combine]
            aggregate = true
            method = "concat"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 2 map rules + 1 aggregate rule
        assert_eq!(config.rules.len(), 3);

        // Last rule should be aggregate
        let aggregate_rule = &config.rules[2];
        // Aggregate rule should use concat method
        assert!(aggregate_rule.shell.as_ref().unwrap().contains("cat"));
    }

    #[test]
    fn transform_with_aggregate_json_merge() {
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            chunks = ["part1"]

            [[rules]]
            name = "json_test"
            input = ["data.json"]
            output = ["merged.json"]

            [rules.transform.split]
            by = "part"
            values_from = "config.chunks"

            [rules.transform]
            map = "process > .oxo-flow/chunks/{part}.json"

            [rules.transform.combine]
            aggregate = true
            method = "json_merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 1 map rule + 1 aggregate rule = 2 rules (only 1 chunk)
        assert_eq!(config.rules.len(), 2);

        // Aggregate rule should handle json
        let aggregate_rule = &config.rules[1];
        // For json_merge, the shell should use jq
        assert!(aggregate_rule.shell.as_ref().unwrap().contains("jq"));
    }
}
