//! Tool Expert Agent — recommends tools with resource hints and alternatives.
//!
//! Maps analysis intent → tool recommendations with optimal parameters,
//! resource requirements, and alternative tools.

use super::types::*;

/// Recommend tools for a given analysis intent and data context.
pub fn recommend_tools(intent: &str, data_findings: &[DataFinding]) -> Vec<ToolRecommendation> {
    let intent_lower = intent.to_lowercase();
    let mut recommendations = Vec::new();

    // Detect read length for splice-aware aligners
    let _read_len = data_findings
        .iter()
        .find(|f| f.field == "read_type")
        .and_then(|f| f.value.as_str())
        .unwrap_or("unknown");

    let paired_end = data_findings
        .iter()
        .find(|f| f.field == "paired_end")
        .and_then(|f| f.value.as_bool())
        .unwrap_or(true);

    let adapter_contam = data_findings
        .iter()
        .find(|f| f.field == "adapter_contamination")
        .and_then(|f| f.value.as_u64())
        .unwrap_or(0);

    // Check for available tools based on intent
    if intent_lower.contains("rna-seq")
        || intent_lower.contains("rnaseq")
        || intent_lower.contains("transcriptome")
        || intent_lower.contains("differential expression")
    {
        // QC step
        let mut fastp_params = serde_json::json!({
            "--detect_adapter_for_pe": paired_end,
            "--cut_front": true,
            "--cut_tail": true,
            "--threads": "auto"
        });
        if adapter_contam > 0 {
            fastp_params["--detect_adapter_for_pe"] = serde_json::json!(true);
        }
        recommendations.push(ToolRecommendation {
            rule_name: "fastp".into(),
            tool: "fastp".into(),
            purpose: format!(
                "QC and adapter trimming{}",
                if paired_end { " (PE)" } else { "" }
            ),
            key_params: fastp_params,
            resource_hint: ResourceHint {
                threads: 4,
                memory_gb: 8,
                disk_gb: Some(10),
                wall_time: Some("30min".into()),
            },
            alternatives: vec!["fastqc".into(), "trimmomatic".into(), "cutadapt".into()],
            confidence: 0.95,
        });

        // Alignment step
        let _aligner = "STAR";
        let mut star_params = serde_json::json!({
            "--genomeDir": "/data/references/{genome}/star",
            "--readFilesCommand": "zcat",
            "--outSAMtype": "BAM SortedByCoordinate",
            "--outWigType": "wiggle read1",
            "--quantMode": "TranscriptomeSAM GeneCounts"
        });
        if !paired_end {
            star_params["--outSAMstrandField"] = serde_json::json!("intronMotif");
        }
        recommendations.push(ToolRecommendation {
            rule_name: "star_align".into(),
            tool: "STAR".into(),
            purpose: "Splice-aware alignment to reference genome".into(),
            key_params: star_params,
            resource_hint: ResourceHint {
                threads: 16,
                memory_gb: 32,
                disk_gb: Some(50),
                wall_time: Some("2h".into()),
            },
            alternatives: vec!["HISAT2".into(), "bowtie2".into(), "kallisto".into()],
            confidence: 0.9,
        });

        // Quantification step
        recommendations.push(ToolRecommendation {
            rule_name: "featurecounts".into(),
            tool: "featureCounts".into(),
            purpose: "Gene-level quantification from BAM files".into(),
            key_params: serde_json::json!({
                "-a": "/data/references/{genome}/genes.gtf",
                "-t": "exon",
                "-g": "gene_id",
                "--extraAttributes": "gene_name",
                "-s": 0,
                "-p": paired_end,
            }),
            resource_hint: ResourceHint {
                threads: 8,
                memory_gb: 16,
                disk_gb: Some(10),
                wall_time: Some("1h".into()),
            },
            alternatives: vec!["htseq-count".into(), "salmon".into(), "RSEM".into()],
            confidence: 0.9,
        });
    }

    if intent_lower.contains("variant")
        || intent_lower.contains("wgs")
        || intent_lower.contains("germline")
        || intent_lower.contains("somatic")
    {
        recommendations.push(ToolRecommendation {
            rule_name: "bwa_mem".into(),
            tool: "bwa".into(),
            purpose: "Read alignment with BWA-MEM for variant calling".into(),
            key_params: serde_json::json!({
                "-t": "${threads}",
                "-M": true,
                "-R": "@RG\\tID:{sample}\\tSM:{sample}\\tPL:ILLUMINA"
            }),
            resource_hint: ResourceHint {
                threads: 16,
                memory_gb: 24,
                disk_gb: Some(30),
                wall_time: Some("3h".into()),
            },
            alternatives: vec!["minimap2".into(), "bowtie2".into()],
            confidence: 0.85,
        });

        recommendations.push(ToolRecommendation {
            rule_name: "gatk_hc".into(),
            tool: "GATK".into(),
            purpose: "Variant calling with GATK HaplotypeCaller".into(),
            key_params: serde_json::json!({
                "-R": "/data/references/{genome}/genome.fa",
                "--emit-ref-confidence": "GVCF",
                "-ERC": "GVCF",
            }),
            resource_hint: ResourceHint {
                threads: 8,
                memory_gb: 16,
                disk_gb: Some(20),
                wall_time: Some("4h".into()),
            },
            alternatives: vec!["freebayes".into(), "bcftools".into(), "strelka2".into()],
            confidence: 0.85,
        });
    }

    if intent_lower.contains("chip-seq") || intent_lower.contains("chipseq") {
        recommendations.push(ToolRecommendation {
            rule_name: "bowtie2_align".into(),
            tool: "bowtie2".into(),
            purpose: "Short-read alignment for ChIP-seq".into(),
            key_params: serde_json::json!({
                "-x": "/data/references/{genome}/bowtie2/genome",
                "--very-sensitive": true,
                "-p": "${threads}"
            }),
            resource_hint: ResourceHint {
                threads: 8,
                memory_gb: 8,
                disk_gb: Some(20),
                wall_time: Some("1h".into()),
            },
            alternatives: vec!["BWA".into(), "minimap2".into()],
            confidence: 0.85,
        });

        recommendations.push(ToolRecommendation {
            rule_name: "macs2_callpeak".into(),
            tool: "MACS2".into(),
            purpose: "Peak calling for ChIP-seq".into(),
            key_params: serde_json::json!({
                "-f": "BAMPE",
                "-g": "hs",
                "-q": 0.05,
                "--call-summits": true,
            }),
            resource_hint: ResourceHint {
                threads: 2,
                memory_gb: 8,
                disk_gb: Some(5),
                wall_time: Some("30min".into()),
            },
            alternatives: vec!["SEACR".into(), "epic2".into()],
            confidence: 0.9,
        });
    }

    // QC-only fallback
    if recommendations.is_empty() && intent_lower.contains("qc") {
        recommendations.push(ToolRecommendation {
            rule_name: "fastqc".into(),
            tool: "FastQC".into(),
            purpose: "Quality control of raw sequencing data".into(),
            key_params: serde_json::json!({
                "-t": "${threads}",
                "--nogroup": false,
            }),
            resource_hint: ResourceHint {
                threads: 4,
                memory_gb: 4,
                disk_gb: Some(5),
                wall_time: Some("15min".into()),
            },
            alternatives: vec!["fastp".into(), "MultiQC".into()],
            confidence: 0.95,
        });
    }

    // Generic fallback
    if recommendations.is_empty() {
        recommendations.push(ToolRecommendation {
            rule_name: "qc".into(),
            tool: "fastp".into(),
            purpose: "Quality control and preprocessing".into(),
            key_params: serde_json::json!({
                "-i": "{input}",
                "-o": "{output}",
                "--threads": "${threads}"
            }),
            resource_hint: ResourceHint {
                threads: 4,
                memory_gb: 8,
                disk_gb: Some(10),
                wall_time: Some("30min".into()),
            },
            alternatives: vec!["fastqc".into(), "cutadapt".into()],
            confidence: 0.7,
        });
    }

    recommendations
}

