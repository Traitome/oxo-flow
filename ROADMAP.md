# oxo-flow Roadmap

> **Mission**: Build a Rust-native bioinformatics pipeline engine with first-principles design,
> reporting designed for clinical rigor, environment management, and a full-featured web interface.
>
> Licensing is per crate: the core, AI, CLI, and desktop components are
> Apache-2.0; the web crate uses separate Academic/Commercial terms. See the
> licensing table in README.md.
>
> **Roadmap status**: Phases 1–11 preserve historical implementation milestones. A checked item
> records that work landed; it is not evidence that the current security, release, scientific, or
> user-experience gate has passed. The audit ledger linked from Phase 12 is authoritative for
> maturity and release blockers.

---

## Role-Based Evaluation Synthesis

The following evaluation is a role-based design review: the 30 roles are analytical
lenses, not evidence that 30 independent domain experts reviewed or validated the
implementation. The table records recommendations and hypotheses that must be tested
against code, users, and domain truth sets.

### Role-Based Review Summary

| # | Role | Key Recommendation |
|---|------|--------------------|
| 1 | Bioinformatics Expert | Prioritize wildcard compatibility and file-based dependency resolution |
| 2 | Tumor Bioinformatics Expert | The pipeline must support tumor-only, normal-only, and paired modes with clinical annotations |
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
| 21 | Regulatory Affairs Specialist | Audit trails must be tamper-evident; pipeline runs need immutable logs for FDA 21 CFR Part 11 |
| 22 | Single-Cell Bioinformatician | Must handle millions of cells efficiently; scatter-gather with chunked processing essential |
| 23 | Database Engineer | SQLite for persistence is appropriate at current scale; design schema for future migration |
| 24 | Rust Performance Engineer | Avoid unnecessary clones; use zero-copy parsing; pre-allocate buffers in hot paths |
| 25 | MLOps Engineer | Pipeline artifacts need versioning and lineage tracking for model reproducibility |
| 26 | Clinical Laboratory Director | Turnaround time matters; parallel execution of independent samples is critical for clinical throughput |
| 27 | Open-Source Community Manager | Clear CONTRIBUTING.md, responsive issue triage, and semantic versioning build trust |
| 28 | Accessibility Engineer | CLI output should support screen readers; web UI needs WCAG 2.1 AA compliance |
| 29 | Financial Analyst | Cost tracking per rule enables budget-aware scheduling; resource waste should be minimized |
| 30 | Educator/Trainer | Progressive learning path from simple to complex workflows; gallery examples are key |

### Consensus Design Decisions

1. **Environment trait with async support** — `EnvironmentBackend` trait provides detect/create/activate/run interface; resolver selects backend per rule
2. **Resource-aware scheduler** — Track CPU, memory, GPU availability; only dispatch jobs when resources are available
3. **Template-based reporting** — Tera engine for HTML reports with CSS styling; JSON for programmatic access
4. **REST API design** — CRUD for workflows, runs, reports; SSE for real-time execution monitoring
5. **Container-first packaging** — Generate Dockerfile/Singularity definitions from workflow specs automatically
6. **Reference pipeline** — A clinical tumor variant calling workflow is used as a worked example; it did not validate the entire engine stack

---

## Architecture Overview

### Design Principles (First Principles Thinking)

1. **DAG is the fundamental abstraction** — Every bioinformatics workflow is a directed acyclic graph of tasks. The engine must natively construct, validate, optimize, and execute DAGs with maximum parallelism.

2. **Environment isolation is non-negotiable** — Bioinformatics tools have conflicting dependencies. Each task must run in its own isolated environment (conda, mamba, pixi, docker, singularity, venv, modules, system).

3. **Reproducibility through determinism** — Given the same inputs, configuration, and environment specifications, the pipeline must produce identical outputs. Lock files, checksums, and container pinning ensure this.

4. **Inverse design** — Start from what the user needs (clinical report) and work backward to determine what data, tools, and steps are required. The engine supports both forward (input→output) and reverse (output→input) dependency resolution.

5. **Performance through Rust** — Zero-cost abstractions, fearless concurrency, and efficient memory management make Rust ideal for orchestrating thousands of concurrent bioinformatics tasks.

