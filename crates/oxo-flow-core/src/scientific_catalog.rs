//! Enumerable view of the SCI-* scientific-preflight catalog.
//!
//! The codes are declared ONCE, as the `sci_codes` constants in this module.
//! Every warning site in [`crate::scientific_preflight`] references the
//! matching constant (`code: sci_codes::…`) instead of retyping the literal,
//! and [`SCI_CATALOG`] pairs each constant with its one-line `summary`
//! (prose that only the CLI surfaces — the runtime `message` is longer and
//! distinct). Two compile-time drift tests keep the pairing honest:
//! `source_codes_match_catalog` (the catalog enumerates exactly the declared
//! constants) and `preflight_emits_codes_via_shared_constants_only` (no
//! preflight site may fall back to an inline `code: "SCI-…"` literal) — so a
//! code can never be retyped and silently diverge between the two files.
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

/// Named constants for every `SCI-*` code the preflight engine emits.
///
/// These are the single source of the code strings: both [`SCI_CATALOG`]
/// and the `code: sci_codes::…` sites in `scientific_preflight.rs`
/// reference these constants, so a code can never be retyped. The name
/// is the code minus the `SCI-` prefix with `-` -> `_`.
pub mod sci_codes {
    pub const AGG_RACE: &str = "SCI-AGG-RACE";
    pub const ATAC_SHIFT_COORDINATES: &str = "SCI-ATAC-SHIFT-COORDINATES";
    pub const BEDTOOLS_GENOME_FILE: &str = "SCI-BEDTOOLS-GENOME-FILE";
    pub const BISMARK_NON_DIRECTIONAL: &str = "SCI-BISMARK-NON-DIRECTIONAL";
    pub const BOWTIE_BISULFITE_MISMATCH: &str = "SCI-BOWTIE-BISULFITE-MISMATCH";
    pub const BOWTIE2_NO_RG: &str = "SCI-BOWTIE2-NO-RG";
    pub const BQSR_NO_KNOWN_SITES: &str = "SCI-BQSR-NO-KNOWN-SITES";
    pub const BRACKEN_READ_LEN: &str = "SCI-BRACKEN-READ-LEN";
    pub const BWA_MEM_NO_RG: &str = "SCI-BWA-MEM-NO-RG";
    pub const CELLRANGER_INTRONS: &str = "SCI-CELLRANGER-INTRONS";
    pub const CHECKM_NO_LINEAGE: &str = "SCI-CHECKM-NO-LINEAGE";
    pub const CHIP_NO_CONTROL: &str = "SCI-CHIP-NO-CONTROL";
    pub const DEEPVARIANT_MODEL_TYPE: &str = "SCI-DEEPVARIANT-MODEL-TYPE";
    pub const DESEQ2_MIN_REPLICATES: &str = "SCI-DESEQ2-MIN-REPLICATES";
    pub const FASTP_DEDUPLICATION: &str = "SCI-FASTP-DEDUPLICATION";
    pub const FEATURECOUNTS_MULTI_OVERLAP: &str = "SCI-FEATURECOUNTS-MULTI-OVERLAP";
    pub const FEATURECOUNTS_STRAND: &str = "SCI-FEATURECOUNTS-STRAND";
    pub const GENOTYPEGVCFS_NO_NEW_QUAL: &str = "SCI-GENOTYPEGVCFS-NO-NEW-QUAL";
    pub const HAPLOTYPECALLER_GVCF_ERC: &str = "SCI-HAPLOTYPECALLER-GVCF-ERC";
    pub const HISAT2_NO_SPLICE_SITES: &str = "SCI-HISAT2-NO-SPLICE-SITES";
    pub const HUMANN_NUCLEOTIDE_DB: &str = "SCI-HUMANN-NUCLEOTIDE-DB";
    pub const KALLISTO_SINGLE_END_FRAG: &str = "SCI-KALLISTO-SINGLE-END-FRAG";
    pub const KRAKEN2_CONFIDENCE: &str = "SCI-KRAKEN2-CONFIDENCE";
    pub const MACS2_BROAD_PEAKS: &str = "SCI-MACS2-BROAD-PEAKS";
    pub const MACS2_GENOME_SIZE: &str = "SCI-MACS2-GENOME-SIZE";
    pub const MACS2_PAIRED_END_FORMAT: &str = "SCI-MACS2-PAIRED-END-FORMAT";
    pub const MANTA_NO_CONFIG: &str = "SCI-MANTA-NO-CONFIG";
    pub const MARKDUPLICATES_REMOVE_DUPS: &str = "SCI-MARKDUPLICATES-REMOVE-DUPS";
    pub const METAPHLAN_UNKNOWN_ESTIMATION: &str = "SCI-METAPHLAN-UNKNOWN-ESTIMATION";
    pub const MINIMAP2_PRESET: &str = "SCI-MINIMAP2-PRESET";
    pub const MULTI_TENANT_PORT_COLLISION: &str = "SCI-MULTI-TENANT-PORT-COLLISION";
    pub const MUTECT2_NO_GERMLINE: &str = "SCI-MUTECT2-NO-GERMLINE";
    pub const MUTECT2_NO_PON: &str = "SCI-MUTECT2-NO-PON";
    pub const MUTECT2_TUMOR_ONLY: &str = "SCI-MUTECT2-TUMOR-ONLY";
    pub const PIPE_FAIL_DISABLED: &str = "SCI-PIPE-FAIL-DISABLED";
    pub const QIIME2_DENOISE_TRIM: &str = "SCI-QIIME2-DENOISE-TRIM";
    pub const RSEM_NO_STRAND: &str = "SCI-RSEM-NO-STRAND";
    pub const SALMON_LIBTYPE: &str = "SCI-SALMON-LIBTYPE";
    pub const SALMON_NO_GC_BIAS: &str = "SCI-SALMON-NO-GC-BIAS";
    pub const SAMTOOLS_INDEX_UNSORTED: &str = "SCI-SAMTOOLS-INDEX-UNSORTED";
    pub const SEURAT_NO_NORMALIZATION: &str = "SCI-SEURAT-NO-NORMALIZATION";
    pub const SORT_THREADS_MEM_PRODUCT: &str = "SCI-SORT-THREADS-MEM-PRODUCT";
    pub const STAR_GENOME_LOAD: &str = "SCI-STAR-GENOME-LOAD";
    pub const STAR_SJDB_OVERHANG: &str = "SCI-STAR-SJDB-OVERHANG";
    pub const STRELKA_NO_MANTA: &str = "SCI-STRELKA-NO-MANTA";
    pub const STRELKA_TARGETS_UNCOMPRESSED: &str = "SCI-STRELKA-TARGETS-UNCOMPRESSED";
    pub const STRINGTIE_NO_GUIDE: &str = "SCI-STRINGTIE-NO-GUIDE";
    pub const TMPDIR_NOT_SCRATCH: &str = "SCI-TMPDIR-NOT-SCRATCH";
    pub const VEP_OFFLINE_CACHE: &str = "SCI-VEP-OFFLINE-CACHE";
    pub const VQSR_COHORT: &str = "SCI-VQSR-COHORT";
    pub const ZERO_BYTE_COMPLETION: &str = "SCI-ZERO-BYTE-COMPLETION";
}

