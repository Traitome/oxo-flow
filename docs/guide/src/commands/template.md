# oxo-flow template

Generate a workflow from a predefined gallery template.

## Usage

```
oxo-flow template [OPTIONS] [TEMPLATE]
```

## Description

Lists available templates when called without arguments. When a template name
is provided, generates a `.oxoflow` file based on that template, substituting
the workflow name appropriately.

Templates are drawn from the [Workflow Gallery](../gallery/index.md) and range
from a one-rule hello-world to production-grade multi-omics pipelines. The
gallery is embedded in the binary at build time, so it works identically
whether oxo-flow is installed from a release or run from a source checkout.

Templates that reference auxiliary files (scripts and report templates —
`09_single_cell_rnaseq`, `11_conditional_workflow`,
`14_paired_experiment_control`, `15_paired_experiment_control_pairs`) copy
them next to the generated workflow, so the generated pipeline is
immediately complete and runnable.

## Options

| Option | Description |
|--------|-------------|
| `-o, --output <OUTPUT>` | Output file or directory (a trailing slash forces a directory, created if missing). Defaults to current directory with template name |
| `--ai` | Generate the workflow with AI from a natural language description |
| `--from-url <URL>` | URL(s) to use as reference material for AI generation (repeatable) |
| `--from-file <PATH>` | File(s) to use as reference material for AI generation (repeatable) |
| `--ai-max-retries <N>` | Maximum AI correction rounds (overrides config) |

## Examples

```bash
# List all available templates
oxo-flow template

# Generate the hello-world template in the current directory
oxo-flow template 01_hello_world

# Generate to a specific file
oxo-flow template 06_rnaseq_quantification -o my_rnaseq.oxoflow

# Generate into a specific directory  
oxo-flow template 07_wgs_germline -o projects/wgs/
```

## Available Templates

| Name | Description |
|------|-------------|
| `01_hello_world` | Minimal single-rule workflow |
| `02_file_pipeline` | Linear three-step file processing |
| `03_parallel_samples` | Parallel sample processing with wildcards |
| `04_scatter_gather` | Chromosome-based scatter-gather pattern |
| `05_conda_environments` | Multi-environment workflow |
| `06_rnaseq_quantification` | RNA-seq quantification pipeline |
| `07_wgs_germline` | WGS germline variant calling |
| `08_multiomics_integration` | Multi-omics integration |
| `09_single_cell_rnaseq` | Single-cell RNA-seq processing |
| `10_transform_operator` | Transform operator demo |
| `11_conditional_workflow` | Conditional execution |
| `12_cohort_analysis` | Cohort-level QC aggregation |
| `13_simple_variant_calling` | Simple germline variant calling |
| `14_paired_experiment_control` | Somatic variant calling (single pair) |
| `15_paired_experiment_control_pairs` | Somatic variant calling (multiple pairs) |
| `16_16s_qiime2_amplicon` | 16S amplicon analysis with QIIME2 |

## See Also

- [Workflow Gallery](../gallery/index.md) — detailed explanations of each template
- [oxo-flow init](init.md) — scaffold a new project from scratch
