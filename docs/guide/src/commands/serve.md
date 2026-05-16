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
| `--host` | — | `127.0.0.1` | Host address to bind to |
| `--port` | `-p` | `8080` | Port to listen on |
| `--base-path` | — | `/` | Base path for mounting under a sub-path (e.g., `/oxo-flow`) |
| `--verbose` | `-v` | — | Enable debug-level logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |

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

When using `--base-path`, all API endpoints will be prefixed:
```
GET /oxo-flow/api/health
POST /oxo-flow/api/workflows/validate
```

---

## Output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
Starting web server at 127.0.0.1:8080 ...
```

---

## API Endpoints

Once the server is running, the following REST endpoints are available:

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/health` | Health check (status + version) |
| `GET` | `/api/workflows` | List loaded workflows |
| `POST` | `/api/workflows/validate` | Validate TOML workflow content |
| `POST` | `/api/workflows/graph` | Get DAG in DOT format |
| `GET` | `/api/environments` | List available environment backends |

### Example: Health check

```bash
curl http://127.0.0.1:8080/api/health
```

```json
{
  "status": "ok",
  "version": "0.4.1"
}
```

### Example: Validate a workflow

```bash
curl -X POST http://127.0.0.1:8080/api/workflows/validate \
  -H "Content-Type: application/json" \
  -d '{"toml_content": "[workflow]\nname = \"test\"\n[[rules]]\nname = \"s1\"\ninput = []\noutput = [\"out.txt\"]\nshell = \"echo hi > out.txt\""}'
```

```json
{
  "valid": true,
  "errors": [],
  "rules_count": 1,
  "edges_count": 0
}
```

---

## Notes

- The web server is built with [axum](https://github.com/tokio-rs/axum) and runs on the tokio async runtime
- CORS is enabled by default, allowing requests from any origin
- The server is intended for development and internal use — for production deployments, place it behind a reverse proxy (nginx, Caddy)
- See the [Web API reference](../reference/web-api.md) for complete endpoint documentation

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
