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

/// Built-in tool reference table — curated from Bioconda channel metadata
/// (oxo-call-extends). The AI uses this to select appropriate tools, set
/// correct resource allocations, and understand each tool's purpose.
///
/// Tool VERSIONS are intentionally not pinned here — the LLM determines
/// current stable versions from its own knowledge or web search.
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
        name: "fastqc",
        domain: "QC / read quality assessment",
        key_params: "-o output_dir, -t threads",
        recommended_threads: "2",
        recommended_memory: "4GB",
        input_types: "fastq, BAM/SAM",
        output_types: "HTML report, ZIP archive",
        notes: "Standard per-sample quality report. Pair with multiqc for aggregation.",
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
        name: "hisat2",
        domain: "RNA-seq alignment",
        key_params: "-x index, -1/-2 paired reads, --dta",
        recommended_threads: "8",
        recommended_memory: "16GB",
        input_types: "fastq, hisat2 index",
        output_types: "SAM/BAM",
        notes: "Splice-aware RNA-seq aligner. Faster and lower-memory than STAR; use --dta for StringTie.",
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
        name: "salmon",
        domain: "RNA-seq quantification",
        key_params: "quant -i index, -l A, -1/-2 reads, -o output",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "fastq, salmon index",
        output_types: "quant.sf abundance table",
        notes: "Alignment-free transcript quantification. Needs transcriptome FASTA index.",
    },
    ToolRef {
        name: "GATK HaplotypeCaller",
        domain: "Variant calling",
        key_params: "-R reference, -I input.bam, -O output.vcf, --native-pair-hmm-threads",
        recommended_threads: "4",
        recommended_memory: "32GB",
        input_types: "BAM (sorted, marked duplicates), reference genome",
        output_types: "VCF/ GVCF",
        notes: "Requires pre-processing: mark duplicates, base recalibration. Java parallelism limited to ~4 threads; 32GB heap for human WGS.",
    },
    ToolRef {
        name: "GATK Mutect2",
        domain: "Somatic variant calling",
        key_params: "-R reference, -I tumor, -I normal, -normal name, --germline-resource",
        recommended_threads: "4",
        recommended_memory: "16GB",
        input_types: "BAM tumor + matched normal (recalibrated), germline resource VCF",
        output_types: "VCF (somatic calls with TLOD filtering)",
        notes: "Paired tumor-normal mode requires -normal and af-only-gnomad germline resource. Pair with FilterMutectCalls.",
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
        name: "bcftools",
        domain: "VCF manipulation",
        key_params: "filter, view, sort, stats, concat",
        recommended_threads: "2",
        recommended_memory: "4GB",
        input_types: "VCF/BCF (+ index)",
        output_types: "VCF/BCF, stats report",
        notes: "Standard VCF filtering and manipulation. Use -Oz for compressed output and index -t after.",
    },
    ToolRef {
        name: "bedtools",
        domain: "Genomic interval operations",
        key_params: "intersect, merge, sort, coverage",
        recommended_threads: "1",
        recommended_memory: "4GB",
        input_types: "BED/GFF/VCF",
        output_types: "BED/GFF intervals",
        notes: "Most subcommands are single-threaded. Keep intervals sorted when required.",
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
        name: "kraken2",
        domain: "Metagenomics classification",
        key_params: "--db database, --threads, --report, --paired",
        recommended_threads: "8",
        recommended_memory: "64GB",
        input_types: "fastq (single or paired)",
        output_types: "classification report, read assignments",
        notes: "Memory-bound: standard database requires 50-80GB RAM. Use Bracken for abundance estimation.",
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
    ToolRef {
        name: "VEP",
        domain: "Variant annotation",
        key_params: "--input_file, --output_file, --cache, --offline, --vcf",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "VCF",
        output_types: "annotated VCF, tabular summary",
        notes: "Ensembl Variant Effect Predictor. Requires downloaded cache for --offline mode.",
    },
    ToolRef {
        name: "StringTie",
        domain: "Transcript assembly",
        key_params: "-e -B -G annotation.gtf -o output",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "sorted BAM (RNA-seq)",
        output_types: "GTF transcript models, abundance tables",
        notes: "Transcript structure recovery and abundance estimation from RNA-seq alignments. Use --dta-aligned BAM from HISAT2.",
    },
    ToolRef {
        name: "kallisto",
        domain: "RNA-seq quantification",
        key_params: "quant -i index -o output -t threads",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "fastq, kallisto index",
        output_types: "abundance.tsv",
        notes: "Ultra-fast pseudo-alignment quantification. Needs a transcriptome index.",
    },
    ToolRef {
        name: "cutadapt",
        domain: "Adapter trimming",
        key_params: "-a ADAPTER -o output -j cores",
        recommended_threads: "4",
        recommended_memory: "4GB",
        input_types: "fastq",
        output_types: "fastq",
        notes: "Trims adapters and low-quality ends from high-throughput reads. Python-based.",
    },
    ToolRef {
        name: "Trimmomatic",
        domain: "Read trimming",
        key_params: "PE/SE, ILLUMINACLIP, SLIDINGWINDOW, MINLEN",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "fastq (paired or single)",
        output_types: "fastq (paired/unpaired/singletons)",
        notes: "Flexible read trimming tool for Illumina NGS data (Java). Classic but fastp is now preferred.",
    },
    ToolRef {
        name: "seqtk",
        domain: "FASTA/FASTQ processing",
        key_params: "seq, sample, subseq, trimfq",
        recommended_threads: "1",
        recommended_memory: "1GB",
        input_types: "FASTA/FASTQ",
        output_types: "FASTA/FASTQ",
        notes: "Fast lightweight toolkit for FASTA/FASTQ sequence processing.",
    },
    ToolRef {
        name: "SRA Toolkit",
        domain: "Data download",
        key_params: "prefetch, fasterq-dump, vdb-validate",
        recommended_threads: "4",
        recommended_memory: "4GB",
        input_types: "SRA accession",
        output_types: "fastq",
        notes: "NCBI SRA Toolkit — download and convert sequencing data from the Sequence Read Archive.",
    },
    ToolRef {
        name: "SnpEff",
        domain: "Variant annotation",
        key_params: "ann, build, databases",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "VCF, reference genome build",
        output_types: "annotated VCF",
        notes: "Genetic variant annotation and effect prediction (Java). SnpSift complements for filtering.",
    },
    ToolRef {
        name: "htslib",
        domain: "Sequencing format library",
        key_params: "tabix, bgzip",
        recommended_threads: "1",
        recommended_memory: "1GB",
        input_types: "VCF/BCF/TSV",
        output_types: "compressed + indexed files",
        notes: "C library for high-throughput sequencing formats; provides bgzip compression and tabix indexing.",
    },
    ToolRef {
        name: "vcftools",
        domain: "VCF statistics",
        key_params: "--vcf, --freq, --depth, --missing",
        recommended_threads: "1",
        recommended_memory: "2GB",
        input_types: "VCF",
        output_types: "stats tables, filtered VCF",
        notes: "Classic VCF statistics/filtering toolkit. Largely superseded by bcftools but still common.",
    },
    ToolRef {
        name: "minimap2",
        domain: "Long-read alignment",
        key_params: "-ax map-ont / map-pb / sr, -t threads",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "fastq (ONT/PacBio), reference FASTA",
        output_types: "SAM/BAM, PAF",
        notes: "Versatile pairwise aligner for genomic and spliced nucleotide sequences. Default for ONT and PacBio.",
    },
    ToolRef {
        name: "Canu",
        domain: "Long-read assembly",
        key_params: "-p prefix -d dir genomeSize=... -nanopore reads.fq",
        recommended_threads: "16",
        recommended_memory: "64GB",
        input_types: "ONT/PacBio reads",
        output_types: "contig FASTA",
        notes: "High-noise single-molecule assembler; three phases: correction, trimming, assembly.",
    },
    ToolRef {
        name: "SPAdes",
        domain: "Short-read assembly",
        key_params: "-o out --isolate / --meta -1 -2 reads",
        recommended_threads: "16",
        recommended_memory: "32GB",
        input_types: "Illumina reads",
        output_types: "contigs/scaffolds",
        notes: "De Bruijn graph assembler for isolates and single-cell. --meta mode for metagenomes.",
    },
    ToolRef {
        name: "MEGAHIT",
        domain: "Metagenomics assembly",
        key_params: "-1 -2 -o out --num-cpu-threads",
        recommended_threads: "16",
        recommended_memory: "32GB",
        input_types: "metagenomic reads",
        output_types: "contigs",
        notes: "Ultra-fast single-node metagenomics assembler via succinct de Bruijn graphs.",
    },
    ToolRef {
        name: "DeepVariant",
        domain: "ML variant calling",
        key_params: "--model_type, --ref, --reads, --output_vcf",
        recommended_threads: "8",
        recommended_memory: "16GB",
        input_types: "BAM + reference",
        output_types: "VCF",
        notes: "Deep neural network variant caller from Google. Supports WGS/WES/PacBio/ONT via model_type.",
    },
    ToolRef {
        name: "freebayes",
        domain: "Variant calling",
        key_params: "-f reference, --min-alternate-fraction",
        recommended_threads: "4",
        recommended_memory: "16GB",
        input_types: "BAM + reference",
        output_types: "VCF",
        notes: "Bayesian haplotype-based polymorphism discovery and genotyping. Good for pooled and low-coverage samples.",
    },
    ToolRef {
        name: "VarScan",
        domain: "Variant detection",
        key_params: "mpileup2snp, mpileup2indel, somatic",
        recommended_threads: "1",
        recommended_memory: "8GB",
        input_types: "mpileup input",
        output_types: "VCF",
        notes: "Variant detection in massively parallel sequencing data (Java). Common in somatic pipelines.",
    },
    ToolRef {
        name: "CNVkit",
        domain: "Copy number detection",
        key_params: "batch, reference, fix, segment, call",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "BAM (tumor + normals)",
        output_types: "segments, calls, plots",
        notes: "Copy number variant detection and visualization from targeted or whole-genome sequencing.",
    },
    ToolRef {
        name: "DELLY",
        domain: "Structural variants",
        key_params: "call -t DEL/DUP/INV -g reference",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "BAM + reference",
        output_types: "VCF/BCF",
        notes: "Integrated paired-end and split-read structural variant discovery.",
    },
    ToolRef {
        name: "smoove",
        domain: "SV calling/genotyping",
        key_params: "call, merge, genotype, annotate",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "BAM + reference",
        output_types: "VCF",
        notes: "Structural variant calling and genotyping wrapper around lumpy-sv with filtering.",
    },
    ToolRef {
        name: "Qualimap",
        domain: "Alignment QC",
        key_params: "bamqc, rnaseq, multi-bamqc",
        recommended_threads: "4",
        recommended_memory: "8GB",
        input_types: "BAM",
        output_types: "HTML report, PDF",
        notes: "Quality control of alignment data: coverage, mapping quality, feature counts.",
    },
    ToolRef {
        name: "RSeQC",
        domain: "RNA-seq QC",
        key_params: "bam_stat.py, infer_experiment.py, junction_annotation.py",
        recommended_threads: "4",
        recommended_memory: "4GB",
        input_types: "BAM (RNA-seq)",
        output_types: "stats text files",
        notes: "Comprehensive QC package for RNA-seq BAM files. Python scripts.",
    },
    ToolRef {
        name: "Cufflinks",
        domain: "Transcript assembly",
        key_params: "-G annotation.gtf -o output",
        recommended_threads: "8",
        recommended_memory: "8GB",
        input_types: "BAM (RNA-seq)",
        output_types: "GTF, FPKM tables",
        notes: "Transcriptome assembly and differential expression for RNA-seq. Legacy — StringTie is the modern replacement.",
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
            "- {} [{}]: {}; resources: {} threads/{}; {}",
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
        "## Tools\n"
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
        assert!(table.contains("## Tools"));
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
