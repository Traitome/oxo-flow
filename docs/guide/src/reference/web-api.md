# Web API

oxo-flow includes a built-in REST API server for remotely building, validating, and monitoring workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and runs on the tokio async runtime.

---

## Starting the Server

```bash
oxo-flow serve                         # localhost:8080
oxo-flow serve --host 0.0.0.0 -p 3000 # all interfaces, port 3000
```

---

## Endpoints

### Health Check

```
GET /api/health
```

Returns server status and version.

**Response:**

```json
{
  "status": "ok",
  "version": "0.3.0"
}
```

---

### List Workflows

```
GET /api/workflows
```

Returns a list of loaded workflows.

**Response:**

```json
{
  "workflows": [
    {
      "name": "my-pipeline",
      "version": "1.0.0",
      "rules_count": 5
    }
  ]
}
```

---

### Validate Workflow

```
POST /api/workflows/validate
Content-Type: application/json
```

Validates TOML workflow content and returns parse/DAG results.

**Request:**

```json
{
  "toml_content": "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\ninput = []\noutput = [\"out.txt\"]\nshell = \"echo hi > out.txt\""
}
```

**Response (valid):**

```json
{
  "valid": true,
  "errors": [],
  "rules_count": 1,
  "edges_count": 0
}
```

**Response (invalid):**

```json
{
  "valid": false,
  "errors": ["expected `=`, found newline at line 5 column 1"],
  "rules_count": null,
  "edges_count": null
}
```

---

### Get Workflow Graph

```
POST /api/workflows/graph
Content-Type: application/json
```

Returns the DAG in DOT format for a given workflow.

**Request:**

```json
{
  "toml_content": "..."
}
```

**Response:**

```json
{
  "dot": "digraph workflow { ... }"
}
```

---

### List Environments

```
GET /api/environments
```

Returns available environment backends.

**Response:**

```json
{
  "available": ["conda", "docker", "singularity", "venv"]
}
```

---

## Data Types

### WorkflowSummary

```json
{
  "name": "string",
  "version": "string",
  "rules_count": 0
}
```

### WorkflowDetail

```json
{
  "name": "string",
  "version": "string",
  "description": "string | null",
  "author": "string | null",
  "rules_count": 0,
  "rules": [
    {
      "name": "string",
      "inputs": ["string"],
      "outputs": ["string"],
      "environment": "string",
      "threads": 0
    }
  ]
}
```

### ValidateRequest

```json
{
  "toml_content": "string"
}
```

### ValidateResponse

```json
{
  "valid": true,
  "errors": ["string"],
  "rules_count": 0,
  "edges_count": 0
}
```

---

## CORS

CORS is enabled by default, allowing requests from any origin. This makes the API accessible from web-based frontends and development tools.

---

## Authentication

The current version does not include authentication. For production deployments, place the server behind a reverse proxy (nginx, Caddy, Traefik) that handles TLS and authentication.

---

## Error Handling

All endpoints return standard HTTP status codes:

| Status | Meaning |
|---|---|
| `200` | Success |
| `400` | Bad request (invalid TOML, missing fields) |
| `404` | Resource not found |
| `500` | Internal server error |

Error responses include a JSON body with an `error` field and an optional `detail` field:

```json
{
  "error": "description of what went wrong",
  "detail": "more specific context or internal error message"
}
```

---

## Metrics

### Runtime Metrics

```
GET /api/metrics
```

Returns current system usage, request counts, and execution metrics.

**Response:**

```json
{
  "uptime_secs": 86400.5,
  "version": "0.3.0",
  "pid": 1234,
  "os": "linux",
  "arch": "x86_64",
  "cpu_count": 16,
  "total_requests": 1542,
  "active_workflows": 3
}
```

---

## See Also

- [`serve` command](../commands/serve.md) — CLI reference
- [System Architecture](./architecture.md) — how the web layer fits in
