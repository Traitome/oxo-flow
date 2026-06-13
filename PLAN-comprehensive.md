# Comprehensive Audit & Development Plan: oxo-flow → AI-Native Pipeline Platform

## Current State Assessment

After thorough examination of all three crates (`oxo-flow-core`, `oxo-flow-cli`, `oxo-flow-web`), here's where we stand:

---

## Audit Summary

### ✅ What's Already Strong

| Layer | Strength |
|-------|----------|
| **Core** | DAG engine with topological sort, cycle detection, DOT export |
| **Core** | Full environment support: conda, pixi, docker, singularity, venv, HPC modules |
| **Core** | Wildcard expansion engine (`{sample}`, `{chr}` patterns) |
| **Core** | Rule model with scatter/gather, transform, checkpoint, shadow, when conditions |
| **Core** | Resource scheduling (CPU, memory, GPU spec) |
| **Core** | Checkpoint/restart with output verification |
| **Core** | Storage backends (local, S3, GCS) |
| **Core** | Report generation, webhook integration, shell security |
| **CLI** | 20+ commands covering run, validate, lint, graph, init, resume, clean, batch, publish |
| **CLI** | Batch execution with parallel jobs |
| **CLI** | Checkpoint-based resume with output existence verification |
| **CLI** | Profile support for environment/config overrides |
| **CLI** | Container packaging |
| **Web** | 30+ REST endpoints covering the full lifecycle |
| **Web** | SQLite persistence (users, runs, templates, saved workflows, scheduled runs, audit logs) |
| **Web** | Session-based auth with roles (admin, user) |
| **Web** | SSE real-time event streaming |
| **Web** | HPC scheduler detection (SLURM, PBS, LSF, SGE) |
| **Web** | Rate limiting, audit logging, workspace sandboxing |
| **Web** | License enforcement (oxo-license) |

### 🚩 Gaps by Layer

#### Core Gaps

| Gap | Impact | Description |
|-----|--------|-------------|
| No structured output model | **Critical** | Results are files. No way to query "which sample failed QC" across runs. The checkpoint tracks completion but not content. |
| No intent-to-pipeline API | **Critical** | Users must write TOML. No way to take a natural-language description and generate a validated pipeline. |
| No cross-run comparison | **High** | Each run is isolated. No endpoint to diff outputs or compare QC metrics across runs. |
| No "data product" concept | **High** | Everything is file-based. No concept of a published dataset with metadata and versioning. |
| No semantic annotation model | **Medium** | Rules know about inputs/outputs but can't express "this BAM file has GRCh38 alignment". |

#### CLI Gaps

| Gap | Impact | Description |
|-----|--------|-------------|
| No machine-readable output | **Critical** | Everything goes to stderr with colored formatting. No `--json` flag on most commands. An AI agent can't parse the output. |
| No "generate" command | **Critical** | Can't ask the CLI to generate a pipeline from a description. |
| No remote API client mode | **High** | CLI is standalone only. Can't talk to a running web server (`oxo-flow remote submit`). |
| No structured results view | **High** | Results are terminal-only. No `oxo-flow results` command to show structured output as JSON/CSV. |
| No "suggest" mode | **Medium** | Can't say "I have these FASTQ files, what pipeline should I run?" |

#### Web Gaps

| Gap | Impact | Description |
|-----|--------|-------------|
| **No frontend UI** | **Critical** | The only route is a placeholder `fn frontend()` returning "UI coming soon". Zero visual interface. |
| No intent-to-pipeline endpoint | **Critical** | `/api/workflows/validate` and `parse` exist, but no `/api/workflows/generate` that takes intent. |
| No structured results endpoints | **Critical** | `/api/runs/{id}` gives status but no structured output data. Results are files you must go find on disk. |
| No run comparison | **High** | No `/api/runs/compare` to diff two runs' outputs, parameters, or provenance. |
| No AI-friendly error format | **High** | Errors are free-text strings, not structured `{code, message, detail, suggestion}`. |
| No API schema/documentation | **Medium** | No OpenAPI spec, no `/api/openapi.json` for agent discovery. |
| No dataset catalog | **Medium** | No concept of datasets that can be shared, versioned, or discovered. |
| No collaboration features | **Medium** | Multi-user exists but no pipeline sharing, forking, or commenting. |
| No webhook event system | **Medium** | SSE exists for live UI, but no webhooks for external integration (Slack, Teams, CI/CD). |
| No output data indexing | **Medium** | Pipeline outputs are opaque files. No extraction or indexing of QC metrics, variant calls, etc. |
| No search over workflows | **Low** | Can list saved workflows by name, but no semantic or full-text search over pipeline content. |

