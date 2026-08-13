# Contributing to oxo-flow

## Development Setup

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --workspace
```

## CI Checks (run before PR)

```bash
cargo fmt --all -- --check     # Formatting
cargo clippy --workspace -- -D warnings  # Lint
cargo build --workspace         # Compile
cargo test --workspace --lib    # Tests (1,280+)
```

## Project Structure

```
crates/oxo-flow-core/    # DAG engine, executor, config, scheduling
crates/oxo-flow-ai/      # AI companion: providers, skills, agents
crates/oxo-flow-cli/     # CLI binary (29 subcommands)
crates/oxo-flow-web/     # Web server (axum REST API + React SPA)
frontend/                # React 19 TypeScript SPA
```

## Commit Convention

```
<type>: <description>
```
Types: feat, fix, refactor, docs, test, chore, perf, ci

## Testing

- **Rust**: 1,280+ unit, integration, and doc tests across workspace
- **Run**: `cargo test --workspace`

## Code Style

- Rust 2024 edition, `#![forbid(unsafe_code)]` in core and web crates
- TypeScript strict mode, no `any`, explicit return types
- Immutability preferred — create new objects, never mutate
- Early returns over deep nesting
