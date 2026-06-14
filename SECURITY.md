# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.8.x   | ✅ Active          |
| < 0.8   | ❌ No longer supported |

## Reporting a Vulnerability

**Do not open a public issue.** Email: **w_shixiang@163.com**

Response within 48 hours. Fixes published within 7 days.

## Security Design

- **`#![forbid(unsafe_code)]`** — No unsafe Rust in core, CLI, or web
- **Shell injection prevention** — Commands parsed and sanitized before execution
- **Path traversal protection** — I/O paths validated against workspace boundaries
- **API key encryption** — Provider keys encrypted at rest
- **Rate limiting** — Per-IP on auth and chat endpoints
- **No secrets in source** — Credentials via env vars or encrypted config
- **Input validation** — Structured error responses `{code, message, detail}`

## Trust Boundary

AI service layer: zero DB write, zero process management, zero filesystem write.
AI never executes autonomously — every action requires human approval.

## Dependency Auditing

`cargo audit` runs in CI. `Cargo.lock` committed. Vulnerabilities addressed within 7 days.
