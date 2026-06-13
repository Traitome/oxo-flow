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

## Architecture Overview

```
+-----------------------------------------------+
|              Web Client (SPA)                 |
|     Interactive DAG Editor / Dashboard        |
+------------------+----------------------------+
                   | HTTP/SSE
+------------------v----------------------------+
|          oxo-flow-web (Axum Server)           |
|                                                |
|  +---------+ +----------+ +----------------+ |
|  | Auth    | | Workflow | | Run            | |
|  | Layer   | | Handlers | | Handlers       | |
|  +---------+ +----------+ +----------------+ |
|  +---------+ +----------+ +----------------+ |
|  | SSE     | | OpenAPI  | | Result         | |
|  | Stream  | | Schema   | | Endpoints      | |
|  +---------+ +----------+ +----------------+ |
|  +-------------------------------------------+|
|  |           Middleware Stack                 | |
|  |  Rate Limit -> Auth -> Audit -> Logging   | |
|  +-------------------------------------------+|
|  +-------------------------------------------+|
|  |  SQLite (Users, Runs, Workflows, Audit)   | |
|  +-------------------------------------------+|
+------------------+----------------------------+
                   | API calls
+------------------v----------------------------+
|          oxo-flow-core (Engine)               |
|  DAG . Executor . Environment . Wildcards    |
+-----------------------------------------------+
```

---

## Module Responsibilities

| Module | Responsibility |
|--------|---------------|
| `lib.rs` | Router construction, global state, public types, metrics |
| `main.rs` | Server entry point, CLI arguments |
| `db.rs` | SQLite connection pool, CRUD operations |
| `handlers/workflow.rs` | Workflow validation, parsing, DAG, dry-run, run, clean, export, format, lint, stats, diff |
| `handlers/runs.rs` | Run details, logs, cancellation, HPC submission |
| `handlers/auth.rs` | Login, session management, license verification |
| `handlers/users.rs` | User CRUD (admin) |
| `handlers/templates.rs` | Workflow template CRUD |
| `handlers/saved.rs` | Saved workflow CRUD |
| `handlers/scheduled.rs` | Cron-scheduled runs |
| `handlers/system.rs` | Health check, version, system info, metrics, environments, SSE events, audit, HPC |
| `handlers/reports.rs` | Report generation |
| `executor.rs` | Background workflow execution (spawns CLI process) |
| `workspace.rs` | Filesystem-based sandbox workspace setup |
| `sse.rs` | Server-Sent Events broadcast |
| `hpc.rs` | HPC scheduler detection (SLURM, PBS, LSF, SGE) |
| `audit.rs` | Audit logging |
| `rate_limit.rs` | Sliding window rate limiter |
| `sys.rs` | Host resource detection |

---

## API Namespace

```
/api
├── /health                 # Health check
├── /version                # Version info
├── /system                 # System info
├── /metrics                # Runtime metrics
├── /openapi.json           # OpenAPI 3.0 spec
├── /events                 # SSE event stream
├── /environments           # Available environment backends
├── /audit                  # Audit logs
├── /hpc                    # HPC status
│
├── /auth
│   ├── /login              # Login
│   ├── /me                 # Current session
│   └── /logout             # Logout
│
├── /license
│   ├── /                   # License status
│   └── /upload             # Upload license file
│
├── /users                  # User CRUD (admin)
│
├── /workflows
│   ├── /validate           # Validate TOML
│   ├── /parse              # Parse TOML, return detail
│   ├── /dag                # Build DAG, return DOT
│   ├── /dag-json           # Build DAG, return JSON
│   ├── /dry-run            # Simulate execution
│   ├── /run                # Launch run
│   ├── /generate           # [AI] Intent to pipeline
│   ├── /clean              # List files to clean
│   ├── /export             # Export Dockerfile/Singularity
│   ├── /format             # Format TOML
│   ├── /lint               # Lint workflow
│   ├── /lint/paginated     # Paginated lint
│   ├── /stats              # Workflow statistics
│   ├── /diff               # Workflow comparison
│   ├── /saved              # Saved workflows list
│   ├── /saved/{id}         # Get/delete saved workflow
│   ├── /save               # Save workflow
│   └── /search             # [AI] Semantic pipeline search
│
├── /runs
│   ├── /                   # Run list
│   ├── /{id}               # Run detail
│   ├── /{id}/logs          # Run logs
│   ├── /{id}/results       # [AI] Structured results
│   ├── /{id}/results/{rule}# [AI] Per-rule results
│   ├── /{id}/hpc-submit    # Submit to HPC
│   └── /compare            # Run comparison
│
├── /scheduled              # Scheduled runs list/create
├── /scheduled/{id}         # Get/cancel scheduled run
│
├── /templates              # Template list/create
├── /templates/{id}         # Get/delete template
│
└── /datasets               # [AI] Dataset catalog
```

(Endpoints marked `[AI]` are planned AI-native features.)

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
