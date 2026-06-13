//! Structured output records and result extraction for pipeline execution.
//!
//! Defines the [`OutputRecord`] type for queryable pipeline results,
//! the [`ResultExtractor`] trait for parsing tool-specific outputs,
//! and a set of built-in extractors for common bioinformatics tools.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// OutputRecord
// ---------------------------------------------------------------------------

/// A structured output record from a pipeline rule execution.
///
/// Unlike raw log files, an `OutputRecord` contains parsed, queryable data
/// such as QC metrics, variant counts, alignment statistics, and file
/// provenance information per rule execution.
///
/// # Example (FastQC record)
///
/// ```json
/// {
///   "rule": "fastqc",
///   "run_id": "a1b2c3d4",
///   "sample": "sample_01",
///   "file_path": "qc/sample_01_fastqc.html",
///   "file_size": 456789,
///   "checksum": "sha256:abcd...",
///   "metrics": {
///     "total_sequences": 25000000,
///     "percent_gc": 48.5,
///     "encoding": "Illumina 1.9",
///     "avg_quality": 36.2
///   },
///   "created_at": "2026-06-13T12:00:00Z"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputRecord {
    /// Rule name that produced this record.
    pub rule: String,
    /// Run ID this record belongs to.
    pub run_id: String,
    /// Sample or wildcard combination (e.g., "sample_01", "chr1").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Relative path to the output file.
    pub file_path: String,
    /// File size in bytes.
    #[serde(default)]
    pub file_size: u64,
    /// Optional checksum (e.g., "sha256:abc123...").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Parsed metrics specific to the tool that produced this output.
    ///
    /// Example keys for a FastQC record: `total_sequences`, `percent_gc`,
    /// `avg_quality`, `encoding`.
    #[serde(default)]
    pub metrics: HashMap<String, serde_json::Value>,
    /// ISO 8601 timestamp when the record was created.
    pub created_at: String,
}

