# Contributing to oxo-flow

Thank you for considering contributing to oxo-flow! This guide will help you
get set up and familiar with our workflow.

## Development Setup

### Prerequisites

- **Rust 1.85+** (edition 2024) — [rustup.rs](https://rustup.rs)
- **Git 2.x+**
- **MkDocs 1.5+** (optional, for docs) — `pip install mkdocs-material`

### Clone and Build

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --workspace --verbose
```

### Run Tests

```bash
cargo test --workspace --verbose
```

### Run All CI Checks Locally

```bash
make ci
# This runs: cargo fmt --check, cargo clippy -D warnings, cargo build, cargo test
```

## Project Structure

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/   # Core library: DAG, executor, environments, reporting
│   ├── oxo-flow-cli/    # CLI binary (oxo-flow command)
│   ├── oxo-flow-web/    # Web REST API (axum-based)
│   └── venus/           # Venus tumor variant calling pipeline
├── pipelines/           # Pipeline definitions (.oxoflow files)
├── examples/            # Example workflows
├── tests/               # Integration tests
└── docs/                # Documentation (MkDocs)
```

## Code Style

- **Formatting**: `cargo fmt` is enforced in CI
- **Linting**: `cargo clippy -- -D warnings` — all warnings are errors
- **Safety**: `#![forbid(unsafe_code)]` in all crates — zero unsafe code allowed
- **Error handling**: Use `thiserror` for library errors, `anyhow` for binary errors
- **Result types**: Add `#[must_use]` to public functions returning `Result`
- **Naming**: `snake_case` for functions, `PascalCase` for types
- **Async**: Use `tokio` for async runtime
- **Logging**: Use `tracing` for structured logging

## Making Changes

### 1. Create a Branch

```bash
git checkout -b feat/my-feature
```

### 2. Make Your Changes

- Keep changes focused — one feature or fix per PR
- Add tests for new functionality
- Update documentation if behavior changes

### 3. Run CI Checks

```bash
make ci
```

All four checks must pass:
- `cargo fmt -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo build --workspace`
- `cargo test --workspace`

### 4. Commit with Conventional Commits

We use [Conventional Commits](https://www.conventionalcommits.org/) for
automated changelog generation:

```
feat: add SLURM job array support
fix: correct wildcard expansion for nested patterns
docs: update CLI reference for new subcommands
test: add integration tests for web API
refactor: simplify DAG cycle detection
chore: update dependencies
```

### 5. Submit a Pull Request

- Target the `main` branch
- Use the [PR template](.github/PULL_REQUEST_TEMPLATE.md) — it includes a checklist
- Include a clear description of what changed and why
- Reference related issues if applicable
- Ensure CI passes

## Adding a New CLI Subcommand

1. Add the Clap subcommand variant in `crates/oxo-flow-cli/src/main.rs`
2. Implement the handler function
3. Add integration tests
4. Update CLI documentation in `docs/guide/src/commands/`

## Adding a New Core Feature

1. Add or modify the relevant module in `crates/oxo-flow-core/src/`
2. Update `lib.rs` to export new public types
3. Add unit tests in the same file
4. Update API documentation

## Testing Guidelines

- **Unit tests**: Place in `#[cfg(test)]` modules within the source file
- **Integration tests**: Place in `tests/` directory at workspace root
- **Web API tests**: Use axum's test utilities in `crates/oxo-flow-web/src/lib.rs`
- **CLI tests**: Test command-line behavior in `crates/oxo-flow-cli/src/main.rs`

Run all tests:

```bash
cargo test --workspace --verbose
```

## Documentation

- MkDocs source lives in `docs/guide/src/`
- Build locally: `cd docs/guide && mkdocs serve`
- API documentation: `cargo doc --workspace --open`
- When CLI behavior changes, update the corresponding command reference page

## Licensing

- **oxo-flow-core**, **oxo-flow-cli**, **oxo-flow-venus**: Apache 2.0
- **oxo-flow-web**: Dual license (Academic free / Commercial paid)
  - See `LICENSE-ACADEMIC` and `LICENSE-COMMERCIAL`

By contributing, you agree that your contributions will be licensed under
the same terms as the project.

## Questions?

- 🐛 **Bug reports** — Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.md)
- 💡 **Feature requests** — Use the [feature request template](.github/ISSUE_TEMPLATE/feature_request.md)
- 🔒 **Security issues** — See [SECURITY.md](SECURITY.md) for responsible disclosure
- ❓ **General questions** — Open a [GitHub Issue](https://github.com/Traitome/oxo-flow/issues)

## Governance

See [GOVERNANCE.md](GOVERNANCE.md) for the project's decision-making process.
