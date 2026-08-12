# Your First Workflow

This tutorial walks you through building a realistic bioinformatics workflow from scratch. You will create a quality-control pipeline that processes FASTQ files through FastQC and fastp, then generates a summary report.

---

## Prerequisites

- [oxo-flow installed](./installation.md)
- Paired-end FASTQ files (or the willingness to create test files)
- **conda** or **mamba** available for environment management. 
    - *If you don't have either, we recommend [Miniforge](https://github.com/conda-forge/miniforge#miniforge3) or [Mambaforge](https://github.com/conda-forge/miniforge#mambaforge).*

---

## 1. Set up the project

```bash
oxo-flow init qc-pipeline
cd qc-pipeline
```

---

## 2. Create environment files

Create a conda environment file for the QC tools:

```yaml
# envs/qc.yaml
name: qc
channels:
  - bioconda
  - conda-forge
dependencies:
  - fastqc=0.12.1
  - fastp=0.23.4
  - multiqc=1.21
```

---

## 3. Write the workflow

Replace `qc-pipeline.oxoflow` with:

!!! tip "Configuration Syntax"
    `{config.samples_dir}` refers to the `samples_dir` variable defined in the `[config]` section. This allows you to centralize paths and settings.

```toml
[workflow]
name = "qc-pipeline"
version = "1.0.0"
description = "Quality control for paired-end sequencing data"
author = "Your Name"

[config]
samples_dir = "raw_data"
results_dir = "results"

[defaults]
threads = 4
memory = "8G"

!!! info "Wildcard Patterns"
    The `{sample}` in the file paths below is a **wildcard**. oxo-flow will scan your `raw_data` directory for files matching the pattern `{sample}_R1.fastq.gz`, extract the sample name, and automatically generate a task for every sample it finds.

[[rules]]
name = "fastqc_raw"
input = [
    "{config.samples_dir}/{sample}_R1.fastq.gz",
    "{config.samples_dir}/{sample}_R2.fastq.gz"
]
output = [
    "{config.results_dir}/fastqc/{sample}_R1_fastqc.html",
    "{config.results_dir}/fastqc/{sample}_R1_fastqc.zip",
    "{config.results_dir}/fastqc/{sample}_R2_fastqc.html",
    "{config.results_dir}/fastqc/{sample}_R2_fastqc.zip"
]
environment = { conda = "envs/qc.yaml" }
shell = """
mkdir -p {config.results_dir}/fastqc
fastqc {input} -o {config.results_dir}/fastqc -t {threads}
"""

[[rules]]
name = "fastp_trim"
input = [
    "{config.samples_dir}/{sample}_R1.fastq.gz",
    "{config.samples_dir}/{sample}_R2.fastq.gz"
]
output = [
    "{config.results_dir}/trimmed/{sample}_R1.fastq.gz",
    "{config.results_dir}/trimmed/{sample}_R2.fastq.gz",
    "{config.results_dir}/trimmed/{sample}_fastp.html",
    "{config.results_dir}/trimmed/{sample}_fastp.json"
]
environment = { conda = "envs/qc.yaml" }
shell = """
mkdir -p {config.results_dir}/trimmed
fastp \
  --in1 {config.samples_dir}/{sample}_R1.fastq.gz \
  --in2 {config.samples_dir}/{sample}_R2.fastq.gz \
  --out1 {config.results_dir}/trimmed/{sample}_R1.fastq.gz \
  --out2 {config.results_dir}/trimmed/{sample}_R2.fastq.gz \
  --html {config.results_dir}/trimmed/{sample}_fastp.html \
  --json {config.results_dir}/trimmed/{sample}_fastp.json \
  --thread {threads}
"""

[[rules]]
name = "fastqc_trimmed"
input = [
    "{config.results_dir}/trimmed/{sample}_R1.fastq.gz",
    "{config.results_dir}/trimmed/{sample}_R2.fastq.gz"
]
output = [
    "{config.results_dir}/fastqc_trimmed/{sample}_R1_fastqc.html",
    "{config.results_dir}/fastqc_trimmed/{sample}_R1_fastqc.zip"
]
environment = { conda = "envs/qc.yaml" }
shell = """
mkdir -p {config.results_dir}/fastqc_trimmed
fastqc {input} -o {config.results_dir}/fastqc_trimmed -t {threads}
"""

[[rules]]
name = "multiqc"
input = [
    "{config.results_dir}/fastqc/",
    "{config.results_dir}/fastqc_trimmed/",
    "{config.results_dir}/trimmed/"
]
output = [
    "{config.results_dir}/multiqc/multiqc_report.html"
]
environment = { conda = "envs/qc.yaml" }
shell = """
mkdir -p {config.results_dir}/multiqc
multiqc {config.results_dir} -o {config.results_dir}/multiqc --force
"""

[rules.resources]
threads = 1
```

