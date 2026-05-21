# Contributing

Thank you for considering contributing to oxo-flow! This guide covers everything you need to get started.

---

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
# Runs: cargo fmt --check, cargo clippy -D warnings, cargo build, cargo test
```

---

## Project Structure

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/   # Core library: DAG, executor, environments, reporting
│   ├── oxo-flow-cli/    # CLI binary (oxo-flow command)
│   ├── oxo-flow-web/    # Web REST API (axum-based)
├── pipelines/           # Pipeline definitions (.oxoflow files)
├── examples/            # Example workflows
├── tests/               # Integration tests
└── docs/                # Documentation (MkDocs)
```

---

## Code Style

| Convention | Rule |
|---|---|
| **Formatting** | `cargo fmt` — enforced in CI |
| **Linting** | `cargo clippy -- -D warnings` — all warnings are errors |
| **Error handling** | `thiserror` for library errors, `anyhow` for binary errors |
| **Naming** | `snake_case` for functions, `PascalCase` for types |
| **Async** | `tokio` for async runtime |
| **Logging** | `tracing` for structured logging |

---

## Making Changes

### 1. Create a branch

```bash
git checkout -b feat/my-feature
```

### 2. Make your changes

- Keep changes focused — one feature or fix per PR
- Add tests for new functionality
- Update documentation if behavior changes

### 3. Run CI checks

```bash
make ci
```

All four checks must pass:

- [x] `cargo fmt -- --check`
- [x] `cargo clippy --workspace -- -D warnings`
- [x] `cargo build --workspace`
- [x] `cargo test --workspace`

### 4. Commit with Conventional Commits

We use [Conventional Commits](https://www.conventionalcommits.org/) for automated changelog generation:

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
- Include a clear description of what changed and why
- Reference related issues if applicable
- Ensure CI passes

---

## Adding a New CLI Subcommand

1. Add the Clap subcommand variant in `crates/oxo-flow-cli/src/main.rs`
2. Implement the handler function
3. Add integration tests
4. Update CLI documentation in `docs/guide/src/commands/`

---

## Adding a New Core Feature

1. Add or modify the relevant module in `crates/oxo-flow-core/src/`
2. Update `lib.rs` to export new public types
3. Add unit tests in the same file
4. Update API documentation

---

## Testing Guidelines

| Test type | Location |
|---|---|
| Unit tests | `#[cfg(test)]` modules within source files |
| Integration tests | `tests/` directory at workspace root |
| Web API tests | `crates/oxo-flow-web/src/lib.rs` |
| CLI tests | `crates/oxo-flow-cli/src/main.rs` |

Run all tests:

```bash
cargo test --workspace --verbose
```

---

## Documentation

- MkDocs source: `docs/guide/src/`
- Build locally: `cd docs/guide && mkdocs serve`
- API docs: `cargo doc --workspace --open`
- When CLI behavior changes, update the corresponding command reference page

---

## Licensing

| Crate | License |
|---|---|
| oxo-flow-core | Apache 2.0 |
| oxo-flow-cli | Apache 2.0 |
| oxo-flow-web | Dual (Academic free / Commercial paid) |

By contributing, you agree that your contributions will be licensed under the same terms as the project.

---

## We Need Your Real-World Feedback

oxo-flow is designed for real-world bioinformatics workflows, but we need your help to make it truly robust across diverse deployment scenarios.

### Why Your Feedback Matters

Our CI pipeline validates basic functionality, but it cannot replicate the complexity of real-world deployments:

- **Cluster configurations** vary widely (SLURM partitions, PBS queues, SGE complexes, LSF queues)
- **Environment setups** differ (conda channels, module systems, Singularity versions)
- **Bioinformatics tools** have version-specific quirks
- **GPU scheduling** behavior varies by cluster
- **Data scales** from single samples to population cohorts
- **Workflow patterns** include complex dependencies, conditional execution, scatter-gather

### How to Contribute Feedback

**Success Stories**: Share your workflow that worked perfectly — helps us document proven patterns.

**Problem Reports**: Even "small" issues matter — environment detection failures, unclear error messages, unexpected scheduling behavior.

**Feature Requests**: Missing something? Tell us what would make oxo-flow better for your use case.

### Opening an Issue

When reporting real-world testing results, please use the `[Real-World Testing]` prefix in your issue title:

```
[Real-World Testing] SLURM GPU job scheduling fails on cluster with multiple partitions
[Real-World Testing] Conda environment detection issue with custom channels
[Real-World Testing] Success: Completed 500-sample WGS pipeline on PBS cluster
```

Include details about:

- Your cluster type and version (e.g., "SLURM 22.05.3")
- The workflow or command you ran
- What happened vs. what you expected
- Any error messages or logs (sanitized if needed)

Your feedback directly shapes our roadmap. Thank you for helping make oxo-flow better!

---

## Questions?

Open an issue at [github.com/Traitome/oxo-flow/issues](https://github.com/Traitome/oxo-flow/issues) for questions, bug reports, or feature requests.
