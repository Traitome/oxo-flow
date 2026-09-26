# Web API

oxo-flow includes a built-in REST API server for building, validating, running, and monitoring bioinformatics workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and follows a **domain-driven modular monolith** architecture.

---

## API Design Conventions

- **Envelope**: success responses are bare JSON objects/arrays; errors are `{ code, message, detail?, suggestion? }`
- **Errors**: `{ code: "E001", message, detail?, suggestion? }`
- **Lists**: `GET /api/runs` returns a cursor-paginated envelope `{ items, next_cursor, total }` (limit ≤ 500, `status`/`q` filters); other list endpoints return bare arrays (≤ 100 items)
- **Versioning**: `/api/` prefix for all endpoints
- **Authentication**: in team/hpc mode, protected endpoints accept an `Authorization: Bearer <token>` session token or an `X-API-Key` header. The generated OpenAPI spec at `GET /api/openapi.json` declares both `bearerAuth` and `apiKey` security schemes; public endpoints (health, login, license, openapi.json, etc.) require no auth.
- **Self-discoverable**: OpenAPI 3.1 spec at `GET /api/openapi.json`. The spec is **code-generated** via [utoipa](https://github.com/juhaku/utoipa) from the `#[utoipa::path]` annotations on every route handler — there is no hand-maintained static file. `crates/oxo-flow-web/tests/openapi_gate.rs` is the drift gate: it asserts every route in the router appears in the generated spec, so a new route without an annotation fails CI.

### Structured Error Format

All errors return a unified JSON format: