//! Built-in domain knowledge — tool references, error patterns, best practices.
//!
//! All data is compiled into the binary as static constants.

// ── Tool reference ─────────────────────────────────────────────────────────

/// A bioinformatics tool known to oxo-flow.
#[derive(Debug, Clone)]
pub struct ToolRef {
    pub name: &'static str,
    pub domain: &'static str,
    pub key_params: &'static str,
    pub recommended_threads: &'static str,
    pub recommended_memory: &'static str,
    pub input_types: &'static str,
    pub output_types: &'static str,
    pub notes: &'static str,
}

/// Built-in tool reference table — the AI uses this to select appropriate
/// tools and set correct resource allocations.
pub static TOOL_TABLE: &[ToolRef] = &[
    ToolRef {
        name: "fastp",
        domain: "QC / read trimming",
        key_params: "--detect_adapter_for_pe, --thread, --json, --html",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "fastq (.fq, .fastq, .fq.gz, .fastq.gz)",
        output_types: "fastq (.fq.gz, .fastq.gz), JSON report, HTML report",
        notes: "Auto-detects adapters for paired-end reads. Generates QC reports.",
    },
    ToolRef {
        name: "STAR",
        domain: "RNA-seq alignment",
        key_params: "--genomeDir, --runThreadN, --outSAMtype, --readFilesCommand",
        recommended_threads: "16",
        recommended_memory: "32GB",
        input_types: "fastq, genome index directory",
        output_types: "BAM/SAM, splice junctions, log files",
        notes: "Requires pre-built genome index. Memory-intensive — 32GB minimum for human genome.",
    },
    ToolRef {
        name: "BWA-MEM",
        domain: "DNA alignment",
        key_params: "-t threads, -M, -R readgroup",
        recommended_threads: "8",
        recommended_memory: "24GB",
        input_types: "fastq, reference genome (.fa)",
        output_types: "SAM",
        notes: "Standard for DNA-seq alignment. 4-8 threads optimal.",
    },
    ToolRef {
        name: "bowtie2",
        domain: "ChIP-seq / DNA alignment",
        key_params: "-x index, --very-sensitive, -p threads",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "fastq, bowtie2 index",
        output_types: "SAM",
        notes: "Good for short reads. Lower memory than BWA.",
    },
    ToolRef {
        name: "featureCounts",
        domain: "Quantification",
        key_params: "-a annotation.gtf, -o output, -T threads, -t feature_type",
        recommended_threads: "8",
        recommended_memory: "16GB",
        input_types: "BAM/SAM, GTF/GFF annotation",
        output_types: "count table (.txt)",
        notes: "Part of subread package. Fast, accurate gene-level quantification.",
    },
    ToolRef {
        name: "GATK HaplotypeCaller",
        domain: "Variant calling",
        key_params: "-R reference, -I input.bam, -O output.vcf",
        recommended_threads: "8",
        recommended_memory: "16GB",
        input_types: "BAM (sorted, marked duplicates), reference genome",
        output_types: "VCF/ GVCF",
        notes: "Requires pre-processing: mark duplicates, base recalibration.",
    },
    ToolRef {
        name: "MACS2",
        domain: "Peak calling (ChIP-seq/ATAC-seq)",
        key_params: "-f BAMPE, -g genome_size, -q 0.05, --nomodel",
        recommended_threads: "2",
        recommended_memory: "8GB",
        input_types: "BAM (treatment + control)",
        output_types: "narrowPeak / broadPeak, bedGraph",
        notes: "Use BAMPE for paired-end. --nomodel for ATAC-seq.",
    },
    ToolRef {
        name: "samtools",
        domain: "BAM/SAM manipulation",
        key_params: "sort, index, view, flagstat, idxstats",
        recommended_threads: "4",
        recommended_memory: "4GB",
        input_types: "SAM/BAM/CRAM",
        output_types: "BAM/SAM, index (.bai), stats",
        notes: "Essential utility. sort and index are common workflow steps.",
    },
    ToolRef {
        name: "picard",
        domain: "BAM processing",
        key_params: "MarkDuplicates, CollectInsertSizeMetrics, AddOrReplaceReadGroups",
        recommended_threads: "4",
        recommended_memory: "16GB",
        input_types: "BAM/SAM",
        output_types: "BAM, metrics files",
        notes: "Java tool. Set -Xmx for heap size. MarkDuplicates is standard pre-processing.",
    },
    ToolRef {
        name: "multiqc",
        domain: "QC aggregation",
        key_params: ". -o output_dir, --filename, --config",
        recommended_threads: "1",
        recommended_memory: "2GB",
        input_types: "log files from bioinformatics tools",
        output_types: "HTML report",
        notes: "Aggregates QC reports from multiple tools into a single HTML report.",
    },
];

