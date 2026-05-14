# Bioinformatics Expert Review: oxo-flow Pipeline Engine

**Reviewer**: Senior Bioinformatics Expert (15 years NGS experience)
**Date**: 2026-05-14
**Version Reviewed**: v0.3.1

---

## Executive Summary

oxo-flow demonstrates strong architectural foundations for a Rust-native bioinformatics pipeline engine. The TOML-based workflow format is intuitive, environment management is comprehensive, and the DAG engine is well-designed. However, several critical gaps exist for production bioinformatics use, particularly around wildcard handling for complex sample relationships (tumor-normal pairs, multi-batch experiments) and lack of single-cell workflow support.

**Overall Assessment**: Ready for basic WGS/RNA-seq pipelines, but needs enhancements for production-grade clinical and multi-modal workflows.

---

## Detailed Findings

### 1. Workflow Format (.oxoflow TOML)

#### Strengths

| Aspect | Assessment |
|--------|------------|
| TOML syntax | **EXCELLENT** - Human-readable, easy to edit, no YAML indentation issues |
| Section structure | **GOOD** - `[workflow]`, `[config]`, `[defaults]`, `[[rules]]` hierarchy is intuitive |
| Shell command embedding | **EXCELLENT** - Triple-quoted multi-line strings with proper escaping |
| Config variable interpolation | **GOOD** - `{config.reference}` syntax works well |

#### Issues

| ID | Severity | Issue | Recommendation |
|----|----------|-------|----------------|
| WF-01 | ~~**HIGH**~~ ✅ **DONE** | No conditional execution (if/else) | ~~Add `when = "{config.mode} == 'tumor'"` syntax for conditional rules~~ **Implemented**: `when` field with full expression support |
| WF-02 | **HIGH** | No parameter sweep support | Add `params = [{quality: 20}, {quality: 30}]` for parameter optimization workflows |
| WF-03 | **MEDIUM** | No subworkflow imports | Add `import = "modules/qc.oxoflow"` for modular pipeline composition |
| WF-04 | **MEDIUM** | No workflow inheritance | Consider `[workflow.extends = "base-pipeline.oxoflow"]` for pipeline templates |
| WF-05 | **LOW** | Version pinning not enforced | Add schema validation for `version` field format (semver) |

#### Example Problem

Current format cannot express conditional execution common in bioinformatics:

```toml
# Current: Must define separate rules for each mode
[[rules]]
name = "fastp_tumor"
input = ["raw/TUMOR_01_R1.fq.gz", "raw/TUMOR_01_R2.fq.gz"]
...

[[rules]]
name = "fastp_normal"
input = ["raw/NORMAL_01_R1.fq.gz", "raw/NORMAL_01_R2.fq.gz"]
...

# Desired: Conditional rule with sample type filtering
[[rules]]
name = "fastp"
input = ["raw/{sample}_R{read}.fq.gz"]
when = "{sample.type} == 'tumor' or {sample.type} == 'normal'"
...
```

---

### 2. Wildcard Expansion for Sample Batches

#### Strengths

| Aspect | Assessment |
|--------|------------|
| Basic `{sample}` expansion | **GOOD** - Pattern-to-regex conversion works |
| Multiple wildcards | **GOOD** - `{sample}_R{read}` correctly expands |
| File discovery | **GOOD** - `discover_wildcards_from_pattern()` scans directories |
| Cartesian product | **GOOD** - All combinations generated correctly |

#### Issues

| ID | Severity | Issue | Recommendation |
|----|----------|-------|----------------|
| WC-01 | ~~**CRITICAL**~~ ✅ **DONE** | No tumor-normal paired sample linking | ~~Cannot express `{tumor}`/`{normal}` relationships from sample sheet~~ **Implemented**: `[[pairs]]` section with auto-expansion |
| WC-02 | ~~**CRITICAL**~~ ✅ **DONE** | No multi-batch/time-series grouping | ~~No support for `{batch}` or `{timepoint}` with cross-batch aggregation~~ **Implemented**: `[[sample_groups]]` with `{group}`/`{sample}` expansion |
| WC-03 | **HIGH** | Wildcard constraints not exposed in TOML | `WildcardConstraints` exists in Rust but not in workflow syntax |
| WC-04 | **HIGH** | No wildcard functions | Cannot do `{sample|upper}` or `{chr|replace("chr","")}` transformations |
| WC-05 | **MEDIUM** | No ordered wildcard expansion | Cannot guarantee sample processing order (important for time-series) |
| WC-06 | **MEDIUM** | Paired-end file discovery limited | `paired_end_pattern()` only checks common suffixes, misses `_R{read}_001` |

