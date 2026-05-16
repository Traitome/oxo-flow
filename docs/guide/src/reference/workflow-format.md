# Workflow Format

The `.oxoflow` file format is oxo-flow's TOML-based workflow definition language. This page provides the complete specification, design philosophy, and syntax rules.

---

## Design Principles

The `.oxoflow` format is built on four core principles:

1.  **Declarative over Imperative** — Define *what* should happen (inputs, outputs, tools), not *how* to orchestrate it. The engine handles the execution logic.
2.  **Explicit is better than Implicit** — Every dependency and environment should be clearly visible. No hidden global state.
3.  **Composition over Inheritance** — Reuse logic through modular `include` directives and rule templates rather than complex inheritance hierarchies.
4.  **Traceability by Default** — The format structure directly supports generating clinical-grade provenance and audit trails.

---

## TOML Primer

oxo-flow uses the **TOML (Tom's Obvious, Minimal Language)** format. If you are new to TOML, here are the three essential concepts used in `.oxoflow` files:

1.  **Key-Value Pairs**: `key = "value"`. Strings must be in quotes.
2.  **Tables**: `[name]` defines a section (an object/map).
3.  **Arrays of Tables**: `[[name]]` defines a list of sections. In oxo-flow, rules are defined using double brackets because a workflow contains multiple rules.

For more details, see the [Official TOML Specification](https://toml.io/).

---

## File Extension

Workflow files must use the `.oxoflow` extension (e.g., `qc_pipeline.oxoflow`).

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
| `when` | String | No | Conditional expression — skip rule when `false` |

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

## `[[pairs]]` — Experiment-Control Pairing (WC-01)

`[[pairs]]` defines experiment-control sample pairs for somatic variant calling and other comparative analyses.

```toml
[[pairs]]
pair_id = "CASE_001"
experiment = "EXP_01"
control    = "CTRL_01"

[[pairs]]
pair_id = "CASE_002"
experiment = "EXP_02"
control    = "CTRL_02"
```

| Field | Type | Required | Description |
|---|---|---|---|
| `pair_id` | String | **Yes** | Unique identifier for this pair |
| `experiment` | String | **Yes** | Experiment sample name |
| `control` | String | **Yes** | Matched control sample name |

Any rule that references `{experiment}`, `{control}`, or `{pair_id}` in its `input`, `output`, or `shell` fields is **automatically expanded** into one concrete rule instance per pair.  Rules that do not reference any pair wildcard are kept as-is.

**Expanded rule naming:** `{rule_name}_{pair_id}` (e.g., `mutect2_CASE_001`).

### Example

```toml
[[pairs]]
pair_id = "CASE_001"
experiment = "EXP_01"
control    = "CTRL_01"

[[rules]]
name   = "mutect2"
input  = ["aligned/{experiment}.bam", "aligned/{control}.bam"]
output = ["variants/{pair_id}.vcf.gz"]
shell  = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal {control} -O {output[0]}"
```

Produces rule `mutect2_CASE_001` with concrete file paths.

See [`examples/paired_experiment_control_pairs.oxoflow`](../../../examples/paired_experiment_control_pairs.oxoflow) for a full clinical somatic calling pipeline.

---

## `[[sample_groups]]` — Multi-Sample Cohorts (WC-02)

`[[sample_groups]]` organises samples into named groups (e.g., case vs. control) for cohort studies.

```toml
[[sample_groups]]
name    = "control"
samples = ["CTRL_001", "CTRL_002", "CTRL_003"]

[[sample_groups]]
name    = "case"
samples = ["CASE_001", "CASE_002"]
```

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | **Yes** | Group name |
| `samples` | Array of strings | **Yes** | Sample identifiers in this group |
| `metadata` | Table | No | Arbitrary group-level metadata |

Any rule that references `{sample}` or `{group}` is expanded once per `(group, sample)` pair across all groups.

**Expanded rule naming:** `{rule_name}_{group}_{sample}` (e.g., `align_control_CTRL_001`).

### Example

```toml
[[sample_groups]]
name    = "treatment"
samples = ["S001", "S002"]

[[rules]]
name   = "align"
input  = ["raw/{sample}_R1.fq.gz"]
output = ["aligned/{sample}.bam"]
shell  = "bwa mem ref.fa {input[0]} > {output[0]}"
```

Produces `align_treatment_S001` and `align_treatment_S002`.

See [`examples/cohort_analysis.oxoflow`](../../../examples/cohort_analysis.oxoflow) for a complete cohort study pipeline.

---

## `when` — Conditional Rule Execution (WF-01)

The optional `when` field on a rule contains an expression evaluated against `[config]` values.  When the expression evaluates to **false** the rule is skipped entirely and removed from the DAG.

```toml
[[rules]]
name  = "fastqc"
when  = "config.run_qc"
input = ["raw/sample_R1.fq.gz"]
output = ["qc/sample_fastqc.html"]
shell = "fastqc {input[0]} -o qc/"
```

### Expression syntax

| Form | Example | Description |
|---|---|---|
| `config.<key>` | `config.run_qc` | Truthy check (true, non-zero, non-empty string) |
| `config.<key> == "value"` | `config.mode == "WGS"` | String equality |
| `config.<key> != "value"` | `config.mode != "WES"` | String inequality |
| `config.<key> == true\|false` | `config.skip == false` | Boolean equality |
| `config.<key> > N` | `config.min_cov >= 20` | Numeric comparison (`>`, `>=`, `<`, `<=`) |
| `file_exists("path")` | `file_exists("panel.bed")` | File existence test |
| `!<expr>` | `!config.skip` | Logical NOT |
| `<expr> && <expr>` | `config.run_qc && config.min_cov >= 20` | Logical AND |
| `<expr> \|\| <expr>` | `config.wgs \|\| config.wes` | Logical OR |
| `(<expr>)` | `(config.a && config.b) \|\| config.c` | Grouping |

### Example

```toml
[config]
run_annotation = true
min_coverage   = 30
mode           = "WGS"

[[rules]]
name = "vep_annotate"
when = 'config.run_annotation && config.min_coverage >= 20'
# ...

[[rules]]
name = "wgs_coverage"
when = 'config.mode == "WGS"'
# ...
```

See [`examples/conditional_workflow.oxoflow`](../../../examples/conditional_workflow.oxoflow) for a full example.

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

## `transform` — Unified Scatter-Gather Operator

The `transform` operator unifies split → map → combine patterns into a single rule declaration, similar to dplyr's `group_by() %>% summarize()` or pandas' `groupby().apply()`.

### Structure

```toml
[[rules]]
name = "variant_calling"
input = ["aligned/sample.bam"]
output = ["variants/sample.vcf.gz"]

[rules.transform.split]
by = "chr"
values_from = "config.chromosomes"

[rules.transform]
map = "gatk HaplotypeCaller -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz"
cleanup = true

[rules.transform.combine]
shell = "gatk GatherVcfs {chunks} -O {output}"
```

### Split Configuration

| Field | Type | Description |
|---|---|---|
| `by` | String | **Required**. Variable name for splitting (e.g., `"chr"`, `"sample"`) |
| `values` | Array | Direct list of split values |
| `values_from` | String | Reference to config variable (e.g., `"config.chromosomes"`) |
| `n` | String | Number of chunks (generates indices 0, 1, ..., n-1) |
| `glob` | String | Glob pattern to find split values from files |

Priority: `values` → `values_from` → `n` → `glob`

### Combine Configuration

| Field | Type | Description |
|---|---|---|
| `shell` | String | Shell command to combine chunks |
| `aggregate` | Boolean | Enable automatic aggregation |
| `method` | String | Aggregation method: `"concat"` or `"json_merge"` |
| `header` | String | Header line for concat aggregation |

### Built-in Variables

| Variable | Expands to |
|---|---|
| `{split_var}` | Current split value (e.g., `{chr}` → `"chr1"`) |
| `{chunks}` | Space-separated list of all chunk outputs |
| `{input}` | Original rule input (in combine) |
| `{output}` | Original rule output (in combine) |

### Modes

**Mode A: Split → Map → Combine**

Classic scatter-gather with explicit combine command:

```toml
[rules.transform.split]
by = "chr"
values_from = "config.chromosomes"

[rules.transform]
map = "gatk HaplotypeCaller -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz"

[rules.transform.combine]
shell = "gatk GatherVcfs {chunks} -O {output}"
```

**Mode B: Split → Map → Aggregate**

Automatic aggregation (concat or json_merge):

```toml
[rules.transform.split]
by = "chunk"
n = "5"

[rules.transform]
map = "process {input} > .oxo-flow/chunks/{chunk}.txt"

[rules.transform.combine]
aggregate = true
method = "concat"
```

**Mode C: Split → Map (No Combine)**

Parallel processing without merging — each split produces independent output:

```toml
[rules.transform.split]
by = "chr"
values_from = "config.chromosomes"

[rules.transform]
map = "samtools flagstat {input} > qc/{chr}.flagstat.txt"
# No combine section
```

### Cleanup

When `cleanup = true`, chunk files are automatically cleaned up after combine succeeds:

```toml
[rules.transform]
cleanup = true
```

### Expanded Rule Naming

Transform rules expand into:

- Map rules: `{rule_name}_{split_value}` (e.g., `variant_calling_chr1`)
- Combine rule: `{rule_name}_combine` (e.g., `variant_calling_combine`)

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