/// Curated enumeration of every SCI-* code the preflight engine can emit.
///
/// Authored in code order so the enumeration is deterministic without a sort.
/// Kept in lockstep with `scientific_preflight.rs` by the drift test at the
/// bottom of this file. Add a rule here (and to the checks) together.
pub const SCI_CATALOG: &[(&str, &str)] = &[
    (
        sci_codes::AGG_RACE,
        "Fan-out dimensions not keyed by declared outputs cause concurrent races.",
    ),
    (
        sci_codes::ATAC_SHIFT_COORDINATES,
        "ATAC-seq peak calling runs without Tn5 coordinate shifting.",
    ),
    (
        sci_codes::BEDTOOLS_GENOME_FILE,
        "bedtools slop/flank runs without chromosome lengths (-g).",
    ),
    (
        sci_codes::BISMARK_NON_DIRECTIONAL,
        "Bismark PBAT library alignment lacks --non_directional.",
    ),
    (
        sci_codes::BOWTIE_BISULFITE_MISMATCH,
        "Bisulfite alignment uses excessively strict --score-min L,0,0.",
    ),
    (
        sci_codes::BOWTIE2_NO_RG,
        "bowtie2 alignment is missing a read group (--rg-id / --rg).",
    ),
    (
        sci_codes::BQSR_NO_KNOWN_SITES,
        "BaseRecalibrator runs without --known-sites resources.",
    ),
    (
        sci_codes::BRACKEN_READ_LEN,
        "bracken abundance estimation lacks a read length (-r).",
    ),
    (
        sci_codes::BWA_MEM_NO_RG,
        "bwa mem is invoked without a read group (-R).",
    ),
    (
        sci_codes::CELLRANGER_INTRONS,
        "cellranger count on snRNA-seq lacks --include-introns.",
    ),
    (
        sci_codes::CHECKM_NO_LINEAGE,
        "checkm lineage_wf is missing the genome fasta extension (-x).",
    ),
    (
        sci_codes::CHIP_NO_CONTROL,
        "macs2 callpeak runs without a matched control/input (-c).",
    ),
    (
        sci_codes::DEEPVARIANT_MODEL_TYPE,
        "DeepVariant invocation lacks the required --model_type.",
    ),
    (
        sci_codes::DESEQ2_MIN_REPLICATES,
        "DESeq2 runs with too few biological replicates.",
    ),
    (
        sci_codes::FASTP_DEDUPLICATION,
        "fastp --dedup enabled without UMI tagging on RNA-seq.",
    ),
    (
        sci_codes::FEATURECOUNTS_MULTI_OVERLAP,
        "featureCounts runs with -O assigning reads to all overlaps.",
    ),
    (
        sci_codes::FEATURECOUNTS_STRAND,
        "featureCounts runs without an explicit strandness flag.",
    ),
    (
        sci_codes::GENOTYPEGVCFS_NO_NEW_QUAL,
        "GenotypeGVCFs runs without an explicit --new-qual model.",
    ),
    (
        sci_codes::HAPLOTYPECALLER_GVCF_ERC,
        "HaplotypeCaller targets a gVCF path without -ERC GVCF.",
    ),
    (
        sci_codes::HISAT2_NO_SPLICE_SITES,
        "hisat2 alignment runs without --known-splicesite-infile.",
    ),
    (
        sci_codes::HUMANN_NUCLEOTIDE_DB,
        "HUMAnN runs without an explicit --nucleotide-database.",
    ),
    (
        sci_codes::KALLISTO_SINGLE_END_FRAG,
        "kallisto single-end quant lacks a fragment length (-l).",
    ),
    (
        sci_codes::KRAKEN2_CONFIDENCE,
        "kraken2 runs with the default confidence threshold of 0.0.",
    ),
    (
        sci_codes::MACS2_BROAD_PEAKS,
        "macs2 callpeak targets broad histone marks without --broad.",
    ),
    (
        sci_codes::MACS2_GENOME_SIZE,
        "macs2 callpeak is missing explicit effective genome size (-g).",
    ),
    (
        sci_codes::MACS2_PAIRED_END_FORMAT,
        "macs2 specifies single-end -f BAM for a paired-end library.",
    ),
    (
        sci_codes::MANTA_NO_CONFIG,
        "Manta SV config lacks a reference genome FASTA specification.",
    ),
    (
        sci_codes::MARKDUPLICATES_REMOVE_DUPS,
        "MarkDuplicates configured with REMOVE_DUPLICATES=true.",
    ),
    (
        sci_codes::METAPHLAN_UNKNOWN_ESTIMATION,
        "MetaPhlAn --unknown_estimation lacks an explicit marker DB.",
    ),
    (
        sci_codes::MINIMAP2_PRESET,
        "minimap2 is invoked without an explicit preset (-x).",
    ),
    (
        sci_codes::MULTI_TENANT_PORT_COLLISION,
        "A server rule hardcodes a port, colliding across concurrent runs.",
    ),
    (
        sci_codes::MUTECT2_NO_GERMLINE,
        "Tumor-only Mutect2 is missing --germline-resource.",
    ),
    (
        sci_codes::MUTECT2_NO_PON,
        "Tumor-only Mutect2 is missing a Panel of Normals (PoN).",
    ),
    (
        sci_codes::MUTECT2_TUMOR_ONLY,
        "Mutect2 runs without a matched normal (tumor-only mode).",
    ),
    (
        sci_codes::PIPE_FAIL_DISABLED,
        "A shell pipeline uses '|' without 'set -o pipefail'.",
    ),
    (
        sci_codes::QIIME2_DENOISE_TRIM,
        "QIIME2 DADA2 denoising runs without 3' quality truncation.",
    ),
    (
        sci_codes::RSEM_NO_STRAND,
        "rsem-calculate-expression lacks --strand-specific configuration.",
    ),
    (
        sci_codes::SALMON_LIBTYPE,
        "salmon quant uses automatic library-type detection (-l A).",
    ),
    (
        sci_codes::SALMON_NO_GC_BIAS,
        "salmon quant runs without --gcBias correction.",
    ),
    (
        sci_codes::SAMTOOLS_INDEX_UNSORTED,
        "samtools index invoked on input that is not sorted BAM/CRAM.",
    ),
    (
        sci_codes::SEURAT_NO_NORMALIZATION,
        "Single-cell clustering runs without apparent prior normalization.",
    ),
    (
        sci_codes::SORT_THREADS_MEM_PRODUCT,
        "samtools sort threads times memory per thread exceeds budget.",
    ),
    (
        sci_codes::STAR_GENOME_LOAD,
        "STAR --genomeLoad LoadAndKeep is unsafe in batch/cluster environments.",
    ),
    (
        sci_codes::STAR_SJDB_OVERHANG,
        "STAR index build uses a GTF but omits --sjdbOverhang.",
    ),
    (
        sci_codes::STRELKA_NO_MANTA,
        "Strelka somatic run lacks Manta candidate indels.",
    ),
    (
        sci_codes::STRELKA_TARGETS_UNCOMPRESSED,
        "Strelka target BED is not bgzip-compressed and tabix-indexed.",
    ),
    (
        sci_codes::STRINGTIE_NO_GUIDE,
        "stringtie assembly runs without a reference annotation guide (-G).",
    ),
    (
        sci_codes::TMPDIR_NOT_SCRATCH,
        "A tool writes temp files to /tmp instead of engine scratch space.",
    ),
    (
        sci_codes::VEP_OFFLINE_CACHE,
        "VEP runs without local --cache or --offline flags.",
    ),
    (
        sci_codes::VQSR_COHORT,
        "VariantRecalibrator trained on too few samples for reliable VQSR.",
    ),
    (
        sci_codes::ZERO_BYTE_COMPLETION,
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

    /// The distinct `SCI-…` code literals declared in the `sci_codes` module
    /// of THIS file, parsed from the source at compile time so the set is
    /// never retyped in the test (the constants are the single source of the
    /// code strings for both the catalog and the preflight sites).
    fn declared_codes() -> BTreeSet<String> {
        let src = include_str!("scientific_catalog.rs");
        let body = src
            .split_once("pub mod sci_codes {")
            .expect("sci_codes module")
            .1
            .split_once('}')
            .expect("sci_codes module end")
            .0;
        let mut ids = BTreeSet::new();
        for part in body.split("pub const ").skip(1) {
            let value = part.split('"').nth(1).expect("const value");
            assert!(
                value.starts_with("SCI-"),
                "sci_codes const must hold a SCI- code, got {value:?}"
            );
            ids.insert(value.to_string());
        }
        ids
    }

    #[test]
    fn source_codes_match_catalog() {
        let declared = declared_codes();
        assert!(
            !declared.is_empty(),
            "failed to parse any SCI- code from sci_codes"
        );

        let catalog_ids: BTreeSet<String> = sci_catalog().iter().map(|r| r.code.clone()).collect();

        assert_eq!(
            catalog_ids, declared,
            "SCI_CATALOG must enumerate exactly the sci_codes constants"
        );
        assert_eq!(catalog_ids.len(), 51, "expected 51 curated SCI-* rules");
    }

    /// The preflight engine must emit codes ONLY through the shared
    /// `sci_codes` constants — no inline `code: "SCI-…"` literal may survive
    /// (that would re-introduce the retyped, unguarded duplication this
    /// module exists to remove). Falsifiable: add an inline literal at a new
    /// warning site and this fails.
    #[test]
    fn preflight_emits_codes_via_shared_constants_only() {
        let src = include_str!("scientific_preflight.rs");
        assert!(
            !src.contains("code: \"SCI-"),
            "scientific_preflight.rs must reference sci_codes::… , not inline SCI- literals"
        );
        let sites = src.matches("code: sci_codes::").count();
        assert_eq!(
            sites, 51,
            "every SCI- warning site must use a sci_codes const"
        );
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
