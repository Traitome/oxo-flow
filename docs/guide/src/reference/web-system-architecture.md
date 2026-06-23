# Web System Architecture

oxo-flow-web is the web control plane for oxo-flow, designed as the **primary interaction surface** in the AI era (rather than a thin wrapper around the CLI).

---

## Design Principles

### 1. AI-Native API Design

All endpoints are designed to be consumable by both AI agents and browsers:

- Unified structured responses
- Long-running operations streamed via SSE
- Self-discoverable via `GET /api/openapi.json`
- Errors include `code` + `message` + `detail` + `suggestion` fields

### 2. Intent-First Pipeline Authoring

Users describe *what* they want ("run standard RNA-seq differential expression on this cohort"), and the system generates a validated pipeline. The TOML format becomes the compilation target, not the authoring surface.

### 3. DAG as the Primary Interaction Surface

An interactive visual graph where users can drag nodes, connect inputs to outputs, and inspect environment/resource configuration. The graph *is* the workspace.

### 4. Results as Queryable Data

Pipeline outputs are not just files — they are structured data that can be queried, compared, and visualized via the API.

### 5. Shareable, Forkable, Citable Pipelines

A pipeline is a URL, not a file. Share it, fork it. Each run has a permanent provenance URL.

### 6. HPC/Cloud as a Detail

The execution backend is a dropdown — local, HPC queue, Docker, cloud batch. Everything else (workspace setup, file staging, environment resolution) happens transparently.

---

## Architecture Overview (v0.8+)

The web crate follows a **domain-driven modular monolith** pattern. Each domain has:
- `types.rs` — request/response structs
- `service.rs` — pure logic (zero HTTP dependency)
- `handlers.rs` — HTTP → service adapters

```
+-----------------------------------------------+
|              Web Client (SPA)                 |
|     Interactive DAG Editor / Dashboard        |
+------------------+----------------------------+
                   | HTTP/SSE
+------------------v----------------------------+
|          oxo-flow-web (Axum Server)           |
|                                                |
|  +---------+ +---------+ +----------------+  |
|  | Auth    | |License  | | Observability  |  |
|  | OAuth2  | |Headers  | | Health/Metrics |  |
|  +---------+ +---------+ +----------------+  |
|  +---------+ +---------+ +----------------+  |
|  |Workflow | |Execution| | AI/Translate   |  |
|  |Pipeline | |Diagnose | | Explain/Interp |  |
|  +---------+ +---------+ +----------------+  |
|  +---------+ +---------+ +----------------+  |
|  |Collabor-| |  Data   | | HPC            |  |
|  |ation    | | Discovery| | Scheduler      |  |
|  +---------+ +---------+ +----------------+  |
|  +-------------------------------------------+|
|  |           Middleware Stack                 | |
|  |  LicenseHeader -> RateLimit -> Auth       | |
|  +-------------------------------------------+|
|  +-------------------------------------------+|
|  | StorageBackend trait (SQLite + PostgreSQL)| |
|  +-------------------------------------------+|
+------------------+----------------------------+
                   | API calls
+------------------v----------------------------+
|          oxo-flow-core (Engine)               |
|  DAG . Executor . Environment . Wildcards    |
+-----------------------------------------------+
```

---

## Domain-Driven Module Structure (v0.8+)

| Domain | Path | Responsibility |
|--------|------|---------------|
| **workflow** | `domains/workflow/` | Pipeline parse, validate, prepare, DAG, format, lint, stats, diff, export, search, data discovery, plugin validation |
| **execution** | `domains/execution/` | Run create/status/cancel/retry, diagnostics engine (30+ error patterns), sandbox workspace, background runner |
| **ai** | `domains/ai/` | AI translate, explain, interpret, optimize; provider dispatch (Claude/OpenAI/Ollama) |
| **collaboration** | `domains/collaboration/` | Fork, diff, share, import pipelines |
| **auth** | `domains/auth/` | Login, session management, ORCID/GitHub OAuth2, RBAC |
| **observability** | `domains/observability/` | Health check, system info, runtime metrics, structured logging (3-layer), audit, SSE |
| **infra/db** | `infra/db/` | StorageBackend trait with SQLite and PostgreSQL implementations |
| **infra/license** | `infra/license.rs` | License notice text, banner, footer HTML, X-OxoFlow-License header middleware |
| **infra/sse** | `infra/sse.rs` | Real-time SSE broadcast channel for execution events |
| **infra/hpc** | `infra/hpc.rs` | Slurm script generation, scheduler detection |

### Legacy Modules (deprecated since 0.8.0)

The `handlers/` directory contains pre-v0.8 handler modules marked `#[deprecated(since = "0.8.0")]`. These are preserved for backward compatibility and will be removed no earlier than v0.8.1. New code should use `domains/*/` modules.

---

## API Namespace (v0.8+)

