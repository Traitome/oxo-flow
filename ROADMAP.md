# oxo-flow Roadmap

> **Mission**: Build a Rust-native bioinformatics pipeline engine with first-principles design,
> first-class clinical-grade reporting, environment management, and a powerful web interface.
>
> Licensed under Apache 2.0 — fully open source and free.

---

## Multi-Expert System Evaluation

The following evaluation synthesizes perspectives from 20 domain experts to guide the
project's design decisions, priorities, and architecture.

### Expert Panel Summary

| # | Role | Key Recommendation |
|---|------|--------------------|
| 1 | Bioinformatics Expert | Prioritize wildcard compatibility and file-based dependency resolution |
| 2 | Tumor Bioinformatics Expert | Venus pipeline must support tumor-only, normal-only, and paired modes with clinical annotations |
| 3 | Clinical Oncologist | Reports must include variant classification (ACMG/AMP), drug annotations, and trial matching |
| 4 | Software Engineer | Modular crate architecture with clean trait abstractions enables long-term maintainability |
| 5 | Workflow Engine Expert | DAG engine must handle dynamic rule expansion from wildcards before execution |
| 6 | Software Architect | Trait-based environment backend abstraction allows pluggable execution without core changes |
| 7 | CLI Developer | Clap derive macros with subcommands, shell completion, and colored output for UX excellence |
| 8 | Web Developer | Axum REST API with typed endpoints; separate frontend from backend for flexibility |
| 9 | DevOps Engineer | Container packaging must support multi-stage Docker builds and Singularity for HPC |
| 10 | Performance Engineer | Tokio async runtime with semaphore-based concurrency; resource-aware scheduling prevents OOM |
| 11 | QA Engineer | >80% test coverage; integration tests for CLI, core library, and web API |
| 12 | Security Expert | Input sanitization for shell commands; no secret leakage in reports or logs |
| 13 | Clinical Report Expert | Modular report sections with HTML/JSON output; HL7/FHIR compatibility for EHR integration |
| 14 | Reproducibility Expert | Lock files, container pinning, and deterministic execution ensure reproducible results |
| 15 | HPC Systems Admin | SLURM/PBS executor backends with resource declarations (CPU, memory, GPU, wall-time) |
| 16 | Conda/Package Manager Expert | Conda env creation from YAML specs; pixi and venv for Python-only tools |
| 17 | Docker/Container Expert | Automatic bind mount detection; image caching; multi-stage builds for smaller images |
| 18 | Documentation Expert | MkDocs user guide, rustdoc API docs, workflow gallery, migration guides |
| 19 | UX Designer | Clear error messages, progress bars, colored output, and helpful --help text |
| 20 | Data Scientist | Template-based report generation with Tera; interactive plots via embedded JavaScript |

### Consensus Design Decisions

1. **Environment trait with async support** — `EnvironmentBackend` trait provides detect/create/activate/run interface; resolver selects backend per rule
2. **Resource-aware scheduler** — Track CPU, memory, GPU availability; only dispatch jobs when resources are available
3. **Template-based reporting** — Tera engine for HTML reports with CSS styling; JSON for programmatic access
4. **REST API design** — CRUD for workflows, runs, reports; SSE for real-time execution monitoring
5. **Container-first packaging** — Generate Dockerfile/Singularity definitions from workflow specs automatically
6. **Venus as reference pipeline** — Clinical tumor variant calling pipeline validates the entire engine stack

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
name = "experiment-variant-calling"
version = "1.0.0"
description = "End-to-end variant calling pipeline"
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
input = ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"]
output = ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz", "qc/{sample}_fastp.json"]
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

## Phase 1: Foundation

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

## Phase 2: Environment Management

**Goal**: Full support for conda, pixi, docker, singularity, venv environments.

### Milestone 2.1: Environment Abstraction
- [x] Environment trait with detect/create/activate/run interface
- [x] Per-rule environment configuration in .oxoflow
- [x] Async environment backend trait with setup/teardown lifecycle
- [x] Environment resolver with priority-based backend selection

### Milestone 2.2: Conda/Pixi Support
- [x] Conda environment creation from YAML specs
- [x] Pixi environment support
- [x] Environment caching and reuse

### Milestone 2.3: Container Support
- [x] Docker execution backend with volume mounts
- [x] Singularity/Apptainer execution backend
- [x] Automatic bind mount detection
- [x] Container image caching

### Milestone 2.4: Virtual Environment Support
- [x] Python venv creation and activation
- [x] pip requirements.txt installation
- [x] Poetry/uv support

---

## Phase 3: Advanced Execution

**Goal**: Resource-aware scheduling, cluster execution, and checkpointing.

### Milestone 3.1: Resource-Aware Scheduler
- [x] CPU, memory, GPU, disk resource declarations
- [x] Resource pool management
- [x] Priority-based scheduling

### Milestone 3.2: Cluster Backends
- [x] SLURM executor
- [x] PBS/Torque executor
- [x] SGE executor
- [x] LSF executor

