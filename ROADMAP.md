# oxo-flow Roadmap

> **Mission**: Build a Rust-native bioinformatics pipeline engine that fully replaces Snakemake,
> with first-class clinical-grade reporting, environment management, and a powerful web interface.
>
> Licensed under Apache 2.0 — fully open source and free.

---

## Architecture Overview

### Design Principles (First Principles Thinking)

1. **DAG is the fundamental abstraction** — Every bioinformatics workflow is a directed acyclic graph of tasks. The engine must natively construct, validate, optimize, and execute DAGs with maximum parallelism.

2. **Environment isolation is non-negotiable** — Bioinformatics tools have conflicting dependencies. Each task must run in its own isolated environment (conda, pixi, docker, singularity, venv).

3. **Reproducibility through determinism** — Given the same inputs, configuration, and environment specifications, the pipeline must produce identical outputs. Lock files, checksums, and container pinning ensure this.

4. **Inverse design** — Start from what the user needs (clinical report) and work backward to determine what data, tools, and steps are required. The engine supports both forward (input→output) and reverse (output→input) dependency resolution.

5. **Performance through Rust** — Zero-cost abstractions, fearless concurrency, and efficient memory management make Rust ideal for orchestrating thousands of concurrent bioinformatics tasks.

6. **Clinical-grade quality** — Reports must be accurate, traceable, and auditable. Every step logs its provenance, inputs, outputs, software versions, and execution environment.

### System Architecture

```
┌──────────────────────────────────────────────────────┐
│                    oxo-flow CLI                       │
│  (run | dry-run | validate | graph | report | env |  │
│   package | serve | init | status | clean | config)  │
├──────────────────────────────────────────────────────┤
│                   oxo-flow Web                        │
│  (REST API + Web UI for visual workflow editing,     │
│   monitoring, report viewing)                         │
├──────────────────────────────────────────────────────┤
│                 oxo-flow Core Library                 │
│  ┌─────────┐ ┌──────────┐ ┌───────────────┐         │
│  │   DAG   │ │ Executor │ │  Environment  │         │
│  │ Engine  │ │ (local/  │ │  Manager      │         │
│  │         │ │ cluster/ │ │ (conda/pixi/  │         │
│  │         │ │ cloud)   │ │ docker/sing.) │         │
│  └─────────┘ └──────────┘ └───────────────┘         │
│  ┌─────────┐ ┌──────────┐ ┌───────────────┐         │
│  │Wildcard │ │Scheduler │ │   Reporter    │         │
│  │ Engine  │ │(resource │ │  (HTML/PDF/   │         │
│  │         │ │ aware)   │ │   JSON)       │         │
│  └─────────┘ └──────────┘ └───────────────┘         │
│  ┌─────────┐ ┌──────────┐ ┌───────────────┐         │
│  │ Config  │ │Container │ │    Rule       │         │
│  │ Parser  │ │ Packager │ │  Definitions  │         │
│  └─────────┘ └──────────┘ └───────────────┘         │
└──────────────────────────────────────────────────────┘
```

### .oxoflow File Format Specification

The `.oxoflow` format is TOML-based, designed for readability and composability:

```toml
[workflow]
name = "tumor-variant-calling"
version = "1.0.0"
description = "End-to-end tumor variant calling pipeline"
author = "Traitome"

[config]
samples = "samples.csv"
reference = "GRCh38"
threads_max = 64
memory_max = "256G"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "fastp_trim"
input = ["raw/{sample}_R{read}.fastq.gz"]
output = ["trimmed/{sample}_R{read}.fastq.gz", "qc/{sample}_fastp.json"]
threads = 8
memory = "16G"
environment = { conda = "envs/fastp.yaml" }
shell = """
fastp -i {input[0]} -I {input[1]} \
      -o {output[0]} -O {output[1]} \
      --json {output[2]} --thread {threads}
"""

[[rules]]
name = "bwa_mem2_align"
input = ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.sorted.bam"]
threads = 16
memory = "32G"
environment = { singularity = "docker://biocontainers/bwa-mem2:2.2.1" }
shell = """
bwa-mem2 mem -t {threads} {config.reference} {input[0]} {input[1]} \
  | samtools sort -@ 4 -o {output[0]}
"""

[report]
template = "clinical_variant_report"
format = ["html", "pdf"]
sections = ["summary", "variants", "coverage", "qc_metrics"]
```

