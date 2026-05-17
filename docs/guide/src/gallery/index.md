# Workflow Gallery

The Workflow Gallery is a curated collection of complete, validated example workflows that progress from fundamental concepts to production-grade bioinformatics pipelines. Every workflow in this gallery passes `oxo-flow validate` and demonstrates real CLI output.

Use this gallery to learn oxo-flow incrementally — each example builds on concepts from the previous one.

---

## Learning Path

| # | Workflow | Complexity | Concepts Covered |
|---|----------|-----------|------------------|
| 01 | [Hello World](hello-world.md) | ⭐ | Minimal rule, shell commands, output files |
| 02 | [File Pipeline](file-pipeline.md) | ⭐⭐ | Multi-rule DAG, input/output chaining, config variables |
| 03 | [Parallel Samples](parallel-samples.md) | ⭐⭐ | Wildcard expansion, fan-out/fan-in, resource declarations |
| 04 | [Scatter-Gather](scatter-gather.md) | ⭐⭐⭐ | Data partitioning, parallel chunk processing, result merging |
| 05 | [Environment Management](environment-mgmt.md) | ⭐⭐⭐ | Per-rule conda, docker, and singularity environments |
| 06 | [RNA-seq Quantification](rnaseq.md) | ⭐⭐⭐⭐ | Complete transcriptomics pipeline, STAR, featureCounts, MultiQC |
| 07 | [WGS Germline Calling](wgs-germline.md) | ⭐⭐⭐⭐⭐ | GATK best practices, BQSR, HaplotypeCaller, VEP annotation |
| 08 | [Multi-Omics Integration](multiomics.md) | ⭐⭐⭐⭐⭐ | WGS + RNA-seq + Methylation, branching DAG, cross-omics integration |
| 09 | [Single-Cell RNA-seq](single-cell-rnaseq.md) | ⭐⭐⭐⭐ | Droplet-based scRNA-seq, 10x Genomics, CellRanger, Seurat |

---

## Quick Start

Every gallery workflow can be validated, inspected, and dry-run using the oxo-flow CLI:

```bash
# Validate a workflow
oxo-flow validate examples/gallery/01_hello_world.oxoflow

# Preview the execution plan
oxo-flow dry-run examples/gallery/02_file_pipeline.oxoflow

# Visualize the DAG
oxo-flow graph examples/gallery/06_rnaseq_quantification.oxoflow

# Lint for best practices
oxo-flow lint examples/gallery/07_wgs_germline.oxoflow
```

---

## Skill Progression

### Beginner (Workflows 01–02)
Learn the fundamental building blocks: rules, shell commands, inputs, outputs, and how oxo-flow resolves dependencies automatically from file paths.

### Intermediate (Workflows 03–05)
Master wildcards for multi-sample processing, scatter-gather parallelism patterns, and per-rule environment isolation with conda, docker, and singularity.

### Advanced (Workflows 06–09)
Build production-grade bioinformatics pipelines covering RNA-seq, whole-genome sequencing, multi-omics integration, and single-cell analysis with clinical-grade reporting and complex DAG topologies.

---

## All Workflows Are Tested

Every workflow in this gallery is validated as part of oxo-flow's continuous integration pipeline. The validation output shown in each page is the actual CLI output — not simulated.

```
$ oxo-flow validate examples/gallery/01_hello_world.oxoflow
✓ examples/gallery/01_hello_world.oxoflow — 1 rules, 0 dependencies

$ oxo-flow validate examples/gallery/08_multiomics_integration.oxoflow
✓ examples/gallery/08_multiomics_integration.oxoflow — 8 rules, 7 dependencies
```