### Milestone 3.3: Execution Features
- [x] Checkpointing and resume from failure
- [x] File timestamp-based re-execution (like Make)
- [x] Benchmark collection (time, memory, CPU usage per rule)
- [x] Shadow directories for isolation
- [x] Temporary file management

---

## Phase 4: Reporting System

**Goal**: Modular, clinical-grade report generation.

### Milestone 4.1: Report Engine
- [x] Template-based report generation (Tera)
- [x] HTML report output with embedded styles
- [x] JSON structured report output
- [x] Report builder pattern for composable reports

### Milestone 4.2: Report Components
- [x] QC metrics panels (coverage, quality scores)
- [x] Variant summary tables
- [x] Execution provenance section
- [x] Key-value metadata sections

### Milestone 4.3: Clinical Report Standards
- [x] Structured clinical report sections
- [x] Audit trail and traceability
- [x] Report metadata with timestamps and provenance

---

## Phase 5: Web Interface

**Goal**: Full-featured web UI for building, running, and monitoring pipelines.

### Milestone 5.1: REST API
- [x] Workflow CRUD endpoints (list, get, validate, create)
- [x] Run management (start, status)
- [x] Report retrieval
- [x] Environment management endpoints

### Milestone 5.2: Web UI
- [x] DAG visualization endpoint
- [x] Real-time execution status
- [x] Health check and version info
- [x] Workflow validation endpoint

### Milestone 5.3: Workflow Builder
- [x] Workflow file upload and parsing
- [x] Rule listing and inspection
- [x] DAG export in DOT format
- [x] API documentation via structured responses

---

## Phase 6: Container Packaging

**Goal**: Package entire workflows into portable containers.

### Milestone 6.1: Workflow Packaging
- [x] `oxo-flow package` command
- [x] Dockerfile generation from .oxoflow
- [x] Singularity definition file generation
- [x] Multi-stage build optimization
- [x] Reference data bundling options

### Milestone 6.2: Portable Execution
- [x] Self-contained container images
- [x] Label metadata for OCI compliance
- [x] Configurable base images

---

## Phase 7: Venus Pipeline

**Goal**: Clinical-grade tumor variant detection pipeline built on oxo-flow.

### Milestone 7.1: DNA Variant Calling
- [x] FASTQ QC (fastp/FastQC)
- [x] Read alignment (BWA-MEM2/minimap2)
- [x] Duplicate marking (GATK MarkDuplicates / sambamba)
- [x] Base quality recalibration (GATK BQSR)
- [x] Germline SNV/indel calling (GATK HaplotypeCaller, DeepVariant)
- [x] Somatic/experimental SNV/indel calling (Mutect2, Strelka2, VarDict)
- [x] Consensus variant merging

### Milestone 7.2: Copy Number and Structural Variants
- [x] CNV calling (CNVkit, GATK CNV, FACETS)
- [x] SV calling (Manta, DELLY, GRIDSS)
- [x] Tumor purity/ploidy estimation (PURPLE, Sequenza)

### Milestone 7.3: Variant Annotation
- [x] Functional annotation (VEP, SnpEff, ANNOVAR)
- [x] Clinical database annotation (ClinVar, COSMIC, OncoKB)
- [x] Actionability assessment
- [x] Mutational signature analysis

### Milestone 7.4: Three Scenarios
- [x] Tumor-only mode
- [x] Normal-only (germline) mode
- [x] Tumor-normal paired mode

### Milestone 7.5: Clinical Report
- [x] Patient information header
- [x] Variant classification (pathogenic, likely pathogenic, VUS, etc.)
- [x] Drug sensitivity/resistance annotations
- [x] Clinical trial matching
- [x] QC summary and coverage metrics
- [x] PDF report with institutional branding

---

## Phase 8: Production Hardening

**Goal**: Production-ready release with comprehensive documentation and testing.

### Milestone 8.1: Testing
- [x] Comprehensive unit test coverage across all modules
- [x] Integration tests for CLI commands
- [x] Integration tests for web API endpoints
- [x] Example workflow files with validation tests

### Milestone 8.2: Documentation
- [x] Complete API documentation (rustdoc)
- [x] Example workflow files
- [x] ROADMAP with multi-expert evaluation

### Milestone 8.3: Distribution
- [x] Cargo workspace with workspace dependencies
- [x] CI/CD pipeline (GitHub Actions)
- [x] Apache 2.0 license

---

## Phase 9: Multi-Expert Evaluation Response (v0.3.1)

