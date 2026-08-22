# `oxo-flow serve`

Start the web interface server. Provides a REST API for building, validating, and monitoring workflows remotely.

---

## Usage

```
oxo-flow serve [OPTIONS]
```

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--host` | — | `127.0.0.1` | Host address to bind to (env: `OXO_FLOW_HOST`) |
| `--port` | `-p` | `8080` | Port to listen on (env: `OXO_FLOW_PORT`) |
| `--mode` | — | `personal` | Deployment mode: `personal`, `team`, or `hpc` (env: `OXO_FLOW_MODE`) |
| `--base-path` | — | `/` | Base path for mounting under a sub-path, e.g. `/oxo-flow` (env: `OXO_FLOW_BASE_PATH`) |
| `--open` | — | — | Open the interface in the default browser on startup (env: `OXO_FLOW_OPEN_BROWSER`) |
| `--verbose` | `-v` | — | Enable debug-level logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |
| `--json` | — | — | Output machine-readable JSON to stdout (suppresses human-readable stderr output) |

---

## Examples

### Start with defaults

```bash
oxo-flow serve
```

### Bind to all interfaces on a custom port

```bash
oxo-flow serve --host 0.0.0.0 -p 3000
```

### Mount under a sub-path (for reverse proxy)

```bash
oxo-flow serve --base-path /oxo-flow
```

### Desktop-app experience

```bash
oxo-flow serve --open
```

Starts the server and opens the interface in the default browser. See
[Desktop App Packaging](../how-to/desktop-app.md) for single-file
`.app`/`.dmg` (macOS) and `.deb`/`.rpm`/`.AppImage` (Linux) bundles.

When using `--base-path`, all API endpoints will be prefixed:
```
GET /oxo-flow/api/health
POST /oxo-flow/api/pipelines/validate
```

---

## Output

```
oxo-flow 0.14.1 — Bioinformatics Pipeline Engine
Serve: Starting oxo-flow web server in personal mode on 127.0.0.1:8080
```

---

## API Endpoints

Once the server is running, the following REST endpoints are available. The full specification is served at `/api/openapi.json`.

!!! warning "Legacy `/api/workflows/*` routes"
    Older documentation referenced `GET /api/workflows`, `POST /api/workflows/validate`,
    `POST /api/workflows/graph`, and `GET /api/environments`. These routes come from a
    legacy router and are **not served** by `oxo-flow serve`. Use the `/api/pipelines/*`
    endpoints below instead.

### Pipeline routes

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/health` | Health check (status + version) |
| `GET` | `/api/openapi.json` | OpenAPI 3.1 specification |
| `GET` / `POST` | `/api/pipelines` | List / save pipelines |
| `POST` | `/api/pipelines/parse` | Parse TOML content into a structured pipeline |
| `POST` | `/api/pipelines/validate` | Validate pipeline TOML and DAG |
| `POST` | `/api/pipelines/prepare` | Expand wildcards and resolve environments |
| `POST` | `/api/pipelines/dag` | Build the DAG as JSON |
| `POST` | `/api/pipelines/format` | Canonical TOML formatting |
| `POST` | `/api/pipelines/lint` | Lint pipelines |
| `POST` | `/api/pipelines/stats` | Aggregate pipeline statistics |
| `POST` | `/api/pipelines/diff` | Diff two pipelines |
| `POST` | `/api/pipelines/export` | Export Docker/Singularity packaging |
| `POST` | `/api/pipelines/search` | Search pipelines by name, tags, content |
| `GET` / `PUT` / `DELETE` | `/api/pipelines/{id}` | Get / update / delete a pipeline |
| `POST` | `/api/pipelines/{id}/fork` | Fork a pipeline |
| `POST` | `/api/pipelines/{id}/share` | Share a pipeline |
| `POST` | `/api/pipelines/import` | Import a pipeline from a URL |

### Run routes

| Method | Endpoint | Description |
|---|---|---|
| `GET` / `POST` | `/api/runs` | List / create runs |
| `GET` | `/api/runs/{id}` | Run detail |
| `GET` | `/api/runs/{id}/status` | Real-time status (nodes, timeline, resources) |
| `GET` | `/api/runs/{id}/dag-status` | DAG JSON + per-node live status |
| `GET` | `/api/runs/{id}/diagnostics` | Diagnostic engine results (30+ error patterns) |
| `GET` | `/api/runs/{id}/logs` | Execution logs |
| `GET` | `/api/runs/{id}/results` | Output files and QC metrics |
| `POST` | `/api/runs/{id}/retry` | Smart retry (failed + downstream only) |
| `POST` | `/api/runs/{id}/cancel` | Cancel a running workflow |
| `POST` | `/api/runs/{id}/pause` | Pause a running workflow |
| `POST` | `/api/runs/{id}/resume` | Resume a paused workflow |
| `GET` | `/api/runs/{id}/report` | Run report |
| `POST` | `/api/runs/{id}/report/ask` | Ask a question about the report |
| `POST` | `/api/runs/{id}/report/visualize` | Visualize report data |

### Other routes

| Method | Endpoint | Description |
|---|---|---|
| `POST` | `/api/auth/login` | Login (username/password) |
| `GET` | `/api/auth/me` | Current session info |
| `GET` | `/api/license` | License status |
| `POST` | `/api/license/upload` | Upload a commercial license file |
| `POST` | `/api/ai/translate` | Natural language → validated pipeline (SSE: `/api/ai/translate/stream`) |
| `POST` | `/api/ai/explain` | Explain a run failure and suggest a fix |
| `POST` | `/api/ai/interpret` | Interpret results |
| `POST` | `/api/ai/optimize` | Optimize pipeline parameters |
| `GET` / `POST` | `/api/ai/config` | Get / update AI configuration |
| `POST` | `/api/chat/send` | Chat with the AI companion |
| `POST` | `/api/data/analyze` | Scan files → detect format, suggest pipeline |
| `GET` / `POST` | `/api/templates` | List / create templates |
| `POST` | `/api/plugins/validate` | Validate plugin manifest + signature |
| `GET` | `/api/events` | SSE event stream (real-time execution updates) |
| `GET` | `/api/hpc` | HPC scheduler status (hpc mode only) |

### Example: Health check

```bash
curl http://127.0.0.1:8080/api/health
```

```json
{
  "status": "ok",
  "version": "0.14.1",
  "mode": "personal",
  "uptime_secs": 12,
  "components": {
    "database": { "status": "ok", "latency_ms": null },
    "filesystem": { "status": "ok", "latency_ms": null },
    "scheduler": null,
    "ai_provider": null
  },
  "resources": { "cpu_pct": 0.0, "memory_used_pct": 0.0, "disk_used_pct": 0.0 },
  "license": {
    "license_type": "academic",
    "valid": true,
    "commercial_use": "requires_authorization",
    "contact": "w_shixiang@163.com",
    "message": "Free for academic use. Commercial use requires authorization."
  }
}
```

### Example: Validate a pipeline

```bash
curl -X POST http://127.0.0.1:8080/api/pipelines/validate \
  -H "Content-Type: application/json" \
  -d '{"toml_content": "[workflow]\nname = \"test\"\n[[rules]]\nname = \"s1\"\ninput = []\noutput = [\"out.txt\"]\nshell = \"echo hi > out.txt\""}'
```

```json
{
  "valid": true,
  "errors": []
}
```

---

## Notes

- The web server is built with [axum](https://github.com/tokio-rs/axum) and runs on the tokio async runtime
- CORS is restricted to localhost by default; use `OXO_FLOW_ALLOWED_ORIGINS` to override
- The server is intended for development and internal use — for production deployments, place it behind a reverse proxy (nginx, Caddy)
- See the [Web API reference](../reference/web-api.md) for complete endpoint and authentication documentation

## ⚠️ Security: Configuring Authentication

By default, all user accounts are **disabled**.  You must set at least one of the
following environment variables before starting the server, otherwise no logins
will be accepted:

```bash
export OXO_FLOW_ADMIN_PASSWORD="<strong-password>"
export OXO_FLOW_USER_PASSWORD="<strong-password>"
export OXO_FLOW_VIEWER_PASSWORD="<strong-password>"
oxo-flow serve
```

**Development mode** (local testing only): set `OXO_FLOW_DEV_MODE=1` to re-enable
the default weak passwords (`admin/admin`, `user/user`, `viewer/viewer`).  **Never
use `OXO_FLOW_DEV_MODE=1` in a production or multi-user environment.**
