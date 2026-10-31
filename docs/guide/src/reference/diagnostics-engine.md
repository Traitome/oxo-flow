# Diagnostics Engine

> Deterministic failure analysis — no AI, pure pattern matching.
> Every diagnosis is reproducible given the same inputs.

## Overview

The Diagnostics Engine analyzes failed pipeline runs and returns:

- **Error pattern** identified (e.g., OOM, command not found, file missing)
- **Likely cause** with evidence
- **Fix suggestions** (auto-fixable or manual)
- **Relevant log lines** for context

It does NOT use AI. It matches error signatures (log keywords, exit codes)
against a curated library of 30 error patterns (tool, resource, data,
system, and config classes — see
`crates/oxo-flow-web/src/domains/execution/diagnostics.rs` for the full
list).

## Matching semantics

- `stderr_patterns` are **case-insensitive regular expressions**, compiled
  once per engine and shared process-wide. Entries must stay specific
  (loader lines, tool error prefixes) — near-miss words must not fire
  (issue #708).
- A pattern **without exit codes** fires on any non-zero or unknown exit
  code — never on exit 0. A rule that exited 0 has nothing to diagnose.
- The same pattern table backs `monitor_agent::is_known_error_pattern`, so
  the AI monitor and the diagnostics service never disagree on a
  classification.

## Error Pattern Library

30 patterns total across five categories — Tool (5), Resource (5), Data (7),
System (6), Config (7). The tables below cover the most common; the
authoritative list is
`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`.

### Tool Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Command not found | exit 127, "command not found" | ✅ Suggest `conda install` |
| Version incompatibility | "version mismatch/conflict", "incompatible … version", "unsupported version" | ❌ Suggest version switch |
| Tool crash (SIGSEGV) | exit 139, "segmentation fault" | ❌ Suggest tool update |
| Tool crash (SIGILL) | exit 132, "illegal instruction" | ❌ Check CPU compatibility |
| Tool crash (SIGBUS) | exit 138, "bus error" | ❌ Check filesystem integrity |

### Resource Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Out of memory | exit 137/9, "out of memory", "bad_alloc", `\boom\b` | ✅ Increase memory limit |
| Timeout | "timed out", exit 124 | ✅ Increase time_limit |
| Disk full | ENOSPC, "no space left on device" | ❌ Suggest cleanup |
| Too many open files | "too many open files", EMFILE, "ulimit" + failure word | ✅ Raise `ulimit -n` before starting |
| CPU limit exceeded | "cpu time limit", "cpu limit", EAGAIN | ✅ Increase thread/CPU allocation |

### Data Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Input file missing | "no such file or directory", "cannot open", "file not found", exit 1 | ❌ Point to missing path |
| File truncated | "truncated", "unexpected end", "premature" | ❌ Suggest re-download |
| Corrupt gzip file | "not in gzip format", "invalid compressed data", exit 1 | ❌ Verify file is valid gzip |
| FASTQ quality low | "per base sequence quality … fail/warn", "low quality", "poor quality" | ✅ Suggest fastp insertion |
| Empty file | "empty file", "zero length", "is empty" | ❌ Check upstream rule |
| BAM truncated | "truncated file", "EOF marker", "bgzf", exit 1 | ✅ Suggest `samtools index` |
| BAM index missing | "could not open/retrieve index file", "no index available", exit 1 | ✅ Suggest `samtools index` |

### System Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Permission denied | exit 126/13, "permission denied", EACCES | ❌ Fix file permissions |
| Network error | "connection refused", "could not resolve host", "network is unreachable" | ❌ Check network |
| Shared library missing | "error while loading shared libraries", "cannot open shared object", "undefined symbol" (exit 127/1) | ✅ Suggest conda install |
| Broken pipe (SIGPIPE) | exit 141/13, "broken pipe" | ❌ Check pipeline commands |
| Signal kill (SIGTERM) | exit 143/15, "terminated", `\bkilled\b` | ❌ Check system logs |
| Signal interrupt (SIGINT) | exit 130/2, "interrupt", "cancelled" | ❌ Re-run when ready |

### Config Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Invalid parameter | "invalid option", "unrecognized", "unknown option", exit 1 | ❌ Check tool documentation |
| Missing required parameter | "arguments are required", "missing required <arg>", "must specify", exit 1 | ❌ Add the missing parameter |
| Wildcard expanded empty | "no files/matches", "wildcard … did not match" | ❌ Check naming |
| Unbound wildcard in output | "contains unbound wildcard" (engine fails the rule before the shell runs) | ❌ Declare a value source (`sample_pattern`, `[[values]]`, `[[sample_groups]]`, `input_groups`) or remove the wildcard |
| Conda environment failed | "CondaError", "Solving environment: failed", "environment.yml", exit 1 | ❌ Verify conda env name |
| Docker failed | exit 125/126, "docker daemon", "error response from daemon" | ❌ Check Docker daemon |
| Singularity/Apptainer failed | exit 255, "singularity", "apptainer", "image not found" | ❌ Pull the image |
| Shell syntax error | exit 2, "syntax error", "unexpected token", "parse error" | ❌ Review the command |

## API

```
GET /api/runs/{run_id}/diagnostics

Response:
{
  failed_nodes: [{
    rule: "star_align",
    error_pattern: "oom_killed",
    likely_cause: "STAR alignment needs ~32GB; currently 16GB",
    suggestions: ["Increase memory to 32GB", "Use --limitBAMsortRAM"],
    relevant_log_lines: ["FATAL: out of memory", "EXITING: 137"]
  }],
  warnings: [{
    rule: "qualimap",
    pattern: "skipped",
    suggestion: "This rule was skipped due to upstream failure."
  }],
  resource_bottlenecks: []
}
```

`resource_bottlenecks` lists rules whose measured memory use pressed against
their declared limit (issue #67 §4):

```json
resource_bottlenecks: [{
  "rule": "markdup",
  "metric": "max_memory_mb",
  "actual": 31.0,
  "limit": 32.0
}]
```

- `actual` is the rule's sampled peak RSS in MiB (recorded by the local
  executor into the checkpoint's benchmark records) and `limit` is its
  declared memory limit (`memory` / `resources.memory`, resolved at
  execution time).
- A rule is flagged when `actual ≥ 80% × limit` — a conservative threshold
  because the peak is **sampled every 200 ms** across the rule's process
  subtree, not an exact `getrusage` maximum; sub-interval spikes can be
  missed. Cluster-executed rules carry the scheduler accounting store's
  peak RSS when it reports one (SLURM `sacct`'s max `MaxRSS` across task
  rows); the value is `None` only for schedulers without accounting
  (e.g. LSF) or a missing accounting row. Legacy checkpoints degrade to
  an empty list.

## Extending the Pattern Library

Add new patterns in `crates/oxo-flow-web/src/domains/execution/diagnostics.rs`
(inside `DiagnosticsEngine::new()`):

```rust
Pattern {
    id: "new_pattern_id",
    category: ErrorCategory::Tool,
    exit_codes: vec![134],
    // Case-insensitive regexes — keep them specific enough that benign
    // output cannot match ("tool version 1.2" must not fire).
    stderr_patterns: compile(&["specific error text"]),
    likely_cause: "Description of the likely cause",
    auto_fixable: false,
    fix_desc: Some("Suggested fix"),
    fix_config_path: None,
}
```

Every new pattern must ship with two test rows in the module's tests
(issue #708):

- a canonical trigger line in `CANONICAL_TRIGGERS` (proves the pattern is
  not dead — the test fails if a pattern has no row, or a row no pattern),
- and, when near-miss words exist, a benign line in `BENIGN_LOG_LINES`
  that mentions the vocabulary without being a real error.

## Scientific Preflight & Biological Contract Catalog

While runtime diagnostics analyze failed executions, the **Scientific Preflight Engine** (`crates/oxo-flow-core/src/scientific_preflight.rs`) and **Biological Semantic Subtyping System** (`crates/oxo-flow-core/src/types/`) inspect workflow DAGs *prior to execution*. They catch scientifically unsound configurations (e.g. training VQSR on single samples, unstranded quantification on stranded libraries, or uncompressed tabix inputs) before compute resources are wasted.

### Biological Subtyping Violations (`TYPE-*`)

The biological type system verifies formal format and coordination contracts across DAG edges.

| Diagnostic Code | Category | Biological Violation | Automated Coercion / Remediation |
|-----------------|----------|----------------------|-----------------------------------|
| `TYPE-001` | Format | Data format incompatibility across producer and consumer channels (e.g. SAM passed into BAM consumer) | Synthesizes conversion rules (e.g. `samtools view -b`) |
| `TYPE-002` | Sort Order | Sort order conflict (e.g. queryname-sorted BAM piped into coordinate-indexed consumer) | Inserts sort step (`samtools sort`) |
| `TYPE-003` | Index | Missing companion index artifact (e.g. BAM without `.bai`, VCF without `.tbi`) | Generates indexer rule (`samtools index`, `tabix -p vcf`) |
| `TYPE-004` | Assembly | Reference build conflict (e.g. GRCh38 BAM consumed by GRCh37 variant caller) | Re-aligns or re-indexes with matching reference genome |
| `TYPE-005` | Metadata | Missing read group `@RG` metadata fields required by downstream tools | Injects read group tags (`bwa mem -R '@RG...'`) |
| `TYPE-006` | Compression | Block compression stream mismatch (e.g. plain gzip passed where random-access BGZF is required) | Re-compresses stream (`bgzip`) |

### Curated Scientific Preflight Catalog (`SCI-*`)

Deterministic, evidence-backed rules covering genomics, transcriptomics, epigenomics, metagenomics, and DAG hardware execution.

#### 1. Germline & Somatic Variant Calling

- `SCI-VQSR-COHORT`: VariantRecalibrator runs on fewer than 30 samples; GATK recommends minimum ~30 samples for WGS/WES to train Gaussian mixture models. Use hard filtering (`VariantFiltration`) for small cohorts.
- `SCI-BQSR-NO-KNOWN-SITES`: BaseRecalibrator invoked without `--known-sites` resources (e.g. dbSNP, Mills); recalibration cannot model true variation without baseline polymorphic sites.
- `SCI-MUTECT2-TUMOR-ONLY`: Mutect2 executed without matched normal sample; somatic calls depend strictly on population allele frequencies and miss private germline variants and mapping artifacts.
- `SCI-MUTECT2-NO-PON`: Mutect2 in tumor-only mode missing Panel of Normals (`--panel-of-normals`); recurrent sequencing artifacts will be called as somatic mutations.
- `SCI-MUTECT2-NO-GERMLINE`: Mutect2 in tumor-only mode missing `--germline-resource`; population allele frequency priors cannot be applied to filter common germline leakage.
- `SCI-HAPLOTYPECALLER-GVCF-ERC`: HaplotypeCaller producing a `.g.vcf` path without `-ERC GVCF` or `--emit-ref-confidence GVCF`; emits flat VCF without reference confidence blocks.
- `SCI-GENOTYPEGVCFS-NO-NEW-QUAL`: GenotypeGVCFs invoked without explicit quality scoring flags; legacy Phred calculations inflate variant scores in large cohorts.
- `SCI-DEEPVARIANT-MODEL-TYPE`: DeepVariant executed without `--model_type`; CNN weights must match sequencing chemistry (`WGS`, `WES`, `PACBIO`, `ONT`).
- `SCI-MANTA-NO-CONFIG`: Manta structural variant caller configured without reference genome FASTA specification.
- `SCI-STRELKA-NO-MANTA`: Somatic Strelka execution omitting `--indelCandidates` from upstream Manta; small somatic indel sensitivity is substantially reduced.
- `SCI-STRELKA-TARGETS-UNCOMPRESSED`: Strelka target region BED is uncompressed; Strelka requires bgzip-compressed and tabix-indexed target regions (`.bed.gz`).
- `SCI-VEP-OFFLINE-CACHE`: Ensembl VEP running without local `--cache` or `--offline` flags; live REST queries break offline reproducibility and throttle batch jobs.

#### 2. Alignment & Coordinate Sorting

- `SCI-BWA-MEM-NO-RG`: `bwa mem` alignment lacks read group specification (`-R`); missing `@RG` headers prevent downstream GATK/Picard processing.
- `SCI-BOWTIE2-NO-RG`: `bowtie2` alignment lacks read group specification (`--rg-id` / `--rg`).
- `SCI-MINIMAP2-PRESET`: `minimap2` invoked without technology preset (`-x`); default parameters degrade alignment accuracy on PacBio, ONT, or short reads.
- `SCI-SAMTOOLS-SORT-MEM-OVERFLOW`: `samtools sort -m` per-thread memory multiplied by thread count exceeds rule allocated memory; leads to kernel OOM kills.
- `SCI-SAMTOOLS-INDEX-UNSORTED`: `samtools index` applied to SAM or unsorted BAM; requires coordinate-sorted BAM/CRAM.
- `SCI-MARKDUPLICATES-REMOVE-DUPS`: Picard/GATK `MarkDuplicates` configured with `REMOVE_DUPLICATES=true`; permanently discards reads needed for discordant pair and split-read CNV/SV analyses.
- `SCI-HISAT2-NO-SPLICE-SITES`: `hisat2` RNA-seq alignment executed without `--known-splicesite-infile`; reduces junction discovery sensitivity across annotated exons.
- `SCI-FASTP-DEDUPLICATION`: `fastp` PCR deduplication enabled without UMI barcodes on RNA-seq; removes real biological read depth for highly expressed transcripts.

#### 3. Bulk & Single-Cell RNA-seq

- `SCI-FEATURECOUNTS-STRAND`: `featureCounts` executed without explicit strandness flag (`-s`); defaults to unstranded (`-s 0`), miscounting stranded libraries.
- `SCI-FEATURECOUNTS-MULTI-OVERLAP`: `featureCounts` runs with `-O` (multi-overlapping); causes duplicate assignments for overlapping genes on opposite strands.
- `SCI-STAR-SJDB-OVERHANG`: `STAR` index generation with annotation omits `--sjdbOverhang`; default 100 degrades junction mapping for 150bp or 75bp reads.
- `SCI-STAR-GENOME-LOAD`: `STAR` uses `--genomeLoad LoadAndKeep` in shared batch clusters; shared memory segments risk node memory exhaustion and eviction.
- `SCI-SALMON-LIBTYPE`: `salmon quant` uses automatic library type detection (`-l A`); can select inconsistent strandedness models on shallow-coverage libraries.
- `SCI-SALMON-NO-GC-BIAS`: `salmon quant` executed without `--gcBias` correction on Illumina reads; fails to correct for sequence-specific coverage bias.
- `SCI-KALLISTO-SINGLE-END-FRAG`: `kallisto quant --single` executed without mean fragment length (`-l`) or standard deviation (`-s`).
- `SCI-RSEM-NO-STRAND`: `rsem-calculate-expression` lacks explicit `--strand-specific` setting.
- `SCI-STRINGTIE-NO-GUIDE`: `stringtie` novel assembly running without reference guide (`-G`); leads to fragmented transcript models.
- `SCI-DESEQ2-MIN-REPLICATES`: Differential expression analysis configured with fewer than 3 biological replicates; insufficient degrees of freedom for negative binomial dispersion estimation.
- `SCI-CELLRANGER-INTRONS`: `cellranger count` on single-nucleus RNA-seq (snRNA-seq) lacks `--include-introns`; pre-mRNA intronic reads account for up to 50% of nuclear signal.
- `SCI-SEURAT-NO-NORMALIZATION`: Single-cell clustering or dimensionality reduction executed on raw counts without prior `NormalizeData` or `SCTransform`.

#### 4. Epigenomics, ChIP-seq & ATAC-seq

- `SCI-MACS2-GENOME-SIZE`: `macs2 callpeak` missing explicit effective genome size (`-g`); default human (`hs`) invalidates peak p-values and FDR on other organisms.
- `SCI-MACS2-BROAD-PEAKS`: `macs2 callpeak` targeting broad marks (H3K27me3, H3K9me3) without `--broad`; fragments diffuse enrichment into artificial narrow peaks.
- `SCI-MACS2-PAIRED-END-FORMAT`: `macs2 callpeak` on paired-end data specifying `-f BAM` instead of `-f BAMPE`; discards exact paired fragment length modeling.
- `SCI-CHIP-NO-CONTROL`: `macs2 callpeak` running without matched input control (`-c`); causes elevated false-positive peak calling due to chromatin accessibility biases.
- `SCI-ATAC-SHIFT-COORDINATES`: ATAC-seq peak calling executed on unshifted BAMs; reads must be shifted (+4 bp on +, -5 bp on - strand) to center on Tn5 cut sites.
- `SCI-BISMARK-NON-DIRECTIONAL`: `bismark` alignment on non-directional bisulfite library (PBAT) lacks `--non_directional`; discards over 50% of sequencing reads.
- `SCI-BOWTIE-BISULFITE-MISMATCH`: Bisulfite alignment uses default strict mismatch scoring; bisulfite conversion artifacts require relaxed mismatch tolerance.
- `SCI-BEDTOOLS-GENOME-FILE`: `bedtools slop`/`flank` executed without chromosome length definitions (`-g`).

#### 5. Metagenomics & Microbiome

- `SCI-KRAKEN2-CONFIDENCE`: `kraken2` runs with default confidence threshold of 0.0; shared k-mers cause high false-positive genus and species classifications.
- `SCI-BRACKEN-READ-LEN`: `bracken` abundance re-estimation lacks read length parameter (`-r`) matching the database distribution.
- `SCI-QIIME2-DENOISE-TRIM`: QIIME2 DADA2 denoising configured without 3' quality truncation (`--p-trunc-len 0`); degraded read ends inflate error models and spurious ASVs.
- `SCI-HUMANN-NUCLEOTIDE-DB`: `humann` metabolic profiling executed without explicit ChocoPhlAn database path (`--nucleotide-database`).
- `SCI-METAPHLAN-UNKNOWN-ESTIMATION`: `metaphlan` with `--unknown_estimation` lacks explicit marker database index (`--bowtie2db`).
- `SCI-CHECKM-NO-LINEAGE`: `checkm lineage_wf` missing genome FASTA extension flag (`-x`).

#### 6. Workflow Concurrency, Hardware & Dataflow

- `SCI-AGG-RACE`: Rule fans out across samples, pairs, or values dimensions but declares instance-independent output paths; concurrent instances race and overwrite each other.
- `SCI-SORT-THREADS-MEM-PRODUCT`: Thread count multiplied by per-thread memory buffer exceeds rule resource budget.
- `SCI-TMPDIR-NOT-SCRATCH`: Heavy I/O tools write intermediate files to system `/tmp` instead of engine-managed scratch space (`${TMPDIR}`).
- `SCI-PIPE-FAIL-DISABLED`: Multi-stage shell pipelines (`cmd1 | cmd2 | cmd3`) lack `set -o pipefail`; upstream tool crashes are silently masked.
- `SCI-ZERO-BYTE-COMPLETION`: Rule masks failures by creating empty binary outputs (`|| touch {output}`), causing downstream tools to crash on corrupt streams.
- `SCI-MULTI-TENANT-PORT-COLLISION`: Interactive server rules specify hardcoded server ports (e.g. `--port 8888`), causing collision failures on multi-tenant nodes.