**Goal**: Address the 300-item expert evaluation report (Issue #29) to improve
framework extensibility, code quality, and documentation.

### Milestone 9.1: Core Framework Enhancements (Completed)
- [x] `GpuSpec` with model, memory, compute capability fields (Expert 1.8, 21.3)
- [x] `ResourceHint` for resource estimation and dynamic scheduling (Expert 2.1)
- [x] `format_hint` on Rule for file format awareness (Expert 2.3, 5.1)
- [x] `pipe` flag on Rule for streaming/FIFO support (Expert 1.3)
- [x] `checksum` on Rule for output integrity verification (Expert 1.7, 15.6)
- [x] `rule_metadata` for arbitrary domain metadata (Expert 2.9)
- [x] `cache_key` for content-based caching (Expert 15.2)
- [x] `modules` field on EnvironmentSpec for HPC module systems (Expert 13.4)
- [x] `ReferenceDatabase` config section for reference DB versioning (Expert 4.3)
- [x] Data lineage tracking on ExecutionProvenance (Expert 8.10)
- [x] Checkpoint persistence with save/load to filesystem (Expert 1.1, 12.3)
- [x] JobStatus expanded with Queued/Cancelled/TimedOut states (Expert 8.5)
- [x] File checksum computation and non-determinism detection (Expert 1.6)
- [x] `Checkpoint` and `Integrity` error variants with context (Expert 6.6)

### Milestone 9.2: Wildcard Enhancements (Completed)
- [x] `WildcardConstraints` for regex pattern validation (Expert 1.4)
- [x] `pattern_to_regex` for file discovery with named captures (Expert 1.4)
- [x] `discover_wildcards_from_pattern` directory scanner (Expert 1.4)

### Milestone 9.3: CLI Enhancements (Completed)
- [x] `oxo-flow debug` subcommand for inspecting expanded commands (Expert 17.5)
- [x] Project root detection for `.oxoflow` file discovery (Expert 16.8)

### Milestone 9.4: Code Quality (Completed)
- [x] Enhanced rustdoc on lib.rs, error.rs, and public API types (Expert 6.10)
- [x] New error variants with richer context (Expert 6.6)
- [x] Additional re-exports from lib.rs for convenience (Expert 6)
- [x] RuleBuilder complete with all field setters (Expert 6.8)

### Milestone 9.5: Documentation (Completed)
- [x] Updated LIMITATIONS.md with all known limitations (Expert 24.7)
- [x] Updated ROADMAP.md with expert evaluation response phases
- [x] Troubleshooting guide in docs (Expert 29.7)

### Milestone 9.6: Future Work (Planned)
- [ ] Kubernetes operator / CRD for cloud-native deployment (Expert 14.2)
- [ ] Native S3/GCS object storage integration (Expert 14.4, 15.7)
- [ ] OpenAPI/Swagger specification for REST API (Expert 10.6, 30.5)
- [ ] Named inputs/outputs syntax for rules (e.g., `input.reads = [...]`)
- [ ] PDF export support for clinical reports
- [ ] CWL/WDL import/export converters (Expert 30.3)
- [ ] GA4GH TES/WES API compatibility (Expert 30.4)
- [ ] Interactive CLI wizard for new workflow creation (Expert 16.7)
- [ ] Web UI with DAG visualization (d3.js) (Expert 11.5)
- [ ] Webhook support for external notifications (Expert 10.10)
- [ ] Benchmark regression tracking in CI (Expert 7.5, 9.9)
- [ ] cargo-audit / cargo-deny integration (Expert 7.4)
- [ ] Code coverage reporting (Expert 9.4)
- [ ] Property-based testing with proptest (Expert 6.9)
- [ ] Fuzz testing for parser and wildcard engine (Expert 9.6)
- [ ] FHIR/HL7 report output format (Expert 30.6)
- [ ] Multi-tenancy and per-user resource quotas (Expert 18.4, 18.5)
- [ ] Job arrays for cluster backends (Expert 13.6)
- [ ] Node feature matching for HPC scheduling (Expert 13.4)
- [ ] Spot/preemptible instance support (Expert 14.10)

## Phase 10: Industrial-Grade Web UI System (Completed)

**Goal**: Transform oxo-flow-web into a multi-tenant, resource-aware scheduling portal.

### Milestone 10.1: Persistence Layer
- [x] SQLite integration with `sqlx`
- [x] User management schema (Viewer/User/Admin)
- [x] Execution history and job metadata persistence
- [x] Audit log implementation for host-level actions

### Milestone 10.2: Identity & Multi-Tenancy
- [x] OS account binding mechanism (Sudo/SSH)
- [x] Physical directory isolation (`workspace/users/<name>/runs/<id>/`)
- [x] Credential management (encrypted SSH keys)
- [x] Permission-aware file browser API

### Milestone 10.3: Resource Awareness
- [x] Real-time host metrics via `sysinfo`
- [x] HPC cluster polling (SLURM/PBS/SGE/LSF)
- [x] Smart queuing/blocking based on resource availability
- [x] Resource usage dashboards for Admins

### Milestone 10.4: Advanced Lifecycle UI
- [x] Dynamic DAG status coloring (Live WebSocket/SSE)
- [x] Real-time log streaming from backend to UI
- [x] Interactive task control (Pause/Stop/Checkpoint Resume)
- [x] Automated sample discovery from folders via Regex
- [x] Notification system (Email, Webhook)

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
