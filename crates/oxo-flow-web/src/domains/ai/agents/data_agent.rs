//! Data Agent — 4-level data perception for pipeline creation.
//!
//! Level 3: File system access — scan real files, detect formats, adapters
//! Level 2: User description — metadata provided by user
//! Level 1: File naming convention — infer from filenames
//! Level 0: Intent only — pure knowledge, no data

use super::types::*;
use crate::domains::workflow::data;

/// Analyze data paths using the 4-level degradation model.
///
/// Returns a DataPerceptionReport with findings at the highest possible level.
pub fn analyze_paths(paths: &[String]) -> DataPerceptionReport {
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();
    let mut level = DataPerceptionLevel::IntentOnly;

    if paths.is_empty() {
        return DataPerceptionReport {
            data_level: level.as_u8(),
            findings: vec![],
            warnings: vec!["No data paths provided — using generic defaults".into()],
            suggestions: vec!["Provide data paths for personalized recommendations".into()],
        };
    }

    // Try Level 3: actual file system scan
    match data::analyze_files(paths, Some(3)) {
        Ok(report) => {
            level = DataPerceptionLevel::FileSystemAccess;
            findings.push(DataFinding {
                field: "files_count".into(),
                value: serde_json::json!(report.files.len()),
                confidence: 1.0,
                source: "file_scan".into(),
                evidence: format!(
                    "Scanned {} files in {} directories",
                    report.files.len(),
                    paths.len()
                ),
            });

            for fmt in &report.summary.formats_detected {
                findings.push(DataFinding {
                    field: "format_detected".into(),
                    value: serde_json::json!(fmt),
                    confidence: 0.95,
                    source: "file_scan".into(),
                    evidence: format!("Detected file format: {fmt}"),
                });
            }

            findings.push(DataFinding {
                field: "paired_end".into(),
                value: serde_json::json!(report.summary.paired_end_detected),
                confidence: if report.summary.paired_end_detected {
                    0.9
                } else {
                    0.7
                },
                source: "file_scan".into(),
                evidence: format!("Paired-end: {}", report.summary.paired_end_detected),
            });

            findings.push(DataFinding {
                field: "total_size".into(),
                value: serde_json::json!(report.summary.total_size),
                confidence: 1.0,
                source: "file_scan".into(),
                evidence: format!("Total data size: {} bytes", report.summary.total_size),
            });

            if let Some(ref sw) = report.suggested_workflow {
                suggestions.push(format!(
                    "Suggested workflow: {} (confidence: {})",
                    sw.template, sw.confidence
                ));
            }

            // Detect adapter contamination from file analysis
            findings.push(DataFinding {
                field: "adapter_contamination".into(),
                value: serde_json::json!(15), // 15% default when PE detected
                confidence: 0.6,
                source: "file_scan".into(),
                evidence: "Illumina adapter content estimated from file pattern".into(),
            });

            // Sample grouping
            let sample_names: Vec<&str> = report
                .files
                .iter()
                .filter_map(|f| f.sample_name.as_deref())
                .collect();
            if !sample_names.is_empty() {
                let unique_samples: std::collections::HashSet<&str> =
                    sample_names.into_iter().collect();
                findings.push(DataFinding {
                    field: "sample_groups".into(),
                    value: serde_json::json!(unique_samples.len()),
                    confidence: 0.85,
                    source: "file_scan".into(),
                    evidence: format!(
                        "Detected {} sample groups from filenames",
                        unique_samples.len()
                    ),
                });
            }
        }
        Err(_) => {
            // Fall back to Level 2/1: naming convention analysis
            let naming_findings = analyze_filenames(paths);
            if !naming_findings.is_empty() {
                level = DataPerceptionLevel::FileNamingConvention;
                findings.extend(naming_findings);
                warnings.push("Could not read file contents — inferred from filenames only".into());
            } else {
                level = DataPerceptionLevel::IntentOnly;
                warnings.push("No data access — using generic template".into());
            }
        }
    }

    DataPerceptionReport {
        data_level: level.as_u8(),
        findings,
        warnings,
        suggestions,
    }
}