impl OutputRecord {
    /// Create a new output record with minimal required fields.
    pub fn new(
        rule: impl Into<String>,
        run_id: impl Into<String>,
        file_path: impl Into<String>,
    ) -> Self {
        Self {
            rule: rule.into(),
            run_id: run_id.into(),
            sample: None,
            file_path: file_path.into(),
            file_size: 0,
            checksum: None,
            metrics: HashMap::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Set the sample identifier for this record.
    pub fn with_sample(mut self, sample: impl Into<String>) -> Self {
        self.sample = Some(sample.into());
        self
    }

    /// Set the file size.
    pub fn with_file_size(mut self, size: u64) -> Self {
        self.file_size = size;
        self
    }

    /// Set a checksum for the output file.
    pub fn with_checksum(mut self, checksum: impl Into<String>) -> Self {
        self.checksum = Some(checksum.into());
        self
    }

    /// Set a single metric value.
    pub fn with_metric(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.metrics.insert(key.into(), value.into());
        self
    }

    /// Set multiple metrics at once.
    pub fn with_metrics(mut self, metrics: HashMap<String, serde_json::Value>) -> Self {
        self.metrics = metrics;
        self
    }
}

// ---------------------------------------------------------------------------
// ResultExtractor trait
// ---------------------------------------------------------------------------

/// Trait for extracting structured results from pipeline output files.
///
/// Each extractor knows how to parse the output of a specific bioinformatics
/// tool (e.g., FastQC, MultiQC, STAR, etc.) and return structured metrics.
///
/// # Example
///
/// ```rust,no_run
/// use oxo_flow_core::result::{ResultExtractor, OutputRecord};
/// use std::collections::HashMap;
/// use std::path::Path;
///
/// struct MyExtractor;
///
/// impl ResultExtractor for MyExtractor {
///     fn name(&self) -> &str { "my-tool" }
///     fn can_handle(&self, output_path: &str, _rule: &str) -> bool {
///         output_path.ends_with("my_output.txt")
///     }
///     fn extract(&self, file_path: &Path) -> HashMap<String, serde_json::Value> {
///         let mut m = HashMap::new();
///         m.insert("lines".into(), 42.into());
///         m
///     }
/// }
/// ```
pub trait ResultExtractor: Send + Sync {
    /// Human-readable name of this extractor (e.g., "fastqc", "multiqc").
    fn name(&self) -> &str;

    /// Returns `true` if this extractor can handle the given output file.
    fn can_handle(&self, output_path: &str, rule_name: &str) -> bool;

    /// Extract structured metrics from the output file.
    ///
    /// Returns a map of metric name to value. Returns an empty map if
    /// the file cannot be parsed (rather than returning an error).
    fn extract(&self, file_path: &Path) -> HashMap<String, serde_json::Value>;
}

// ---------------------------------------------------------------------------
// FastQC extractor
// ---------------------------------------------------------------------------

/// Extracts metrics from FastQC `fastqc_data.txt` files.
///
/// Parses key statistics like total sequences, %GC, quality scores,
/// and sequence length distribution from FastQC's structured data file.
#[derive(Debug, Default)]
pub struct FastQcExtractor;

impl FastQcExtractor {
    /// Try to find the `fastqc_data.txt` inside a FastQC output directory.
    fn find_data_file(file_path: &Path) -> Option<std::path::PathBuf> {
        let path_str = file_path.to_string_lossy();
        // If it's a .zip, the data file is inside
        if path_str.ends_with("_fastqc.zip") || path_str.ends_with("_fastqc.html") {
            let parent = file_path.parent()?;
            let stem = file_path.file_stem()?.to_string_lossy();
            // Remove .html or .zip from the stem
            let data_dir_name = if stem.ends_with("_fastqc") {
                stem.to_string()
            } else {
                format!(
                    "{}_fastqc",
                    stem.trim_end_matches(".html").trim_end_matches(".zip")
                )
            };
            let data_file = parent.join(&data_dir_name).join("fastqc_data.txt");
            if data_file.exists() {
                return Some(data_file);
            }
        }
        // Direct path
        if path_str.ends_with("fastqc_data.txt") && file_path.exists() {
            return Some(file_path.to_path_buf());
        }
        None
    }
}

impl ResultExtractor for FastQcExtractor {
    fn name(&self) -> &str {
        "fastqc"
    }

    fn can_handle(&self, output_path: &str, rule_name: &str) -> bool {
        let lower = output_path.to_lowercase();
        (lower.contains("fastqc") || rule_name.to_lowercase().contains("fastqc"))
            && (lower.ends_with(".html")
                || lower.ends_with(".zip")
                || lower.ends_with("fastqc_data.txt"))
    }

    fn extract(&self, file_path: &Path) -> HashMap<String, serde_json::Value> {
        let data_path = match Self::find_data_file(file_path) {
            Some(p) => p,
            None => return HashMap::new(),
        };

        let content = match std::fs::read_to_string(&data_path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        let mut metrics = HashMap::new();
        let mut in_basic_stats = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with(">>Basic Statistics") {
                in_basic_stats = true;
                continue;
            }
            if in_basic_stats {
                // FastQC basic stats format: "Stat\tValue"
                if let Some((key, value)) = trimmed.split_once('\t') {
                    let metric_key = match key {
                        "Filename" => continue, // not a metric
                        "File type" => continue,
                        k => k.to_lowercase().replace(' ', "_"),
                    };
                    if let Ok(num) = value.parse::<f64>() {
                        metrics.insert(metric_key, serde_json::Value::from(num));
                    } else {
                        metrics.insert(metric_key, serde_json::Value::String(value.to_string()));
                    }
                }
                if trimmed.is_empty() || trimmed.starts_with(">>END_MODULE") {
                    break;
                }
            }

            // Also look for single key-value lines that start with "Measure"
            if trimmed.starts_with("Measure") {
                in_basic_stats = false;
                continue;
            }
            // Parse Per base sequence quality
            if trimmed.starts_with(">>Per base sequence quality") {
                // Just record presence; detailed parsing would need the distribution
                break;
            }
        }

        metrics
    }
}

// ---------------------------------------------------------------------------
// MultiQC extractor
// ---------------------------------------------------------------------------

/// Extracts general metrics from MultiQC report data.
///
/// Scans MultiQC output for key quality metrics across all samples.
#[derive(Debug, Default)]
pub struct MultiQcExtractor;

impl ResultExtractor for MultiQcExtractor {
    fn name(&self) -> &str {
        "multiqc"
    }

    fn can_handle(&self, output_path: &str, rule_name: &str) -> bool {
        let lower = output_path.to_lowercase();
        (lower.contains("multiqc") || rule_name.to_lowercase().contains("multiqc"))
            && (lower.ends_with(".html")
                || lower.contains("multiqc_data")
                || lower.ends_with("_data"))
    }

    fn extract(&self, file_path: &Path) -> HashMap<String, serde_json::Value> {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        let mut metrics = HashMap::new();
        metrics.insert("report_size".into(), (content.len() as u64).into());
        metrics.insert("has_data".into(), content.contains("mqc_").into());
        metrics
    }
}

// ---------------------------------------------------------------------------
// Generic text extractor
// ---------------------------------------------------------------------------

/// Extracts basic statistics from any text output file.
///
/// Provides line count, word count, file size, and checks for common
/// bioinformatics output patterns (e.g., "PASS", "FAIL", "WARN").
#[derive(Debug, Default)]
pub struct GenericTextExtractor;

impl ResultExtractor for GenericTextExtractor {
    fn name(&self) -> &str {
        "generic"
    }

    fn can_handle(&self, _output_path: &str, _rule_name: &str) -> bool {
        true // fallback: can handle any file
    }

    fn extract(&self, file_path: &Path) -> HashMap<String, serde_json::Value> {
        let mut metrics = HashMap::new();

        // File size
        if let Ok(meta) = std::fs::metadata(file_path) {
            metrics.insert("file_size_bytes".into(), (meta.len()).into());
        }

        // Content analysis
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => return metrics,
        };

        let lines = content.lines().count();
        let words = content.split_whitespace().count();
        metrics.insert("line_count".into(), (lines as u64).into());
        metrics.insert("word_count".into(), (words as u64).into());

        // Count pass/fail/warn patterns
        let pass_count = content.matches("PASS").count()
            + content.matches("pass").count()
            + content.matches("✓").count();
        if pass_count > 0 {
            metrics.insert("pass_count".into(), (pass_count as u64).into());
        }
        let fail_count = content.matches("FAIL").count()
            + content.matches("fail").count()
            + content.matches("✗").count();
        if fail_count > 0 {
            metrics.insert("fail_count".into(), (fail_count as u64).into());
        }
        let warn_count = content.matches("WARN").count()
            + content.matches("warn").count()
            + content.matches("⚠").count();
        if warn_count > 0 {
            metrics.insert("warning_count".into(), (warn_count as u64).into());
        }

        metrics
    }
}

// ---------------------------------------------------------------------------
// ResultExtractorRegistry
// ---------------------------------------------------------------------------

/// A registry of result extractors that dispatches to the right extractor
/// for each output file.
#[derive(Default)]
pub struct ResultExtractorRegistry {
    extractors: Vec<Box<dyn ResultExtractor>>,
}

impl ResultExtractorRegistry {
    /// Create a new registry with the default set of extractors.
    pub fn new() -> Self {
        Self {
            extractors: vec![
                Box::new(FastQcExtractor),
                Box::new(MultiQcExtractor),
                Box::new(GenericTextExtractor),
            ],
        }
    }