### Cross-Cutting Architecture Gaps

1. **No unified data model for pipeline results** — The engine produces files; the web stores status; there's nothing connecting "this VCF file" to "this sample" to "this run" in a structured way.
2. **No agent-native interface** — No endpoints specifically designed for AI consumption: clean JSON-only responses, structured error codes, `/api` discovery endpoint.
3. **No LLM integration surface** — No `GET /api` with link relations (HATEOAS), no streaming-compatible response patterns beyond SSE, no vector-search for pipelines.
4. **Runtime vs platform separation is unclear** — The web crate acts as both API layer and runtime (spawning background processes). This works but creates tight coupling.

---

## The North Star: An AI-Native Pipeline Platform

Based on the six principles we agreed on, here's what we're building toward:

### Principle 1: Intent-Driven Pipeline Authoring

**What it means:** Users describe *what* they want to accomplish, not *how* to configure TOML. The AI generates, validates, and explains the pipeline. The TOML is the compilation target, not the authoring surface.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Core** | `PipelineIntent` struct + `intent_to_rules()` | A type that captures user intent (sample types, tools, reference genomes) and maps it to a set of Rule objects |
| **CLI** | `oxo-flow generate "description"` | CLI command that takes natural language and outputs a validated `.oxoflow` file |
| **Web** | `POST /api/workflows/generate` | API endpoint that accepts an intent description and returns a validated pipeline |
| **Web** | `GET /api/workflows/generate/stream` | SSE endpoint that streams pipeline generation steps in real time |
| **Core** | Template-based suggestion engine | Given input files and their types, suggest applicable pipeline templates |

### Principle 2: The DAG as the Primary Interaction Surface

**What it means:** A visual, interactive graph where users can drag nodes, connect inputs to outputs, click a rule to see its environment/resources/parameters, and see live validation coloring. The graph *is* the workspace.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Web** | Frontend SPA (React/TypeScript) | Interactive DAG editor as the main app interface |
| **Web** | `DagNodeDetail` response type | Rich node data: environment, resources, input/output files, dependencies, when conditions |
| **Web** | WebSocket or SSE for collaborative editing | Real-time sync when multiple users work on the same pipeline |
| **Web** | Live DAG updates | Edit → re-validate → re-render DAG in real time (client-side) |
| **CLI** | `oxo-flow web` | Single command to start the web UI (already exists but serves a placeholder) |

### Principle 3: AI-Native API Design

**What it means:** Every endpoint is designed for agent consumption as much as browser consumption: clean JSON, streaming for long operations, structured error responses, discoverable API schema.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Web** | Structured `ApiError` | Error responses with `{code, message, detail, suggestion}` fields |
| **Web** | `/api/openapi.json` | Auto-generated OpenAPI 3.0 spec for agent discovery |
| **Web** | `/api` root with link relations | Discoverable API entry point listing all available resources |
| **Web** | Streaming for all long ops | `/api/workflows/generate/stream`, `/api/workflows/run/stream` |
| **Web** | Pagination envelope for all list endpoints | Consistent `{data, meta: {total, page, per_page}}` |
| **CLI** | `--json` flag on all commands | Machine-readable output for every CLI command |
| **CLI** | `oxo-flow api` remote mode | CLI can act as a client to a running web server |

### Principle 4: Results as Queryable Data

**What it means:** Pipeline outputs aren't just files — they're structured data you can query, compare, and visualize. "Show me all samples that failed QC" is a single API call, not a grep expedition.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Core** | `OutputRecord` / `RunResult` data model | Structured representation of pipeline outputs: metrics, file hashes, sample metadata |
| **Core** | Result extraction API per rule type | Hooks for rules to declare structured output formats (QC json → parsed metrics, VCF → variant count) |
| **Web** | `/api/runs/{id}/results` | Structured output data endpoint with filtering |
| **Web** | `/api/runs/{id}/results/{rule}` | Per-rule result endpoint (e.g., QC metrics for the `fastqc` rule) |
| **Web** | `/api/runs/compare` | Side-by-side comparison of two runs' outputs |
| **Web** | `/api/datasets` | Dataset catalog: publish pipeline outputs as queryable datasets |
| **CLI** | `oxo-flow view run-id --format json` | View structured results from the terminal |
| **CLI** | `oxo-flow compare run-a run-b` | CLI-based run comparison |

