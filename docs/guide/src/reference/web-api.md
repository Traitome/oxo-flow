# Web API (v0.8+)

oxo-flow includes a built-in REST API server for building, validating, running, and monitoring bioinformatics workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and follows a **domain-driven modular monolith** architecture.

---

## API Design Conventions

- **Envelope**: `{ data, error, meta: { page, per_page, total } }`
- **Errors**: `{ code: "E001", message, detail, suggestion? }`
- **Pagination**: cursor-based for lists > 100 items
- **Versioning**: `/api/` prefix; legacy `/api/workflows/*` endpoints preserved for backward compat
- **Self-discoverable**: OpenAPI 3.1 spec at `GET /api/openapi.json`

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

---

## Starting the Server

```bash
# Mode 1: Personal (default) — SQLite, no auth, localhost
oxo-flow serve

# Mode 2: Team — auth enabled, network-facing
oxo-flow serve --mode team

# Mode 3: HPC — cluster-aware
oxo-flow serve --mode hpc

# Or via the standalone binary:
oxo-flow-web --mode personal -p 3000
```

---

## System & Monitoring

### Health Check
```
GET /api/health
```
Returns status, version, mode, uptime, component health (database, filesystem, scheduler, AI provider), resource usage, and license info.

### System Info
```
GET /api/system
```
Returns OS, architecture, PID, uptime, and version.

### Runtime Metrics
```
GET /api/metrics
```
Returns real-time resource metrics: CPU%, memory (used/total/swap), active workflows, total requests, CPU count.

### Server-Sent Events
```
GET /api/events
Accept: text/event-stream
```
SSE stream for real-time workflow execution events (`run_started`, `run_failed`, `run_completed`, `run_cancelled`). Includes a 5-second heartbeat.

---

## Authentication & Authorization

### Login
```
POST /api/auth/login
Content-Type: application/json

{"username": "admin", "password": "admin"}
```
Returns session token, username, and role.

### Check Session
```
GET /api/auth/me
Authorization: Bearer <token>
```
Returns `{"authenticated": true, "username": "admin", "role": "admin"}` or `{"authenticated": false}`.

### License Status
```
GET /api/license
```
Returns license type, validity, commercial use flag, and contact info.

### Upload License
```
POST /api/license/upload
```
Upload a commercial license file for validation and activation.

---

## Pipeline Lifecycle (v0.8 `/api/pipelines/*`)

> Legacy `/api/workflows/*` endpoints remain functional but are deprecated. Use `/api/pipelines/*` for new integrations.

### Parse
```
POST /api/pipelines/parse
Content-Type: application/json

{"toml_content": "<workflow TOML>", "format_version": "0.8"}
```
Returns structured pipeline: `pipeline_id`, `name`, `version`, `rules` (with summaries), `dag` (nodes + edges), `stats`. Pure function, zero side effects.

### Validate
```
POST /api/pipelines/validate
Content-Type: application/json

{"pipeline_id": "...", "toml_content": "<TOML>"}
```
Returns `{ valid, errors: [{ code, message, rule, suggestion }] }`.

### Prepare
```
POST /api/pipelines/prepare
Content-Type: application/json

{"pipeline_id": "...", "resolve_wildcards": true, "apply_defaults": true}
```
Expands wildcards, resolves environments. Returns `expanded_rules_count`, `wildcard_combinations`, `environment_setup_cmds`.

### Build DAG
```
POST /api/pipelines/dag
Content-Type: application/json

{"pipeline_id": "...", "toml_content": "<TOML>"}
```
Returns `{ nodes, edges, parallel_groups, critical_path, metrics }` as structured JSON.

### Format
```
POST /api/pipelines/format
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns canonical TOML formatting.

### Lint
```
POST /api/pipelines/lint
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns diagnostic findings with pagination support.

### Stats
```
POST /api/pipelines/stats
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns aggregate pipeline statistics.

### Diff
```
POST /api/pipelines/diff
Content-Type: application/json

{"toml_a": "<TOML A>", "toml_b": "<TOML B>"}
```
Returns structured diffs: `{ diffs: [{ path, category, description, severity }] }`.

### Export
```
POST /api/pipelines/export
Content-Type: application/json