/// Get suggested optimization for a given tool and resource constraints.
pub fn suggest_optimization(
    tool: &str,
    current_threads: u32,
    current_memory_gb: u32,
    max_threads: Option<u32>,
    max_memory_gb: Option<u32>,
) -> Vec<String> {
    let mut suggestions = Vec::new();

    let max_t = max_threads.unwrap_or(64);
    let max_m = max_memory_gb.unwrap_or(256);

    // Thread optimization
    if current_threads < 4 && max_t >= 4 {
        suggestions.push(format!(
            "Increase threads from {current_threads} to {} for better throughput",
            4.min(max_t)
        ));
    } else if current_threads > max_t {
        suggestions.push(format!(
            "Reduce threads from {current_threads} to {max_t} (resource limit)"
        ));
    }

    // Memory optimization
    if current_memory_gb < 8 && max_m >= 8 && tool.to_lowercase() != "fastqc" {
        suggestions.push(format!(
            "Increase memory from {current_memory_gb}GB to {}GB for stability",
            8.min(max_m)
        ));
    } else if current_memory_gb > max_m {
        suggestions.push(format!(
            "Reduce memory from {current_memory_gb}GB to {max_m}GB (resource limit)"
        ));
    }

    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_rnaseq() {
        let recs = recommend_tools("RNA-seq differential expression", &[]);
        assert!(
            recs.len() >= 3,
            "should recommend at least 3 tools for RNA-seq"
        );
        assert!(
            recs.iter().any(|r| r.rule_name == "fastp"),
            "should include fastp"
        );
        assert!(
            recs.iter().any(|r| r.rule_name == "star_align"),
            "should include STAR"
        );
        assert!(
            recs.iter().any(|r| r.rule_name == "featurecounts"),
            "should include featureCounts"
        );
    }

    #[test]
    fn test_recommend_variant() {
        let recs = recommend_tools("WGS variant calling", &[]);
        assert!(recs.len() >= 2);
        assert!(recs.iter().any(|r| r.rule_name == "bwa_mem"));
    }

    #[test]
    fn test_recommend_chipseq() {
        let recs = recommend_tools("ChIP-seq peak calling", &[]);
        assert!(recs.len() >= 2);
        assert!(recs.iter().any(|r| r.rule_name == "macs2_callpeak"));
    }

    #[test]
    fn test_recommend_unknown() {
        let recs = recommend_tools("some random analysis", &[]);
        assert!(!recs.is_empty(), "should provide fallback");
    }

    #[test]
    fn test_suggest_optimization() {
        let s = suggest_optimization("STAR", 2, 4, Some(32), Some(64));
        assert!(!s.is_empty(), "should suggest increasing threads");
    }

    #[test]
    fn test_pe_detection_affects_params() {
        let findings = vec![DataFinding {
            field: "paired_end".into(),
            value: serde_json::json!(true),
            confidence: 0.9,
            source: "file_scan".into(),
            evidence: "PE detected".into(),
        }];
        let recs = recommend_tools("RNA-seq", &findings);
        let fastp = recs.iter().find(|r| r.rule_name == "fastp").unwrap();
        assert_eq!(fastp.key_params["--detect_adapter_for_pe"], true);
    }
}
