//! Enumerable view of the SCI-* scientific-preflight catalog.
//!
//! The checks in [`crate::scientific_preflight`] emit their codes as inline
//! string literals inside each `ScientificWarning { code: "SCI-…" }`
//! construction, so there is no in-source struct to iterate. This module
//! carries the curated enumeration list beside those checks and a
//! compile-time drift test (`source_codes_match_catalog`) that parses the
//! check source and fails if the two ever diverge — so the list below can
//! never silently fall out of sync with the codes the engine actually emits.
//!
//! Consumers: `oxo-flow diagnostics list`, and any tooling that needs to
//! discover the rule set without opening source.

use serde::{Deserialize, Serialize};

/// One enumerable scientific-preflight rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SciRuleInfo {
    /// Stable rule code, e.g. `SCI-VQSR-COHORT`.
    pub code: String,
    /// One-line human summary of what the rule flags.
    pub summary: String,
}

/// Curated enumeration of every SCI-* code the preflight engine can emit.
///
/// Authored in code order so the enumeration is deterministic without a sort.
/// Kept in lockstep with `scientific_preflight.rs` by the drift test at the
/// bottom of this file. Add a rule here (and to the checks) together.
pub const SCI_CATALOG: &[(&str, &str)] = &[
    (
        "SCI-AGG-RACE",
        "Fan-out dimensions not keyed by declared outputs cause concurrent races.",
    ),
    (
        "SCI-ATAC-SHIFT-COORDINATES",
        "ATAC-seq peak calling runs without Tn5 coordinate shifting.",
    ),
    (
        "SCI-BEDTOOLS-GENOME-FILE",
        "bedtools slop/flank runs without chromosome lengths (-g).",
    ),
    (
        "SCI-BISMARK-NON-DIRECTIONAL",
        "Bismark PBAT library alignment lacks --non_directional.",
    ),
    (
        "SCI-BOWTIE-BISULFITE-MISMATCH",
        "Bisulfite alignment uses excessively strict --score-min L,0,0.",
    ),
    (
        "SCI-BOWTIE2-NO-RG",
        "bowtie2 alignment is missing a read group (--rg-id / --rg).",
    ),
    (
        "SCI-BQSR-NO-KNOWN-SITES",
        "BaseRecalibrator runs without --known-sites resources.",
    ),
    (
        "SCI-BRACKEN-READ-LEN",
        "bracken abundance estimation lacks a read length (-r).",
    ),
    (
        "SCI-BWA-MEM-NO-RG",
        "bwa mem is invoked without a read group (-R).",
    ),
    (
        "SCI-CELLRANGER-INTRONS",
        "cellranger count on snRNA-seq lacks --include-introns.",
    ),
    (
        "SCI-CHECKM-NO-LINEAGE",
        "checkm lineage_wf is missing the genome fasta extension (-x).",
    ),
    (
        "SCI-CHIP-NO-CONTROL",
        "macs2 callpeak runs without a matched control/input (-c).",
    ),
    (
        "SCI-DEEPVARIANT-MODEL-TYPE",
        "DeepVariant invocation lacks the required --model_type.",
    ),
    (
        "SCI-DESEQ2-MIN-REPLICATES",
        "DESeq2 runs with too few biological replicates.",
    ),
    (
        "SCI-FASTP-DEDUPLICATION",
        "fastp --dedup enabled without UMI tagging on RNA-seq.",
    ),
    (
        "SCI-FEATURECOUNTS-MULTI-OVERLAP",
        "featureCounts runs with -O assigning reads to all overlaps.",
    ),
    (
        "SCI-FEATURECOUNTS-STRAND",
        "featureCounts runs without an explicit strandness flag.",
    ),
    (
        "SCI-GENOTYPEGVCFS-NO-NEW-QUAL",
        "GenotypeGVCFs runs without an explicit --new-qual model.",
    ),
    (
        "SCI-HAPLOTYPECALLER-GVCF-ERC",
        "HaplotypeCaller targets a gVCF path without -ERC GVCF.",
    ),
    (
        "SCI-HISAT2-NO-SPLICE-SITES",
        "hisat2 alignment runs without --known-splicesite-infile.",
    ),
    (
        "SCI-HUMANN-NUCLEOTIDE-DB",
        "HUMAnN runs without an explicit --nucleotide-database.",
    ),
    (
        "SCI-KALLISTO-SINGLE-END-FRAG",
        "kallisto single-end quant lacks a fragment length (-l).",
    ),
    (
        "SCI-KRAKEN2-CONFIDENCE",
        "kraken2 runs with the default confidence threshold of 0.0.",
    ),
    (
        "SCI-MACS2-BROAD-PEAKS",
        "macs2 callpeak targets broad histone marks without --broad.",
    ),
    (
        "SCI-MACS2-GENOME-SIZE",
        "macs2 callpeak is missing explicit effective genome size (-g).",
    ),
    (
        "SCI-MACS2-PAIRED-END-FORMAT",
        "macs2 specifies single-end -f BAM for a paired-end library.",
    ),
    (
        "SCI-MANTA-NO-CONFIG",
        "Manta SV config lacks a reference genome FASTA specification.",
    ),
    (
        "SCI-MARKDUPLICATES-REMOVE-DUPS",
        "MarkDuplicates configured with REMOVE_DUPLICATES=true.",
    ),
    (
        "SCI-METAPHLAN-UNKNOWN-ESTIMATION",
        "MetaPhlAn --unknown_estimation lacks an explicit marker DB.",
    ),
    (
        "SCI-MINIMAP2-PRESET",
        "minimap2 is invoked without an explicit preset (-x).",
    ),
    (
        "SCI-MULTI-TENANT-PORT-COLLISION",
        "A server rule hardcodes a port, colliding across concurrent runs.",
    ),
    (
        "SCI-MUTECT2-NO-GERMLINE",
        "Tumor-only Mutect2 is missing --germline-resource.",
    ),
    (
        "SCI-MUTECT2-NO-PON",
        "Tumor-only Mutect2 is missing a Panel of Normals (PoN).",
    ),
    (
        "SCI-MUTECT2-TUMOR-ONLY",
        "Mutect2 runs without a matched normal (tumor-only mode).",
    ),
    (
        "SCI-PIPE-FAIL-DISABLED",
        "A shell pipeline uses '|' without 'set -o pipefail'.",
    ),
    (
        "SCI-QIIME2-DENOISE-TRIM",
        "QIIME2 DADA2 denoising runs without 3' quality truncation.",
    ),
    (
        "SCI-RSEM-NO-STRAND",
        "rsem-calculate-expression lacks --strand-specific configuration.",
    ),
    (
        "SCI-SALMON-LIBTYPE",
        "salmon quant uses automatic library-type detection (-l A).",
    ),
    (
        "SCI-SALMON-NO-GC-BIAS",
        "salmon quant runs without --gcBias correction.",
    ),
    (
        "SCI-SAMTOOLS-INDEX-UNSORTED",
        "samtools index invoked on input that is not sorted BAM/CRAM.",
    ),
    (
        "SCI-SEURAT-NO-NORMALIZATION",
        "Single-cell clustering runs without apparent prior normalization.",
    ),
    (
        "SCI-SORT-THREADS-MEM-PRODUCT",
        "samtools sort threads times memory per thread exceeds budget.",
    ),
    (
        "SCI-STAR-GENOME-LOAD",
        "STAR --genomeLoad LoadAndKeep is unsafe in batch/cluster environments.",
    ),
    (
        "SCI-STAR-SJDB-OVERHANG",
        "STAR index build uses a GTF but omits --sjdbOverhang.",
    ),
    (
        "SCI-STRELKA-NO-MANTA",
        "Strelka somatic run lacks Manta candidate indels.",
    ),
    (
        "SCI-STRELKA-TARGETS-UNCOMPRESSED",
        "Strelka target BED is not bgzip-compressed and tabix-indexed.",
    ),
    (
        "SCI-STRINGTIE-NO-GUIDE",
        "stringtie assembly runs without a reference annotation guide (-G).",
    ),
    (
        "SCI-TMPDIR-NOT-SCRATCH",
        "A tool writes temp files to /tmp instead of engine scratch space.",
    ),
    (
        "SCI-VEP-OFFLINE-CACHE",
        "VEP runs without local --cache or --offline flags.",
    ),
    (
        "SCI-VQSR-COHORT",
        "VariantRecalibrator trained on too few samples for reliable VQSR.",
    ),
    (
        "SCI-ZERO-BYTE-COMPLETION",
        "A rule masks failure by creating zero-byte biological artifacts.",
    ),
];