6. **Clinical rigor by design** — Reports must be accurate, traceable, and auditable. Every step should log its provenance, inputs, outputs, software versions, and execution environment. Clinical qualification requires separate evidence and is not implied by these design goals.

### System Architecture

```
┌──────────────────────────────────────────────────────┐
│                    oxo-flow CLI                       │
│  (run | dry-run | validate | graph | report | env |  │
│   export | serve | init | status | clean | config)  │
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
│  │ Engine  │ │(resource │ │  (HTML/JSON   │         │
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
environment = { conda = "envs/fastp.yaml" }
shell = """
fastp -i {input[0]} -I {input[1]} \
      -o {output[0]} -O {output[1]} \
      --json {output[2]} --thread {threads}
"""
[rules.resources]
threads = 8
memory = "16G"

[[rules]]
name = "bwa_mem2_align"
input = ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.sorted.bam"]
environment = { singularity = "docker://biocontainers/bwa-mem2:2.2.1" }
shell = """
bwa-mem2 mem -t {threads} {config.reference} {input[0]} {input[1]} \
  | samtools sort -@ 4 -o {output[0]}
"""
[rules.resources]
threads = 16
memory = "32G"
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

**Goal**: Full support for all eight environment backends: conda, mamba, pixi, docker, singularity, venv, modules, system.

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

**Goal**: Modular reporting designed for clinical review. Clinical
qualification requires a separate evidence and governance program.

### Milestone 4.1: Report Engine
- [x] Structured report generation from the run checkpoint (Tera templates were archived in the issue #83 redesign)
- [x] HTML report output with embedded styles
- [x] JSON structured report output
- [x] Report builder pattern for composable reports

### Milestone 4.2: Report Components
- [x] QC metrics panels (coverage, quality scores)
- [x] Variant summary tables
- [x] Execution provenance section
- [x] Key-value metadata sections

### Milestone 4.3: Clinical Review Report Foundations
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
- [x] `oxo-flow package` command (merged into `export` as `-f compose`, issue #76)
- [x] Dockerfile generation from .oxoflow
- [x] Singularity definition file generation
- [x] Multi-stage build optimization
- [x] Reference data bundling options

### Milestone 6.2: Portable Execution
- [x] Self-contained container images
- [x] Label metadata for OCI compliance
- [x] Configurable base images

---

> **Note on numbering** — there is no Phase 7: that number was assigned to
> the Venus pipeline (removed from the repository in commit 287447cf), and
> the remaining phases keep their original numbers so cross-references
> elsewhere (e.g. "Phase 10.4", "Phase 10") stay valid.

---

## Phase 8: Production Hardening Foundations

**Goal**: establish baseline packaging, documentation, and tests. Production readiness is governed by the evidence gates in Phase 12, not by this historical phase heading.

### Milestone 8.1: Testing
- [x] Unit test coverage across all modules
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
- [x] Named inputs/outputs syntax for rules (e.g., `input.reads = [...]`)
- [x] cargo-audit integration verified working (no security vulnerabilities found)
- [x] Performance optimizations: LazyLock for interpreter map, serde_json for event logging
- [x] Webhook support for external notifications (Slack, custom endpoints, HMAC signatures)
- [x] PDF export via wkhtmltopdf — dropped from the issue #83 report redesign, then re-introduced in 0.16 as `-f pdf`/`-f pdf-command` (shells out to wkhtmltopdf)
- [x] Code coverage reporting in Makefile (cargo-tarpaulin target)
- [x] Benchmark regression tracking in Makefile (cargo bench target)
- [ ] Kubernetes operator / CRD for cloud-native deployment (Expert 14.2)
- [ ] CWL/WDL import/export converters (Expert 30.3)
- [ ] GA4GH TES/WES API compatibility (Expert 30.4)
- [ ] Interactive CLI wizard for new workflow creation (Expert 16.7)
- [x] Web UI with DAG visualization (Phase 10.4)
- [ ] Property-based testing with proptest (Expert 6.9)
- [ ] Fuzz testing for parser and wildcard engine (Expert 9.6)
- [ ] FHIR/HL7 report output format (Expert 30.6)
- [ ] Multi-tenancy (Phase 10 provides directory separation, but no OS-level tenant boundary; per-user resource quotas remain open)
- [ ] Job arrays for cluster backends (Expert 13.6)
- [ ] Node feature matching for HPC scheduling (Expert 13.4)
- [ ] Spot/preemptible instance support (Expert 14.10)

## Phase 10: Web UI Implementation Baseline

**Goal**: Transform oxo-flow-web into a multi-tenant, resource-aware scheduling portal.

**Status**: substantial implementation exists, but the industrial and multi-tenant claims remain
blocked by the findings in the Phase 12 audit ledger: no OS-level tenant execution boundary,
non-atomic run admission, mutable run snapshots, and incomplete lifecycle recovery.

### Milestone 10.1: Persistence Layer
- [x] SQLite integration with `sqlx`
- [x] User management schema (Viewer/User/Admin)
- [x] Execution history and job metadata persistence
- [x] Audit log implementation for host-level actions

### Milestone 10.2: Identity & Multi-Tenancy
- [x] Sudo/SSH execution plumbing exists
- [x] User-scoped directory namespaces exist (`workspace/users/<name>/runs/<id>/`)
- [x] Credential storage exists (SSH keys are sealed only when `OXO_FLOW_MASTER_KEY` is configured)
- [x] Permission-aware file browser API

These are implementation primitives, not a tenant security boundary. They do not
establish OS-level account binding for every execution path, resource isolation,
credential scrubbing, or protection from untrusted workflow authors.

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

## Phase 11: v0.8 — Domain-Driven Architecture & AI Integration

**Goal**: Refactor to a deterministic core with AI translation layer, collaboration primitives, and three-mode deployment.

### Phase 11.1: Deterministic Core
- [x] Domain-driven module structure (8 domains + infra layer)
- [x] `StorageBackend` trait + SQLite backend + PostgreSQL backend (feature-gated)
- [x] Pipeline lifecycle API (parse, validate, prepare, DAG, format, lint, stats, diff, export, search)
- [x] Execution API (runs CRUD, status, DAG status, diagnostics, logs, results, retry, cancel)
- [x] Data discovery API (file analysis, reference genome discovery)
- [x] Diagnostics engine with 30+ error patterns
- [x] Three-layer logging (structured events, human-readable, audit)
- [x] RunNode tracking per DAG node
- [x] Smart retry via DAG dependency analysis
- [x] Health check endpoint
- [x] `--mode personal|team|hpc` CLI flag with mode-specific behavior
- [x] ORCID/GitHub OAuth2 authentication module
- [x] License visibility (banner, web footer, API response header)

### Phase 11.2: AI Translation Layer
- [x] `/api/ai/translate` — NL intent → validated .oxoflow pipeline
- [x] `/api/ai/explain` — failure diagnosis with human-readable explanations
- [x] `/api/ai/interpret` — result interpretation with caveats
- [x] `/api/ai/optimize` — parameter optimization
- [x] AI provider architecture with explicit provider selection and credential-based auto-detection; provider behavior is not treated as a silent quality fallback
- [x] AI service zero-write-permission enforcement

### Phase 11.3: Collaboration & Multi-Mode
- [x] Fork, diff, share, import collaboration primitives
- [x] `oxo+https://` import/export protocol
- [x] HPC submission backend (SLURM, PBS)
- [x] Three-mode deployment (personal/team/hpc)
- [x] Full API coverage: 50+ endpoints across 8 domains
- [x] Test suite expanded substantially; exact pass status must be recorded for the evaluated commit because this historical checklist is not current CI evidence