---

## 4. Understand the dependency graph

The workflow forms this DAG:

```mermaid
graph TD
    A[fastqc_raw] --> D[multiqc]
    B[fastp_trim] --> C[fastqc_trimmed]
    B --> D
    C --> D
```

- `fastqc_raw` and `fastp_trim` can run in parallel (no dependency between them)
- `fastqc_trimmed` depends on `fastp_trim`'s output
- `multiqc` depends on all three upstream rules

---

## 5. Prepare Test Data

For this tutorial, create minimal test files so oxo-flow has something to process:

```bash
mkdir -p raw_data
# Create dummy compressed fastq files
echo "@test1" | gzip > raw_data/sample1_R1.fastq.gz
echo "@test1" | gzip > raw_data/sample1_R2.fastq.gz
echo "@test2" | gzip > raw_data/sample2_R1.fastq.gz
echo "@test2" | gzip > raw_data/sample2_R2.fastq.gz
```

---

## 6. Validate and preview

```bash
oxo-flow validate qc-pipeline.oxoflow
# ✓ qc-pipeline.oxoflow — 4 rules, 2 dependencies

oxo-flow dry-run qc-pipeline.oxoflow
```

```
oxo-flow 0.10.1 — Bioinformatics Pipeline Engine
DAG: (dry-run) 4 rules would execute
  1. multiqc
     threads=1
     env=conda
     memory=8G
     outputs: ["results/multiqc/multiqc_report.html"]
     command: mkdir -p results/multiqc
multiqc results -o results/multiqc --force

  2. fastp_trim
     threads=4
     env=conda
     memory=8G
     outputs: ["results/trimmed/{sample}_R1.fastq.gz", "results/trimmed/{sample}_R2.fastq.gz", "results/trimmed/{sample}_fastp.html", "results/trimmed/{sample}_fastp.json"]
     command: mkdir -p results/trimmed
fastp --in1 raw_data/{sample}_R1.fastq.gz --in2 raw_data/{sample}_R2.fastq.gz --out1 results/trimmed/{sample}_R1.fastq.gz --out2 results/trimmed/{sample}_R2.fastq.gz --html results/trimmed/{sample}_fastp.html --json results/trimmed/{sample}_fastp.json --thread 4

  3. fastqc_trimmed
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc_trimmed/{sample}_R1_fastqc.html", "results/fastqc_trimmed/{sample}_R1_fastqc.zip"]
     command: mkdir -p results/fastqc_trimmed
fastqc results/trimmed/{sample}_R1.fastq.gz results/trimmed/{sample}_R2.fastq.gz -o results/fastqc_trimmed -t 4

  4. fastqc_raw
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc/{sample}_R1_fastqc.html", "results/fastqc/{sample}_R1_fastqc.zip", "results/fastqc/{sample}_R2_fastqc.html", "results/fastqc/{sample}_R2_fastqc.zip"]
     command: mkdir -p results/fastqc
fastqc raw_data/{sample}_R1.fastq.gz raw_data/{sample}_R2.fastq.gz -o results/fastqc -t 4


Summary: 4 rules, total 13 threads declared, max 4 threads/rule
         4 rule(s) with memory requirements

To execute:  oxo-flow run qc-pipeline.oxoflow -j 10
```

The dry-run expands `{config.*}` variables but leaves `{sample}` wildcards unexpanded. Unlike concrete input paths (reported as `input ✓`/`input ✗`), wildcard inputs are not checked for existence in dry-run mode.

---

## 7. Visualize the DAG

```bash
oxo-flow graph qc-pipeline.oxoflow
```

---

## 8. Run with parallel execution

```bash
oxo-flow run qc-pipeline.oxoflow -j 4
```

The `-j 4` flag allows up to 4 jobs to run concurrently. oxo-flow will execute `fastqc_raw` and `fastp_trim` in parallel, then `fastqc_trimmed`, then `multiqc`.

---

## Key Concepts Covered

| Concept | Where you saw it |
|---|---|
| **Workflow metadata** | `[workflow]` section with name, version, description |
| **Configuration variables** | `[config]` section referenced as `{config.samples_dir}` |
| **Defaults** | `[defaults]` section applied to all rules |
| **Per-rule overrides** | `multiqc` rule overrides `threads = 1` in `[rules.resources]` |
| **Environment specs** | `environment = { conda = "envs/qc.yaml" }` |
| **Wildcard patterns** | `{sample}` in file paths |
| **Multi-line shell** | Triple-quoted strings with `"""` |
| **Automatic dependencies** | Input/output matching across rules |

---

## Next Steps

- [Variant Calling Pipeline](./variant-calling.md) — build a complete NGS analysis workflow
- [Environment Management](./environment-management.md) — use docker, singularity, and more
- [Create a Workflow](../how-to/create-workflow.md) — reference guide for `.oxoflow` authoring