/// Enumerate the SCI-* catalog, code-ordered (`SCI_CATALOG` is authored in
/// code order and ids are unique, so the ordering is total and stable).
#[must_use]
pub fn sci_catalog() -> Vec<SciRuleInfo> {
    SCI_CATALOG
        .iter()
        .map(|(code, summary)| SciRuleInfo {
            code: (*code).to_string(),
            summary: (*summary).to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The distinct `code: "SCI-…"` ids the preflight engine can emit, parsed
    /// from the check source at compile time so the id list is never retyped
    /// here (mirrors `tests/scientific_preflight_adversarial.rs`).
    fn source_codes() -> BTreeSet<String> {
        let src = include_str!("scientific_preflight.rs");
        let mut ids = BTreeSet::new();
        for part in src.split("code: \"").skip(1) {
            if let Some(end) = part.find('"') {
                let id = &part[..end];
                if id.starts_with("SCI-") {
                    ids.insert(id.to_string());
                }
            }
        }
        ids
    }

    #[test]
    fn source_codes_match_catalog() {
        let src_ids = source_codes();
        assert!(
            !src_ids.is_empty(),
            "failed to parse any SCI- id from source"
        );

        let catalog_ids: BTreeSet<String> = sci_catalog().iter().map(|r| r.code.clone()).collect();

        assert_eq!(
            catalog_ids, src_ids,
            "SCI_CATALOG must enumerate exactly the codes the engine emits"
        );
        assert_eq!(catalog_ids.len(), 51, "expected 51 curated SCI-* rules");
    }

    #[test]
    fn sci_catalog_is_code_ordered_and_nonempty() {
        let rows = sci_catalog();
        assert_eq!(rows.len(), SCI_CATALOG.len());
        let codes: Vec<&str> = rows.iter().map(|r| r.code.as_str()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        assert_eq!(codes, sorted, "sci_catalog must be code-sorted");
        assert!(rows.iter().all(|r| !r.summary.is_empty()));
    }
}