### Principle 5: Shareable, Forkable, Citable Pipelines

**What it means:** A pipeline is a URL, not a file. Share it, fork it, get a DOI for it. Each run has a persistent URL with full provenance.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Web** | Pipeline sharing | Share workflows between users with view/edit permissions |
| **Web** | Pipeline forking | "Clone this pipeline" creates a user-owned copy |
| **Web** | Run detail page (permanent URL) | `/runs/{id}` with full provenance, parameters, results |
| **Web** | Public/private visibility | Opt-in public pipelines with stable URLs |
| **Web** | Collaborative editing | Multiple users editing the same pipeline DAG simultaneously |
| **Core** | Pipeline checksum / versioning | Content-addressed pipeline versions for reproducibility |
| **CLI** | `oxo-flow publish` | Already exists — extend with registry integration |

### Principle 6: HPC/Cloud as a Detail

**What it means:** The execution backend is a dropdown — local, HPC queue, Docker, cloud batch. Everything else (workspace setup, file staging, environment resolution) happens transparently.

**What needs to be built:**

| Add to | Component | Description |
|--------|-----------|-------------|
| **Web** | Backend selector in run endpoint | `POST /api/runs` accepts `backend: "local" | "slurm" | "pbs" | "docker"` |
| **Web** | Cloud batch backend | AWS Batch / Google Batch integration for elastic compute |
| **Web** | File staging API | `/api/staging` for transparent S3/GCS ↔ local file transfer before/after runs |
| **Web** | Job status aggregation | Single endpoint that reports status across all backends |
| **Core** | Abstract `ExecutionBackend` trait | Uniform interface for local, HPC, cloud executors |
| **CLI** | `--backend` flag on `oxo-flow run` | Choose execution backend from CLI |

---

## Phased Implementation Plan

### Phase 1: Foundation — Make the API Agent-Ready (High Priority, Quick Wins)

These are the cheapest changes with the highest impact for AI integration:

1. **Structured error responses** — Replace free-text errors with `{code, message, detail, suggestion}`
2. **CLI `--json` flag** — Add `--json` output mode to `run`, `dry-run`, `validate`, `status`
3. **`POST /api/workflows/generate`** — Minimal intent-to-pipeline endpoint (template matching)
4. **OpenAPI schema** — Generate `/api/openapi.json` from route handlers
5. **Streaming for long running ops** — SSE for `run` and `generate`
6. **Pagination envelope** — Consistent pagination for all list endpoints

### Phase 2: The Visual Platform (Frontend + DAG)

This is the biggest lift — the frontend that makes the platform real:

1. **React SPA with interactive DAG** — Using a graph library (Cytoscape.js, vis-network, or custom canvas)
2. **DAG editing** — Drag-drop nodes, connect rules, inline validation
3. **Pipeline library** — Browse, search, fork templates and saved pipelines
4. **Run detail view** — Real-time progress via SSE, structured results display
5. **Comparison view** — Side-by-side run comparison

### Phase 3: Structured Results & Intelligence

1. **`OutputRecord` data model** — Core types for structured results
2. **Result extraction per rule type** — Hooks for common tools (FastQC, STAR, GATK, bcftools)
3. **`/api/runs/{id}/results`** — Structured results endpoint
4. **Run comparison** — `/api/runs/compare`
5. **Dataset catalog** — `POST/GET /api/datasets`

### Phase 4: Advanced AI Integration

1. **Semantic pipeline search** — Vector search over workflow descriptions and rules
2. **Intelligent pipeline suggestion** — "I have RNA-seq data from human blood samples" → suggests full pipeline
3. **Anomaly detection** — Compare run metrics against historical runs to detect QC failures
4. **AI-assisted debugging** — Given a failed run, suggest fixes

---

## Decision: Where to Start?

We have a solid foundation. The most impactful first step is **Phase 1** — the changes that make the existing system AI-consumable with minimal effort. Specifically:

1. **`POST /api/workflows/generate`** — This is the single most transformative endpoint. It changes the system from "you write TOML" to "you describe what you want."
2. **Structured errors + CLI `--json`** — Makes everything machine-readable with almost no new logic.
3. **OpenAPI schema** — Lets any AI agent discover and use the full API.

After Phase 1, Phase 2 (the frontend) is the biggest unlock — it's what makes the system feel like a *platform* rather than just an API.

Do you want to start with Phase 1, or should we discuss priorities further?
