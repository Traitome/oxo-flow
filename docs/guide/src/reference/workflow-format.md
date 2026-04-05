# Workflow Format

The `.oxoflow` file format is oxo-flow's TOML-based workflow definition language. This page is the complete specification.

---

## File Extension

Workflow files use the `.oxoflow` extension: `my-pipeline.oxoflow`.

---

## Top-level Structure

```toml
[workflow]          # Required: metadata
[config]            # Optional: user variables
[defaults]          # Optional: rule defaults
[report]            # Optional: report configuration
[[rules]]           # Required: one or more rules
```

---

## `[workflow]` — Metadata

```toml
[workflow]
name = "my-pipeline"
version = "1.0.0"
description = "A short description"
author = "Your Name"
```

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | String | **Yes** | — | Pipeline name |
| `version` | String | No | `"0.1.0"` | Semantic version |
| `description` | String | No | — | Human-readable description |
| `author` | String | No | — | Author name or email |

---

## `[config]` — Configuration Variables

User-defined key-value pairs accessible in rules as `{config.<key>}`:

```toml
[config]
reference = "/data/ref/hg38.fa"
samples_dir = "raw_data"
results_dir = "results"
min_quality = "30"
```

Values are TOML strings, integers, booleans, or arrays. String interpolation in rules uses `{config.key}` syntax.

---

## `[defaults]` — Default Settings

Applied to all rules unless explicitly overridden:

```toml
[defaults]
threads = 4
memory = "8G"
environment = { conda = "envs/base.yaml" }
```

| Field | Type | Description |
|---|---|---|
| `threads` | Integer | Default CPU thread count |
| `memory` | String | Default memory allocation |
| `environment` | Table | Default environment specification |

---

## `[report]` — Report Configuration

```toml
[report]
template = "clinical"
format = ["html", "json"]
sections = ["summary", "variants", "quality"]
```

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template name |
| `format` | Array | Output formats to generate |
| `sections` | Array | Report sections to include |

---

## `[[rules]]` — Rule Definitions

Each `[[rules]]` entry defines a pipeline step. The double brackets indicate a TOML array of tables.

### Basic example

```toml
[[rules]]
name = "align"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
threads = 16
memory = "32G"
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"
```

### All fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | **Yes** | Unique rule identifier |
| `input` | Array of strings | **Yes** | Input file paths |
| `output` | Array of strings | **Yes** | Output file paths |
| `shell` | String | **Yes** | Shell command to execute |
| `threads` | Integer | No | CPU threads (overrides defaults) |
| `memory` | String | No | Memory allocation (overrides defaults) |
| `environment` | Table | No | Environment specification |

### Environment specification

```toml
# Conda
environment = { conda = "envs/tools.yaml" }

# Pixi
environment = { pixi = "envs/pixi.toml" }

# Docker
environment = { docker = "biocontainers/bwa:0.7.17" }

# Singularity
environment = { singularity = "docker://biocontainers/bwa:0.7.17" }

# Python venv
environment = { venv = "envs/requirements.txt" }
```

### Resources (extended)

For rules needing GPU, disk, or time limits, use the `resources` sub-table:

```toml
[[rules]]
name = "gpu_task"
input = ["data.h5"]
output = ["model.pt"]
threads = 8
memory = "64G"
shell = "python train.py"

[rules.resources]
gpu = 1
disk = "200G"
time_limit = "48h"
```

| Field | Type | Example | Description |
|---|---|---|---|
| `gpu` | Integer | `1` | Number of GPUs |
| `disk` | String | `"200G"` | Local disk space |
| `time_limit` | String | `"48h"` | Wall-time limit |

---

## Wildcards

### Pattern syntax

Use `{name}` in file paths for dynamic expansion:

```toml
input = ["{sample}_R1.fastq.gz"]
output = ["aligned/{sample}.bam"]
```

### Built-in placeholders

| Placeholder | Expands to |
|---|---|
| `{input}` | Space-separated input files |
| `{output}` | Space-separated output files |
| `{threads}` | Thread count for this rule |
| `{config.*}` | Value from `[config]` section |

### Custom wildcards

Any `{name}` pattern not matching a built-in placeholder is treated as a wildcard. oxo-flow expands wildcards by matching against available files or explicit sample lists.

---

## Dependency Resolution

Dependencies are inferred automatically: if rule B lists a file in its `input` that appears in rule A's `output`, then B depends on A.

```toml
[[rules]]
name = "step1"
output = ["intermediate.txt"]
# ...

[[rules]]
name = "step2"
input = ["intermediate.txt"]   # depends on step1
# ...
```

No explicit dependency declaration is needed.

---

## Multi-line Strings

Use triple quotes for multi-line shell commands:

```toml
shell = """
mkdir -p results
bwa mem -t {threads} ref.fa {input} | \
  samtools sort -@ {threads} -o {output}
"""
```

---

## Complete Example

```toml
[workflow]
name = "ngs-pipeline"
version = "2.0.0"
description = "Complete NGS analysis pipeline"
author = "Genomics Core <core@example.org>"

[config]
reference = "/data/ref/hg38.fa"
known_sites = "/data/ref/known_sites.vcf.gz"
results = "results"

[defaults]
threads = 4
memory = "8G"
environment = { conda = "envs/base.yaml" }

[report]
format = ["html"]

[[rules]]
name = "fastqc"
input = ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"]
output = ["{config.results}/qc/{sample}_R1_fastqc.html"]
shell = "fastqc {input} -o {config.results}/qc/ -t {threads}"

[[rules]]
name = "trim"
input = ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"]
output = ["{config.results}/trimmed/{sample}_R1.fastq.gz"]
environment = { docker = "biocontainers/fastp:0.23.4" }
shell = "fastp --in1 {input[0]} --in2 {input[1]} --out1 {output[0]} --thread {threads}"

[[rules]]
name = "align"
input = ["{config.results}/trimmed/{sample}_R1.fastq.gz"]
output = ["{config.results}/aligned/{sample}.bam"]
threads = 16
memory = "32G"
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"
```

---

## See Also

- [Create a Workflow](../how-to/create-workflow.md) — practical authoring guide
- [DAG Engine](./dag-engine.md) — how dependencies are resolved
- [Environment System](./environment-system.md) — environment specification details
