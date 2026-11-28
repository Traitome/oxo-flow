# oxo-flow Roadmap

> **Mission**: Build a Rust-native bioinformatics pipeline engine with a reproducibility-first design,
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

### Design Principles

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
- [ ] Poetry/uv support (not implemented — venv + `pip install -r requirements.txt` is the full Python venv surface)

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
- [ ] Shadow directories for isolation (never implemented; removed from scope)
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
- [x] Generic QC content types (`QcStatus`/`QcIndicatorGroup`) with a MultiQC-JSON rollup adapter (rule-specific coverage/quality panels remain open)
- [ ] Variant summary tables (no variant-specific section generator; generic table/metadata sections ship today)
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
- [x] Environment management endpoints (removed in the v0.8 legacy-handler refactor; environment info now flows through run/report and cluster endpoints)

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
- [x] `format_hint` on Rule parsed and schema-documented — parsed-but-inert: the engine performs no format-specific I/O and `lint` W038 flags every use (Expert 2.3, 5.1)
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
- [ ] Multi-tenancy (directory separation plus the dimensional #1129 OS boundary are landed: environment allowlist, Linux CPU/memory/process rlimits, and opt-in Landlock and per-run user + PID namespace modes (#1401); per-run uid/container isolation and default-on sandboxing remain open)
- [x] Job arrays for cluster backends (Expert 13.6) — SLURM/PBS/SGE/LSF array submission plus k8s/AWS/GCP array specs
- [ ] Node feature matching for HPC scheduling (Expert 13.4)
- [ ] Spot/preemptible instance support (Expert 14.10)

## Phase 10: Web UI Implementation Baseline

**Goal**: Transform oxo-flow-web into a multi-tenant, resource-aware scheduling portal.

**Status**: substantial implementation exists. The Phase 12 audit findings that originally blocked
the multi-tenant claim have since been addressed dimensionally: an opt-in OS-level tenant boundary
(environment allowlist always on; Landlock and per-run user + PID namespace modes, fail-closed),
atomic run admission, immutable run snapshots, and lifecycle/lease recovery — each with adversarial
or live evidence (see Stage 12.1). Production scale and multi-host behavior remain unproven.

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
- [x] Notification system (Webhook: Slack/Discord/custom endpoints, HMAC-SHA256 signatures; email is not implemented)

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
- [x] Full API coverage: 100+ OpenAPI-documented operations across 9 domains
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
| Unified run-ledger storage abstraction | Implemented and verified | Live PostgreSQL measured (issue #1251): N-way concurrent guarded inserts on one workdir key admit exactly one (`admitted=1`, `errors=0`, one row on disk) at N=16 and N=32, with N distinct keys admitting N (non-vacuity control); a writer killed mid-transaction leaves zero ghost rows and a committed admission stays visible to a fresh connection; two independent `PostgresBackend` instances on one database share the guard without double-claim (`workdir_admitted=1`, `pipeline_claims=1`) and agree on pagination totals. Each measurement was shown able to fail (committing before the kill surfaces a ghost row; dropping `idx_runs_live_workdir` admits >1 winner) (`crates/oxo-flow-web/tests/postgres_concurrency.rs`, harness `scripts/postgres-ledger-campaign.sh`; see [`docs/superpowers/audits/postgres-ledger-2026-11-19.md`](docs/superpowers/audits/postgres-ledger-2026-11-19.md)). Backend-equivalent admission verdicts are additionally asserted by the differential harness `crates/oxo-flow-web/tests/admission_backend_parity.rs` (issue #1312), which runs the SAME claim scenarios through the SAME `claim_run_row` against both backends and compares each verdict vector to one canonical contract (the PostgreSQL arm gates on `OXO_PG_TEST_URL`). **2026-11-26 #1402 campaign** ([`postgres-two-instance-2026-11-26.md`](docs/superpowers/audits/postgres-two-instance-2026-11-26.md); raw output [`data/postgres-1402-campaign-2026-11-26.log`](docs/superpowers/audits/data/postgres-1402-campaign-2026-11-26.log)): two instances on one host and two instances in separate Linux network namespaces share admission and the audit chain (10/20 and 10/10 admitted; byte-identical 40- and 70-event exports); a full outbound partition reports `/api/health` degraded in ~4 s (was 30 s) and a run-list `500 DB_ERROR` in ~5 s; connection exhaustion yields 8/10 structured 500s at ≤5.0 s (was 8/10 client timeouts); a clean restart and a `SIGKILL` crash during an uncommitted transaction both recover (uncommitted rows roll back, WAL recovery logged, same web process, live run completes); per-user quota is isolated across identities. Remaining exit evidence: separate physical hosts, production-scale row counts and sustained load, packet loss/latency/routing partitions, per-query network timeouts (a request already holding a black-holed connection can still block), database media corruption, and cross-host clock skew are NOT measured |
| Kubernetes and cloud batch executors | Prototype (split per backend) | The adapters shell out through `kubectl`, `aws`, `gcloud`, and `tccli`. **Kubernetes** is live-qualified for the tested scope: single-node k3s campaigns (2026-11-24: submit/status/artifacts/logs/SIGINT cancel and image-based environment rendering) and a two-node k3s + NFS campaign (2026-11-26: Indexed array settling per element across both nodes, a dependent rule forced onto the second node by resource fit, outputs returned through the shared volume, `provenance verify` 3/3) — see [`run-on-cluster.md`](docs/guide/src/how-to/run-on-cluster.md) and [`kubernetes-multinode-2026-11-26.md`](docs/superpowers/audits/kubernetes-multinode-2026-11-26.md). **Tencent Batch** has a 2026-11-26 live campaign ([`tencent-batch-live-2026-11-26.md`](docs/superpowers/audits/tencent-batch-live-2026-11-26.md)): named-env submit `SUCCEED`, state poll, `DescribeTaskLogs` decode, and `TerminateJob` cancel verified; the shared cloud workdir bug was fixed in PR #1421, and arrays require `[cluster.volume]` (fail closed without it). Array/artifact-pull evidence remains outstanding. **AWS Batch** and **GCP Batch** remain blocked at CLI wrappers with mock/wire tests only: the live campaign was not run (no AWS credentials; the GCP endpoint was unreachable from the qualification host), #1390 is closed as not planned for live, and the prerequisites/protocol are in [`cloud-live-campaigns-todo-2026-11-26.md`](docs/superpowers/audits/cloud-live-campaigns-todo-2026-11-26.md). Every cloud backend stays **Prototype** until the per-backend submit/queue/run/cancel/fail/retry/artifact campaign (CL-05/#1158) exists; residual Kubernetes gaps are PVC provisioning, HA/multi-control-plane, cloud-provider volumes, and image mode combined with a multi-node `[cluster.volume]` |
| Range I/O and object-storage staging | Implemented and verified | Measured 2026-11-19 (`docs/superpowers/audits/range-io-staging-2026-11-19.md`): streaming vs eager throughput/peak-RSS, interrupt/resume, bounded backpressure, and scratch footprint over a live S3-compatible store. Residual: WAN object store, GCS/Azure, multi-node, and network-partition campaigns |
| Hardware topology and storage-tier discovery | Prototype | Multi-socket affinity, NUMA locality, NVMe pressure, and fallback performance evidence |
| Team/HPC isolation and RBAC | Implemented but unproven | Dimensional OS boundary landed on the web spawn path (issue #1129): environment allowlist (always on), `RLIMIT_DATA`/`RLIMIT_CPU`/`RLIMIT_NPROC` ceilings (Linux, opt-in knobs), and an opt-in kernel Landlock sandbox (`OXO_FLOW_WEB_SANDBOX=landlock`, Linux 5.13+) that confines the spawned tree to the run workdir + an explicit grant set, denies TCP bind/connect on kernel 6.7+ (ABI v4), and scopes abstract UNIX sockets/signals on 6.10+/6.12+ — fail-closed when the requested sandbox cannot be honored. A second opt-in mode, `OXO_FLOW_WEB_ISOLATION=userns` (Linux), runs the CLI in a per-run user + PID namespace (root inside, server uid outside; a private `/proc`; the run cannot see or signal host/sibling processes, while host-side signals still reach it — that is how cancel works); its dimensions are adversarially tested through the real spawn path (`rb01_userns_adversarial.rs`) and were live-confirmed on a prepared host as uid 1000 ([`userns-isolation-2026-11-27.md`](docs/superpowers/audits/userns-isolation-2026-11-27.md)). The two-tenant adversarial suite (`crates/oxo-flow-web/tests/rb01_sandbox_adversarial.rs`, control run + sandbox run + real-CLI leg) proves every enforced dimension through the real spawn path; RB-01a's environment/rlimit dimensions are CI-verified. The suite was executed on the designated Linux host in the 2026-11-24 qualification (full `make ci` PASS, including `rb01_sandbox_adversarial`; [`linux-qualification-2026-11-24.md`](docs/superpowers/audits/linux-qualification-2026-11-24.md)). Remaining exit evidence: UDP/ICMP, per-run uid identities, and per-tenant containers remain unenforced (residual matrix in the web API reference); both kernel modes are OFF by default and mutually exclusive, so deployments that set neither keep the pre-boundary behavior |
| Immutable run history and atomic admission | Implemented and verified | Per-run immutable workdirs (#1130/RB-02: every run owns `users/<u>/runs/<run_id>/`; retry/resume are the only deliberate continuations) and fail-closed create/stage/spawn with compensation (#1131/RB-03: a pre-spawn failure releases the reservation, deletes the run row, and removes the workdir — `tests/pre_spawn_fault_no_spawn.rs`). Admission is one guarded `INSERT ... SELECT ... WHERE NOT EXISTS ... ON CONFLICT DO NOTHING` backed by partial unique indexes on SQLite and PostgreSQL, with live single-host concurrency evidence (`postgres_concurrency.rs`, N=16/32), backend-parity verdicts (`admission_backend_parity.rs`, #1312), and snapshot isolation/concurrency tests (`run_snapshot_isolation.rs`, `run_snapshot_concurrency.rs`). Monotonic compare-and-set lifecycle transitions and restart-safe cancellation are covered by #1256/#1257/#1260 tests (`process_control_integration::cancel_targets_persisted_cluster_after_registry_restart`, compare-and-set tests). Owner leases (`runs.owner_instance`/`heartbeat_at`, a 10 s heartbeat, 90 s TTL, and a 30 s reaper, issue #1402) reconcile a run whose web process died without a terminal write: the measured `kill -9` ghost row is now finalized automatically (`tests/run_lease_reaper.rs`), attributing the wrapper's `.exit-code` when the workdir is reachable (finished orphan → `completed`, still-running orphan → `failed`, quota released). Remaining exit evidence: separate physical hosts, production-scale load, per-query network timeouts, database media corruption, and cross-host clock skew are NOT measured |

### Stage 12.2: Biological Contracts & Human-Centered Visual Design

| Capability | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| Backwards-compatible BioTypes declarations and `TYPE-*` diagnostics | Implemented and verified (governed 83-case expected-reject set) | Governed campaign #1163/#1396, 2026-11-27: pre-registration amendment 2 fixed `τ_FA = 0.05` and `N_min = 73` before data; the 116 sealed cases (83 expected-reject + 24 authored controls + 9 pre-existing gallery controls) scored 33 TA / 0 FA / 83 TR / 0 FR, codes 116/116, exact 95% upper bound 4.35% ≤ `τ_FA` → POSITIVE ([report](docs/superpowers/audits/biotype-truth-set-governed-2026-11-27.md); [erratum](docs/superpowers/audits/biotype-truthset-governed-erratum-2026-11-28.md)). The result is **static `validate`-surface validation only, single arm** — not execution evidence or biological truth. Remaining exit evidence: independent reanalysis (PR-01 §0 rule 5) and PR-05 reviewer sign-off PENDING; PR-05 must adjudicate the ≥30 control floor (24 held-out authored controls, 9 pre-existing) and the pilot-disjointness interpretation; biological truth and real-pipeline false-positive analysis beyond the sealed corpus; Gate 3 remains unpassed |
| Cohort and metadata contracts (`[workflow.metadata_schema]`, `META-*`) | Implemented and verified | Deterministic harness (N=18 implementer-authored, non-independent fixture workflows through the real `validate --json` surface, 13 of them pinning full `(code,severity,row,column,id)` tuples; CSV/TSV/JSON of one logical table and duplicate-id row shuffles produce one byte-identical typed-canonical digest) and a fixed-seed 10k/100k-row linearity check (100k `evaluate` ≈ 0.59 s and `digest` ≈ 0.59 s in debug) are in-tree (`tests/metadata_contract.rs`, `crates/oxo-flow-core/src/config/metadata/`; [`docs/superpowers/audits/metadata-contracts-2026-11-25.md`](docs/superpowers/audits/metadata-contracts-2026-11-25.md)). Remaining exit evidence: independent (non-implementer) cases and real-world cohort sheets, per-column validators beyond type/regex/enum, and web-admission parity (the web path currently gates through the spawned CLI, not at admission) |
| Header inspection and scientific preflight | Implemented and verified | Evidence landed (issue #1244): all 51 curated `SCI-*` rules carry a checkable authority (no `UNVERIFIED`; five engine-invariant rules cite the oxo-flow spec by path), an adversarial fixture table (`crates/oxo-flow-core/tests/scientific_preflight_adversarial.rs`) asserts positive + corrected-silent + mention-only-silent per rule, and preflight latency is measured (about 13 ms for 200 rules x 1000 samples). See [`docs/superpowers/audits/scientific-preflight-2026-11.md`](docs/superpowers/audits/scientific-preflight-2026-11.md); `SCI-MUTECT2-NO-GERMLINE` is positive-only (see its `NOT_ADVERSARIALLY_EXERCISED` entry) |
| Hybrid re-entry and `[[values]]` expansion | Implemented and verified | Cohort replay measured: N=200 × M=3 → 1609 instances, cold 166.95 s vs resume 0.59 s (283.3×), cold == resume identity set and repeated resume stable (fast CI proof `tests/reentry_cohort_replay.rs`, harness `scripts/reentry-cohort-replay.sh`); adversarial crash matrix all **fail-safe** — SIGTERM (exit 143, in-flight rule failed → re-run), SIGKILL (stale `running` cleared → re-run), truncated checkpoint (Execute refuses, preview degrades), unknown-vs-missing required field (harmless vs refuse), and a NEWER `checkpoint_format_version` (Execute refuses — `run`, `--resume-failed`, standalone `resume` before its banner; preview degrades; `status` warns; issue #1309) (`tests/reentry_crash_matrix.rs`; `docs/superpowers/audits/reentry-2026-11.md`). Remaining exit evidence: real power-cut mid-rename and a genuine two-binary cross-version checkpoint matrix are NOT_VALIDATED (audit §5.3) |
| Metropolis design tokens and Metro-DAG renderer | Prototype | Accessibility, semantic safety, mobile, 5,000-10,000-node performance, and visual regression tests |
| Guided editor and cross-surface state | Prototype | Round-trip semantic preservation on unrepresentable constructs is the remaining exit evidence. Proposal diff acceptance (#1147), identity-scoped drafts with fail-closed account confirmation (#1144, `frontend/e2e/identity-scoped-drafts.spec.ts`), and fail-closed validation of stale edits (#1145, `editor-validation-failclosed.spec.ts`) are implemented and covered by real-server e2e tests |

### Stage 12.3: Ecosystem Interoperability & Bounded Remediation

| Capability | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| Nextflow/Snakemake import (+ Nextflow DSL2 export, #1319) | Prototype | Representative corpus coverage and round-trip review; **declared-artifact equivalence measured** over all 27 `tests/fixtures/import/**` fixtures (matched 8 / drifted 0 / importer-rejected 19 / not-attempted 0; `docs/superpowers/audits/import-equivalence-2026-11-19.md`, harness `scripts/import-equivalence.sh`) with unsupported-construct reporting verified to name documented out-of-subset constructs. Export round trip (`oxo-flow export-dsl`, `tests/transpile_roundtrip.rs`): 5 accepted fixtures (4 Nextflow + the Snakemake fixture without unexportable fields) export and re-import byte-identically, the 3 accepted Snakemake fixtures declaring `[wildcard_constraints]`/`benchmark`/`log` are export-refused naming their constructs, 19 importer-rejected fixtures have no model to export. Remaining exit evidence: execution equivalence measured for a first 3-fixture native triplet (matched 3 / drifted 0 / skipped 0; `docs/superpowers/audits/execution-equivalence-2026-11-21.md`) and a first real-world corpus — an nf-core `samtools/faidx` module and a community Snakemake tutorial rule both executed SOURCE vs IMPORTED with byte-identical declared outputs (executed 2 / matched 2 / drifted 0 / skipped 6 with reasons; `docs/superpowers/audits/import-execution-realworld-2026-11-27.md`, harness `scripts/import-execution-equivalence.sh`); a corpus-wide, multi-tool, multi-host campaign remains |
| Causal fault diagnosis | Prototype | Seeded fault-injection campaign (13 cases: the 10-case baseline plus three held-out repairable classes added by #1278): 7/13 static detection, 10/13 diagnostic attribution, 10/13 repair proposal, 13/13 executability (`eval/frontier/PILOT_FINDINGS.md`) |
| Repair application and successful rerun | Prototype | Seeded campaign (v0.32.0 verified-repair loop, #1187 fence corrective + #1278 held-out repairable classes): 6/13 applied, 6/13 verified, 6/13 rerun successfully — all 6 repairable cases (the 3 original infra seeds plus 3 distinct classes added by #1278: cpu_limit, ulimit, shared_library); the 7 non-repairable cases are blocked upstream (`eval/frontier/PILOT_FINDINGS.md`). Remaining exit evidence: broader coverage beyond the seeded repairable classes on real (non-seeded) faults and larger N |
| CLI/Web/VS Code remediation continuity | Prototype | The reviewed patch now applies, verifies, and audits on every surface: the CLI `run --apply-patch` seam (#1308), the web `POST /api/runs/{id}/apply-repair` route + `GET /api/runs/{id}/repair` record reader, and the VS Code `oxo-flow.applyRepair` command shell the same CLI verified-repair path. The cross-surface parity harness (`tests/remediation_continuity.rs`) proves byte-identical applied workflow + identical `REPAIR_*` checkpoint outcome across the three surfaces (the extension leg replays the CLI path under the extension's exact argv — the extension shells the CLI by construction, and its argv shape is pinned by its own unit tests). Remaining exit evidence: adoption on real (non-seeded) repair workflows and the SPA affordance for pasting a reviewed patch |

### Stage 12.4: Empirical Science, Human Factors & Publication

| Study | Maturity | Remaining exit evidence |
| :--- | :--- | :--- |
| AI synthesis and fault-injection benchmark | Operational field evidence (pre-registered execution on the development intent set; held-out campaign outstanding) | A pre-registered 36-intent × 3-arm × 3-seed execution (324 cells, 2026-11-28) measured Arm A `unconstrained` 0/108, Arm B `syntax-only` 92/108 (85.2%), and Arm C `full-contract` 49/108 (45.4%) intention-to-treat; Edwards+Holm B vs C p=9.0e-10, exact sensitivity p=1.6e-11. All 59 C failures are ONE contiguous provider transport outage (zero tokens, ~7.1 s each, never reached the model), so the ITT B > C is not a contract-feedback effect; the post-hoc per-protocol matched subset is null (B 47/48 vs C 48/48, p=1.0) and C costs roughly 1.9× input / 2.5× output tokens and 1.8× wall time on those cells. The intents are **not held out**; an independent post-hoc audit found 15 specified SOUND + 3 vague SOUND, 10 AMBIGUOUS, 6 GATE-UNFAIR, 1 FLAWED (`wgbs-bismark`, including a gate-passing bwa-mem → bismark_extractor counterexample), and 1 DUPLICATE (`chipseq-broad`). Evidence: [`ai-synthesis-ablation-governed-2026-11-28.md`](docs/superpowers/audits/ai-synthesis-ablation-governed-2026-11-28.md) plus the sanitized 324-cell manifest, four-population statistics, mechanism evidence, and intent audit. Remaining: held-out intent set v2, gate/fidelity corrections, the vague double-count fix, multi-model/multi-provider runs, and a stable-provider or interleaved-arm design; no publication decision is claimed. |
| Cross-architecture reproduction | Governed campaign measured (native x86_64 vs native aarch64) | PR-06 §C4 amendment 6 froze `τ_ARCH = 0.80`/`n_min = 3` before data collection: two deterministic workflows, 24 declared artifacts in eight classes of three, one pinned multi-arch manifest-list digest, native `linux/amd64` (`tx-ubuntu`) vs native `linux/arm64` (Apple Silicon Docker Desktop VM, no qemu), excluded `uname -m` negative control. Result: **24/24 SHA-256 identical** (100%, exact 95% CI [85.75%, 100%]), eight/eight classes stable, category POSITIVE; the negative control differed. Evidence: [`cross-arch-provenance-governed-2026-11-27.md`](docs/superpowers/audits/cross-arch-provenance-governed-2026-11-27.md). Remaining exit evidence: PR-05 independent sign-off (PENDING), independent replication, more workflows/tool versions, a third architecture (RISC-V), other environment backends, and the environment-lock matrix with differing tool versions |
| Cohort and hardware scalability | Operational field evidence only; no controlled benchmark | The cluster path is exercised by live production Slurm deployments. Crash-restart (#1083), cluster observability (#1085), dispatch-order (#1089), and CPU-oversubscription (#1080) findings were addressed with regression tests on `main`. Still required: a completed production campaign, measured throughput/cost data, and pre-registered scalability campaigns on named hardware |
| Human-factors study | Simulation only | Ethics approval, real participants, randomized task order, and complete SUS/TLX analysis |
| Manuscript and artifact package | Planned / blocked | No journal submission, acceptance, independent replication, or journal-ready package exists; a bioRxiv preprint is cited in the README |

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
