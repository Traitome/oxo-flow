# Copilot Instructions for `oxo-flow`

## ⚠️ Mandatory pre-commit CI gate

**Before every call to `report_progress`, ALL of the following checks MUST pass locally with zero errors.**
Pushing code that fails any of these checks will break the CI "Test" job and is not acceptable.

```bash
# Option A – run everything in one command (preferred):
make ci

# Option B – run each step individually:
cargo fmt -- --check          # formatting (MUST pass – most commonly forgotten)
cargo clippy -- -D warnings   # zero lint warnings allowed
cargo build                   # must compile
cargo test                    # all unit + integration tests must pass
```

If `cargo fmt -- --check` reports diff output, fix it first with `cargo fmt` and re-run the check.
**Never call `report_progress` until `make ci` (or all four individual commands) exits with code 0.**

## Project overview

**oxo-flow** is a Rust-based bioinformatics pipeline engine built from first principles for performance, reproducibility, and clinical-grade rigor.
It provides a core library, CLI, and web interface for building, running, and managing
reproducible bioinformatics workflows with first-class support for multiple software
environment managers (conda, pixi, venv, docker, singularity, etc.).

Licensed under Apache 2.0 — fully open source and free.

## Workspace layout

```
oxo-flow/                         # Cargo workspace root
├── Cargo.toml                    # workspace manifest
├── crates/
│   ├── oxo-flow-core/            # Core library: DAG engine, environment, config, reporting
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            # Public API surface
│   │       ├── dag.rs            # DAG construction, validation, topological sort
│   │       ├── executor.rs       # Task execution engine (local, cluster, cloud)
│   │       ├── environment.rs    # Environment management (conda, pixi, docker, singularity, venv)
│   │       ├── config.rs         # Workflow configuration and .oxoflow file parsing
│   │       ├── rule.rs           # Rule/step definitions with inputs, outputs, shell, resources
│   │       ├── scheduler.rs      # Job scheduling with resource constraints
│   │       ├── report.rs         # Modular report generation system
│   │       ├── wildcard.rs       # Wildcard pattern expansion ({sample}, {chr}, etc.)
│   │       ├── error.rs          # Unified error types
│   │       └── container.rs      # Container build & packaging utilities
│   ├── oxo-flow-cli/             # CLI binary crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs           # Clap-based CLI entry point
│   ├── oxo-flow-web/             # Web interface crate (axum-based API + frontend)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs            # Web server with REST API
│   └── venus/                    # Tumor variant calling pipeline built on oxo-flow
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs            # Venus pipeline definitions
├── pipelines/
│   └── venus/                    # Venus pipeline .oxoflow files and resources
│       ├── rules/                # Individual step definitions
│       ├── envs/                 # Conda/container environment specs
│       ├── schemas/              # Validation schemas for config
│       └── report/               # Report templates
├── examples/                     # Example .oxoflow files
├── docs/                         # Documentation (MkDocs)
│   └── guide/
│       └── src/
└── tests/                        # Integration tests
```

## Build, test, lint (individual command reference)

```bash
cargo build              # build all workspace members
cargo test               # run all tests (unit + integration)
cargo fmt -- --check     # check formatting
cargo clippy -- -D warnings  # lint (zero warnings allowed)
```

Integration tests should test the CLI binary and core library public API.

## Key design principles

1. **DAG-first execution** — All workflows are compiled into a Directed Acyclic Graph before execution. The DAG engine handles dependency resolution, topological ordering, and parallel execution.

2. **Environment-agnostic** — Every rule/step can declare its software environment (conda, pixi, docker, singularity, venv). The executor resolves and activates the correct environment before running each task.

3. **Wildcard expansion** — oxo-flow supports `{sample}`, `{chr}` style wildcards in file paths. The engine expands wildcards based on input file discovery or explicit configuration.

4. **Modular reporting** — The report system generates structured HTML/PDF/JSON reports from templates. Clinical pipelines like Venus use this for patient-facing reports.

5. **Container-first reproducibility** — Workflows can be packaged into self-contained containers with all dependencies, data references, and configuration baked in.

6. **Resource-aware scheduling** — Jobs declare CPU, memory, GPU, and disk requirements. The scheduler respects resource constraints and supports cluster backends (SLURM, PBS, SGE).

## Critical conventions

**Error handling** — Use `thiserror` for library error types, `anyhow` for CLI/binary error handling. Every public API function returns `Result<T, OxoFlowError>`.

**Serialization** — Use `serde` with TOML as the primary config format. The `.oxoflow` file format is TOML-based.

**Async runtime** — Use `tokio` for async execution. The DAG executor runs tasks concurrently via tokio tasks.

**Logging** — Use `tracing` for structured logging throughout the codebase.

**CLI structure** — Use `clap` with derive macros. Subcommands mirror the web API endpoints for consistency:
- `oxo-flow run` — Execute a workflow
- `oxo-flow dry-run` — Simulate execution, show DAG
- `oxo-flow validate` — Validate .oxoflow file
- `oxo-flow graph` — Visualize DAG
- `oxo-flow report` — Generate reports
- `oxo-flow env` — Manage environments
- `oxo-flow package` — Package workflow into container
- `oxo-flow serve` — Start web interface

## Adding / editing things

**New rule type or feature in core:**
1. `crates/oxo-flow-core/src/` — add/modify the relevant module
2. Update `lib.rs` — export new public types
3. Add unit tests in the same file or `tests/` directory
4. Update documentation

**New CLI subcommand:**
1. `crates/oxo-flow-cli/src/main.rs` — add Clap subcommand variant
2. Implement handler function
3. Add integration test
4. Update CLI docs

**New pipeline (like Venus):**
1. Create `.oxoflow` files in `pipelines/<name>/`
2. Add environment specs in `pipelines/<name>/envs/`
3. Add report templates if applicable
4. Document in `docs/`

## .oxoflow file format

The `.oxoflow` file format is TOML-based with the following top-level sections:

```toml
[workflow]
name = "my-pipeline"
version = "1.0.0"

[config]
samples = "samples.csv"
reference = "/path/to/ref.fa"

[[rules]]
name = "fastqc"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["qc/{sample}_R1_fastqc.html"]
threads = 4
memory = "8G"
environment = { conda = "envs/qc.yaml" }
shell = "fastqc {input} -o qc/"

[[rules]]
name = "bwa_align"
input = ["trimmed/{sample}_R1.fastq.gz"]
output = ["aligned/{sample}.bam"]
threads = 16
memory = "32G"
environment = { docker = "biocontainers/bwa:0.7.17" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"
```

## Documentation

MkDocs source lives under `docs/guide/src/`.

When CLI behavior changes, update docs accordingly.
When architecture changes, update this file and relevant docs.
