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
| **execution** | `domains/execution/` | Run create/status/cancel/retry, diagnostics engine (30+ error patterns), sandbox workspace; runs delegate to the `oxo-flow` CLI subprocess, which owns checkpointing and invalidation |
| **ai** | `domains/ai/` | AI translate, explain, interpret, optimize; provider dispatch (DeepSeek/Claude/OpenAI/Ollama) |
| **collaboration** | `domains/collaboration/` | Fork, diff, share, import pipelines |
| **auth** | `domains/auth/` | Login, session management, ORCID/GitHub OAuth2, RBAC |
| **observability** | `domains/observability/` | Health check, system info, runtime metrics, structured logging (3-layer), audit, SSE |
| **infra/db** | `infra/db/` | StorageBackend trait with SQLite and PostgreSQL implementations |
| **infra/license** | `infra/license.rs` | License notice text, banner, footer HTML, X-OxoFlow-License header middleware |
| **infra/sse** | `infra/sse.rs` | Real-time SSE broadcast channel for execution events |
| **infra/hpc** | `infra/hpc.rs` | Slurm script generation, scheduler detection |

### Legacy Modules (deprecated since 0.8.0)

The `handlers/` directory contains pre-v0.8 handler modules marked `#[deprecated(since = "0.8.0")]`. These are preserved so the crate still builds (via the crate-level `#![allow(deprecated)]`), but they are **not mounted in the served router**: both `oxo-flow serve` and the `oxo-flow-web` binary assemble their routes exclusively from the `domains/*` modules via `server.rs::build_router`. The legacy routes (including the old `/api/workflows/*` endpoints) are not reachable on a running server and are scheduled for removal in a future release. New code should use `domains/*/` modules.

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
├── /hpc                    # HPC scheduler status (SLURM, PBS, etc.) — hpc mode only
│
├── /auth
│   ├── /login              # Login (username/password)
│   ├── /me                 # Current session info
│   └── /oauth/...          # OAuth2 authorize / callback
│
├── /license
│   ├── /                   # License status (type, validity, contact)
│   └── /upload             # Upload commercial license file
│
├── /users                  # User list/create/delete (admin)
│
├── /pipelines (new v0.8 API — replaces /workflows)
│   ├── /parse              # Parse TOML → structured pipeline
│   ├── /validate           # Validate pipeline DAG
│   ├── /prepare            # Prepare (expand wildcards, resolve envs)
│   ├── /dag                # Build DAG as JSON
│   ├── /format             # Canonical TOML formatting
│   ├── /lint               # Lint pipeline (valid + errors/lint findings)
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
│   ├── /{id}/results       # Output files with sizes
│   ├── /{id}/retry         # Smart retry (failed + downstream only)
│   ├── /{id}/cancel        # Cancel running workflow
│   ├── /{id}/pause         # Pause running workflow
│   └── /{id}/resume        # Resume paused workflow
│
├── /data
│   ├── /analyze            # Scan files → detect format, suggest pipeline
│   ├── /reference          # Reference genome discovery
│   ├── /perceive           # Data perception (AI Companion)
│   ├── /reference/status   # Reference genome status
│   └── /samplesheet/parse  # Parse a samplesheet
│
├── /templates
│   ├── /                   # GET: list templates; POST: create
│   └── /{id}               # GET/DELETE template
│
├── /plugins
│   └── /validate           # Validate plugin manifest + signature
│
├── /ai
│   ├── /translate          # Natural language → validated .oxoflow (JSON)
│   ├── /translate/stream   # Same, streamed over SSE
│   ├── /explain            # Explain run failure + suggest fix
│   ├── /interpret          # Interpret results with caveats
│   ├── /optimize           # Optimize pipeline parameters
│   └── /config, /test      # AI provider configuration
│
├── /chat                   # AI Companion: /send, /send/json, /sessions
│
└── /pipeline/{id}          # DAG edit API: /command, /undo, /redo
```
(See [Web API](./web-api.md) and [openapi.json](https://github.com/Traitome/oxo-flow/blob/main/docs/schema/openapi.yaml) for the complete API reference.)
(Old `/workflows/*` endpoints marked `#[deprecated]` exist only as unserved legacy modules — see [Legacy Modules](#legacy-modules-deprecated-since-080).)

---

## Structured Error Response

All errors follow this format:

```json
{
  "code": "AUTH_REQUIRED",
  "message": "Authentication is required for this endpoint",
  "detail": "The request did not include a valid session token or Bearer token",
  "suggestion": "Please login at POST /api/auth/login to obtain a session token"
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `BAD_REQUEST` | 400 | Input validation failed |
| `MISSING_FIELD` | 400 | Required request field missing (e.g. `toml_content`) |
| `DAG_EDIT_ERROR` | 400 | DAG edit command rejected (unknown operation, missing fields) |
| `AUTH_REQUIRED` | 401 | Authentication required |
| `AUTH_FAILED` | 401 | Invalid login credentials |
| `TOKEN_EXPIRED` | 401 | Session token missing or expired |
| `ACCESS_DENIED` | 403 | Permission denied |
| `NOT_FOUND` | 404 | Resource not found |
| `NO_UNDO` / `NO_REDO` | 404 | DAG edit undo/redo stack empty |
| `RATE_LIMITED` | 429 | Request rate exceeded |
| `DB_ERROR` | 500 | Internal database error |

Domain-specific codes (all 400) include `PARSE_ERROR`, `VALIDATE_ERROR`, `PREPARE_ERROR`, `DAG_ERROR`, `LINT_ERROR`, `STATS_ERROR`, `DIFF_ERROR`, `EXPORT_ERROR`, `SEARCH_ERROR`, `RUN_ERROR`, `RETRY_ERROR`, `DATA_ERROR`, `REF_ERROR`, `PLUGIN_ERROR`, `AI_TRANSLATE_ERROR`, `CHAT_ERROR`, `INVALID_URL`.

---

## List Responses

List endpoints return bare JSON arrays. There is currently no pagination envelope: `GET /api/pipelines` returns at most 100 items (ordered by last update), and `GET /api/templates` returns all templates.

```json
[{ "id": "...", "name": "...", ... }]
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
2. POST /api/auth/login         # Authenticate (team/hpc mode)
3. POST /api/ai/translate       # [AI] "Run standard RNA-seq differential expression"
4. POST /api/runs               # Execute the pipeline (POST the TOML content)
5. GET  /api/events             # SSE real-time progress (optional)
6. GET  /api/runs/{id}/results  # Get structured results
```

### API Streaming

AI pipeline generation supports SSE streaming via the dedicated streaming endpoint:

```
POST /api/ai/translate/stream
Accept: text/event-stream

event: progress
data: {"step": "intent", "message": "Intent received", "intent": "..."}

event: progress
data: {"step": "match", "message": "Matching templates...", "templates_count": 5}

event: progress
data: {"step": "generate", "message": "Generating pipeline via AI..."}

event: done
data: {"pipeline_id": "...", ...}
```