---

## Phase 12: Research Program Toward Industrial and Publication-Grade Workflow Science

**Status**: staged research program. No Stage 12 exit gate has passed.

**Goal**: establish a defensible, evidence-driven path toward a secure industrial workflow engine and a publication-tier scientific contribution. The target includes *Nature Biotechnology* / *Nature Methods*, but acceptance is not assumed and no manuscript claim is currently supported.

The program must co-design hardware, software, visual design, and user experience. Existing code, prototypes, projections, and simulations are starting points, not completion evidence.

Supporting documents:

- Audit evidence and release blockers: [`docs/superpowers/audits/industrial-scientific-audit-ledger.md`](docs/superpowers/audits/industrial-scientific-audit-ledger.md)
- Research proposal and architectural specification: [`docs/superpowers/specs/industrial-scientific-roadmap.md`](docs/superpowers/specs/industrial-scientific-roadmap.md)
- Implementation and scientific execution protocol: [`docs/superpowers/plans/industrial-scientific-implementation-plan.md`](docs/superpowers/plans/industrial-scientific-implementation-plan.md)

Maturity labels used below:

- **Implemented and verified**: exercised by deterministic tests or measured evidence.
- **Implemented but unproven**: code exists, but production-scale or scientific evidence is absent.
- **Prototype**: usable path with material semantic or operational gaps.
- **Simulation only**: model, fixture, mock, or synthetic result.
- **Planned / blocked**: not implemented or dependent on a release blocker.

