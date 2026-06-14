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
cargo test --workspace --lib    # Tests (890+)
```

## Project Structure

```
crates/oxo-flow-core/    # DAG engine, executor, config, scheduling
crates/oxo-flow-cli/     # CLI binary (31 subcommands)
crates/oxo-flow-web/     # Web server (axum REST API + React SPA)
frontend/                # React 19 TypeScript SPA
```

## Commit Convention

```
<type>: <description>
```
Types: feat, fix, refactor, docs, test, chore, perf, ci

## Testing

- **Rust**: 890 unit + integration tests across workspace
- **Browser**: Playwright E2E — 100 scenarios × 10 lifecycle stages
- **Run**: `cargo test --workspace --lib`

## Code Style

- Rust 2024 edition, `#![forbid(unsafe_code)]`
- TypeScript strict mode, no `any`, explicit return types
- Immutability preferred — create new objects, never mutate
- Early returns over deep nesting
