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
[[include]]         # Optional: include external workflow files
[[rules]]           # Required: one or more rules
```

---

## `[[include]]` — Modular Workflow Composition

Include external workflow files to enable modular, reusable workflow design:

```toml
[[include]]
path = "common/qc.oxoflow"
namespace = "qc"

[[include]]
path = "align.oxoflow"
```

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | String | **Yes** | Path to the included `.oxoflow` file |
| `namespace` | String | No | Optional namespace prefix for included rule names |

### Namespace Behavior

When a `namespace` is specified:

1. All rule names from the included file are prefixed: `namespace::rule_name`
2. Internal `depends_on` references within the included file are automatically prefixed
3. External `depends_on` references (to rules outside the included file) remain unchanged

**Example:**

```toml
# qc.oxoflow
[[rules]]
name = "fastqc"
input = ["{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
shell = "fastqc {input}"

[[rules]]
name = "trim"
input = ["{sample}.fastq.gz"]
output = ["trimmed/{sample}.fastq.gz"]
depends_on = ["fastqc"]  # Internal reference - will become "qc::fastqc"
shell = "fastp {input} -o {output}"
```

```toml
# main.oxoflow
[[include]]
path = "qc.oxoflow"
namespace = "qc"

[[rules]]
name = "align"
input = ["trimmed/{sample}.fastq.gz"]
depends_on = ["qc::trim"]  # Reference to included rule with namespace
shell = "bwa mem ref.fa {input} > aligned/{sample}.bam"
```

Resulting rules: `qc::fastqc`, `qc::trim`, `align`

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
| `interpreter_map` | Table | No | `{}` | Custom interpreter mapping for script extensions |

### Custom Interpreters (`interpreter_map`)

By default, oxo-flow auto-detects interpreters based on file extensions:
- `.py` → `python3`
- `.R`, `.r` → `Rscript`
- `.sh` → `sh`
- `.jl` → `julia`

You can override or extend this mapping in the `[workflow]` section:

```toml
[workflow]
name = "custom-interpreters"

[workflow.interpreter_map]
".m" = "octave"
".sas" = "sas"
".py" = "/opt/conda/bin/python"  # Override default
```

This mapping applies only to the [`script`](#rules-script-field) field.

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
| `shell` | String | No | Shell command to execute |
| `script` | String | No | Script file path (auto-detects interpreter) |
| `threads` | Integer | No | CPU threads (overrides defaults) |
| `memory` | String | No | Memory allocation (overrides defaults) |
| `environment` | Table | No | Environment specification |
| `when` | String | No | Conditional expression — skip rule when `false` |
| `envvars` | Table | No | Dictionary of environment variables to inject |
| `params` | Table | No | User-defined parameters for shell templates |
| `pre_exec` | String | No | Command to run *before* the main shell command |
| `on_success` | String | No | Command to run after rule succeeds |
| `on_failure` | String | No | Command to run after rule fails (all retries exhausted) |
| `retries` | Integer | No | Number of retry attempts on failure (default: 0) |
| `interpreter` | String | No | Explicit interpreter for script execution |
| `checkpoint` | Boolean | No | Rebuild DAG after this rule completes |

**Note:** At least one of `shell` or `script` must be provided. If both are defined, they execute sequentially: shell first, then script.

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

### Environment Variables (`envvars`)

Inject rule-specific environment variables directly into the execution context:

```toml
[[rules]]
name = "deep_learning"
shell = "python train.py"

[rules.envvars]
CUDA_VISIBLE_DEVICES = "0"
PYTHONPATH = "./src"
```

Variables defined here are available to the main `shell` command as well as all lifecycle hooks (`pre_exec`, etc.).

### Parameters (`params`)

Define custom variables for use in shell templates. Unlike `[config]`, which is global, `params` are specific to a single rule and take precedence during interpolation:

```toml
[[rules]]
name = "count_reads"
shell = "samtools view -c -q {params.min_qual} {input} > {output}"

[rules.params]
min_qual = 20
```

### Script Execution (`script`)

The `script` field allows you to execute external script files (Python, R, etc.) with automatic interpreter detection.

```toml
[[rules]]
name = "analyze"
script = "scripts/analysis.py --min-quality {params.q}"
interpreter = "python3" # Optional: overrides auto-detection
```

**Interpreter Detection Order:**
1.  **Explicit `interpreter` field** on the rule.
2.  **Custom `[workflow.interpreter_map]`** in the metadata.
3.  **Built-in defaults** based on file extension.
4.  **Shebang line** (if file is executable).

### Lifecycle Hooks

Hooks allow you to run auxiliary logic at different stages of a rule's life:

```toml
[[rules]]
name = "process_data"
shell = "python process.py"
pre_exec = "mkdir -p tmp_workspace"
on_success = "echo 'Success!' | slack-notify"
on_failure = "rm -rf tmp_workspace && echo 'Cleanup done'"
retries = 3
```

- **`pre_exec`**: Runs *before* the main command. If it fails, the rule is aborted.
- **`on_success`**: Runs only after the main command completes with exit code 0.
- **`on_failure`**: Runs if the main command fails, *after* all `retries` have been exhausted.

---

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

## Resource Management

### Declaration vs Enforcement

oxo-flow tracks declared resources for scheduling but does not strictly enforce them in local execution. On HPC clusters, resources are enforced by the scheduler.

**Local execution:**
- Resources are tracked to prevent over-allocation
- Warnings emitted when declaring resources exceeding system capacity
- Jobs may oversubscribe if user intentionally requests more than available

**HPC clusters:**
- Resources translated to scheduler directives (SLURM, PBS, SGE, LSF)
- Scheduler enforces limits - jobs requesting more than allocated will fail

### Platform Detection

| Platform | Thread Detection | Memory Detection |
|---|---|---|
| Linux | `num_cpus` crate | `sysinfo` crate |
| macOS | `num_cpus` crate | `sysinfo` crate |

### Validation Warnings

When a rule declares resources exceeding system capacity, oxo-flow emits warnings during validation but does not block execution:

```
⚠️  rule 'bwa_align' requests 128 threads but system has 64 (will oversubscribe)
⚠️  rule 'big_sort' requests 128GB but system has 32GB (may OOM)
```

This allows intentional oversubscription for testing or when user knows better.

### Cleanup Behavior

oxo-flow automatically cleans up temporary outputs:

| Scenario | Cleanup |
|---|---|---|
| Success + `temp_output` | Cleaned after successful completion |
| Failure + `temp_output` | Cleaned to prevent stale partial files |
| Transform with `cleanup=true` | Chunk files cleaned after combine succeeds |

### Timeout Enforcement

On Unix systems (Linux, macOS), timeout kills the entire process group, ensuring child processes don't survive:

```toml
[rules.resources]
time_limit = "4h"  # SIGKILL sent to process group after 4 hours
```

### GPU Specification

For detailed GPU requirements:

```toml
[rules.resources.gpu_spec]
count = 2
model = "A100"           # SLURM: --gres=gpu:a100:2
memory_gb = 40           # SLURM: --mem-per-gpu=40G
compute_capability = "8.0"  # For filtering (not scheduler directive)
```

Note: PBS/SGE GPU syntax varies by site. Use `extra_args` for site-specific flags.

### Resource Hints

When exact requirements unknown, provide hints for estimation:

```toml
[rules.resource_hint]
input_size = "medium"     # small (~1GB), medium (~10GB), large (~100GB), xlarge (~500GB)
memory_scale = 2.0        # Estimated memory = input_size × scale
runtime = "slow"          # fast (<10min), medium (10min-1h), slow (>1h)
io_bound = true           # true = I/O bound, false = CPU bound
```

Memory estimation formula: `estimated_mb = input_size_mb × memory_scale`

---

## Script Execution

### Script Field

Execute a script file instead of (or in addition to) a shell command:

```toml
[[rules]]
name = "analysis"
input = ["data.csv"]
output = ["results.json"]
script = "scripts/analyze.py"  # Auto-detects interpreter from extension
```

When both `shell` and `script` are defined, they execute sequentially: **shell first, then script**.

```toml
[[rules]]
name = "qc_and_report"
shell = "fastqc {input} -o qc/"
script = "reports/qc_report.qmd"  # Runs after shell completes
```

### Interpreter Detection

oxo-flow automatically detects the interpreter from script file extension:

| Extension | Interpreter | Notes |
|-----------|-------------|-------|
| `.py` | `python` | Python script |
| `.R` / `.r` | `Rscript` | R script |
| `.jl` | `julia` | Julia script |
| `.sh` / `.bash` | `bash` | Shell script |
| `.pl` | `perl` | Perl script |
| `.rb` | `ruby` | Ruby script |
| `.qmd` | `quarto render` | Quarto document |
| `.Rmd` / `.rmd` | `quarto render` | R Markdown |
| `.ipynb` | `jupyter nbconvert --to notebook --execute` | Jupyter notebook |
| `.smk` | `snakemake` | Snakemake workflow |
| `.nextflow` | `nextflow run` | Nextflow script |
| `.wdl` | `miniwdl run` | WDL workflow |

### Explicit Interpreter Override

Override auto-detection with `interpreter` field:

```toml
[[rules]]
name = "custom_python"
script = "analyze.py3"
interpreter = "python3.11"  # Override default python
```

### Custom Interpreter Map

Configure custom interpreter mappings at workflow level:

```toml
[workflow]
name = "pipeline"

[workflow.interpreter_map]
".m" = "octave"        # MATLAB/Octave
".sas" = "sas"         # SAS
".do" = "stata-mp"     # Stata
".stan" = "cmdstan"    # Stan
```

---

## Additional Rule Fields

### Output Management

| Field | Type | Description |
|-------|------|-------------|
| `temp_output` | Array | Temporary outputs cleaned after downstream rules complete |
| `protected_output` | Array | Protected outputs never overwritten or deleted |

```toml
[[rules]]
name = "align"
output = ["aligned/{sample}.bam", "aligned/{sample}.bam.bai"]
temp_output = ["aligned/{sample}.tmp.bam"]  # Cleaned after downstream use
```

### Execution Control

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `depends_on` | Array | — | Explicit rule dependencies (not inferred from files) |
| `localrule` | Boolean | `false` | Always run locally, never submit to cluster |
| `workdir` | String | — | Per-rule working directory override |
| `shadow` | String | — | Atomic execution mode: `"minimal"`, `"shallow"`, `"full"` |
| `checkpoint` | Boolean | `false` | Enable dynamic DAG modification |

```toml
[[rules]]
name = "setup"
shell = "mkdir -p results"
depends_on = []  # Run first, before file-based dependencies

[[rules]]
name = "local_only"
shell = "echo 'local task'"
localrule = true  # Never submitted to HPC cluster
```

### Retry Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `retries` | Integer | 0 | Number of automatic retry attempts |
| `retry_delay` | String | — | Delay between retries (`"5s"`, `"30s"`, `"2m"`) |

```toml
[[rules]]
name = "network_task"
shell = "curl https://api.example.com/data"
retries = 3
retry_delay = "30s"
```

### Input/Output Hints

| Field | Type | Description |
|-------|------|-------------|
| `ancient` | Array | Inputs that never trigger re-execution (reference files) |
| `format_hint` | Array | File format hints for I/O optimization (`"bam"`, `"vcf"`) |
| `pipe` | Boolean | Enable FIFO streaming mode for inputs |
| `checksum` | String | Output checksum algorithm (`"md5"`, `"sha256"`) |

```toml
[[rules]]
name = "align"
input = ["reads/{sample}.fastq.gz", "ref/hg38.fa"]
ancient = ["ref/hg38.fa"]  # Reference never triggers rebuild
format_hint = ["bam"]
checksum = "sha256"
```

### Organization

| Field | Type | Description |
|-------|------|-------------|
| `tags` | Array | Categorization tags (`["qc", "alignment"]`) |
| `extends` | String | Base rule to inherit settings from |

```toml
[[rules]]
name = "align_default"
threads = 8
memory = "32G"
tags = ["alignment", "production"]

[[rules]]
name = "align_fast"
extends = "align_default"  # Inherits threads, memory, tags
threads = 16  # Override inherited value
```

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

See [`examples/paired_experiment_control_pairs.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/paired_experiment_control_pairs.oxoflow) for a full clinical somatic calling pipeline.

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

See [`examples/cohort_analysis.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/cohort_analysis.oxoflow) for a complete cohort study pipeline.

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

See [`examples/conditional_workflow.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/conditional_workflow.oxoflow) for a full example.

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