### Stage 12.1: Truthful Execution, Persistence & Safe Operations

| Capability | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| Unified run-ledger storage abstraction | Implemented but unproven | Live PostgreSQL parity, concurrency, crash recovery, and horizontal-instance tests |
| Kubernetes and cloud batch executors | Prototype | They currently shell out through `kubectl`, `aws`, and `gcloud`; live submit/run/cancel/retrieve campaigns are required |
| Range I/O and object-storage staging | Implemented but unproven | End-to-end streaming, interruption, backpressure, and scratch-footprint measurements |
| Hardware topology and storage-tier discovery | Prototype | Multi-socket affinity, NUMA locality, NVMe pressure, and fallback performance evidence |
| Team/HPC isolation and RBAC | Prototype / blocked | No OS-level tenant execution boundary; secrets and host files are not isolated |
| Immutable run history and atomic admission | Planned / blocked | Unique artifact snapshots, atomic create/stage/spawn, and truthful lifecycle transitions |

### Stage 12.2: Biological Contracts & Human-Centered Visual Design

| Capability | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| Backwards-compatible BioTypes declarations and `TYPE-*` diagnostics | Implemented but unproven | Broad biological truth-set validation and false-positive analysis |
| Header inspection and scientific preflight | Implemented but unproven | Literature-backed rule review, adversarial fixtures, and measured latency |
| Hybrid re-entry and `[[values]]` expansion | Implemented but unproven | Deterministic replay at cohort scale and crash/interruption tests |
| Metropolis design tokens and Metro-DAG renderer | Prototype | Accessibility, semantic safety, mobile, 5,000-10,000-node performance, and visual regression tests |
| Guided editor and cross-surface state | Prototype / blocked | Round-trip semantic preservation, explicit diff acceptance, tenant-scoped drafts, and stale-validation blocking |

### Stage 12.3: Ecosystem Interoperability & Bounded Remediation

| Capability | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| Nextflow/Snakemake import | Prototype | Representative corpus coverage, semantic equivalence tests, unsupported-construct reporting, and round-trip review |
| Causal fault diagnosis | Prototype | Current CLI simulation: 8/10 detected, 7/10 attributed, 7/10 proposed |
| Repair application and successful rerun | Planned / blocked | Current CLI simulation: 0/10 applied, 0/10 verified, 0/10 rerun successfully |
| CLI/Web/VS Code remediation continuity | Prototype | The exact approved patch must be applied, verified, and audited; currently UI actions and validation state can diverge |

### Stage 12.4: Empirical Science, Human Factors & Publication

| Study | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| AI synthesis and fault-injection benchmark | Simulation only | Matched Arms A/B/C, held-out intents, multiple seeds, run-level data, and pre-registered analysis |
| Cohort and hardware scalability | Simulation or smoke test only | Measured campaigns on named hardware; current cohort evidence is a 10-sample, 30-node CLI pilot |
| Human-factors study | Simulation only | Ethics approval, real participants, randomized task order, and complete SUS/TLX analysis |
| Manuscript and artifact package | Planned / blocked | No submission, acceptance, independent replication, or journal-ready package exists |

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
| Testing | cargo test + integration tests |
| CI/CD | GitHub Actions |
| Documentation | MkDocs + rustdoc |
| License | Per crate: Apache-2.0 except `oxo-flow-web` (Academic/Commercial) |
