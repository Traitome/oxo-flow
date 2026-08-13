# AI Agent Context & Instructions: oxo-flow

This document is the primary source of truth for AI agents working on oxo-flow.

**⚠️ CHANGE MANAGEMENT: When you make any structural change (new crate, new subcommand, renamed flag, version bump, port change, provider change, domain change), you MUST update this file AND the corresponding docs in `docs/guide/src/`. Cross-reference README.md for consistency. This is not optional — stale docs cause cascading errors in AI-assisted development.**

## 🎯 Project Overview
`oxo-flow` is a high-performance bioinformatics pipeline engine built in Rust. It focuses on reproducibility, environment isolation, and AI-assisted workflow development.

## 🏗️ Workspace Layout
- `crates/oxo-flow-core`: DAG engine, executor, environment management, config, scheduling, reporting
- `crates/oxo-flow-ai`: AI companion — provider abstraction, skill system, agent orchestration, MCP
- `crates/oxo-flow-cli`: CLI binary (`oxo-flow`) — 33 subcommands via clap derive
- `crates/oxo-flow-web`: Axum web server + React 19 SPA, 50+ REST endpoints across 7 domains
- `examples/`: Reference `.oxoflow` (TOML-based) pipeline files
- `tests/`: Integration tests covering CLI and core functionality

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
2. **Environment Agnostic:** Support 8 backends: conda, mamba, pixi, docker, singularity, venv, system, HPC modules.
3. **Wildcard Expansion:** Native support for `{sample}`, `{chr}` patterns with regex constraints.
4. **Reproducibility First:** Every execution must be reproducible and auditable via provenance.

## 📝 Coding Standards
- **Type Safety:** No `unsafe` unless strictly justified.
- **Documentation:** Public APIs in `oxo-flow-core` should have doc comments.
- **Testing:** New features require corresponding unit or integration tests.
- **Errors:** Return `Result` early; use context where helpful.
- **Markdown lists:** Always insert a blank line between a text paragraph and a list (`-`/`*`). A paragraph followed immediately by `-` on the next line will NOT render as a list in many Markdown engines — the `-` is shown as literal text. Do NOT insert blank lines between items within the same list; only between a preceding paragraph and the list start.

## 📚 Documentation System
- `docs/guide/` — MkDocs-based user guide (run `mkdocs serve` in `docs/guide/` to preview)
- `docs/guide/src/reference/web-api.md` — REST API reference (structured errors, endpoints)
- `docs/guide/src/reference/web-system-architecture.md` — Web system architecture
- `docs/guide/src/reference/architecture.md` — Overall system architecture
- `docs/guide/src/reference/ai-cli.md` — Complete AI CLI reference
- `docs/schema/openapi.yaml` — OpenAPI 3.1 schema
- `docs/schema/oxoflow-v1.schema.json` — Workflow format JSON Schema

## 🌐 Web System (AI-Native API)
The web crate (`oxo-flow-web`) is designed as an AI-native API surface:

- **Structured errors**: All responses use `{code, message, detail, suggestion}` format
- **API discovery**: `GET /api/openapi.json` returns full OpenAPI 3.1 schema
- **Intent-driven authoring**: `POST /api/workflows/generate` maps natural language to pipelines
- **SSE streaming**: `GET /api/events` provides real-time execution events
- **Pagination**: List endpoints use `{data, meta: {page, per_page, total_items, total_pages}}` envelope

## 🖥️ Frontend SPA
The frontend is a React/TypeScript SPA built with Vite, located in `frontend/`:

```bash
# Development mode (two terminals):
cd frontend && npm run dev    # Vite dev server on :5173, proxies /api to :8080
cargo run -p oxo-flow-web      # API server on :8080

# Build for production:
cd frontend && npm run build   # Outputs to frontend/dist/
```

### Web Module Structure

The web crate uses a domain-driven modular monolith with 8 domains:

- `domains/workflow/` — Pipeline parsing, validation, DAG building, formatting
- `domains/execution/` — Run lifecycle, diagnostics engine (30+ patterns), retry logic
- `domains/ai/` — AI copilot prompts, service orchestration, handlers, agents
- `domains/auth/` — Authentication, OAuth stubs, license endpoints
- `domains/collaboration/` — Pipeline fork, share, import
- `domains/observability/` — Health checks, system info, metrics
- `domains/dag/` — DAG-specific types and operations
- `domains/chat/` — Real-time chat and SSE streaming

Each domain follows: `types.rs` (data) → `service.rs` (pure logic) → `handlers.rs` (HTTP adapters).

## 🐳 Docker Deployment