```
/api
├── /health                 # Health check (with license, mode, component status)
├── /system                 # System info
├── /metrics                # Runtime metrics (CPU, memory, active runs)
├── /openapi.json           # OpenAPI 3.1 spec
├── /events                 # SSE event stream (real-time execution updates)
├── /audit                  # Audit logs (structured, with result field)
├── /hpc                    # HPC scheduler status (SLURM, PBS, etc.)
│
├── /auth
│   ├── /login              # Login (username/password)
│   └── /me                 # Current session info
│
├── /license
│   ├── /                   # License status (type, validity, contact)
│   └── /upload             # Upload commercial license file
│
├── /users                  # User CRUD (admin)
│
├── /pipelines (new v0.8 API — replaces /workflows)
│   ├── /parse              # Parse TOML → structured pipeline
│   ├── /validate           # Validate pipeline DAG
│   ├── /prepare            # Prepare (expand wildcards, resolve envs)
│   ├── /dag                # Build DAG as JSON
│   ├── /format             # Canonical TOML formatting
│   ├── /lint               # Lint pipeline with pagination
│   ├── /stats              # Aggregate pipeline statistics
│   ├── /diff               # Diff two pipelines (by TOML content)
│   ├── /export             # Export Docker/Singularity packaging
│   ├── /search             # Search pipelines by name, tags, content
│   ├── /                   # GET: list pipelines; POST: save pipeline
│   ├── /{id}               # GET/PUT/DELETE pipeline by ID
│   ├── /{id}/fork          # Fork into user workspace (v0.8 collab)
│   └── /{id}/share         # Share pipeline (v0.8 collab)
│
├── /pipelines/import       # Import from oxo+https:// URL (v0.8 collab)
│
├── /runs
│   ├── /                   # POST: create run; GET: list runs
│   ├── /{id}               # Run detail with log tail
│   ├── /{id}/status        # Real-time status (nodes, timeline, resources)
│   ├── /{id}/dag-status    # DAG JSON + per-node live status
│   ├── /{id}/diagnostics   # Diagnostic engine results (30+ error patterns)
│   ├── /{id}/logs          # Execution logs
│   ├── /{id}/results       # Output files and QC metrics
│   ├── /{id}/retry         # Smart retry (failed + downstream only)
│   └── /{id}/cancel        # Cancel running workflow
│
├── /data
│   ├── /analyze            # Scan files → detect format, suggest pipeline
│   └── /reference          # Reference genome discovery
│
├── /templates
│   ├── /                   # GET: list templates; POST: create
│   └── /{id}               # GET/DELETE template
│
├── /plugins
│   └── /validate           # Validate plugin manifest + signature
│
├── /ai
│   ├── /translate          # Natural language → validated .oxoflow (SSE)
│   ├── /explain            # Explain run failure + suggest fix
│   ├── /interpret          # Interpret results with caveats
│   └── /optimize           # Optimize pipeline parameters
│
├── /scheduled              # Scheduled runs list/create
```
(See [Web API](./web-api.md) and [openapi.json](../../schema/openapi.yaml) for the complete API reference.)
(Old `/workflows/*` endpoints marked `#[deprecated]` remain functional but will be removed in v0.8.1.)

---

## Structured Error Response

All errors follow this format:

```json
{
  "error": {
    "code": "AUTH_REQUIRED",
    "message": "Authentication is required for this endpoint",
    "detail": "The request did not include a valid session token or Bearer token",
    "suggestion": "Please login at POST /api/auth/login to obtain a session token"
  }
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `BAD_REQUEST` | 400 | Input validation failed |
| `INVALID_WORKFLOW` | 400 | Workflow TOML parsing failed |
| `AUTH_REQUIRED` | 401 | Authentication required |
| `INVALID_CREDENTIALS` | 401 | Invalid login credentials |
| `ACCESS_DENIED` | 403 | Permission denied |
| `NOT_FOUND` | 404 | Resource not found |
| `ALREADY_EXISTS` | 409 | Resource conflict |
| `UNPROCESSABLE_ENTITY` | 422 | Entity unprocessable |
| `RATE_LIMITED` | 429 | Request rate exceeded |
| `LICENSE_ERROR` | 403 | License invalid |
| `INTERNAL_ERROR` | 500 | Internal server error |

---

## Pagination Envelope

List endpoints use a consistent pagination format:

```json
{
  "data": [...],
  "meta": {
    "total": 142,
    "page": 1,
    "per_page": 20,
    "total_pages": 8
  }
}
```

---

## AI Agent Integration Guide

### Discovery

```
# Start here to discover the full API surface
GET /api/openapi.json
```

### End-to-End Workflow

```
1. GET /api/health              # Check server availability
2. POST /api/auth/login         # Authenticate
3. POST /api/workflows/generate # [AI] "Run standard RNA-seq differential expression"
4. POST /api/workflows/run      # Execute the pipeline
5. GET  /api/events             # SSE real-time progress (optional)
6. GET  /api/runs/{id}/results  # Get structured results
7. GET  /api/runs/compare       # Compare different runs
```

### API Streaming

All long-running operations (generate, run) support SSE streaming:

```
POST /api/workflows/generate?stream=true
Accept: text/event-stream

event: step
data: {"step": "parsing_intent", "status": "in_progress"}

event: step
data: {"step": "generating_rules", "status": "in_progress", "rules_found": 5}

event: result
data: {"status": "complete", "pipeline": {...}}
```
