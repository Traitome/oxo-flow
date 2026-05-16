# Create a Workflow

This guide covers the complete process of authoring an `.oxoflow` workflow file, from project scaffolding to production-ready pipelines.

---

## Scaffold a new project

```bash
oxo-flow init my-pipeline
cd my-pipeline
```

This generates a project directory with a starter `.oxoflow` file, `envs/` and `scripts/` directories, and a `.gitignore`.

---

## Workflow file structure

Every `.oxoflow` file is TOML with four top-level sections:

```toml
[workflow]      # Required: name, version, metadata
[config]        # Optional: user-defined variables
[defaults]      # Optional: default settings for all rules
[[rules]]       # Required: one or more pipeline steps
```

---

## The `[workflow]` section

```toml
[workflow]
name = "my-pipeline"
version = "1.0.0"
description = "Short description of what this pipeline does"
author = "Your Name <you@example.com>"
```

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Pipeline name (used in reports and logs) |
| `version` | No | Semantic version (defaults to `"0.1.0"`) |
| `description` | No | Human-readable description |
| `author` | No | Author name or organization |

---

## The `[config]` section

Define variables that are referenced throughout the workflow:

```toml
[config]
reference = "/data/ref/hg38.fa"
samples_dir = "raw_data"
results_dir = "results"
min_quality = "30"
```

Reference them in rule fields with `{config.variable_name}`:

```toml
shell = "bwa mem {config.reference} {input} > {output}"
```

---

## The `[defaults]` section

Set default values applied to all rules unless overridden:

```toml
[defaults]
threads = 4
memory = "8G"
environment = { conda = "envs/base.yaml" }
```

---

## Defining rules

Each `[[rules]]` entry defines one step in the pipeline:

```toml
[[rules]]
name = "step_name"
input = ["path/to/input1.txt", "path/to/input2.txt"]
output = ["path/to/output.txt"]
threads = 8
memory = "16G"
environment = { conda = "envs/tools.yaml" }
shell = "my-tool --threads {threads} {input} > {output}"
```

### Rule fields

| Field | Required | Type | Description |
|---|---|---|---|
| `name` | Yes | String | Unique rule identifier |
| `input` | Yes | Array | Input file paths (may contain wildcards) |
| `output` | Yes | Array | Output file paths (may contain wildcards) |
| `shell` | Yes | String | Shell command to execute |
| `threads` | No | Integer | CPU threads (overrides `[defaults]`) |
| `memory` | No | String | Memory requirement (e.g., `"16G"`) |
| `environment` | No | Table | Environment specification |
| `resources` | No | Table | Additional resources (GPU, disk, time_limit) |

---

## Wildcards

Use `{name}` syntax for dynamic file patterns:

```toml
[[rules]]
name = "align"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
shell = "bwa mem ref.fa {input} | samtools sort -o {output}"
```

oxo-flow expands `{sample}` from the available input files or from explicit configuration.

### Built-in placeholders

| Placeholder | Expands to |
|---|---|
| `{input}` | Space-separated list of all input files |
| `{output}` | Space-separated list of all output files |
| `{threads}` | Thread count for this rule |
| `{config.*}` | Value from the `[config]` section |

---

## Dependencies

oxo-flow infers dependencies automatically: if rule B's input matches rule A's output, B depends on A. You do not need to declare dependencies explicitly.

```toml
[[rules]]
name = "step1"
output = ["intermediate.txt"]
# ...

[[rules]]
name = "step2"
input = ["intermediate.txt"]   # ← automatically depends on step1
# ...
```

---

## Multi-line shell commands

Use triple-quoted strings for complex commands:

```toml
shell = """
mkdir -p results
bwa mem -t {threads} {config.reference} {input} | \
  samtools sort -@ {threads} -o {output}
samtools index {output}
"""
```

---

## Best practices

!!! tip "Keep rules focused"
    Each rule should do one logical step. This makes the DAG clearer and allows better parallelism.

!!! tip "Use config variables"
    Put paths and parameters in `[config]` so they can be changed without editing rule definitions.

!!! tip "Lock environment versions"
    Pin tool versions in your conda YAML or Docker tags to ensure reproducibility.

!!! tip "Validate early"
    Run `oxo-flow validate` before executing to catch syntax errors and circular dependencies.

!!! tip "Use batch for simple tasks"
    For quick parallel operations (e.g., running the same command on multiple files), use [`oxo-flow batch`](../commands/batch.md) instead of writing a full workflow:
    ```bash
    # Instead of writing a workflow, use batch for simple tasks
    oxo-flow batch "samtools flagstat {item}" *.bam -j 8
    oxo-flow batch "fastqc {item}" *.fastq.gz
    ```

---

## Complete example

See the [Workflow Format](../reference/workflow-format.md) reference for the full specification.
