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
  - fastp=1.3.6
  - multiqc=1.35
```

---

## 3. Write the workflow

Replace `qc-pipeline.oxoflow` with:

!!! tip "Configuration Syntax"
    `{config.samples_dir}` refers to the `samples_dir` variable defined in the `[config]` section. This allows you to centralize paths and settings.

!!! info "Wildcard Patterns"
    The `{sample}` in the file paths below is a **wildcard**. To expand it, declare a `sample_pattern` in the `[workflow]` section: oxo-flow scans your `raw_data` directory for files matching the pattern `{sample}_R1.fastq.gz`, extracts the sample name, and automatically generates a task for every sample it finds.

```toml
[workflow]
name = "qc-pipeline"
version = "1.0.0"
description = "Quality control for paired-end sequencing data"
author = "Your Name"
sample_pattern = "raw_data/{sample}_R1.fastq.gz"   # ← plain paths only; {config.*} is not expanded here

[config]
samples_dir = "raw_data"
results_dir = "results"

[defaults]
threads = 4
memory = "8G"

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
    "{config.results_dir}/fastqc/{sample}_R1_fastqc.html",
    "{config.results_dir}/fastqc_trimmed/{sample}_R1_fastqc.html"
]
output = [
    "{config.results_dir}/multiqc/multiqc_report.html"
]
threads = 1  # override [defaults] threads = 4 — aggregation is I/O-light
environment = { conda = "envs/qc.yaml" }
shell = """
mkdir -p {config.results_dir}/multiqc
multiqc {config.results_dir} -o {config.results_dir}/multiqc --force
"""
```

---

## 4. Understand the dependency graph

The workflow forms this DAG:

```mermaid
graph TD
    A[fastqc_raw] --> D[multiqc]
    B[fastp_trim] --> C[fastqc_trimmed]
    C --> D
```

- `fastqc_raw` and `fastp_trim` can run in parallel (no dependency between them)
- `fastqc_trimmed` depends on `fastp_trim`'s output — inferred automatically because its `input` files match `fastp_trim`'s `output` files
- `multiqc` aggregates the two QC rounds — its two inputs are the report files produced by `fastqc_raw` and `fastqc_trimmed`. Note that `fastp_trim` has **no direct edge** to `multiqc`: the trimmed data itself is not a multiqc input; multiqc implicitly waits for it via `fastqc_trimmed`'s transitive dependency.

!!! tip "Parallel scheduling"
    The engine does not serialize the whole pipeline. `fastqc_raw` starts **immediately, in parallel with `fastp_trim`** — it never waits for trimmed data. Only `fastqc_trimmed` waits for `fastp_trim` to finish, and `multiqc` waits for both QC rounds. On a multi-core machine, raw QC and trimming run simultaneously.

!!! info "Two dependency mechanisms"
    oxo-flow supports two ways to declare dependencies:

    1. **File-based (automatic)** — if rule B's `input` matches rule A's `output`, the edge is inferred. This tutorial uses only this mechanism: `fastp_trim → fastqc_trimmed` (trimmed reads) and `fastqc_raw / fastqc_trimmed → multiqc` (QC reports).
    2. **`depends_on` (explicit)** — list rule names that must finish first, even when no direct file match exists. Use this for setup rules with no outputs:

    ```toml
    [[rules]]
    name = "setup_dirs"
    output = []                  # no files to match!
    shell = "mkdir -p results"

    [[rules]]
    name = "align"
    depends_on = ["setup_dirs"]  # ← explicit ordering, no file to match
    shell = "bwa mem ..."
    ```

    Prefer file-based inference when possible — it makes the data flow self-documenting. Use `depends_on` only when the ordering can't be expressed through file matching alone.

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
# ✓ qc-pipeline.oxoflow — 4 rules, 4 dependencies

oxo-flow dry-run qc-pipeline.oxoflow
```