impl ToolRef {
    /// Look up a tool by name.
    pub fn find(name: &str) -> Option<&'static ToolRef> {
        TOOL_TABLE
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Format the tool reference as a markdown table row for prompts.
    pub fn to_table_row(&self) -> String {
        format!(
            "| {} | {} | {} | {} threads, {} | {} |",
            self.name,
            self.domain,
            self.key_params,
            self.recommended_threads,
            self.recommended_memory,
            self.notes
        )
    }

    /// Format the full tool table as a markdown section.
    pub fn table_header() -> &'static str {
        "| Tool | Domain | Key Parameters | Resources | Notes |\n\
         |------|--------|---------------|-----------|-------|"
    }
}

// ── Error patterns ─────────────────────────────────────────────────────────

/// A known error pattern for AI diagnosis.
#[derive(Debug, Clone)]
pub struct ErrorPattern {
    pub pattern: &'static str,
    pub symptom: &'static str,
    pub likely_cause: &'static str,
    pub fix_action: &'static str,
    pub severity: &'static str,
}

pub static ERROR_PATTERNS: &[ErrorPattern] = &[
    ErrorPattern {
        pattern: "exit code 137",
        symptom: "Process killed by kernel (SIGKILL)",
        likely_cause: "Out of memory (OOM). The system killed the process because it exceeded available memory.",
        fix_action: "Increase memory allocation, reduce thread count, or split input into smaller chunks.",
        severity: "critical",
    },
    ErrorPattern {
        pattern: "exit code 139",
        symptom: "Segmentation fault (SIGSEGV)",
        likely_cause: "Memory corruption, incompatible library, or tool bug with given parameters.",
        fix_action: "Reduce threads, check tool version compatibility, verify input data integrity.",
        severity: "critical",
    },
    ErrorPattern {
        pattern: "No such file or directory",
        symptom: "Missing input file or reference data",
        likely_cause: "Dependency rule failed silently, path typo, or reference not downloaded.",
        fix_action: "Check dependency rules, verify file paths, add explicit depends_on, download reference first.",
        severity: "high",
    },
    ErrorPattern {
        pattern: "No space left on device",
        symptom: "Disk full during execution",
        likely_cause: "Intermediate files consuming all available disk space.",
        fix_action: "Clean intermediate files, increase disk quota, or add cleanup rules.",
        severity: "high",
    },
    ErrorPattern {
        pattern: "Permission denied",
        symptom: "Cannot read/write file",
        likely_cause: "Incorrect file permissions or user mismatch.",
        fix_action: "Check file ownership and permissions. Verify write access to output directory.",
        severity: "medium",
    },
    ErrorPattern {
        pattern: "command not found",
        symptom: "Tool executable not in PATH",
        likely_cause: "Missing environment setup (conda/docker) or tool not installed.",
        fix_action: "Add environment declaration (conda/docker) to rule, or install the tool.",
        severity: "high",
    },
];

