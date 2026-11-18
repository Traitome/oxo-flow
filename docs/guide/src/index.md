# Introduction

**oxo-flow** is a Rust-native bioinformatics pipeline engine for verifiable, AI-native science. It compiles declarative TOML workflows into a static Directed Acyclic Graph, manages software environments automatically, and runs jobs in parallel — all from a single, fast binary, with no Python or JVM runtime for the engine itself. Bioinformatics tools, containers, and environment managers remain explicit external dependencies.

```bash
# Define your workflow in TOML
cat pipeline.oxoflow

# Execute it
oxo-flow run pipeline.oxoflow -j 8
```

```
oxo-flow v0.32.0 — Rust-native bioinformatics pipeline engine
DAG: 5 rules in dependency order
  1. fastqc
  2. trim_reads
  3. bwa_align
  4. sort_bam
  5. call_variants
  Running: fastqc
  ✓ fastqc (3.2s)
  ⋮
Done: 5 succeeded, 0 skipped, 0 failed
✓ 9 output files verified (3.4GB total)
```

---

!!! tip "Community catalog"
    Looking for ready-to-run workflows? The community maintains a curated,
    rated catalog at **[oxo-flow-community](https://oxo-flow-community.github.io/)** —
    verified ports of popular nf-core & Snakemake pipelines, original workflows,
    and community submissions.

---

## What Is oxo-flow?

oxo-flow is a high-performance workflow engine built from the ground up in Rust for bioinformatics and clinical genomics. You define pipelines in a clean TOML format (`.oxoflow` files), and oxo-flow handles dependency resolution, environment activation, parallel execution, and report generation — with Rust's memory safety and zero interpreter overhead.

### Core capabilities

| Capability | Description |
|---|---|
| **DAG engine** | Automatic dependency resolution, topological sorting, cycle detection, and parallel execution of independent rules |
| **AI Companion & Scientist Team** | Engine-validated pipeline generation and refinement, domain diagnostic patterns, experimental repair proposals, and a multi-agent review profile |
| **Disk-pressure & scratch safety** | Multi-stage runtime disk-pressure guard (Normal → Reclaim → Hold → Abort) with cascade reclamation, plus global `tmp_dir` scratch isolation |
| **Cryptographic provenance** | With `--provenance`, run checkpoints record SHA-256 output checksums on both the local and cluster execution paths, plus configuration fingerprints and protected data tree audits (`protected_data_trees`); recorded checksums take precedence, with fragile file timestamps (`mtime`) demoted to a bounded fallback |
| **Environment management** | First-class support for 8 backends: conda, mamba, pixi, docker, singularity, venv, system, HPC modules — per rule |
| **Reporting** | Generate structured HTML, JSON, Markdown, and PDF reports from checkpoint data — execution truth, failure diagnosis, and file checksums |
| **Web API** | Built-in REST API (axum-based) for building, validating, and monitoring workflows remotely |
| **VS Code extension** | First-party `traitome.oxo-flow` IDE for `.oxoflow` — schema-driven completion, background diagnostics, canonical formatting, one-click run / dry-run / graph / status / clean / resume / AI-generate (published to [Open VSX](https://open-vsx.org/extension/traitome/oxo-flow) and the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=traitome.oxo-flow); also attached as a VSIX to every release) |
| **Container packaging** | Bundle workflows with their environment files and container references (`publish` / `pull`) for portable execution; building and verifying the images themselves remain external |
| **Cluster backends** | Submit jobs to SLURM, PBS, SGE, LSF, Kubernetes, AWS Batch, and GCP Batch clusters with resource-aware scheduling; `run --profile` folds terminal verdicts into the checkpoint as jobs settle and re-queues interrupted rules on restart, `cluster status --json` / `cluster logs` map scheduler ids back to rule instances, and `[engine] dispatch_order = "sample"` orders equal-priority ties by sample so a sample's stages run adjacent (priority aging still applies) |
| **Wildcard expansion** | `{sample}`, `{chr}` patterns that expand automatically from inputs or config |

---

## Design Principles

oxo-flow rethinks bioinformatics workflow orchestration from the ground up:

1. **DAG as pure declarative data, not imperative code** — `.oxoflow` TOML is strictly validated before any compute begins (known-key whitelist enforced at parse time, drift-gated against the shipped JSON Schema), eliminating runtime side-effects and arbitrary code execution at parse time.
2. **Cryptographic determinism over file timestamps** — Change detection records SHA-256 output checksums with `--provenance` and content-addressable environment hashing; recorded checksums take precedence, replacing fragile 40-year-old `mtime` heuristics as the freshness signal. Checksums prove declared state, not biological or bitwise equivalence.
3. **Physical resource boundaries as first-class constraints** — Built-in multi-stage runtime disk-pressure protection and scratch volume isolation (`tmp_dir`) prevent catastrophic out-of-disk failures on long-running pipelines.
4. **Environment isolation** — 8 per-rule backends (conda, mamba, pixi, docker, singularity, venv, system, HPC modules) reduce tool-version conflicts; this is not a complete host or tenant boundary.
5. **Rust-native compiled performance & safety** — Compiled native binary with no embedded Python or JVM runtime, `#![forbid(unsafe_code)]`, and fast startup; bioinformatics tools, containers, and environment managers remain explicit external dependencies.
6. **AI as an architectural citizen** — Grounded on deterministic engine APIs with structured `{code, message, detail, suggestion}` error contracts, unified validation feedback loops, and 31 domain diagnostic patterns. Generation output is engine-validated; diagnosis is model-authored and advisory; repair remains experimental.

---

## Why a new engine for the AI era?

The dominant workflow engines were designed for humans writing code by hand. Snakemake embeds arbitrary Python and Nextflow wires data through Groovy channel operators — expressive for humans, but in both the DAG is whatever the script computes at runtime, so fully verifying it means executing the script itself. oxo-flow declares the entire workflow as schema-validated TOML data and compiles a static DAG first: the same shape an LLM can emit, a schema can reject, and an engine can verify before a single CPU-hour is spent. That is what makes the machine contract possible — stable `E`/`W`/`SCI-*`/`TYPE-*` diagnostic codes, `--json` on every gate, and AI-generated pipelines that must pass the engine's own validate/dry-run gates before they can run.

The trade-off is stated honestly: dynamic DSLs remain unmatched for free-form programmatic logic, and ecosystem gravity (nf-core modules, community Snakemake rules) is real. oxo-flow bets on the verifiability side for cohort-scale production runs and offers `oxo-flow import` for existing Nextflow/Snakemake work (a deterministic subset with hard errors for unsupported constructs; coverage is prototype-grade — see [LIMITATIONS](https://github.com/Traitome/oxo-flow/blob/main/LIMITATIONS.md)).

The research program behind this bet is organized around four design bets — **BioTypes** (biological type contracts on DAG edges, checked statically with `TYPE-*` diagnostics, remediation hints, and synthesized coercion commands where mechanical), an **autonomous self-healing runtime** (31 diagnostic patterns, `SCI-*` scientific preflight, `--ai-recover` safety-gated repair proposals, and a seeded-failure `report --ai` interpretation benchmark), a **universal ecosystem transpiler** (Nextflow/Snakemake import: deterministic subset, hard errors, prototype-grade coverage), and **cryptographic provenance** (SHA-256 output checksums, config fingerprints, protected-data audits). Each is anchored to shipped functionality and carries an explicit maturity label in the [Vision](vision.md) page and [ROADMAP Phase 12](https://github.com/Traitome/oxo-flow/blob/main/ROADMAP.md). None of these bets is established science — the labels distinguish what ships from what still needs evidence.

!!! note "Exercised beyond CI"
    The cluster path is exercised by live production Slurm deployments as well as CI. Engineering fixes for failure modes observed there ship with regression tests on `main`; this is operational evidence, not a controlled benchmark.

!!! warning "Maturity & evidence"
    Nothing on this page is peer-reviewed evidence. Generation is engine-validated; diagnosis is model-authored and advisory; repair is experimental; provenance records declared-output checksums plus configuration and environment fingerprints — not bitwise reproducibility across architectures. Per-capability maturity labels and exit criteria live in the [Vision](vision.md) page and [ROADMAP Phase 12](https://github.com/Traitome/oxo-flow/blob/main/ROADMAP.md).

---

## Who Is This For?

**Bioinformaticians** who build and maintain analysis pipelines — oxo-flow gives you a fast workflow engine with clear error messages, explicit reproducibility controls, and no Python or JVM runtime dependency for the engine itself.

**Research laboratories and core facilities** running genomics workflows — the reporting system records execution evidence (engine version, workflow checksum, checkpoint provenance), and container/environment references make the software stack explicit and portable.

**Researchers** who need reproducible science — the engine records execution evidence and supports pinned environments, but identical biological results still require controlled inputs, references, software, commands, hardware, and external services.

**Core facility staff** managing multi-sample, multi-assay workloads — the DAG engine and cluster backends handle parallelism and resource scheduling automatically.

---

## How to Use This Guide

This documentation follows the [Diátaxis](https://diataxis.fr/) framework and is organized into four sections:

### If you are new to oxo-flow

Start with the **Tutorials** in order:

1. [Installation](./tutorials/installation.md) — install the binary
2. [Quick Start](./tutorials/quickstart.md) — run your first workflow in 5 minutes
3. [Your First Workflow](./tutorials/first-workflow.md) — build a pipeline from scratch
4. [Writing Custom Scripts](./tutorials/custom-scripts.md) — embed Python, R, and Bash scripts
5. [Variant Calling Pipeline](./tutorials/variant-calling.md) — complete NGS analysis
6. [Environment Management](./tutorials/environment-management.md) — conda, mamba, pixi, docker, singularity, venv, system, HPC modules

### If you want to learn by example

Explore the **[Workflow Gallery](./gallery/index.md)** — 16 curated workflows from hello-world to multi-omics integration and applied study designs, each with validation output, DAG visualizations, and scientific context:

1. [Hello World](./gallery/hello-world.md) ⭐ — Minimal rule structure
2. [File Pipeline](./gallery/file-pipeline.md) ⭐⭐ — Multi-rule dependencies
3. [Parallel Samples](./gallery/parallel-samples.md) ⭐⭐ — Wildcard expansion
4. [Scatter-Gather](./gallery/scatter-gather.md) ⭐⭐⭐ — Parallel chunk processing
5. [Environment Management](./gallery/environment-mgmt.md) ⭐⭐⭐ — Per-rule isolation
6. [RNA-seq Quantification](./gallery/rnaseq.md) ⭐⭐⭐⭐ — Transcriptomics pipeline
7. [WGS Germline Calling](./gallery/wgs-germline.md) ⭐⭐⭐⭐⭐ — GATK best practices
8. [Multi-Omics Integration](./gallery/multiomics.md) ⭐⭐⭐⭐⭐ — WGS + RNA-seq + Methylation
9. [Single-Cell RNA-seq](./gallery/single-cell-rnaseq.md) ⭐⭐⭐⭐ — scRNA-seq analysis
10. [Transform Operator](./gallery/transform-operator.md) ⭐⭐⭐ — Unified scatter-gather
11. [Conditional Execution](./gallery/conditional-workflow.md) ⭐⭐⭐ — `when`-gated WGS/WES branches
12. [Cohort Analysis](./gallery/cohort-analysis.md) ⭐⭐⭐⭐ — Multi-group cohorts and QC aggregation
13. [Germline Variant Calling](./gallery/simple-variant-calling.md) ⭐⭐⭐⭐ — Per-sample GATK chain
14. [Paired Experiment-Control](./gallery/paired-experiment-control.md) ⭐⭐⭐⭐⭐ — Single-pair somatic calling
15. [Paired Experiment-Control (Pairs)](./gallery/paired-experiment-control-pairs.md) ⭐⭐⭐⭐⭐ — `[[pairs]]`-scaled somatic calling
16. [16S Amplicon (QIIME2)](./gallery/16s-qiime2.md) ⭐⭐⭐⭐ — Amplicon denoising and diversity

### If you need to accomplish a specific task

Jump to the **How-to Guides**:

- [Create a Workflow](./how-to/create-workflow.md)
- [Use Environments](./how-to/use-environments.md)
- [Run on a Cluster](./how-to/run-on-cluster.md)
- [Generate Reports](./how-to/generate-reports.md)

### If you need exact syntax and options

See the **Command Reference** for all 36 CLI subcommands with usage, options, and examples, or the [VS Code Extension](./reference/vscode-extension.md) reference for the first-party editor integration.

### If you want the full technical details

See **Architecture & Design** for in-depth documentation of the DAG engine, environment system, `.oxoflow` format specification, and web API.

---

## Quick Example

Here is a complete workflow that aligns paired-end reads and sorts the output:

```toml
# align.oxoflow
[workflow]
name = "align-and-sort"
version = "1.0.0"
sample_pattern = "{sample}_R1.fastq.gz"

[config]
reference = "/data/ref/hg38.fa"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "bwa_align"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
environment = { docker = "quay.io/biocontainers/bwa:0.7.19--h577a1d6_1" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"

[rules.resources]
threads = 16
memory = "32G"

[[rules]]
name = "index_bam"
input = ["aligned/{sample}.bam"]
output = ["aligned/{sample}.bam.bai"]
environment = { conda = "envs/samtools.yaml" }
shell = "samtools index {input}"
```

`{sample}` is a wildcard: `sample_pattern` tells oxo-flow to discover one sample per matching read file (`S1_R1.fastq.gz`, `S2_R1.fastq.gz`, … become one `bwa_align` task per sample). See [Wildcards](./reference/wildcards.md) for the other expansion sources.

Run it:

```bash
# Validate the workflow
oxo-flow validate align.oxoflow

# Preview the execution plan
oxo-flow dry-run align.oxoflow

# Execute (dry-run prints a suggested -j; the resource pool also
# schedules by thread capacity, so jobs queue instead of oversubscribing)
oxo-flow run align.oxoflow -j 1

# Visualize the DAG (write the dot description, then render it)
oxo-flow graph -f dot -o dag.dot align.oxoflow
dot -Tpng -o dag.png dag.dot
```

!!! note "stdout is for machines, stderr is for humans"
    Progress, warnings, and log lines go to **stderr**; **stdout** is
    reserved for machine-readable output — `--json`, `graph --format dot`,
    and every `--format`/`-o` deliverable. That is why the `graph` example
    above writes the DOT to a file with `-o` (or redirect stdout) instead of
    mixing it with run logs.

---

## Key Concepts

If you are new to pipeline engines, here are the three core concepts used in oxo-flow:

### Workflow
A **Workflow** is the entire pipeline definition (usually a `.oxoflow` file). It contains a collection of rules, configuration settings, and software requirements.

### Rule
A **Rule** is a single processing step. It defines:

- **Input**: The files needed to run (e.g., raw reads).
- **Output**: The files produced by the step (e.g., aligned BAM).
- **Command**: The actual shell command to execute (e.g., `bwa mem`).
- **Environment**: The software tools needed (e.g., a specific Conda environment).

### DAG (Directed Acyclic Graph)
A **DAG** is a mathematical representation of your workflow. It is a "map" that shows how rules are connected by their inputs and outputs. oxo-flow builds this map automatically to determine which rules can run in parallel and which must wait for others to finish.

---

## Project Status

oxo-flow is under active development. The current release (`v0.32.0`) includes the complete core engine, CLI, and web API. See the [Changelog](./development/changelog.md) for release history and the [Contributing guide](./development/contributing.md) if you want to get involved.

---


## How to Cite

If you use oxo-flow in academic research, please cite:

> **Shixiang Wang**, *oxo-flow: compiled, memory-safe bioinformatics workflow orchestration*, bioRxiv, 2026, [https://doi.org/10.64898/2026.06.11.731578](https://doi.org/10.64898/2026.06.11.731578)
>
> Jia Ding, Yun Peng, Ruochen Wei, Boquan Wang, Jian-Guo Zhou, **Shixiang Wang**, *BLIT: an R package for seamless integration of command-line bioinformatics tool universe*, Bioinformatics Advances, Volume 6, Issue 1, 2026, vbag088, [https://doi.org/10.1093/bioadv/vbag088](https://doi.org/10.1093/bioadv/vbag088)

## Join the Community

The core, AI, CLI, and desktop components are Apache-2.0 licensed; the web crate uses a separate Academic/Commercial license. Bug reports, feature requests, and contributions are welcome. See the repository licensing table for the authoritative per-crate terms.

| How to contribute | Link |
|---|---|
| Report a bug | [Bug report](https://github.com/Traitome/oxo-flow/issues/new) |
| Request a feature | [Feature request](https://github.com/Traitome/oxo-flow/issues/new) |
| Contribute code | [Contributing guide](./development/contributing.md) |

> **Try it, break it, and tell us what happened.** Even a short comment about what worked — or didn't — helps improve oxo-flow for everyone.