#### Critical Example: Tumor-Normal Pairing

The `paired_tumor_normal.oxoflow` example demonstrates the problem:

```toml
# Current approach: Hardcoded sample names
[[rules]]
name = "mutect2"
input = ["recal/TUMOR_01.recal.bam", "recal/NORMAL_01.recal.bam"]
output = ["variants/TUMOR_01.mutect2.vcf.gz"]
shell = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal NORMAL_01 ..."
```

This requires:
- Separate rules for each tumor-normal pair
- Manual duplication of sample names
- No sample sheet-driven expansion

**Recommended Solution**: Add paired sample groups to config:

```toml
[config.samples]
pairs = [
  { tumor: "TUMOR_01", normal: "NORMAL_01" },
  { tumor: "TUMOR_02", normal: "NORMAL_02" }
]

[[rules]]
name = "mutect2"
input = ["recal/{tumor}.recal.bam", "recal/{normal}.recal.bam"]
output = ["variants/{tumor}.mutect2.vcf.gz"]
shell = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal {normal} ..."
```

#### Critical Example: Per-Chromosome Scatter

WGS pipeline should support per-chromosome processing:

```toml
# Current: No chromosome-based scatter
# Desired:
[[rules]]
name = "haplotype_caller"
input = ["bqsr/{sample}.recal.bam"]
output = ["variants/{sample}.{chr}.g.vcf.gz"]
scatter = { chr = ["chr1", "chr2", ..., "chrX", "chrY", "chrM"] }
shell = "gatk HaplotypeCaller -L {chr} ..."
```

---

### 3. Environment Management

#### Strengths

| Aspect | Assessment |
|--------|------------|
| Backend diversity | **EXCELLENT** - Conda, Docker, Singularity, Pixi, Venv all supported |
| Per-rule isolation | **EXCELLENT** - Each rule can have its own environment |
| Mixed environments | **EXCELLENT** - Can mix conda/Docker/Singularity in one pipeline |
| Container image caching | **GOOD** - Images pulled once and reused |
| HPC compatibility | **GOOD** - Singularity for rootless environments |

#### Issues

| ID | Severity | Issue | Recommendation |
|----|----------|-------|----------------|
| ENV-01 | **HIGH** | No environment module system support | Add `modules = ["bioinfo/bwa", "bioinfo/samtools"]` for HPC environments |
| ENV-02 | **HIGH** | No conda environment lockfile generation | Auto-generate conda-lock files for reproducibility |
| ENV-03 | **MEDIUM** | No environment inheritance | Rules cannot inherit from `[defaults.environment]` with modifications |
| ENV-04 | **MEDIUM** | No environment validation before run | `--skip-env-setup` should still validate env exists |
| ENV-05 | **LOW** | No environment rebuild option | Add `--rebuild-env` flag to recreate environments |

#### Production Concern

Bioinformatics cores need environment module support for HPC:

```toml
# Current: Only container/conda options
[rules.environment]
conda = "envs/bwa.yaml"

# Needed: HPC module system
[rules.environment]
modules = ["bioinfo/bwa-mem2/2.2.1", "bioinfo/samtools/1.20"]
```

---

### 4. Resource Declarations

#### Strengths

| Aspect | Assessment |
|--------|------------|
| Thread declaration | **EXCELLENT** - `{threads}` placeholder in shell commands |
| Memory declaration | **GOOD** - Supports G/M/K suffixes |
| Resource defaults | **EXCELLENT** - `[defaults]` section for fallback values |
| Extended resources | **GOOD** - GPU, disk, time_limit in `[rules.resources]` |
| Resource-aware scheduling | **GOOD** - Prevents over-subscription |

#### Issues

| ID | Severity | Issue | Recommendation |
|----|----------|-------|----------------|
| RES-01 | **HIGH** | Memory not enforced in containers | Docker/Singularity should use `--memory` limits |
| RES-02 | **HIGH** | No per-tool memory profile hints | Add `memory_hint = "genome_size * 10"` for tools like STAR |
| RES-03 | **MEDIUM** | No resource scaling by input size | Cannot scale threads/memory by FASTQ size |
| RES-04 | **MEDIUM** | No queue-specific resource mapping | SLURM/PBS resources need different field names |
| RES-05 | **LOW** | GPU model specification incomplete | `GpuSpec` has model/memory but no driver version |