{"toml_content": "<TOML>", "format": "docker|singularity"}
```
Generates Dockerfile or Singularity definition.

### List / Save / Get / Update / Delete
```
GET    /api/pipelines              # List pipelines (paginated)
POST   /api/pipelines              # Save new pipeline
GET    /api/pipelines/{id}         # Get pipeline with TOML content
PUT    /api/pipelines/{id}         # Update pipeline
DELETE /api/pipelines/{id}         # Delete pipeline
POST   /api/pipelines/search       # Search by name, tags, content
```

---

## Execution & Runs

### Create Run
```
POST /api/runs
Content-Type: application/json

{"pipeline_id": "...", "config": {"max_jobs": 4, "dry_run": false, "keep_going": false}}
```
Returns `{ run_id, status: "queued", estimated_resources, execution_plan }`.

### Run Status
```
GET /api/runs/{id}/status
```
Real-time status: `{ status, phase, nodes: [{ rule, status, started_at, duration_ms, exit_code }], timeline, resources }`.

### DAG Status
```
GET /api/runs/{id}/dag-status
```
DAG JSON with per-node live status. Color-coded: green=completed, blue=running, gray=pending, red=failed.

### Diagnostics
```
GET /api/runs/{id}/diagnostics
```
Deterministic error analysis: `{ failed_nodes: [{ rule, error_pattern, likely_cause, suggestions, auto_fixable, fix_action, relevant_log_lines }], warnings, resource_bottlenecks }`. Uses 30+ deterministic error patterns — zero AI in this endpoint.

### Smart Retry
```
POST /api/runs/{id}/retry
Content-Type: application/json

{"from_rule": "fastqc", "skip_succeeded": true}
```
Only re-runs failed nodes and their downstream dependents. Returns `{ new_run_id, will_rerun: [...], will_skip: [...] }`.

### Cancel
```
POST /api/runs/{id}/cancel
```
Cancels a running/pending run.

### Logs
```
GET /api/runs/{id}/logs
```
Returns full execution log.

### Results
```
GET /api/runs/{id}/results
```
Returns output file tree with sizes and types.

---

## Data Discovery

### Analyze Data
```
POST /api/data/analyze
Content-Type: application/json

{"paths": ["/data/*.fastq.gz", "/data/*.bam"], "max_depth": 2}
```
Deterministic file scanning + format inference + pipeline recommendation. Returns `{ files: [{ path, size, format, format_confidence, paired_with? }], summary, suggested_workflow }`. Format detection uses magic bytes + extension — **not AI**.

### Reference Discovery
```
POST /api/data/reference
Content-Type: application/json

{"genome": "hg38", "components": ["fasta", "gtf", "star_index"]}
```
Finds installed reference genome components and reports missing ones with download commands.

---

## Templates

```
GET    /api/templates?category=rnaseq&tags=star,featurecounts
POST   /api/templates
GET    /api/templates/{id}
DELETE /api/templates/{id}
```
Built-in and user-created pipeline templates. System templates are read-only.

---

## Plugins

### Validate Plugin
```
POST /api/plugins/validate
Content-Type: application/json

{"manifest": {"name": "...", "version": "1.0", "plugin_type": "rule"}, "trusted_keys": {"key1": "hex..."}}
```
Validates a plugin manifest and optionally verifies its HMAC signature against trusted keys. Returns `{ valid, name, version, plugin_type, signature_valid, errors }`.

---

## AI (Phase 2 — calls deterministic APIs above)

```
POST /api/ai/translate   # NL intent → validated .oxoflow (SSE streaming)
POST /api/ai/explain     # Explain run failure + suggest fix
POST /api/ai/interpret   # Interpret results with caveats
POST /api/ai/optimize    # Optimize pipeline parameters
```

See [AI Translation Layer](ai-translation.md) for details.

---

## Collaboration (Phase 3)

```
POST /api/pipelines/{id}/fork    # Fork into workspace
POST /api/pipelines/{id}/share   # Share (link or workspace)
POST /api/pipelines/import       # Import from oxo+https:// URL
POST /api/pipelines/diff         # Compare two pipelines
```

See [Collaboration](../how-to/collaboration.md) for details.

---

## HPC

```
GET /api/hpc
```
Returns scheduler status (SLURM, PBS/Torque, LSF, SGE), available queues, and node count.

---

## See Also

- [System Architecture](architecture.md) — Domain-driven module structure
- [Web System Architecture](web-system-architecture.md) — Router design and middleware
- [AI Translation Layer](ai-translation.md) — AI integration design
- [Diagnostics Engine](diagnostics-engine.md) — Error pattern library
- [Deployment Modes](../how-to/deploy-modes.md) — Personal/Team/HPC