### Quick Start
```bash
# Build and start (single command)
docker compose up -d

# Or build manually
docker build -t oxo-flow .
docker run -d -p 8080:8080 -v oxo-flow-data:/app/data oxo-flow
```

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `OXO_FLOW_AI_PROVIDER` | No | `disabled` | `"claude"`, `"openai"`, `"deepseek"`, `"ollama"`, or `"disabled"` |
| `OXO_FLOW_AI_API_KEY` | No | — | Generic API key fallback |
| `OXO_FLOW_AI_API_URL` | No | (provider default) | Custom API endpoint URL |
| `OXO_FLOW_AI_MODEL` | No | (provider default) | Model name override |
| `ANTHROPIC_AUTH_TOKEN` | No | — | Claude/Anthropic API key (overrides OXO_FLOW_AI_API_KEY) |
| `ANTHROPIC_BASE_URL` | No | `https://api.anthropic.com` | Anthropic-compatible API base URL |
| `ANTHROPIC_MODEL` | No | `claude-sonnet-4-20250514` | Claude model name |
| `OPENAI_API_KEY` | No | — | OpenAI-compatible API key (overrides OXO_FLOW_AI_API_KEY) |
| `OPENAI_BASE_URL` | No | `https://api.openai.com/v1` | OpenAI-compatible API base URL |
| `OPENAI_MODEL` | No | `gpt-4o` | OpenAI-compatible model name |
| `OXO_FLOW_FRONTEND_DIR` | No | — | Path to built frontend dist directory |

### AI Provider Examples

**Claude (Anthropic):**
```bash
OXO_FLOW_AI_PROVIDER=claude ANTHROPIC_AUTH_TOKEN=sk-ant-<YOUR-KEY> docker compose up -d
```

**DeepSeek via Anthropic-compatible API:**
```bash
OXO_FLOW_AI_PROVIDER=claude \
  ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic \
  ANTHROPIC_AUTH_TOKEN=sk-<YOUR-KEY> \
  ANTHROPIC_MODEL=deepseek-chat \
  docker compose up -d
```

**OpenAI-compatible (DeepSeek, Groq, Azure, etc.):**
```bash
OXO_FLOW_AI_PROVIDER=openai \
  OPENAI_BASE_URL=https://api.deepseek.com/v1 \
  OPENAI_API_KEY=sk-<YOUR-KEY> \
  OPENAI_MODEL=deepseek-chat \
  docker compose up -d
```

**Ollama (local):**
```bash
OXO_FLOW_AI_PROVIDER=ollama docker compose up -d
```

## 🔧 AI Provider Architecture

The AI provider system supports four backends via an enum-based dispatcher:

- **Claude** — Anthropic Messages API (including Anthropic-compatible third-party endpoints)
- **OpenAI** — OpenAI Chat Completions API (compatible with DeepSeek, Groq, Azure, Together, etc.)
- **DeepSeek** — Native DeepSeek API (default provider, auto-detected at startup)
- **Ollama** — Local Ollama API (default: `http://localhost:11434`)

Providers are selected at startup via `OXO_FLOW_AI_PROVIDER` env var and initialized once through `AiProviderRegistry::global()`. The `try_ai_generate()` function in `handlers/ai.rs` uses the configured provider, falling back to template matching if AI is disabled or fails.

## 🤖 AI CLI Integration (v0.10.0+)

The `oxo-flow-ai` crate provides shared AI infrastructure for CLI and web.

### Quick Start

```bash
# Configure any AI provider
export OXO_FLOW_AI_PROVIDER=deepseek    # or claude, openai, ollama
export DEEPSEEK_API_KEY="sk-<YOUR-KEY>"

# Check status
oxo-flow ai                            # Shows provider, endpoint, test connectivity

# One-line enable in .oxoflow file
[ai]
enabled = true                          # All AI commands auto-activate

# Generate workflows
oxo-flow template "RNA-seq with STAR" --ai
oxo-flow template "variant calling" --ai --from-url https://example.com/protocol

# Analyze workflows (auto-detect — no --ai needed if [ai] enabled)
oxo-flow dry-run workflow.oxoflow      # AI resource/safety audit
oxo-flow validate workflow.oxoflow     # AI semantic validation
oxo-flow lint workflow.oxoflow         # AI best-practice check
oxo-flow debug workflow.oxoflow        # AI command explanation

# Error recovery (auto-detect from [ai] section)
oxo-flow run workflow.oxoflow          # AI diagnoses + fixes on failure
oxo-flow run workflow.oxoflow --ai-recover --ai-max-retries 3
```

### Architecture

| Crate | Role |
|-------|------|
| `oxo-flow-ai` | L1 Provider (4 backends) + L2 Knowledge + L3 Agent/Tools |
| `oxo-flow-cli` | L4 Command integration: template, dry-run, validate, lint, debug, run, resume, ai |
| `oxo-flow-web` | Compatibility shim over oxo-flow-ai |

### Key Design Decisions

- **AI is optional**: no `[ai]` section → zero AI overhead
- **One line to enable**: `[ai] enabled = true` in .oxoflow → all commands auto-detect
- **Provider-agnostic**: OpenAI-compatible (DeepSeek, Groq, Together), Anthropic-compatible (Claude), Ollama local
- **Everything archived**: AI sessions + modifications auto-saved to `.oxo-flow/ai_sessions/`
- **5-layer architecture**: L1 Provider → L2 Knowledge/Config → L3 Agent/Tools → L4 Commands → Session

### Frontend Architecture
- **Framework**: React 19 + TypeScript, Vite build
- **Routing**: React Router (client-side SPA routing)
- **DAG Visualization**: Cytoscape.js with dagre layout
- **API Integration**: Fetch-based client with structured error handling
- **Real-time**: SSE via EventSource for run lifecycle events
- **Styling**: CSS custom properties, light theme, responsive layout

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