#### Tool-Specific Resource Concerns

| Tool | Recommended Resources | Current Example | Issue |
|------|----------------------|-----------------|-------|
| STAR | 32-64G for human genome | 32G (correct) | Needs genome size scaling |
| BWA-MEM2 | 8-16G + reference cache | 32G (oversized) | Memory should scale by batch size |
| GATK HaplotypeCaller | 4-8G per interval | 16G (oversized) | Should be smaller per-chromosome |
| fastp | 2-4G + threading | 8G (reasonable) | OK |
| VEP | 4-8G + cache location | 8G (OK) | Cache path should be configurable |

---

### 5. Gallery Workflow Review

#### Workflow Coverage Assessment

| Workflow | Real-World Relevance | Completeness | Production Ready? |
|----------|---------------------|--------------|-------------------|
| 01_hello_world | LOW | Complete | Yes (demo) |
| 02_file_pipeline | LOW | Complete | Yes (demo) |
| 03_parallel_samples | HIGH | Partial | No - missing sample sheet input |
| 04_scatter_gather | HIGH | Partial | No - missing per-chromosome example |
| 05_conda_environments | HIGH | Complete | Yes |
| 06_rnaseq_quantification | HIGH | Partial | No - missing differential expression |
| 07_wgs_germline | HIGH | Partial | No - missing joint genotyping |
| 08_multiomics_integration | MEDIUM | Partial | No - placeholder shell commands |
| paired_tumor_normal | HIGH | Partial | No - hardcoded sample names |

#### Missing Gallery Workflows

| Priority | Workflow Type | Use Case |
|----------|--------------|----------|
| **CRITICAL** | Single-cell RNA-seq | Cellranger/Seurat pipelines (ubiquitous in research) |
| **CRITICAL** | Tumor-normal somatic | Mutect2 with sample sheet (clinical standard) |
| **HIGH** | ChIP-seq | Peak calling with MACS2 |
| **HIGH** | ATAC-seq | Chromatin accessibility analysis |
| **HIGH** | 16S microbiome | QIIME2/dada2 pipelines |
| **MEDIUM** | Single-cell ATAC | scATAC-seq with Signac |
| **MEDIUM** | Spatial transcriptomics | Visium/Spatial data processing |
| **MEDIUM** | Long-read sequencing | PacBio/Oxford Nanopore workflows |
| **LOW** | Proteomics | Mass spectrometry data processing |

#### Gallery Workflow Issues

| ID | Workflow | Severity | Issue |
|----|----------|----------|-------|
| GAL-01 | rnaseq | **HIGH** | No differential expression (DESeq2/edgeR) - stops at counts |
| GAL-02 | wgs_germline | **HIGH** | No joint genotyping (GenotypeGVCFs needs multiple samples) |
| GAL-03 | wgs_germline | **HIGH** | No VQSR - only hard filters (not clinical-grade) |
| GAL-04 | multiomics | **MEDIUM** | Shell commands are placeholders, not real integration |
| GAL-05 | paired_tumor_normal | **CRITICAL** | Hardcoded sample names, not production-useful |
| GAL-06 | scatter_gather | **MEDIUM** | Uses synthetic data, not chromosome-based splitting |

---

## Critical Path Items for Production Bioinformatics

### Must Fix Before Clinical Use (CRITICAL)

1. **WC-01: Tumor-Normal Sample Pairing**
   - Clinical pipelines require sample sheet-driven tumor-normal matching
   - Current approach requires manual rule duplication per patient
   - **Impact**: Cannot scale to cohort studies (>10 patients)

2. **WC-02: Multi-Batch/Group Processing**
   - Time-series experiments (drug response) need batch grouping
   - Multi-platform studies need sample group tracking
   - **Impact**: Cannot run longitudinal studies

3. **GAL-05: Paired Workflow Not Production-Ready**
   - The example workflow demonstrates the pairing gap
   - Need working sample sheet parsing

### Should Fix Soon (HIGH)

4. **WF-01: Conditional Execution**
   - Essential for pipeline modes (tumor-only, paired, germline)
   - Current: Must maintain separate workflow files

