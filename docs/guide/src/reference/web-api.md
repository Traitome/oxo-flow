# Web API

oxo-flow includes a built-in REST API server for remotely building, validating, running, and monitoring bioinformatics workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and runs on the tokio async runtime.

---


## AI-Native API Design

oxo-flow-web is designed as an **AI-native API** — all endpoints are consumable by both browsers and AI agents.

### Structured Error Format

All errors return a unified JSON format:

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
|------|-----------|-------|
| `BAD_REQUEST` | 400 | Input validation failed |
| `INVALID_WORKFLOW` | 400 | Workflow TOML parsing failed |
| `AUTH_REQUIRED` | 401 | Authentication required |
| `INVALID_CREDENTIALS` | 401 | Invalid login credentials |
| `NOT_FOUND` | 404 | Resource not found |
| `ACCESS_DENIED` | 403 | Permission denied |
| `UNPROCESSABLE_ENTITY` | 422 | Entity unprocessable |
| `RATE_LIMITED` | 429 | Request rate exceeded |
| `INTERNAL_ERROR` | 500 | Internal server error |

### API Discovery

```
GET /api/openapi.json
```

Returns the full OpenAPI 3.0 schema. An AI agent can discover the complete API surface from this endpoint.

### Pagination Envelope

List endpoints use a consistent pagination format:

```json
{
  "data": [...],
  "meta": {
    "page": 1,
    "per_page": 20,
    "total_items": 142,
    "total_pages": 8,
    "has_next": true,
    "has_prev": false
  }
}
```

### SSE Streaming Events

Long-running operations provide real-time progress via Server-Sent Events:

```
GET /api/events
Accept: text/event-stream
```

The stream includes workflow execution events and a 5-second heartbeat.

### AI Agent Navigation Sequence

The following sequence is designed for AI agents:

1. `GET /api/health` — Check server availability
2. `POST /api/auth/login` — Authenticate
3. `POST /api/workflows/generate` — Generate a pipeline from intent
4. `POST /api/workflows/run` — Execute the pipeline
5. `GET  /api/runs/{id}` — Monitor run progress
6. `GET  /api/events` — Stream real-time events

The `GET /api/openapi.json` endpoint includes an `x-ai-navigation` field providing the same navigation sequence.

---


## Starting the Server

```bash
oxo-flow serve                         # localhost:8080
oxo-flow serve --host 0.0.0.0 -p 3000 # all interfaces, port 3000
oxo-flow serve --base-path /oxo-flow   # mount under sub-path

# Or via the standalone binary:
oxo-flow-web --host 127.0.0.1 -p 3000
```

Environment variables: `OXO_FLOW_HOST`, `OXO_FLOW_PORT`, `OXO_FLOW_ADMIN_PASSWORD`.

---

## Endpoints

### System & Monitoring

#### Health Check
```
GET /api/health
```
Returns `{"status":"ok","version":"0.6.1"}`.

#### Version
```
GET /api/version
```
Returns crate version, name, and Rust version.

#### System Info
```
GET /api/system
```
Returns OS, architecture, PID, uptime, and version info.

#### Runtime Metrics
```
GET /api/metrics
```
Returns real-time resource metrics: CPU usage, memory (used/total/swap), active workflows, total requests, CPU count.

#### Server-Sent Events
```
GET /api/events
```
SSE stream for real-time workflow execution events. Includes 5-second heartbeat.

---

### Authentication & Authorization

#### Login
```
POST /api/auth/login
Content-Type: application/json

{"username": "admin", "password": "admin"}
```
Returns session token, username, and role. Sets `oxo_session` HttpOnly cookie.

**Response:**
```json
{
  "token": "a1b2c3...",
  "username": "admin",
  "role": "admin"
}
```

Authentication: Bearer token in `Authorization` header or `oxo_session` cookie.

#### Check Session
```
GET /api/auth/me
```
Returns `{"authenticated":true,"username":"admin","role":"admin"}` or `{"authenticated":false}`.

#### License Status
```
GET /api/license
```
Returns license validity, type, and issued-to information.

---

### Workflow CRUD

#### Validate
```
POST /api/workflows/validate
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns `{"valid":true,"errors":[],"rules_count":N,"edges_count":N}`.

#### Parse
```
POST /api/workflows/parse
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns full workflow detail with name, version, description, author, and per-rule summary (inputs, outputs, environment, threads).

#### Build DAG
```
POST /api/workflows/dag
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns DOT graph representation with node/edge counts.

#### Dry Run
```
POST /api/workflows/dry-run
Content-Type: application/json

{"toml_content": "<workflow TOML>", "config": {"max_jobs": 4, "dry_run": true}}
```
Returns execution order, rule summaries, and run configuration.

#### Format
```
POST /api/workflows/format
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns `{"formatted": "<canonical TOML>"}`.

#### Lint
```
POST /api/workflows/lint
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns diagnostics with error/warning/info counts.

#### Paginated Lint
```
POST /api/workflows/lint/paginated?page=1&per_page=20
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns paginated diagnostics with summary counts.

#### Statistics
```
POST /api/workflows/stats
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns rule counts, shell/script breakdown, dependency count, parallel groups, thread totals, environments, and wildcard info.

#### Diff
```
POST /api/workflows/diff
Content-Type: application/json

{"toml_a": "<TOML A>", "toml_b": "<TOML B>"}
```
Returns `{"diff_count":N,"diffs":[{"category":"...","description":"..."}]}`.

#### Export
```
POST /api/workflows/export
Content-Type: application/json