---

## Phase 1: Foundation (v0.1.0)

**Goal**: Core library with DAG engine, basic execution, and CLI skeleton.

### Milestone 1.1: Project Setup
- [x] Cargo workspace with multi-crate layout
- [x] CI/CD pipeline (GitHub Actions: fmt, clippy, build, test)
- [x] Apache 2.0 license
- [x] copilot-instructions.md
- [x] ROADMAP.md and design documentation

### Milestone 1.2: Core Data Structures
- [x] Rule definition types (inputs, outputs, shell, resources, environment)
- [x] Wildcard pattern types and basic expansion
- [x] Configuration data model (.oxoflow TOML parsing)
- [x] Unified error types with `thiserror`

### Milestone 1.3: DAG Engine
- [x] DAG construction from rule dependencies (output→input matching)
- [x] Cycle detection (Kahn's algorithm or DFS-based)
- [x] Topological sorting for execution order
- [x] DAG visualization (DOT format export)
- [x] Dry-run simulation

### Milestone 1.4: Basic Executor
- [x] Local process executor (shell command runner)
- [x] Job status tracking (pending, running, success, failed, skipped)
- [x] Parallel execution with configurable concurrency
- [x] Basic logging with `tracing`

### Milestone 1.5: CLI Skeleton
- [x] `oxo-flow run <workflow.oxoflow>` — execute pipeline
- [x] `oxo-flow dry-run` — simulate execution
- [x] `oxo-flow validate` — validate .oxoflow file
- [x] `oxo-flow graph` — output DAG in DOT format
- [x] `oxo-flow --version` / `--help`

---

## Phase 2: Environment Management (v0.2.0)

**Goal**: Full support for conda, pixi, docker, singularity, venv environments.

### Milestone 2.1: Environment Abstraction
- [ ] Environment trait with detect/create/activate/run interface
- [ ] Per-rule environment configuration in .oxoflow

### Milestone 2.2: Conda/Pixi Support
- [ ] Conda environment creation from YAML specs
- [ ] Pixi environment support
- [ ] Environment caching and reuse

### Milestone 2.3: Container Support
- [ ] Docker execution backend
- [ ] Singularity/Apptainer execution backend
- [ ] Automatic bind mount detection
- [ ] Container image caching

### Milestone 2.4: Virtual Environment Support
- [ ] Python venv creation and activation
- [ ] pip requirements.txt installation
- [ ] Poetry/uv support

---

## Phase 3: Advanced Execution (v0.3.0)

**Goal**: Resource-aware scheduling, cluster execution, and checkpointing.

### Milestone 3.1: Resource-Aware Scheduler
- [ ] CPU, memory, GPU, disk resource declarations
- [ ] Resource pool management
- [ ] Priority-based scheduling

### Milestone 3.2: Cluster Backends
- [ ] SLURM executor
- [ ] PBS/Torque executor
- [ ] SGE executor
- [ ] LSF executor

### Milestone 3.3: Execution Features
- [ ] Checkpointing and resume from failure
- [ ] File timestamp-based re-execution (like Make)
- [ ] Benchmark collection (time, memory, CPU usage per rule)
- [ ] Shadow directories for isolation
- [ ] Temporary file management

---

## Phase 4: Reporting System (v0.4.0)

**Goal**: Modular, clinical-grade report generation.

### Milestone 4.1: Report Engine
- [ ] Template-based report generation (Tera/Handlebars)
- [ ] HTML report output with embedded charts
- [ ] PDF generation
- [ ] JSON structured report output

### Milestone 4.2: Report Components
- [ ] QC metrics panels (coverage, quality scores)
- [ ] Variant summary tables
- [ ] Interactive plots (via embedded JS)
- [ ] Execution provenance section

### Milestone 4.3: Clinical Report Standards
- [ ] HL7/FHIR compatible structured output
- [ ] Audit trail and traceability
- [ ] Digital signatures for report integrity

---

## Phase 5: Web Interface (v0.5.0)

**Goal**: Full-featured web UI for building, running, and monitoring pipelines.

### Milestone 5.1: REST API
- [ ] Workflow CRUD endpoints
- [ ] Run management (start, stop, status, logs)
- [ ] Report retrieval
- [ ] Environment management

### Milestone 5.2: Web UI
- [ ] Visual DAG editor (drag-and-drop rule creation)
- [ ] Real-time execution monitoring
- [ ] Log viewer with streaming
- [ ] Report browser

### Milestone 5.3: Workflow Builder
- [ ] No-code pipeline construction
- [ ] Rule library browser
- [ ] Parameter configuration forms
- [ ] Import/export .oxoflow files

---

## Phase 6: Container Packaging (v0.6.0)

**Goal**: Package entire workflows into portable containers.

### Milestone 6.1: Workflow Packaging
- [ ] `oxo-flow package` command
- [ ] Dockerfile generation from .oxoflow
- [ ] Multi-stage build optimization
- [ ] Reference data bundling

### Milestone 6.2: Portable Execution
- [ ] Self-contained container images
- [ ] Cloud deployment support (AWS Batch, Google Batch, Azure Batch)
- [ ] Kubernetes job submission

---

## Phase 7: Venus Pipeline (v0.7.0)

**Goal**: Clinical-grade tumor variant detection pipeline built on oxo-flow.

### Milestone 7.1: DNA Variant Calling
- [ ] FASTQ QC (fastp/FastQC)
- [ ] Read alignment (BWA-MEM2/minimap2)
- [ ] Duplicate marking (GATK MarkDuplicates / sambamba)
- [ ] Base quality recalibration (GATK BQSR)
- [ ] Germline SNV/indel calling (GATK HaplotypeCaller, DeepVariant)
- [ ] Somatic SNV/indel calling (Mutect2, Strelka2, VarDict)
- [ ] Consensus variant merging

### Milestone 7.2: Copy Number and Structural Variants
- [ ] CNV calling (CNVkit, GATK CNV, FACETS)
- [ ] SV calling (Manta, DELLY, GRIDSS)
- [ ] Tumor purity/ploidy estimation (PURPLE, Sequenza)

### Milestone 7.3: Variant Annotation
- [ ] Functional annotation (VEP, SnpEff, ANNOVAR)
- [ ] Clinical database annotation (ClinVar, COSMIC, OncoKB)
- [ ] Actionability assessment
- [ ] Mutational signature analysis

### Milestone 7.4: Three Scenarios
- [ ] Tumor-only mode
- [ ] Normal-only (germline) mode
- [ ] Tumor-normal paired mode

### Milestone 7.5: Clinical Report
- [ ] Patient information header
- [ ] Variant classification (pathogenic, likely pathogenic, VUS, etc.)
- [ ] Drug sensitivity/resistance annotations
- [ ] Clinical trial matching
- [ ] QC summary and coverage metrics
- [ ] PDF report with institutional branding

---

## Phase 8: Production Hardening (v1.0.0)

**Goal**: Production-ready release with comprehensive documentation and testing.

### Milestone 8.1: Testing
- [ ] >80% code coverage
- [ ] End-to-end integration tests with test datasets
- [ ] Performance benchmarks vs Snakemake/Nextflow
- [ ] Fuzzing for parser robustness

### Milestone 8.2: Documentation
- [ ] Complete API documentation (rustdoc)
- [ ] User guide (MkDocs)
- [ ] Tutorial series
- [ ] Migration guide from Snakemake

### Milestone 8.3: Distribution
- [ ] Pre-built binaries for Linux/macOS/Windows
- [ ] Homebrew formula
- [ ] Conda package
- [ ] Docker image

---

## Technology Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust (2024 edition) |
| Async runtime | tokio |
| CLI framework | clap (derive) |
| Serialization | serde + toml |
| HTTP client | reqwest |
| Web framework | axum |
| Logging | tracing |
| Error handling | thiserror (library) + anyhow (binary) |
| Template engine | tera |
| Testing | cargo test + integration tests |
| CI/CD | GitHub Actions |
| Documentation | MkDocs + rustdoc |
| License | Apache 2.0 |
