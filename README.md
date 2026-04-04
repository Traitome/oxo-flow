# oxo-flow

[![CI](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml/badge.svg)](https://github.com/Traitome/oxo-flow/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

**A Rust-native bioinformatics pipeline engine — reimagining workflow management.**

oxo-flow is a high-performance, modular bioinformatics pipeline engine designed to fully replace Snakemake. Built from first principles in Rust, it provides:

- 🔀 **DAG-based execution** — Automatic dependency resolution and parallel execution
- 📦 **Environment management** — First-class support for conda, pixi, docker, singularity, venv
- 🧬 **Bioinformatics-first** — Designed for genomics, clinical pipelines, and reproducible research
- 📊 **Clinical-grade reporting** — Modular HTML/PDF/JSON report generation
- 🌐 **CLI + Web interface** — Powerful command-line tool with a visual web editor
- 🐳 **Container packaging** — Package workflows into portable Docker/Singularity images
- ⚡ **Rust performance** — Fearless concurrency, zero-cost abstractions, minimal memory footprint

## Quick Start

```bash
# Install (from source)
cargo install --path crates/oxo-flow-cli

# Create a new pipeline project
oxo-flow init my-pipeline

# Validate a workflow
oxo-flow validate my-pipeline.oxoflow

# Dry-run (preview execution plan)
oxo-flow dry-run my-pipeline.oxoflow

# Execute the workflow
oxo-flow run my-pipeline.oxoflow -j 8

# Visualize the DAG
oxo-flow graph my-pipeline.oxoflow > dag.dot
dot -Tpng dag.dot -o dag.png

# Generate a report
oxo-flow report my-pipeline.oxoflow -f html -o report.html
```

## Workflow Format (.oxoflow)

oxo-flow uses a TOML-based workflow format that is readable and composable:

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

## CLI Commands

| Command | Description |
|---------|-------------|
| `oxo-flow run` | Execute a workflow |
| `oxo-flow dry-run` | Preview execution without running |
| `oxo-flow validate` | Validate an .oxoflow file |
| `oxo-flow graph` | Export DAG in DOT format |
| `oxo-flow report` | Generate execution reports |
| `oxo-flow env list` | List available environment backends |
| `oxo-flow env check` | Verify environment requirements |
| `oxo-flow package` | Package workflow as container |
| `oxo-flow serve` | Start web interface |
| `oxo-flow init` | Create a new pipeline project |
| `oxo-flow status` | Show checkpoint execution status |
| `oxo-flow clean` | Clean workflow outputs |
| `oxo-flow completions` | Generate shell completions |

## Web API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/health` | Health check |
| `GET` | `/api/version` | Version info |
| `GET` | `/api/workflows` | List workflows |
| `GET` | `/api/environments` | Available env backends |
| `POST` | `/api/workflows/validate` | Validate workflow TOML |
| `POST` | `/api/workflows/parse` | Parse and return workflow detail |
| `POST` | `/api/workflows/dag` | Build DAG, return DOT |
| `POST` | `/api/workflows/dry-run` | Simulate execution |
| `POST` | `/api/workflows/run` | Start workflow execution |
| `POST` | `/api/workflows/clean` | List outputs to clean |
| `POST` | `/api/reports/generate` | Generate report (HTML/JSON) |

## Venus Pipeline 🌟

Venus (启明星 — "Morning Star") is a clinical-grade tumor variant detection pipeline built on oxo-flow.
It supports tumor-only, normal-only, and tumor-normal paired analysis modes for WGS, WES, and panel sequencing.

### Pipeline Steps

```
FASTQ → fastp → bwa-mem2 → MarkDuplicates → BQSR → Mutect2 → FilterMutectCalls → VEP → Report
                                                   → HaplotypeCaller → VEP → Report
                                                   → Strelka2 (paired mode)
```

### Analysis Modes

- **Tumor-only**: Mutect2 somatic calling + FilterMutectCalls + annotation + clinical report
- **Normal-only**: HaplotypeCaller germline calling + annotation
- **Tumor-Normal**: Paired Mutect2 + Strelka2 + HaplotypeCaller + annotation + clinical report

See [pipelines/venus/](pipelines/venus/) for full pipeline configuration and [ROADMAP.md](ROADMAP.md) for the project roadmap.

## Architecture

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/   # Core library: DAG, executor, environments, reporting
│   ├── oxo-flow-cli/    # CLI binary
│   ├── oxo-flow-web/    # Web interface (axum-based REST API)
│   └── venus/           # Venus tumor variant calling pipeline
├── pipelines/           # Pipeline definitions (.oxoflow files)
├── examples/            # Example workflows
└── docs/                # Documentation
```

## Development

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Run all CI checks
make ci

# Format code
cargo fmt
```

## License

Apache License 2.0 — see [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please see [ROADMAP.md](ROADMAP.md) for the project roadmap and areas where help is needed.