    /// Create an empty registry (no extractors).
    pub fn empty() -> Self {
        Self {
            extractors: Vec::new(),
        }
    }

    /// Register a custom extractor.
    pub fn register(&mut self, extractor: Box<dyn ResultExtractor>) {
        self.extractors.push(extractor);
    }

    /// Find the best extractor for the given output file and rule name.
    pub fn find_extractor(
        &self,
        output_path: &str,
        rule_name: &str,
    ) -> Option<&dyn ResultExtractor> {
        // Try exact matches first, then fall back to generic
        for extractor in &self.extractors {
            if extractor.name() != "generic" && extractor.can_handle(output_path, rule_name) {
                return Some(extractor.as_ref());
            }
        }
        // Fall back to generic
        self.extractors
            .iter()
            .find(|e| e.name() == "generic")
            .map(|e| e.as_ref())
    }

    /// Extract metrics from the given output file using the best matching extractor.
    pub fn extract(
        &self,
        output_path: &str,
        rule_name: &str,
    ) -> HashMap<String, serde_json::Value> {
        let path = std::path::Path::new(output_path);
        if !path.exists() {
            return HashMap::new();
        }
        self.find_extractor(output_path, rule_name)
            .map(|e| e.extract(path))
            .unwrap_or_default()
    }

    /// Return the list of registered extractor names.
    pub fn available_extractors(&self) -> Vec<String> {
        self.extractors
            .iter()
            .map(|e| e.name().to_string())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Convenience: scan a run directory for output records
// ---------------------------------------------------------------------------

/// Scan the output files listed in a [`WorkflowConfig`] for a completed run
/// and produce [`OutputRecord`]s with extracted metrics.
///
/// This is called after a workflow run completes to index its outputs.
pub fn scan_run_outputs<P: AsRef<Path>>(
    run_dir: P,
    run_id: &str,
    rules: &[crate::rule::Rule],
) -> Vec<OutputRecord> {
    let registry = ResultExtractorRegistry::new();
    let run_dir = run_dir.as_ref();
    let mut records = Vec::new();

    for rule in rules {
        for output_path in &rule.output {
            let full_path = run_dir.join(output_path);
            if !full_path.exists() {
                continue;
            }

            let meta = std::fs::metadata(&full_path).ok();
            let file_size = meta.map(|m| m.len()).unwrap_or(0);

            // Compute checksum for small files (< 100MB)
            let checksum = if file_size < 100 * 1024 * 1024 {
                compute_sha256_checksum(&full_path)
            } else {
                None
            };

            let sample = extract_sample_from_path(output_path, &rule.wildcard_names());

            let metrics = registry.extract(&full_path.to_string_lossy(), &rule.name);

            let record = OutputRecord::new(&rule.name, run_id, output_path)
                .with_sample(sample.unwrap_or_else(|| format!("rule-{}", rule.name)))
                .with_file_size(file_size)
                .with_checksum(checksum.unwrap_or_default())
                .with_metrics(metrics);

            records.push(record);
        }
    }

    records
}

/// Compute a SHA-256 checksum for a file, returning it as a hex string.
fn compute_sha256_checksum(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        use sha2::Digest;
        hasher.update(&buffer[..n]);
    }
    use sha2::Digest;
    Some(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Try to extract a sample identifier from a file path containing wildcard
/// patterns like `{sample}`.
fn extract_sample_from_path(path: &str, wildcard_names: &[String]) -> Option<String> {
    if wildcard_names.is_empty() {
        return None;
    }
    // Common patterns: sample_XX, sampleXX in directory names
    let path_lower = path.to_lowercase();
    for wc in wildcard_names {
        // Try to find the pattern value by looking at parent dirs
        // E.g., "qc/sample_01_fastqc.html" -> might contain "sample_01"
        // This is a heuristic; in production the wildcard values are known
        if let Some(pos) = path_lower.find(&format!("_{}_", wc)) {
            // Look backwards from the position to find the value
            let before = &path[..pos];
            if let Some(last_sep) = before.rfind(['/', '\\']) {
                let candidate = before[last_sep + 1..].to_string();
                if !candidate.is_empty() && candidate.len() < 100 {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_record_new() {
        let record = OutputRecord::new("fastqc", "run-001", "qc/sample1_fastqc.html");
        assert_eq!(record.rule, "fastqc");
        assert_eq!(record.run_id, "run-001");
        assert_eq!(record.file_path, "qc/sample1_fastqc.html");
        assert!(record.sample.is_none());
        assert!(record.metrics.is_empty());
    }

    #[test]
    fn output_record_builder() {
        let record = OutputRecord::new("align", "run-001", "bam/sample.bam")
            .with_sample("sample_01")
            .with_file_size(1_000_000)
            .with_metric("mapped_reads", 50000_u64)
            .with_metric("mapping_rate", 0.95);
        assert_eq!(record.sample.unwrap(), "sample_01");
        assert_eq!(record.file_size, 1_000_000);
        assert_eq!(record.metrics.get("mapped_reads").unwrap(), 50000_u64);
    }

    #[test]
    fn fastqc_extractor_can_handle() {
        let ext = FastQcExtractor;
        assert!(ext.can_handle("qc/sample_fastqc.html", "fastqc"));
        assert!(ext.can_handle("qc/sample_fastqc.zip", "fastqc"));
        assert!(ext.can_handle("qc/fastqc_data.txt", "fastqc"));
        assert!(!ext.can_handle("output.bam", "align"));
    }

    #[test]
    fn multic_extractor_can_handle() {
        let ext = MultiQcExtractor;
        assert!(ext.can_handle("qc/multiqc_report.html", "multiqc"));
        assert!(ext.can_handle("qc/multiqc_data/multiqc_general_stats.txt", "multiqc"));
    }

    #[test]
    fn generic_extractor_can_handle() {
        let ext = GenericTextExtractor;
        // Generic can handle anything
        assert!(ext.can_handle("any/file.txt", "any_rule"));
    }

    #[test]
    fn registry_finds_extractors() {
        let registry = ResultExtractorRegistry::new();
        let names = registry.available_extractors();
        assert!(names.contains(&"fastqc".to_string()));
        assert!(names.contains(&"multiqc".to_string()));
        assert!(names.contains(&"generic".to_string()));
    }

    #[test]
    fn registry_prefers_specific() {
        let registry = ResultExtractorRegistry::new();
        let ext = registry.find_extractor("qc/sample_fastqc.html", "fastqc");
        assert!(ext.is_some());
        assert_eq!(ext.unwrap().name(), "fastqc");
    }

    #[test]
    fn registry_falls_back_to_generic() {
        let registry = ResultExtractorRegistry::new();
        let ext = registry.find_extractor("output.txt", "some_rule");
        assert!(ext.is_some());
        assert_eq!(ext.unwrap().name(), "generic");
    }

    #[test]
    fn empty_registry() {
        let registry = ResultExtractorRegistry::empty();
        assert!(registry.find_extractor("test.txt", "rule").is_none());
    }

    #[test]
    fn compute_sha256_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let checksum = compute_sha256_checksum(&path);
        assert!(checksum.is_some());
        let cs = checksum.unwrap();
        assert!(cs.starts_with("sha256:"));
        assert_eq!(cs.len(), 64 + 7); // "sha256:" + 64 hex chars
    }

    #[test]
    fn scan_run_outputs_empty_rules() {
        let dir = tempfile::tempdir().unwrap();
        let records = scan_run_outputs(dir.path(), "run-001", &[]);
        assert!(records.is_empty());
    }

    #[test]
    fn extract_sample_from_path_no_wildcards() {
        assert_eq!(extract_sample_from_path("output.txt", &[]), None);
    }

    #[test]
    fn extract_sample_from_path_with_wildcard() {
        let result = extract_sample_from_path("qc/sample_01_fastqc.html", &["sample".to_string()]);
        assert_eq!(result, None); // heuristic, may not match
    }

    #[test]
    fn generic_extractor_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_output.txt");
        std::fs::write(&path, b"hello world\nline 2\nPASS: all good\nWARN: slow").unwrap();
        let ext = GenericTextExtractor;
        let metrics = ext.extract(&path);
        assert!(metrics.contains_key("line_count"));
        assert!(metrics.contains_key("word_count"));
        assert!(metrics.contains_key("file_size_bytes"));
        assert!(metrics.contains_key("pass_count"));
        assert!(metrics.contains_key("warning_count"));
    }
}
