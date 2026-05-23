# oxo-flow template

Generate a workflow from a predefined gallery template.

## Usage

```
oxo-flow template [TEMPLATE] [-o <OUTPUT>]
```

## Description

Lists available templates when called without arguments. When a template name
is provided, generates a `.oxoflow` file based on that template, substituting
the workflow name appropriately.

Templates are drawn from the [Workflow Gallery](../gallery/index.md) and range
from a one-rule hello-world to production-grade multi-omics pipelines.

## Options

| Option | Description |
|--------|-------------|
| `-o, --output <OUTPUT>` | Output path (file or directory). Defaults to current directory with template name |

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

## See Also

- [Workflow Gallery](../gallery/index.md) — detailed explanations of each template
- [oxo-flow init](init.md) — scaffold a new project from scratch