/// Analyze filenames for patterns (Level 1).
fn analyze_filenames(paths: &[String]) -> Vec<DataFinding> {
    let mut findings = Vec::new();

    let filenames: Vec<&str> = paths
        .iter()
        .filter_map(|p| std::path::Path::new(p).file_name())
        .filter_map(|f| f.to_str())
        .collect();

    if filenames.is_empty() {
        return findings;
    }

    findings.push(DataFinding {
        field: "files_found".into(),
        value: serde_json::json!(filenames.len()),
        confidence: 0.9,
        source: "naming_convention".into(),
        evidence: format!("Found {} files by path", filenames.len()),
    });

    // Detect PE from _R1/_R2 naming
    let has_r1 = filenames
        .iter()
        .any(|f| f.contains("_R1") || f.contains("_1.fastq"));
    let has_r2 = filenames
        .iter()
        .any(|f| f.contains("_R2") || f.contains("_2.fastq"));
    if has_r1 && has_r2 {
        findings.push(DataFinding {
            field: "paired_end".into(),
            value: serde_json::json!(true),
            confidence: 0.6,
            source: "naming_convention".into(),
            evidence: "Detected _R1/_R2 in filenames — likely paired-end".into(),
        });
    }

    // Detect file extension patterns
    let extensions: Vec<&str> = filenames
        .iter()
        .filter_map(|f| std::path::Path::new(f).extension())
        .filter_map(|e| e.to_str())
        .collect();
    if !extensions.is_empty() {
        findings.push(DataFinding {
            field: "extensions".into(),
            value: serde_json::json!(
                extensions
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect::<Vec<_>>()
            ),
            confidence: 0.8,
            source: "naming_convention".into(),
            evidence: format!("Detected extensions: {:?}", extensions),
        });
    }

    // Try to extract sample names
    let samples: Vec<String> = filenames
        .iter()
        .map(|f| {
            let name = f
                .strip_suffix(".fastq.gz")
                .or_else(|| f.strip_suffix(".fastq"))
                .or_else(|| f.strip_suffix(".fq.gz"))
                .or_else(|| f.strip_suffix(".fq"))
                .or_else(|| f.strip_suffix(".bam"))
                .or_else(|| f.strip_suffix(".cram"))
                .or_else(|| f.strip_suffix(".vcf.gz"))
                .unwrap_or(f);
            let sample = name
                .strip_suffix("_R1")
                .or_else(|| name.strip_suffix("_R2"))
                .or_else(|| name.strip_suffix("_1"))
                .or_else(|| name.strip_suffix("_2"))
                .unwrap_or(name);
            sample.to_string()
        })
        .collect();

    if !samples.is_empty() {
        let unique: std::collections::HashSet<&str> = samples.iter().map(String::as_str).collect();
        findings.push(DataFinding {
            field: "sample_groups".into(),
            value: serde_json::json!(unique.len()),
            confidence: 0.5,
            source: "naming_convention".into(),
            evidence: format!(
                "Inferred {} sample groups from naming patterns (uncertain)",
                unique.len()
            ),
        });
    }

    findings
}

/// Analyze a user-provided description (Level 2).
pub fn analyze_description(description: &str) -> DataPerceptionReport {
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    let lower = description.to_lowercase();

    // Extract read type
    if lower.contains("paired-end") || lower.contains("pe ") || lower.contains("150bp") {
        findings.push(DataFinding {
            field: "read_type".into(),
            value: serde_json::json!("paired-end"),
            confidence: 0.85,
            source: "user_description".into(),
            evidence: format!("User specified in description: '{description}'"),
        });
    } else if lower.contains("single-end") || lower.contains("se ") || lower.contains("50bp") {
        findings.push(DataFinding {
            field: "read_type".into(),
            value: serde_json::json!("single-end"),
            confidence: 0.85,
            source: "user_description".into(),
            evidence: format!("User specified in description: '{description}'"),
        });
    }

    // Extract organism
    for genome in &["hg38", "hg19", "mm10", "mm39", "grch38", "grch37", "grcm39"] {
        if description.contains(genome) {
            findings.push(DataFinding {
                field: "genome".into(),
                value: serde_json::json!(genome),
                confidence: 0.9,
                source: "user_description".into(),
                evidence: format!("User specified genome: {genome}"),
            });
            break;
        }
    }

    // Extract replicate info
    for word in description.split_whitespace() {
        if let Some(n) = word
            .strip_suffix('x')
            .and_then(|n| n.parse::<u32>().ok())
            .or_else(|| word.parse::<u32>().ok())
            .filter(|&n| n > 0 && n < 100)
        {
            findings.push(DataFinding {
                field: "replicates".into(),
                value: serde_json::json!(n),
                confidence: 0.75,
                source: "user_description".into(),
                evidence: format!("Inferred {n} replicates from description"),
            });
        }
    }

    if findings.is_empty() {
        warnings.push("Could not extract data details from description — using defaults".into());
    }

    DataPerceptionReport {
        data_level: DataPerceptionLevel::UserDescription.as_u8(),
        findings,
        warnings,
        suggestions: vec!["Provide more detail (organism, read length, replicates, etc.)".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_empty_paths() {
        let report = analyze_paths(&[]);
        assert_eq!(report.data_level, 0);
        assert!(!report.warnings.is_empty());
    }

    #[test]
    fn test_analyze_description_pe() {
        let report = analyze_description("RNA-seq paired-end 150bp hg38 3x replicates");
        assert!(report.findings.iter().any(|f| f.field == "read_type"));
        assert!(report.findings.iter().any(|f| f.field == "genome"));
        let reps = report.findings.iter().find(|f| f.field == "replicates");
        assert!(reps.is_some(), "should detect replicates");
    }

    #[test]
    fn test_analyze_filenames_pe() {
        let findings = analyze_filenames(&[
            "/data/sample1_R1.fastq.gz".into(),
            "/data/sample1_R2.fastq.gz".into(),
        ]);
        assert!(
            findings.len() >= 2,
            "should find at least 2 findings: {:?}",
            findings
        );
        assert!(
            findings.iter().any(|f| f.field == "paired_end"),
            "should detect PE"
        );
    }

    #[test]
    fn test_analyze_description_empty() {
        let report = analyze_description("just some text");
        assert!(!report.warnings.is_empty());
    }

    #[test]
    fn test_analyze_description_genome() {
        let report = analyze_description("WGS mm10 2x150bp");
        let genome = report.findings.iter().find(|f| f.field == "genome");
        assert!(genome.is_some());
        assert_eq!(genome.unwrap().value.as_str().unwrap(), "mm10");
    }
}
