# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.25.x  | Active             |
| 0.24.x  | Security fixes     |
| < 0.24  | No longer supported |

## Reporting a Vulnerability

**Do not open a public issue.** Email: **w_shixiang@163.com**

Response within 48 hours. Fixes published within 7 days.

## Security Design

- **`#![forbid(unsafe_code)]`** — No unsafe Rust in core and web crates
- **Shell injection prevention** — Commands parsed and sanitized before execution
- **Path traversal protection** — I/O paths validated against workspace boundaries
- **API key encryption** — Provider keys are encrypted at rest when `OXO_FLOW_MASTER_KEY` is configured; otherwise the local database uses the documented plaintext compatibility mode
- **Rate limiting** — Per-IP on auth and chat endpoints
- **No secrets in source** — Credentials come from environment variables, a
  configured master-key-sealed store, or the documented plaintext compatibility
  mode; plaintext mode is not suitable for shared production hosts
- **Input validation** — Structured error responses `{code, message, detail}`

## Trust Boundary

The deterministic engine, web authorization layer, AI tools, MCP servers, workflow
shells, and execution backends are separate trust domains. AI output is untrusted
input and must pass deterministic parsing, validation, and the relevant approval
path before it can affect a run.

This separation is not yet a complete security boundary:

- workflow shells can inherit ambient process credentials;
- team/HPC execution has no OS-level tenant isolation boundary;
- expanded sensitive values can reach scheduler and container artifacts;
- malformed tool arguments can fail open, and a generic MCP token can be sent to
  any activated MCP endpoint;
- malformed or partial AI repairs can overwrite a complete workflow until the
  repair path is hardened.

Treat AI-assisted workflows as authoring aids, validate every generated artifact,
and do not expose team/HPC execution to untrusted workflow authors. The
release-blocking evidence and exit criteria are maintained in
[`docs/superpowers/audits/industrial-scientific-audit-ledger.md`](docs/superpowers/audits/industrial-scientific-audit-ledger.md).

## Dependency Auditing

`cargo audit` runs in CI. `Cargo.lock` committed. Vulnerabilities addressed within 7 days.