// ── Best practices ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BestPractice {
    pub id: &'static str,
    pub description: &'static str,
    pub severity: PracticeSeverity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PracticeSeverity {
    Error,
    Warning,
    Info,
}

pub static BEST_PRACTICES: &[BestPractice] = &[
    BestPractice {
        id: "QC-every-rule",
        description: "Every rule that processes data should include or be preceded by a quality control step.",
        severity: PracticeSeverity::Warning,
    },
    BestPractice {
        id: "resource-constraints",
        description: "Every rule must specify threads and memory. Never rely on defaults for production pipelines.",
        severity: PracticeSeverity::Error,
    },
    BestPractice {
        id: "environment-declaration",
        description: "Every rule should declare its software environment (conda, docker, or module).",
        severity: PracticeSeverity::Warning,
    },
    BestPractice {
        id: "no-destructive-commands",
        description: "Never use rm -rf, forceful overwrite without backup, or commands that modify files outside the work directory.",
        severity: PracticeSeverity::Error,
    },
    BestPractice {
        id: "input-validation",
        description: "Validate input files exist before processing. Use explicit depends_on to ensure ordering.",
        severity: PracticeSeverity::Error,
    },
    BestPractice {
        id: "version-pinning",
        description: "Pin tool versions in environment declarations for reproducibility.",
        severity: PracticeSeverity::Info,
    },
];

// ── Format helpers ─────────────────────────────────────────────────────────

/// Format the full tool reference table as a prompt section.
pub fn format_tool_table() -> String {
    let mut s = String::from(ToolRef::table_header());
    s.push('\n');
    for tool in TOOL_TABLE {
        s.push_str(&tool.to_table_row());
        s.push('\n');
    }
    s
}

/// Format error patterns as a prompt section.
pub fn format_error_patterns() -> String {
    let mut s = String::from("| Pattern | Cause | Fix |\n|---------|-------|-----|\n");
    for ep in ERROR_PATTERNS {
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            ep.pattern, ep.likely_cause, ep.fix_action
        ));
    }
    s
}

/// Format best practices as a prompt section.
pub fn format_best_practices() -> String {
    let mut s = String::from("## Oxo-flow Best Practices\n\n");
    for bp in BEST_PRACTICES {
        let severity = match bp.severity {
            PracticeSeverity::Error => "ERROR",
            PracticeSeverity::Warning => "WARNING",
            PracticeSeverity::Info => "INFO",
        };
        s.push_str(&format!("- [{severity}] {}\n", bp.description));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_table_has_entries() {
        assert!(TOOL_TABLE.len() >= 8);
    }

    #[test]
    fn find_tool_by_name() {
        let star = ToolRef::find("STAR").unwrap();
        assert_eq!(star.name, "STAR");
        assert_eq!(star.domain, "RNA-seq alignment");
    }

    #[test]
    fn find_tool_case_insensitive() {
        assert!(ToolRef::find("fastp").is_some());
        assert!(ToolRef::find("FASTP").is_some());
    }

    #[test]
    fn find_nonexistent_tool() {
        assert!(ToolRef::find("nonexistent").is_none());
    }

    #[test]
    fn error_patterns_has_entries() {
        assert!(ERROR_PATTERNS.len() >= 5);
    }

    #[test]
    fn best_practices_has_entries() {
        assert!(BEST_PRACTICES.len() >= 4);
    }

    #[test]
    fn format_tool_table_is_nonempty() {
        let table = format_tool_table();
        assert!(table.contains("STAR"));
        assert!(table.contains("fastp"));
        assert!(table.contains("| Tool |"));
    }

    #[test]
    fn format_error_patterns_is_nonempty() {
        let patterns = format_error_patterns();
        assert!(patterns.contains("exit code 137"));
        assert!(patterns.contains("OOM"));
    }

    #[test]
    fn format_best_practices_is_nonempty() {
        let practices = format_best_practices();
        assert!(practices.contains("ERROR"));
        assert!(practices.contains("WARNING"));
    }
}
