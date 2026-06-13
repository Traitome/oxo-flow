# AI Agent Context & Instructions: oxo-flow

This document serves as the primary source of truth for AI agents (Copilot, Gemini, Cursor, etc.) to ensure consistency, reliability, and adherence to project standards.

## 🎯 Project Overview
`oxo-flow` is a clinical-grade bioinformatics pipeline engine built in Rust. It focuses on performance, reproducibility, and rigorous environment management.

## 🏗️ Workspace Layout
- `crates/oxo-flow-core`: The heart of the engine. DAG resolution, execution logic, environment management, and core types.
- `crates/oxo-flow-cli`: Command-line interface.
- `crates/oxo-flow-web`: Axum-based web server and API.
- `examples/`: Reference `.oxoflow` (TOML-based) pipeline files.
- `tests/`: Integration tests covering CLI and core functionality.

## 🛠️ Tech Stack & Conventions
- **Language:** Rust (Edition 2024).
- **Async:** `tokio` for concurrency.
- **Error Handling:** `thiserror` for library errors; `anyhow` for CLI/Bin.
- **State/Config:** `serde` with TOML as the primary format.
- **Logging:** Structured logging via `tracing`.
- **CLI:** `clap` with the derive API.
- **Graph Logic:** `petgraph` for DAG operations.

## 🚦 Development Workflow (Mandatory)
Before concluding any task, the following suite **must** pass:
```bash
make ci
```
*Included in `make ci`: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo build`, and `cargo test`.*

## 🧠 Key Design Principles
1. **DAG-First Execution:** Everything is a graph. Validate dependencies before execution.
2. **Environment Agnostic:** Support Conda, Pixi, Docker, and Venv seamlessly.
3. **Wildcard Expansion:** Native support for `{sample}`, `{chr}` patterns.
4. **Clinical Rigor:** Every execution must be reproducible and auditable.

## 📝 Coding Standards
- **Type Safety:** No `unsafe` unless strictly justified.
- **Documentation:** Public APIs in `oxo-flow-core` should have doc comments.
- **Testing:** New features require corresponding unit or integration tests.
- **Errors:** Return `Result` early; use context where helpful.
- **Markdown lists:** Always insert a blank line between a text paragraph and a list (`-`/`*`). A paragraph followed immediately by `-` on the next line will NOT render as a list in many Markdown engines — the `-` is shown as literal text. Do NOT insert blank lines between items within the same list; only between a preceding paragraph and the list start.

## 📚 Documentation System
- `docs/guide/` — MkDocs-based user guide (run `mkdocs serve` in `docs/guide/` to preview)
- `docs/guide/src/reference/web-api.md` — REST API reference (structured errors, endpoints)
- `docs/guide/src/reference/web-system-architecture.md` — Web system architecture and AI-native API design
- `docs/guide/src/reference/architecture.md` — Overall system architecture
- `docs/schema/` — OpenAPI 3.0 schema and workflow JSON schema

## 🌐 Web System (AI-Native API)
The web crate (`oxo-flow-web`) is designed as an AI-native API surface:

- **Structured errors**: All responses use `{code, message, detail, suggestion}` format
- **API discovery**: `GET /api/openapi.json` returns full OpenAPI 3.0 schema
- **Intent-driven authoring**: `POST /api/workflows/generate` maps natural language to pipelines
- **SSE streaming**: `GET /api/events` provides real-time execution events
- **Pagination**: List endpoints use `{data, meta: {page, per_page, total_items, total_pages}}` envelope

## 🖥️ Frontend SPA
The frontend is a React/TypeScript SPA built with Vite, located in `frontend/`:

```bash
# Development mode (two terminals):
cd frontend && npm run dev    # Vite dev server on :5173, proxies /api to :3000
cargo run -p oxo-flow-web      # API server on :3000

# Build for production:
cd frontend && npm run build   # Outputs to frontend/dist/
```

### Frontend Architecture
- **Framework**: React 19 + TypeScript, Vite build
- **Routing**: React Router (client-side SPA routing)
- **DAG Visualization**: Cytoscape.js with dagre layout
- **API Integration**: Fetch-based client with structured error handling
- **Real-time**: SSE via EventSource for run lifecycle events
- **Styling**: CSS custom properties, dark theme, responsive layout

### Pages
| Route | Page | Description |
|-------|------|-------------|
| `/` | Dashboard | Engine status, recent runs, quick actions |
| `/editor` | Pipeline Editor | TOML editor + interactive DAG viewer + AI generate |
| `/pipelines` | Pipeline Library | Browse and use pipeline templates |
| `/runs` | Run Monitor | Run history, SSE live events, run details |

### Key Components
- `components/Layout.tsx` — Sidebar navigation + main content outlet
- `components/DagView.tsx` — Interactive DAG using Cytoscape.js
- `api/client.ts` — Typed API client with structured error handling
- `api/types.ts` — TypeScript types matching the Rust API responses
