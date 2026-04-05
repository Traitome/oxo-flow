<div align="center">

# oxo-flow

**A Rust-native bioinformatics pipeline engine — reimagining workflow management.**

[![CI](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml/badge.svg)](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/oxo-flow-core.svg)](https://crates.io/crates/oxo-flow-core)
[![License](https://img.shields.io/badge/license-Apache%202.0%20%7C%20Dual-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-2024_edition-orange.svg)](https://doc.rust-lang.org/edition-guide/)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg)](#quick-start)
[![Docs](https://img.shields.io/badge/docs-guide-blue.svg)](https://traitome.github.io/oxo-flow/documentation/)
[![GitHub Downloads](https://img.shields.io/github/downloads/Traitome/oxo-flow/total.svg)](https://github.com/Traitome/oxo-flow/releases)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Traitome/oxo-flow)

[Documentation](https://traitome.github.io/oxo-flow/documentation/) · [Roadmap](ROADMAP.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## What is oxo-flow?

oxo-flow is a high-performance, modular bioinformatics pipeline engine built from first principles in Rust. It compiles workflows into Directed Acyclic Graphs and orchestrates execution with native concurrency, environment isolation, and clinical-grade reproducibility — all from a single, fast binary.

- 🔀 **DAG-based execution** — Automatic dependency resolution, topological ordering, and parallel execution
- 📦 **Environment management** — First-class support for conda, pixi, docker, singularity, and venv
- 🧬 **Bioinformatics-first** — Purpose-built for genomics workflows and clinical-grade pipelines
- 📊 **Clinical-grade reporting** — Modular HTML/PDF/JSON report generation from templates
- 🌐 **CLI + Web interface** — 13 CLI subcommands and a full REST API with 12+ endpoints
- 🐳 **Container packaging** — Package workflows into portable Docker/Singularity images
- ⚡ **Rust performance** — Fearless concurrency, zero-cost abstractions, minimal memory footprint
- 🔧 **Resource-aware scheduling** — Jobs declare CPU, memory, GPU, and disk; the scheduler respects constraints across local and cluster backends (SLURM, PBS, SGE)

## Why oxo-flow?

| Feature | oxo-flow |
|---------|----------|
| **Language** | Rust (compiled, type-safe, zero-cost abstractions) |
| **Performance** | Native binary with async concurrency — no interpreter overhead |
| **Workflow format** | TOML (`.oxoflow`) — declarative, composable, human-readable |
| **Environment support** | conda, pixi, docker, singularity, venv — per-rule isolation |
| **Web interface** | Built-in REST API + embedded web UI for remote monitoring |
| **Clinical reporting** | First-class HTML/PDF/JSON report system with audit trails |
| **Container packaging** | `oxo-flow package` — one command to produce portable images |
| **Cluster backends** | SLURM, PBS, SGE, LSF — resource-aware scheduling |
| **Type safety** | Compile-time guarantees eliminate entire classes of runtime errors |
| **Startup time** | Instant — native binary, no runtime loading |
| **Reproducibility** | Config checksums, execution provenance, deterministic DAG scheduling |

## Quick Start

### Install from pre-built binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/Traitome/oxo-flow/releases):

```bash
# Linux (x86_64)
curl -LO https://github.com/Traitome/oxo-flow/releases/latest/download/oxo-flow-x86_64-unknown-linux-gnu.tar.gz
tar xzf oxo-flow-x86_64-unknown-linux-gnu.tar.gz
sudo mv oxo-flow /usr/local/bin/

# macOS (Apple Silicon)
curl -LO https://github.com/Traitome/oxo-flow/releases/latest/download/oxo-flow-aarch64-apple-darwin.tar.gz
tar xzf oxo-flow-aarch64-apple-darwin.tar.gz
sudo mv oxo-flow /usr/local/bin/
```

### Install with cargo

```bash
cargo install oxo-flow-cli
```

### Build from source

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --release

# Binaries are in target/release/
# - oxo-flow        (CLI)
# - oxo-flow-web    (Web server)
# - venus           (Venus pipeline)
```

### First workflow

```bash
# Create a new pipeline project
oxo-flow init my-pipeline

# Validate the workflow
oxo-flow validate my-pipeline.oxoflow

# Preview execution plan
oxo-flow dry-run my-pipeline.oxoflow

# Execute with 8 parallel jobs
oxo-flow run my-pipeline.oxoflow -j 8

# Visualize the DAG
oxo-flow graph my-pipeline.oxoflow > dag.dot
dot -Tpng dag.dot -o dag.png

# Generate an HTML report
oxo-flow report my-pipeline.oxoflow -f html -o report.html
```

## Workflow Format (`.oxoflow`)

oxo-flow uses a TOML-based workflow format that is human-readable, composable, and declarative:

```toml
[workflow]
name = "variant-calling"
version = "1.0.0"

[config]
reference = "/data/ref/GRCh38.fa"

[[rules]]
name = "fastp"
input = ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"]
output = ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"]
threads = 8
shell = "fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]}"

[rules.environment]
conda = "envs/fastp.yaml"

[[rules]]
name = "bwa_align"
input = ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
threads = 16
memory = "32G"
shell = "bwa-mem2 mem -t {threads} {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"

[rules.environment]
docker = "biocontainers/bwa-mem2:2.2.1"
```

Wildcards like `{sample}` are expanded automatically based on input file discovery or explicit configuration, enabling concise and powerful pattern-based pipeline definitions.

## CLI Commands

The `oxo-flow` binary provides 13 subcommands for the complete workflow lifecycle:

| Command | Description |
|---------|-------------|
| `oxo-flow run` | Execute a workflow (`-j` parallel jobs, `-k` keep-going, `--timeout` per-job) |
| `oxo-flow dry-run` | Simulate execution — show what would run without executing |
| `oxo-flow validate` | Validate an `.oxoflow` file for syntax and semantic correctness |
| `oxo-flow graph` | Export the workflow DAG in DOT format for visualization |
| `oxo-flow report` | Generate execution reports (`-f html\|json`, `-o` output path) |
| `oxo-flow env list` | List available environment backends on the system |
| `oxo-flow env check` | Verify that all environment requirements are satisfied |
| `oxo-flow package` | Package workflow into a container image (`-f docker\|singularity`) |
| `oxo-flow serve` | Start the web interface (`--host`, `-p` port, default: `127.0.0.1:8080`) |
| `oxo-flow init` | Scaffold a new pipeline project (`-d` output directory) |
| `oxo-flow status` | Show execution status from the checkpoint file |
| `oxo-flow clean` | Clean workflow outputs and temp files (`-n` dry-run, `--force`) |
| `oxo-flow completions` | Generate shell completions (bash, zsh, fish, elvish, PowerShell) |

## Web API Endpoints

The `oxo-flow-web` server exposes a REST API (powered by [axum](https://github.com/tokio-rs/axum)):

```bash
# Start the server
oxo-flow serve --host 0.0.0.0 -p 8080
```

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/health` | Health check |
| `GET` | `/api/version` | Server and engine version info |
| `GET` | `/api/workflows` | List available workflows |
| `GET` | `/api/environments` | List available environment backends |
| `POST` | `/api/workflows/validate` | Validate workflow TOML |
| `POST` | `/api/workflows/parse` | Parse workflow and return structured detail |
| `POST` | `/api/workflows/dag` | Build the DAG and return DOT representation |
| `POST` | `/api/workflows/dry-run` | Simulate execution and return the plan |
| `POST` | `/api/workflows/run` | Start workflow execution |
| `POST` | `/api/workflows/clean` | List output files that would be cleaned |
| `POST` | `/api/workflows/export` | Export workflow for sharing or archival |
| `POST` | `/api/reports/generate` | Generate a report (HTML or JSON) |

## Venus Pipeline 🌟

**Venus** (启明星 — "Morning Star") is a clinical-grade tumor variant detection pipeline built on oxo-flow. It supports tumor-only, normal-only, and tumor-normal paired analysis modes for WGS, WES, and panel sequencing.

### Pipeline Steps

```
FASTQ → fastp → bwa-mem2 → MarkDuplicates → BQSR → Mutect2 → FilterMutectCalls → VEP → Report
                                                   → HaplotypeCaller → VEP → Report
                                                   → Strelka2 (paired mode)
```

### Analysis Modes

| Mode | Callers | Use case |
|------|---------|----------|
| **Tumor-only** | Mutect2 + FilterMutectCalls | Somatic variant calling + annotation + clinical report |
| **Normal-only** | HaplotypeCaller | Germline variant calling + annotation |
| **Tumor-Normal** | Mutect2 + Strelka2 + HaplotypeCaller | Full paired analysis + annotation + clinical report |

See [`pipelines/venus/`](pipelines/venus/) for pipeline configuration, environment specs, and report templates.

## Architecture

oxo-flow is organized as a Cargo workspace with four crates:

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/     # Core library: DAG engine, executor, environment mgmt,
│   │                      # config parsing, scheduler, wildcard expansion, reporting
│   ├── oxo-flow-cli/      # CLI binary ("oxo-flow") — Clap-based, 13 subcommands
│   ├── oxo-flow-web/      # Web server ("oxo-flow-web") — axum REST API + frontend
│   └── venus/             # Venus pipeline ("venus") — tumor variant detection
├── pipelines/
│   └── venus/             # Venus .oxoflow files, envs, schemas, report templates
├── examples/              # Example .oxoflow workflows
├── tests/                 # Integration tests
└── docs/                  # Documentation (MkDocs)
```

| Crate | Type | Binary | License |
|-------|------|--------|---------|
| `oxo-flow-core` | Library | — | Apache-2.0 |
| `oxo-flow-cli` | Binary | `oxo-flow` | Apache-2.0 |
| `oxo-flow-web` | Binary | `oxo-flow-web` | Dual Academic / Commercial |
| `venus` | Binary | `venus` | Apache-2.0 |

### Core modules

| Module | Responsibility |
|--------|----------------|
| `dag.rs` | DAG construction, validation, topological sort |
| `executor.rs` | Task execution (local, cluster, cloud) |
| `environment.rs` | Environment management (conda, pixi, docker, singularity, venv) |
| `config.rs` | Workflow configuration and `.oxoflow` file parsing |
| `rule.rs` | Rule/step definitions with inputs, outputs, shell, resources |
| `scheduler.rs` | Job scheduling with resource constraints |
| `wildcard.rs` | Wildcard pattern expansion (`{sample}`, `{chr}`, etc.) |
| `report.rs` | Modular report generation (HTML/PDF/JSON from Tera templates) |
| `container.rs` | Container build and packaging utilities |
| `error.rs` | Unified error types (`thiserror`) |

## Documentation

Full documentation is available at **[traitome.github.io/oxo-flow/documentation/](https://traitome.github.io/oxo-flow/documentation/)**.

MkDocs source lives under [`docs/guide/src/`](docs/guide/src/).

## Development

```bash
# Build all workspace crates
cargo build

# Run all tests (unit + integration)
cargo test

# Run the full CI suite (format + clippy + build + test)
make ci

# Individual CI steps
cargo fmt -- --check          # Check formatting
cargo clippy -- -D warnings   # Lint (zero warnings)
cargo build                   # Compile
cargo test                    # Test

# Format code
cargo fmt
```

### Tech stack

| Component | Technology |
|-----------|------------|
| Language | Rust (2024 edition) |
| Async runtime | [tokio](https://tokio.rs/) |
| CLI framework | [clap](https://github.com/clap-rs/clap) (derive) |
| Web framework | [axum](https://github.com/tokio-rs/axum) |
| Serialization | [serde](https://serde.rs/) + TOML |
| Graph library | [petgraph](https://github.com/petgraph/petgraph) |
| Templating | [tera](https://github.com/Keats/tera) |
| Error handling | [thiserror](https://github.com/dtolnay/thiserror) (lib) / [anyhow](https://github.com/dtolnay/anyhow) (bin) |
| Logging | [tracing](https://github.com/tokio-rs/tracing) |

## License

This project uses a **split licensing model**:

| Crate | License | Details |
|-------|---------|---------|
| `oxo-flow-core` | [Apache-2.0](LICENSE) | Free and open-source |
| `oxo-flow-cli` | [Apache-2.0](LICENSE) | Free and open-source |
| `venus` | [Apache-2.0](LICENSE) | Free and open-source |
| `oxo-flow-web` | [Academic](LICENSE-ACADEMIC) / [Commercial](LICENSE-COMMERCIAL) | Free for academic and non-commercial use; commercial use requires a separate license |

The core library, CLI, and Venus pipeline are licensed under the **Apache License 2.0** — you are free to use, modify, and distribute them without restriction.

The **web interface** (`oxo-flow-web`) is available under a **dual license**: free for academic and non-commercial use under the Academic License, and requiring a commercial license for commercial deployments. See [LICENSE-ACADEMIC](LICENSE-ACADEMIC) and [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

## Contributing

Contributions are welcome! Please see:

- [CONTRIBUTING.md](CONTRIBUTING.md) — Contribution guidelines
- [ROADMAP.md](ROADMAP.md) — Project roadmap and areas where help is needed
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — Community standards

Before submitting a PR, ensure all checks pass:

```bash
make ci
```

## Community

- 🐛 **Bug reports** — [GitHub Issues](https://github.com/Traitome/oxo-flow/issues)
- 💡 **Feature requests** — [GitHub Issues](https://github.com/Traitome/oxo-flow/issues)
- 📖 **Documentation** — [traitome.github.io/oxo-flow/documentation/](https://traitome.github.io/oxo-flow/documentation/)
- ❓ **Questions** — [Ask DeepWiki](https://deepwiki.com/Traitome/oxo-flow)

---

<div align="center">

**Built with 🧬 by [Traitome](https://github.com/Traitome)**

</div>
