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
