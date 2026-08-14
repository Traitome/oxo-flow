# Web API (v0.8+)

oxo-flow includes a built-in REST API server for building, validating, running, and monitoring bioinformatics workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and follows a **domain-driven modular monolith** architecture.

---

## API Design Conventions

- **Envelope**: success responses are bare JSON objects/arrays; errors are `{ code, message, detail?, suggestion? }`
- **Errors**: `{ code: "E001", message, detail?, suggestion? }`
- **Lists**: list endpoints return bare JSON arrays (e.g. `GET /api/pipelines` returns at most 100 items); no pagination envelope is currently implemented
- **Versioning**: `/api/` prefix for all endpoints
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

### Audit Logs
```
GET /api/audit?days=7
```
Returns structured audit entries: `{ entries: [{ timestamp, user, action, resource, result }], days }`.

### Server-Sent Events
```
GET /api/events
Accept: text/event-stream
```
SSE stream for real-time workflow execution events (`run_started`, `run_completed`, `run_failed`, `run_cancelled`, `run_paused`, `run_resumed`). Includes a 5-second heartbeat (`heartbeat` events).

`run_completed` carries a `summary` field: the CLI's invalidation summary
extracted from the execution log — config changes, edited rule definitions,
and input-set changes that invalidated checkpoint records this run
(`null` when the run had no invalidation activity).

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

### Users (admin)
```
GET    /api/runs/{id}/preview   # Instance-level dry-run plan (will_run/will_skip + expanded rule instances)
GET    /api/users          # List users (requires a valid session)
POST   /api/users          # Create user
DELETE /api/users/{id}     # Delete user
```

---

## Pipeline Lifecycle (v0.8 `/api/pipelines/*`)

> The pre-v0.8 `/api/workflows/*` endpoints exist only as unserved legacy modules (see [Web System Architecture](web-system-architecture.md)); the running server exposes only the `/api/pipelines/*` API below.

### Parse
```
POST /api/pipelines/parse
Content-Type: application/json

{"toml_content": "<workflow TOML>", "format_version": "1.0"}
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

{"toml_content": "<TOML>", "resolve_wildcards": true, "apply_defaults": true}
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
Returns `{ valid, errors: [{ code, message, rule, suggestion }] }` — validation errors plus lint-level findings (e.g. missing description).

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
GET    /api/pipelines              # List pipelines (most recent first, up to 100)
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

{"toml_content": "<workflow TOML>", "max_jobs": 4, "dry_run": false, "keep_going": false, "pipeline_id": "<uuid>"}
```
`max_jobs`, `dry_run`, `keep_going`, and `pipeline_id` are top-level fields (not nested under a `config` object). Returns `{ run_id, status: "queued", estimated_resources, execution_plan }`.

`pipeline_id` (optional) targets a saved pipeline: the run executes in the
pipeline's **persistent working directory** (`workspace/users/<user>/pipelines/<id>`),
so the checkpoint survives across re-runs. Re-running with a changed config
rebuilds exactly the rules referencing the changed keys (plus their DAG
downstream) — the rest keep their checkpoint records and are skipped. Runs
without `pipeline_id` get a fresh per-run sandbox and execute everything.
Malformed or unknown pipeline ids are rejected (`400 INVALID_PIPELINE_ID` /
`404 PIPELINE_NOT_FOUND`).

Execution flags are forwarded to the CLI executor:

- `dry_run: true` spawns the preview subcommand (`oxo-flow dry-run`) —
  nothing executes; the log shows the would-be plan.
- `max_jobs` maps to the executor's `-j` only when explicitly set; without
  it the CLI default (1) applies (the resource estimate assumes 4).
- `keep_going: true` maps to `-k`.

### Run Status
```
GET /api/runs/{id}/status
```
Real-time status: `{ status, phase, nodes: [{ rule, status, started_at, duration_ms, exit_code }], timeline, resources }`.

### DAG Status
```
GET /api/runs/{id}/dag-status
```
DAG JSON with per-node live status. Color-coded: green=completed, blue=running, red=failed, gray=skipped.

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

### Pause / Resume
```
POST /api/runs/{id}/pause
POST /api/runs/{id}/resume
```
Pauses a running run (`{"reason": "..."}` optional) and resumes it (`{"from_rule": "..."}` optional).

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
Deterministic file scanning + format inference + pipeline recommendation. Returns `{ files: [{ path, size, format, format_confidence, paired_with?, sample_name? }], summary, suggested_workflow }`. Format detection uses filename extension matching — **not AI**.

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
GET    /api/templates
POST   /api/templates
GET    /api/templates/{id}
DELETE /api/templates/{id}
```
Built-in and user-created pipeline templates. System templates are read-only. The list endpoint does not accept filter query parameters.

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
POST /api/ai/translate          # NL intent → validated .oxoflow (JSON response)
POST /api/ai/translate/stream   # Same, streamed over SSE (progress → done events)
POST /api/ai/explain            # Explain run failure + suggest fix
POST /api/ai/interpret          # Interpret results with caveats
POST /api/ai/optimize           # Optimize pipeline parameters
GET  /api/ai/config             # Get AI provider configuration
POST /api/ai/config             # Update AI provider configuration
POST /api/ai/test               # Test the configured AI provider
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
Returns scheduler status (SLURM, PBS/Torque, LSF, SGE), available queues, and node count. This route is only mounted when the server runs in `hpc` mode.

---

## DAG Editing

```
POST /api/pipeline/{id}/command   # Apply an edit command (add/remove rule, connect, ...)
POST /api/pipeline/{id}/undo      # Revert the last edit
POST /api/pipeline/{id}/redo      # Re-apply the last undone edit
```

See [DAG Edit API](dag-edit-api.md) for the full command reference.

---

## See Also

- [System Architecture](architecture.md) — Domain-driven module structure
- [Web System Architecture](web-system-architecture.md) — Router design and middleware
- [AI Translation Layer](ai-translation.md) — AI integration design
- [Diagnostics Engine](diagnostics-engine.md) — Error pattern library
- [Deployment Modes](../how-to/deploy-modes.md) — Personal/Team/HPC