{"toml_content": "<workflow TOML>", "format": "docker|singularity"}
```
Returns `{"format":"docker","content":"<Dockerfile or Singularity def>"}`.

#### Clean
```
POST /api/workflows/clean
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Returns list of output files that would be cleaned.



#### Generate Pipeline from Intent [AI]
```
POST /api/workflows/generate
Content-Type: application/json

{"intent": "Run a standard RNA-seq differential expression pipeline"}
```

Maps a natural-language intent description to a pre-built pipeline template. Supported keywords:

- `rna-seq`, `rnaseq`, `transcriptome` → RNA-seq quantification pipeline
- `variant`, `wgs`, `germline`, `somatic` → Variant calling pipeline
- `qc`, `fastqc`, `multiqc` → Quality control pipeline
- `single-cell`, `10x`, `scrna` → Single-cell RNA-seq pipeline
- `chipseq`, `chip-seq` → ChIP-seq pipeline
- `scatter`, `gather`, `parallel` → Scatter-gather template
- `multiomics`, `multi-omics` → Multi-omics integration template

**Response:**
```json
{
  "toml_content": "[workflow]...",
  "workflow_name": "rnaseq-pipeline",
  "rules_count": 3,
  "execution_order": ["fastqc", "star_align", "featurecounts"],
  "description": "RNA-seq quantification pipeline...",
  "valid": true
}
```

This is the entry point for intent-driven pipeline authoring. The returned TOML can be sent directly to `/api/workflows/run` for execution.

---

### Workflow Execution & Runs

#### Launch Run [Auth Required]
```
POST /api/workflows/run
Authorization: Bearer <token>
Content-Type: application/json

{"toml_content": "<workflow TOML>"}
```
Creates an isolated workspace, inserts a run record, and spawns background execution. Returns run ID, status, execution order, and rule count.

#### List Runs [Auth Required]
```
GET /api/runs
Authorization: Bearer <token>
```
Returns all runs for the authenticated user, ordered by start time.

#### Run Detail [Auth Required]
```
GET /api/runs/{id}
Authorization: Bearer <token>
```
Returns run status, timestamps, PID, output file listing, and last 50 lines of execution log.

#### Run Logs [Auth Required]
```
GET /api/runs/{id}/logs
Authorization: Bearer <token>
```
Returns the full execution log content.

#### Cancel Run [Auth Required]
```
DELETE /api/runs/{id}
Authorization: Bearer <token>
```
Cancels a running/pending run. Kills the process if active.

---

### Workflow Library [Auth Required]

#### Save Workflow
```
POST /api/workflows/save
Authorization: Bearer <token>
Content-Type: application/json

{"name": "my-pipeline", "version": "0.8.0", "toml_content": "<TOML>"}
```
Validates TOML and persists to database. Returns `{"id":"<uuid>","status":"saved"}` (HTTP 201).

#### List Saved Workflows
```
GET /api/workflows/saved
Authorization: Bearer <token>
```
Returns array of `{"id","name","version","rules_count","created_at","updated_at"}`.

#### Get Saved Workflow
```
GET /api/workflows/saved/{id}
Authorization: Bearer <token>
```
Returns full workflow detail including TOML content.

#### Delete Saved Workflow
```
DELETE /api/workflows/saved/{id}
Authorization: Bearer <token>
```
Deletes the workflow. Returns `{"status":"deleted"}`. Ownership verified.

---

### Reports

#### Generate Report
```
POST /api/reports/generate
Content-Type: application/json

{"toml_content": "<TOML>", "format": "html|json"}
```
Returns HTML or JSON report with workflow overview and execution order.

---

### Environments

#### List Environments
```
GET /api/environments
```
Returns available environment backends (conda, docker, singularity, venv, pixi).

---

## Authentication

All `/api/runs/*`, `/api/workflows/save*`, and `/api/workflows/saved*` endpoints require authentication via Bearer token or session cookie.

Default users (seeded on first run): `admin` (role: admin).

Passwords are set via environment variables: `OXO_FLOW_ADMIN_PASSWORD`, `OXO_FLOW_USER_PASSWORD`, `OXO_FLOW_VIEWER_PASSWORD`.

---

## Error Handling

All errors return JSON with `error` and optional `detail` fields:
```json
{"error": "Authentication required", "detail": null}
```

HTTP status codes: 200 (success), 201 (created), 400 (bad request), 401 (unauthorized), 404 (not found), 422 (unprocessable), 429 (rate limited).

---

## CORS

CORS is enabled for `localhost:8080` and `127.0.0.1:8080` by default. Configure via `OXO_FLOW_ALLOWED_ORIGINS` (comma-separated).

---

## See Also

- [Architecture](architecture.md) — Server design and component overview
- [Workflow Format](workflow-format.md) — `.oxoflow` TOML specification
- [Environment System](environment-system.md) — Conda/Docker/Singularity backends

---

## Security Model

### Workspace Isolation

Each authenticated user gets an isolated workspace:

- **Physical isolation**: Run directories scoped to `workspace/users/<username>/runs/<run_id>`
- **Database isolation**: All queries scoped by `user_id` (runs, workflows, audit logs)
- **Ownership verification**: Run detail, logs, cancel, and workflow get/delete all check `user_id` matches the authenticated session
- **Defense in depth**: Even if a run ID is guessed, the workspace directory is always under the requesting user's tree

### Authentication

- Session tokens are hex-encoded UUIDv4, stored in SQLite with 24-hour expiry
- Tokens accepted via `Authorization: Bearer <token>` header or `oxo_session` cookie
- Passwords set via environment variables (never stored in database)
- OS username validated against strict regex before sudo escalation

### Rate Limiting

Per-IP sliding window rate limiter (default: 100 requests/60s). Configurable via `RateLimiterConfig`.