5. **RES-01: Container Memory Limits**
   - Without enforcement, pipelines can crash the host
   - Docker: `--memory {memory}`
   - Singularity: `--memory-limit`

6. **ENV-01: HPC Module System**
   - Many cores use Lmod/environment modules
   - Need native support for `module load` integration

### Recommended Enhancements (MEDIUM)

7. **WF-03: Subworkflow Imports**
   - Complex pipelines benefit from modularity
   - Example: Import `modules/qc.oxoflow` into main workflow

8. **WC-03: Wildcard Constraints in TOML**
   - Validation like `sample = "[A-Za-z0-9_]+"` prevents malformed inputs
   - Rust implementation exists, just needs TOML exposure

---

## Comparison with Existing Pipeline Frameworks

| Feature | oxo-flow | Snakemake | Nextflow | WDL |
|---------|----------|-----------|----------|-----|
| Syntax simplicity | **Better** (TOML) | Good (Python) | Medium (DSL2) | Medium (custom) |
| Tumor-normal support | **Missing** | Excellent | Excellent | Excellent |
| Single-cell examples | **Missing** | Good | Excellent | Good |
| Environment isolation | **Excellent** | Good | Excellent | Good |
| HPC module support | **Missing** | Good | Excellent | Medium |
| Conditional execution | **Missing** | Excellent | Excellent | Excellent |
| Per-chromosome scatter | **Missing** | Excellent | Excellent | Excellent |
| Reproducibility | **Good** | Good | Excellent | Medium |
| Language performance | **Excellent** (Rust) | Medium (Python) | Good (Groovy) | Medium (Java) |
| Clinical-grade reporting | **Good** | Medium | Good | Medium |

---

## Recommendations Summary

### Immediate Priorities (Next 2 Weeks)

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| P0 | Tumor-normal sample sheet parsing | Medium | Enables clinical pipelines |
| P0 | Sample group/tuple wildcards | Medium | Enables cohort studies |
| P1 | Container memory enforcement | Low | Prevents crashes |
| P1 | Conditional rule execution | Medium | Reduces workflow duplication |

### Short-term (Next 1 Month)

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| P2 | HPC module system support | Medium | HPC core adoption |
| P2 | Single-cell RNA-seq gallery workflow | Medium | Research user adoption |
| P2 | Per-chromosome scatter example | Low | WGS scalability |
| P2 | Joint genotyping in WGS workflow | Medium | Clinical completeness |

### Medium-term (Next 3 Months)

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| P3 | Subworkflow imports | High | Modularity |
| P3 | Wildcard functions/transforms | Medium | Data normalization |
| P3 | Resource scaling by input size | Medium | Cost optimization |
| P3 | Differential expression in RNA-seq | Medium | Research completeness |

---

## Appendix: Sample Sheet Format Recommendations

### Proposed Format for Complex Sample Relationships

```csv
# samples.csv - Tumor-Normal Paired Study
sample_id,type,pair_id,batch,platform
TUMOR_01,tumor,PAIR_01,BATCH_A,Illumina
NORMAL_01,normal,PAIR_01,BATCH_A,Illumina
TUMOR_02,tumor,PAIR_02,BATCH_A,Illumina
NORMAL_02,normal,PAIR_02,BATCH_A,Illumina
TUMOR_03,tumor,PAIR_03,BATCH_B,Illumina
NORMAL_03,normal,PAIR_03,BATCH_B,Illumina
```

```toml
[config]
samples = "samples.csv"

# oxo-flow should parse and create wildcard groups:
# {sample} expands to all samples
# {tumor}/{normal} expands to paired tumor-normal
# {batch} expands to batch groups
# {sample.type} provides conditional filtering
```

---

## Conclusion

oxo-flow has excellent foundations for a Rust-native pipeline engine. The TOML format is cleaner than Snakemake's Python or Nextflow's DSL2. Environment management is comprehensive. However, critical gaps in sample relationship handling prevent clinical-scale tumor-normal workflows. Adding sample sheet parsing with paired/group wildcards would immediately unlock production use cases.

**Recommendation**: Focus P0 items (tumor-normal pairing, sample groups) before promoting for production bioinformatics use. The architecture supports these additions cleanly via the existing wildcard engine.

---

*Review completed 2026-05-14*
*Reviewer: Senior Bioinformatics Expert*