<div align="center">

<img src="logo.svg" width="140" hspace="10" vspace="6" alt="oxo-flow logo">

<h1>oxo-flow</h1>

<p><strong>A Rust-native bioinformatics pipeline engine with AI Companion — built from first principles for performance, reproducibility, and developer experience.</strong></p>

[![CI](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml/badge.svg)](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml)
[![GitHub release](https://img.shields.io/github/v/release/Traitome/oxo-flow?label=release&style=flat-square)](https://github.com/Traitome/oxo-flow/releases)
[![Crates.io](https://img.shields.io/crates/v/oxo-flow-core?label=crates.io&style=flat-square)](https://crates.io/crates/oxo-flow-core)
[![bioconda](https://anaconda.org/bioconda/oxo-flow-cli/badges/version.svg)](https://bioconda.github.io/recipes/oxo-flow-cli/README.html)
[![MSRV](https://img.shields.io/badge/MSRV-1.97.1-orange?style=flat-square)](rust-toolchain.toml)
[![Docs](https://img.shields.io/badge/docs-guide-blue?style=flat-square)](https://traitome.github.io/oxo-flow/latest/)
[![License](https://img.shields.io/badge/license-Apache%202.0%20%7C%20Dual-blue?style=flat-square)](#license)
[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?style=flat-square)](https://doc.rust-lang.org/edition-guide/)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-lightgrey?style=flat-square)](#quick-start)
[![GitHub downloads](https://img.shields.io/github/downloads/Traitome/oxo-flow/total?label=github%20downloads&style=flat-square)](https://github.com/Traitome/oxo-flow/releases)
[![bioconda downloads](https://anaconda.org/bioconda/oxo-flow-cli/badges/downloads.svg)](https://anaconda.org/bioconda/oxo-flow-cli/files)
[![cargo installs](https://img.shields.io/crates/d/oxo-flow-cli?label=cargo%20installs&style=flat-square)](https://crates.io/crates/oxo-flow-cli)
[![GitHub stars](https://img.shields.io/github/stars/Traitome/oxo-flow?style=flat-square)](https://github.com/Traitome/oxo-flow/stargazers)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Traitome/oxo-flow)

[Documentation](https://traitome.github.io/oxo-flow/latest/) · [Workflow Gallery](https://traitome.github.io/oxo-flow/latest/gallery/) · [Community](https://oxo-flow-community.github.io/) · [Roadmap](ROADMAP.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

</div>

---

## What is oxo-flow?

oxo-flow is a high-performance bioinformatics pipeline engine built in Rust. It compiles workflows into Directed Acyclic Graphs and orchestrates execution with native concurrency, per-rule environment isolation, and AI-powered assistance — all from a single binary.

> 🧬 **Community:** browse curated, rated, ready-to-run workflows — ports of popular nf-core & Snakemake pipelines, original designs, and community submissions — at [oxo-flow-community](https://oxo-flow-community.github.io/).

- 🤖 **AI Companion** — Natural language pipeline generation, intelligent refinement, failure diagnosis, and results interpretation. Powered by Claude, OpenAI, DeepSeek, or local Ollama.
- 🔀 **DAG engine** — Automatic dependency resolution, topological ordering, and parallel execution with resource-aware scheduling (CPU, memory, GPU, disk) across local and cluster backends (SLURM, PBS, SGE, LSF)
- 📦 **8 environment backends** — conda, mamba, pixi, docker, singularity, venv, system, and HPC modules — with per-rule isolation
- ⚡ **Rust performance** — Fearless concurrency, zero-cost abstractions, `#![forbid(unsafe_code)]` in core and web crates
- 🌐 **Professional Web UI** — React 19 SPA with DAG visualization (React Flow + d3-dag), TOML editor (CodeMirror 6), and AI chat
- 📊 **Built-in reporting** — HTML/JSON/Markdown/PDF reports with execution summaries, failure diagnosis, resource metrics, and checkpoint-verified file manifests; QC metrics parsed from real tool outputs (fastp, flagstat, STAR, featureCounts, bcftools, kraken2), R-friendly TSV export (`--r-data`), and a JSON report snapshot auto-written after every run
- 🔒 **Security hardened** — Shell injection prevention, path traversal protection, secret scanning, and per-IP rate limiting
- 🗄️ **Checkpoint & resume** — JSON-persisted execution state; resume interrupted workflows from the last completed rule
- 🚀 **Three deployment modes** — Personal workstation, team server with OAuth2, or HPC submit panel — same binary

## Why oxo-flow?

| Feature | **oxo-flow** | Snakemake | Nextflow |
|---------|------------|-----------|----------|
| **Language** | Rust — compiled, type-safe, `#![forbid(unsafe_code)]` (core + web) | Python | Groovy/JVM |
| **Performance** | Native binary, instant startup | Python + JIT overhead | JVM startup overhead |
| **Workflow format** | TOML (`.oxoflow`) — declarative, composable | Snakefile (Python DSL) | Nextflow DSL (Groovy) |
| **Environment support** | 8 backends — conda, mamba, pixi, docker, singularity, venv, system, modules — per-rule | conda, singularity, docker | conda, docker, singularity, modules |
| **Web interface** | Built-in React 19 SPA + REST API | External Snakemake-UI | Nextflow Tower (commercial) |
| **Reporting** | Built-in HTML/JSON/Markdown/PDF reports from checkpoint data (execution truth, failure diagnosis, file checksums) | Via MultiQC | Via Nextflow Tower |
| **Cluster backends** | SLURM, PBS, SGE, LSF | SLURM, PBS, SGE, LSF | SLURM, PBS, SGE, LSF, k8s |
| **Security** | Shell sanitization, path traversal prevention, rate limiting | Limited | Limited |
| **AI Companion** | Built-in — generate, refine, diagnose, interpret | Not built-in | Not built-in |
| **Testing** | ~2,270 tests (unit, integration, doc) | pytest-based | Varied |

## Design Principles

oxo-flow is built on five principles:

1. **DAG is the fundamental abstraction** — Every workflow is a directed acyclic graph. The engine constructs, validates, and executes DAGs with maximum parallelism.
2. **Environment isolation is non-negotiable** — Each task runs in its own isolated environment via one of 8 backends.
3. **Reproducible by design** — Config checksums, execution provenance, and container pinning guarantee identical outputs from identical inputs.
4. **Performance through Rust** — Zero-cost abstractions and fearless concurrency for orchestrating thousands of concurrent tasks.
5. **Outcome-driven** — The DAG engine's target-aware execution (`-t` flag) computes the minimal rule set needed to produce specific deliverables.

## Workflow Gallery

Learn oxo-flow incrementally with curated, validated example workflows — from a one-rule hello-world to production-grade pipelines. Every workflow passes `oxo-flow validate` and is tested in CI. See the [Workflow Gallery](https://traitome.github.io/oxo-flow/latest/gallery/) for the full catalog with detailed explanations, DAG visualizations, and CLI output.

## Quick Start

### Install from pre-built binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/Traitome/oxo-flow/releases):

```bash
# Linux (x86_64)
curl -LO https://github.com/Traitome/oxo-flow/releases/latest/download/oxo-flow-latest-x86_64-unknown-linux-gnu.tar.gz
tar xzf oxo-flow-latest-x86_64-unknown-linux-gnu.tar.gz
sudo mv oxo-flow /usr/local/bin/

# macOS (Apple Silicon)
curl -LO https://github.com/Traitome/oxo-flow/releases/latest/download/oxo-flow-latest-aarch64-apple-darwin.tar.gz
tar xzf oxo-flow-latest-aarch64-apple-darwin.tar.gz
sudo mv oxo-flow /usr/local/bin/
```

### Install with cargo

```bash
cargo install oxo-flow-cli
```

### Install with Conda

```bash
conda install -c bioconda oxo-flow-cli
```

### Build from source

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --release --workspace

# Install the CLI to your local cargo bin directory:
cargo install --path crates/oxo-flow-cli

# Binaries are in target/release/
# - oxo-flow        (CLI, includes `oxo-flow serve` for web UI)
# - oxo-flow-web    (Standalone web server)
```

### First workflow

```bash
# Create a new pipeline project (creates my-pipeline/ directory and my-pipeline.oxoflow)
oxo-flow init my-pipeline
cd my-pipeline

# Validate the workflow
oxo-flow validate my-pipeline.oxoflow

# Preview execution plan
oxo-flow dry-run my-pipeline.oxoflow

# Execute with 8 parallel jobs
oxo-flow run my-pipeline.oxoflow -j 8

# Visualize the DAG (use -f dot for Graphviz DOT output)
oxo-flow graph my-pipeline.oxoflow -f dot > dag.dot
dot -Tpng dag.dot -o dag.png

# Export a transit-map-style diagram via nf-metro
oxo-flow graph my-pipeline.oxoflow -f metro -o pipeline.mmd
# Render locally (requires nf-metro) or paste the .mmd content into the
# online playground: https://seqeralabs.github.io/nf-metro/latest/playground/
nf-metro render pipeline.mmd -o pipeline.svg

# Generate an HTML report
oxo-flow report my-pipeline.oxoflow -f html -o report.html
```

## Cluster / HPC

oxo-flow supports job submission to HPC cluster schedulers including SLURM, PBS/PBS Pro, SGE/UGE, and LSF. The `oxo-flow cluster` subcommand manages the full submission lifecycle.

```bash
# Submit a workflow to a SLURM cluster
oxo-flow cluster submit workflow.oxoflow --backend slurm --queue short -o jobs/

# Check submission status
oxo-flow cluster status --id <job-id>

# Cancel a submitted job
oxo-flow cluster cancel --id <job-id>
```

**Supported backends:** `slurm`, `pbs`, `sge`, `lsf`. Cluster submissions are configured inline via `oxo-flow cluster submit` flags.

Workflows can also carry reusable config supplements (e.g. per-cluster thread
and memory defaults) in `profiles/<NAME>.toml` next to the workflow:

```bash
# Run a workflow with a config supplement profile
oxo-flow run workflow.oxoflow --profile slurm
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

Wildcards like `{sample}` expand automatically via input file discovery. Features include reference directory conventions, environment groups, optional rules, and directory inputs. See the full [Workflow Format Specification](https://traitome.github.io/oxo-flow/latest/reference/workflow-format/) for details.

## CLI Commands

The `oxo-flow` binary provides **29 subcommands** covering the complete workflow lifecycle. See the full [CLI Reference](https://traitome.github.io/oxo-flow/latest/commands/run/) for details.

| Category | Commands |
|----------|----------|
| **Execution** | `run`, `resume`, `dry-run`, `test`, `batch` |
| **Development** | `init`, `validate`, `format`, `lint`, `debug`, `template` |
| **Inspection** | `graph`, `report`, `status`, `config`, `diff`, `provenance`, `schema` |
| **Environment** | `env`, `export`, `clean`, `touch` |
| **Deployment** | `serve`, `cluster`, `publish`, `pull` |
| **AI & System** | `ai`, `completions`, `license` |

See the full [CLI Reference](https://traitome.github.io/oxo-flow/latest/commands/run/) for detailed usage of each subcommand.

## Web API

The `oxo-flow serve` command starts an [axum](https://github.com/tokio-rs/axum)-powered REST server with **100+ endpoints** across 9 domains (observability, pipeline, execution, AI, auth, collaboration, data, chat, clusters). Full API reference at the [OpenAPI 3.1 spec](https://traitome.github.io/oxo-flow/latest/reference/api/).


oxo-flow is organized as a Cargo workspace with four crates:

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/     # Core library: DAG engine, executor, environment mgmt,
│   │                      # config parsing, scheduler, wildcard expansion, reporting
│   ├── oxo-flow-ai/       # AI companion: provider abstraction, skill system, agents
│   ├── oxo-flow-cli/      # CLI binary ("oxo-flow") — Clap-based, 29 subcommands
│   └── oxo-flow-web/      # Web server ("oxo-flow-web") — axum REST API + frontend
├── examples/              # Example .oxoflow workflows
├── tests/                 # Integration tests
└── docs/                  # Documentation (MkDocs)
```

| Crate | Type | Binary | License |
|-------|------|--------|---------|
| `oxo-flow-core` | Library | — | Apache-2.0 |
| `oxo-flow-ai` | Library | — | Apache-2.0 |
| `oxo-flow-cli` | Binary | `oxo-flow` | Apache-2.0 |
| `oxo-flow-web` | Binary | `oxo-flow-web` | Dual Academic / Commercial |

### Key modules

| Module | Crate | Responsibility |
|--------|-------|----------------|
| `dag.rs` | core | DAG construction, validation, topological sort |
| `executor.rs` | core | Task execution (local, cluster, cloud) |
| `environment.rs` | core | Environment management (conda, mamba, pixi, docker, singularity, venv, system, modules) |
| `config.rs` | core | Workflow configuration and `.oxoflow` file parsing |
| `rule.rs` | core | Rule/step definitions with inputs, outputs, shell, resources |
| `scheduler.rs` | core | Job scheduling with resource constraints |
| `wildcard.rs` | core | Wildcard pattern expansion (`{sample}`, `{chr}`, etc.) |
| `report.rs` | core | Modular report generation (HTML, JSON, PDF via wkhtmltopdf) |
| `format.rs` | core | Workflow formatting, linting, and 30+ diagnostic patterns |
| `plugin.rs` | core | Plugin system for MCP servers and custom tools |
| `ai_provider.rs` | web | Multi-provider AI abstraction (Claude, OpenAI, DeepSeek, Ollama) |
| `domains/ai/` | web | AI translation, chat SSE streaming, agent orchestration |
| `domains/execution/` | web | Run lifecycle, diagnostics, pause/resume, retry |
| `domains/workflow/` | web | Pipeline CRUD, validation, DAG building, templating |
| `provider.rs` | ai | Multi-provider AI client abstraction with auto-detection |
| `skill.rs` | ai | Skill registry and discovery system |
| `agent/` | ai | Agent orchestration and tool-calling framework |
| `mcp.rs` | ai | MCP (Model Context Protocol) server integration |

## Three-Mode Deployment

`oxo-flow serve` starts the web interface with an embedded REST API and React SPA. Run it without arguments for local experimentation:

```bash
# Mode 1: Personal workstation (default) — SQLite, localhost:8080, no auth
oxo-flow serve
# → Open http://127.0.0.1:8080 in your browser

# Mode 2: Team server — SQLite/PG, 0.0.0.0, ORCID/GitHub OAuth2
oxo-flow serve --mode team --db postgres://...

# Mode 3: HPC submit panel — Web UI for cluster job submission
oxo-flow serve --mode hpc --scheduler slurm
```

## Documentation

Comprehensive documentation is available at **[traitome.github.io/oxo-flow/latest/](https://traitome.github.io/oxo-flow/latest/)**.

### 📖 Documentation Quick Links

| If you are... | Recommended Start |
|---|---|
| **New to oxo-flow** | [Quick Start](https://traitome.github.io/oxo-flow/latest/tutorials/quickstart/) · [First Workflow](https://traitome.github.io/oxo-flow/latest/tutorials/first-workflow/) |
| **A Bioinformatician** | [Workflow Gallery](https://traitome.github.io/oxo-flow/latest/gallery/) |
| **A Pipeline Engineer** | [Workflow Format Specification](https://traitome.github.io/oxo-flow/latest/reference/workflow-format/) · [CLI Reference](https://traitome.github.io/oxo-flow/latest/commands/run/) |
| **A DevOps/Cloud Admin** | [Environment Management](https://traitome.github.io/oxo-flow/latest/tutorials/environment-management/) · [Running on Cluster](https://traitome.github.io/oxo-flow/latest/how-to/run-on-cluster/) |
| **A Bioinformatics Core** | [Workflow Gallery](https://traitome.github.io/oxo-flow/latest/gallery/) · [Environment Management](https://traitome.github.io/oxo-flow/latest/tutorials/environment-management/) |

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
| `oxo-flow-ai` | [Apache-2.0](LICENSE) | Free and open-source |
| `oxo-flow-cli` | [Apache-2.0](LICENSE) | Free and open-source |
| `oxo-flow-web` | [Academic](LICENSE-ACADEMIC) / [Commercial](LICENSE-COMMERCIAL) | Free for academic and non-commercial use; commercial use requires a separate license |

The core library, AI companion, and CLI are licensed under the **Apache License 2.0** — you are free to use, modify, and distribute them without restriction.

The **web interface** (`oxo-flow-web`) is available under a **dual license**: free for academic and non-commercial use under the Academic License, and requiring a commercial license for commercial deployments. See [LICENSE-ACADEMIC](LICENSE-ACADEMIC) and [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

## Contributing

Contributions are welcome! Please see:

- [CONTRIBUTING.md](CONTRIBUTING.md) — Contribution guidelines
- [ROADMAP.md](ROADMAP.md) — Project roadmap and areas where help is needed
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — Community standards
- [GOVERNANCE.md](GOVERNANCE.md) — Project governance and decision-making
- [SECURITY.md](SECURITY.md) — Security vulnerability reporting

Before submitting a PR, ensure all checks pass:

```bash
make ci
```

## Citing

If you use oxo-flow in academic research, please cite:

> **Shixiang Wang**, *oxo-flow: compiled, memory-safe bioinformatics workflow orchestration*, bioRxiv, 2026, [https://doi.org/10.64898/2026.06.11.731578](https://doi.org/10.64898/2026.06.11.731578)
>
> Jia Ding, Yun Peng, Ruochen Wei, Boquan Wang, Jian-Guo Zhou, **Shixiang Wang**, *BLIT: an R package for seamless integration of command-line bioinformatics tool universe*, Bioinformatics Advances, Volume 6, Issue 1, 2026, vbag088, [https://doi.org/10.1093/bioadv/vbag088](https://doi.org/10.1093/bioadv/vbag088)

## Community

- 🐛 **Bug reports** — [GitHub Issues](https://github.com/Traitome/oxo-flow/issues) (use [bug report template](.github/ISSUE_TEMPLATE/bug_report.md))
- 💡 **Feature requests** — [GitHub Issues](https://github.com/Traitome/oxo-flow/issues) (use [feature request template](.github/ISSUE_TEMPLATE/feature_request.md))
- 📖 **Documentation** — [traitome.github.io/oxo-flow/latest/](https://traitome.github.io/oxo-flow/latest/)
- ❓ **Questions** — [Ask DeepWiki](https://deepwiki.com/Traitome/oxo-flow)

### 🧪 Real-World Feedback

oxo-flow aims to work reliably across diverse computing environments — laptops, HPC clusters, GPU nodes. We cannot replicate every deployment scenario in CI. If you use oxo-flow, please share your experience (success or failure) as a [GitHub Issue](https://github.com/Traitome/oxo-flow/issues) with the prefix `[Real-World Testing]`. Your feedback directly shapes our priorities.

## Additional Resources

- [LIMITATIONS.md](LIMITATIONS.md) — Known limitations and constraints
- [REPRODUCIBILITY.md](REPRODUCIBILITY.md) — Reproducibility guarantees and methodology
- [RELEASING.md](RELEASING.md) — Release process and versioning policy
- [TRADEMARK.md](TRADEMARK.md) — Trademark usage guidelines
- [docs/CHANGE_CONTROL.md](docs/CHANGE_CONTROL.md) — Change control for regulated environments
- [docs/VALIDATION_PROTOCOL.md](docs/VALIDATION_PROTOCOL.md) — IQ/OQ/PQ validation protocol

---

<div align="center">

**Built with 🧬 by [Traitome](https://github.com/Traitome)**

</div>

---

## Contributors

Thanks goes to these wonderful people:

<a href="https://github.com/ShixiangWang"><img src="https://github.com/ShixiangWang.png" width="50" height="50" alt="Shixiang Wang" title="Shixiang Wang (王诗翔)" style="border-radius:50%"/></a>
<a href="https://github.com/andrewbudge"><img src="https://github.com/andrewbudge.png" width="50" height="50" alt="Andrew Budge" title="Andrew Budge" style="border-radius:50%"/></a>

> Run `make contributors` to refresh the contributor list from git history.
