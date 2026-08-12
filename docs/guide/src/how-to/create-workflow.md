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
author = "Shixiang Wang <w_shixiang@163.com>"
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
environment = { conda = "envs/tools.yaml" }
shell = "my-tool --threads {threads} {input} > {output}"

[rules.resources]
threads = 8
memory = "16G"
```

### Rule fields

| Field | Required | Type | Description |
|---|---|---|---|
| `name` | Yes | String | Unique rule identifier |
| `input` | Yes | Array | Input file paths (may contain wildcards) |
| `output` | Yes | Array | Output file paths (may contain wildcards) |
| `shell` | Yes | String | Shell command to execute |
| `environment` | No | Table | Environment specification |
| `resources` | No | Table | Resource specification (threads, memory, GPU, disk, time_limit) |

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

### How dependency inference works

When the DAG is built, oxo-flow examines every rule's `input` and `output` fields and creates edges wherever a match is found:

```toml
[[rules]]
name = "step1"
output = ["intermediate.txt"]

[[rules]]
name = "step2"
input = ["intermediate.txt"]   # ← matches step1's output → step2 depends on step1
```

Step-by-step, the engine:

1. **Registers outputs**: Each rule's `output` paths are recorded — "rule X produces file Y"
2. **Matches inputs**: Each rule's `input` paths are checked against the output registry
3. **Creates edges**: Every match creates a dependency edge (producer → consumer)
4. **Adds explicit deps**: Any `depends_on` entries also create edges
5. **Validates**: The resulting graph is checked for cycles

### Explicit vs inferred dependencies

| Mechanism | How it works | When to use |
|---|---|---|
| **File-based (inferred)** | `input`/`output` path matching | Standard pipeline steps — the default |
| **`depends_on` (explicit)** | Direct rule name reference | Ordering without shared files (e.g., `mkdir` before downstream tools) |

**Example — when to use `depends_on`:**

```toml
# Setup rule creates directories — no shared output files
[[rules]]
name = "setup_dirs"
shell = "mkdir -p results/qc results/aligned"
output = []  # No output files!

# Downstream rule doesn't consume setup's output, but must run after
[[rules]]
name = "align"
depends_on = ["setup_dirs"]   # ← explicit ordering, no file to match
input = ["raw/sample.fastq.gz"]
output = ["results/aligned/sample.bam"]
shell = "bwa mem ref.fa {input} > {output}"
```

**Tip:** Prefer file-based dependencies whenever possible — they make the data flow explicit and self-documenting. Use `depends_on` only when the ordering can't be expressed through files.

### Verifying your DAG

After writing your workflow, inspect the dependency structure:

```bash
# See the execution order and dependency graph
oxo-flow graph workflow.oxoflow

# See just the execution plan without running
oxo-flow dry-run workflow.oxoflow

# Check for structural issues (cycles, orphans, collisions)
oxo-flow validate workflow.oxoflow
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
