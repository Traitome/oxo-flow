# Introduction

**oxo-flow** is a Rust-native bioinformatics pipeline engine designed to fully replace Snakemake. It compiles workflows into a Directed Acyclic Graph, manages software environments automatically, and runs jobs in parallel — all from a single, fast binary with no Python runtime required.

```bash
# Define your workflow in TOML
cat pipeline.oxoflow

# Execute it
oxo-flow run pipeline.oxoflow -j 8
```

```
oxo-flow 0.1.0 — Bioinformatics Pipeline Engine
DAG: 5 rules in execution order
  1. fastqc
  2. trim_reads
  3. bwa_align
  4. sort_bam
  5. call_variants
Done: 5 succeeded, 0 failed
```

---

## What Is oxo-flow?

oxo-flow is a high-performance workflow engine built from the ground up in Rust for bioinformatics and clinical genomics. Instead of writing Snakefiles in Python, you define pipelines in a clean TOML format (`.oxoflow` files), and oxo-flow handles dependency resolution, environment activation, parallel execution, and report generation.

### Core capabilities

| Capability | Description |
|---|---|
| **DAG engine** | Automatic dependency resolution, topological sorting, cycle detection, and parallel execution groups |
| **Environment management** | First-class support for conda, pixi, docker, singularity, and Python venv — per rule |
| **Clinical reporting** | Generate structured HTML and JSON reports with Tera templates for clinical and research use |
| **Web API** | Built-in REST API (axum-based) for building, validating, and monitoring workflows remotely |
| **Container packaging** | Package entire workflows into Docker or Singularity images for portable, reproducible execution |
| **Cluster backends** | Submit jobs to SLURM, PBS, SGE, and LSF clusters with resource-aware scheduling |
| **Wildcard expansion** | Snakemake-style `{sample}`, `{chr}` patterns that expand automatically from inputs or config |
| **Venus pipeline** | Built-in clinical tumor variant calling pipeline ready for somatic analysis |

---

## Who Is This For?

**Bioinformaticians** who build and maintain analysis pipelines — oxo-flow gives you a faster, type-safe alternative to Snakemake with better error messages and no Python dependency chain.

**Clinical laboratories** running accredited genomics workflows — the reporting system produces structured, auditable reports, and container packaging ensures reproducibility across environments.

**Researchers** who need reproducible science — every workflow execution is deterministic, and environments are locked per rule so results are the same on any machine.

**Core facility staff** managing multi-sample, multi-assay workloads — the DAG engine and cluster backends handle parallelism and resource scheduling automatically.

---

## How to Use This Guide

This documentation follows the [Diátaxis](https://diataxis.fr/) framework and is organized into four sections:

### If you are new to oxo-flow

Start with the **Tutorials** in order:

1. [Installation](./tutorials/installation.md) — install the binary
2. [Quick Start](./tutorials/quickstart.md) — run your first workflow in 5 minutes
3. [Your First Workflow](./tutorials/first-workflow.md) — build a pipeline from scratch
4. [Variant Calling Pipeline](./tutorials/variant-calling.md) — complete NGS analysis
5. [Environment Management](./tutorials/environment-management.md) — use conda, docker, and more

### If you need to accomplish a specific task

Jump to the **How-to Guides**:

- [Create a Workflow](./how-to/create-workflow.md)
- [Use Environments](./how-to/use-environments.md)
- [Run on a Cluster](./how-to/run-on-cluster.md)
- [Generate Reports](./how-to/generate-reports.md)

### If you need exact syntax and options

See the **Command Reference** for all 12 CLI subcommands with usage, options, and examples.

### If you want the full technical details

See **Architecture & Design** for in-depth documentation of the DAG engine, environment system, `.oxoflow` format specification, web API, and Venus pipeline.

---

## Quick Example

Here is a complete workflow that aligns paired-end reads and sorts the output:

```toml
# align.oxoflow
[workflow]
name = "align-and-sort"
version = "1.0.0"

[config]
reference = "/data/ref/hg38.fa"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "bwa_align"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
threads = 16
memory = "32G"
environment = { docker = "biocontainers/bwa:0.7.17" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"

[[rules]]
name = "index_bam"
input = ["aligned/{sample}.bam"]
output = ["aligned/{sample}.bam.bai"]
environment = { conda = "envs/samtools.yaml" }
shell = "samtools index {input}"
```

Run it:

```bash
# Validate the workflow
oxo-flow validate align.oxoflow

# Preview the execution plan
oxo-flow dry-run align.oxoflow

# Execute with 8 parallel jobs
oxo-flow run align.oxoflow -j 8

# Visualize the DAG
oxo-flow graph align.oxoflow | dot -Tpng -o dag.png
```

---

## Project Status

oxo-flow is under active development. The current release (`v0.1.0`) includes the complete core engine, CLI, web API, and Venus pipeline. See the [Changelog](./development/changelog.md) for release history and the [Contributing guide](./development/contributing.md) if you want to get involved.

---

## Join the Community

oxo-flow is a **community-driven, open-source project** licensed under Apache 2.0. Bug reports, feature requests, and contributions are welcome.

| How to contribute | Link |
|---|---|
| 🐛 Report a bug | [Bug report](https://github.com/Traitome/oxo-flow/issues/new) |
| 💡 Request a feature | [Feature request](https://github.com/Traitome/oxo-flow/issues/new) |
| 🤝 Contribute code | [Contributing guide](./development/contributing.md) |

> **Try it, break it, and tell us what happened.** Even a short comment about what worked — or didn't — helps improve oxo-flow for everyone.