```
oxo-flow v0.16.0 — Rust-native bioinformatics pipeline engine
INFO Auto-discovered 2 samples from pattern 'raw_data/{sample}_R1.fastq.gz'
DAG: (dry-run) 8 rules would execute
  1. fastp_trim_auto-discovered_sample1
     threads=4
     env=conda
     memory=8G
     outputs: ["results/trimmed/sample1_R1.fastq.gz", "results/trimmed/sample1_R2.fastq.gz", ...]
     command: mkdir -p results/trimmed
fastp --in1 raw_data/sample1_R1.fastq.gz --in2 raw_data/sample1_R2.fastq.gz --out1 results/trimmed/sample1_R1.fastq.gz --out2 results/trimmed/sample1_R2.fastq.gz --html results/trimmed/sample1_fastp.html --json results/trimmed/sample1_fastp.json --thread 4

  2. fastp_trim_auto-discovered_sample2
     threads=4
     env=conda
     memory=8G
     outputs: ["results/trimmed/sample2_R1.fastq.gz", "results/trimmed/sample2_R2.fastq.gz", ...]

  3. fastqc_raw_auto-discovered_sample1
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc/sample1_R1_fastqc.html", ...]

  4. fastqc_raw_auto-discovered_sample2
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc/sample2_R1_fastqc.html", ...]

  5. fastqc_trimmed_auto-discovered_sample1
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc_trimmed/sample1_R1_fastqc.html", ...]

  6. fastqc_trimmed_auto-discovered_sample2
     threads=4
     env=conda
     memory=8G
     outputs: ["results/fastqc_trimmed/sample2_R1_fastqc.html", ...]

  7. multiqc_auto-discovered_sample1
     threads=1
     env=conda
     memory=8G
     outputs: ["results/multiqc/multiqc_report.html"]
     command: mkdir -p results/multiqc
multiqc results -o results/multiqc --force

  8. multiqc_auto-discovered_sample2
     threads=1
     env=conda
     memory=8G
     outputs: ["results/multiqc/multiqc_report.html"]

Summary: 8 rules, total 26 threads declared, max 4 threads/rule
         8 rule(s) with memory requirements
         1 sample group(s), 0 pair(s)

To execute:  oxo-flow run qc-pipeline.oxoflow -j 2
```

The dry-run has expanded the `{sample}` wildcard into per-sample tasks: each of the 4 template rules became one task per discovered sample (`_auto-discovered_sample1`, `_auto-discovered_sample2`), for 8 tasks in total. Inputs are not checked for existence in dry-run mode.

!!! note "Aggregation rules are expanded per sample too"
    `multiqc` became two tasks, but both aggregate the same `results` directory and write the same `results/multiqc/multiqc_report.html` — the second run overwrites the first. This duplication is harmless here (MultiQC re-scans the whole directory), but for truly single-shot aggregation steps you may want to run them separately or via `depends_on` without a sample wildcard in the inputs.

The suggested `-j 2` comes from dividing the machine's CPU threads by the workflow's maximum per-rule thread declaration (10 ÷ 4 = 2) — running more jobs than that would oversubscribe the CPU. If your rules are I/O-bound you can raise it.

!!! note "Listing order reflects parallel levels"
    `fastp_trim` and `fastqc_raw` are listed adjacent because they are independent and will run **in parallel** at the same DAG level. Rules within a level are sorted alphabetically; `fastqc_trimmed` waits for `fastp_trim`, and `multiqc` waits for all three.

---

## 7. Visualize the DAG

```bash
oxo-flow graph qc-pipeline.oxoflow
```

---

## 8. Run with parallel execution

```bash
oxo-flow run qc-pipeline.oxoflow -j 2
```

The `-j 2` flag allows up to 2 jobs to run concurrently — matching the suggestion from dry-run (machine threads ÷ per-rule threads). oxo-flow will execute the four independent level-0 tasks (two `fastp_trim`, two `fastqc_raw`) two at a time, then the two `fastqc_trimmed` tasks, then the two `multiqc` tasks.

---

## Key Concepts Covered

| Concept | Where you saw it |
|---|---|
| **Workflow metadata** | `[workflow]` section with name, version, description |
| **Configuration variables** | `[config]` section referenced as `{config.samples_dir}` |
| **Defaults** | `[defaults]` section applied to all rules |
| **Per-rule overrides** | `multiqc` rule overrides `threads = 1`, superseding `[defaults] threads = 4` |
| **Environment specs** | `environment = { conda = "envs/qc.yaml" }` |
| **Wildcard patterns** | `{sample}` in file paths |
| **Sample auto-discovery** | `sample_pattern = "raw_data/{sample}_R1.fastq.gz"` in `[workflow]` |
| **Multi-line shell** | Triple-quoted strings with `"""` |
| **Automatic dependencies** | Input/output matching across rules (all edges in this tutorial) |
| **Explicit dependencies** | `depends_on` field — shown in the info box as an alternative for output-less rules |

---

## Next Steps

- [Variant Calling Pipeline](./variant-calling.md) — build a complete NGS analysis workflow
- [Environment Management](./environment-management.md) — use docker, singularity, and more
- [Create a Workflow](../how-to/create-workflow.md) — reference guide for `.oxoflow` authoring
