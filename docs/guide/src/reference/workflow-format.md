# Workflow Format

The `.oxoflow` file format is oxo-flow's TOML-based workflow definition language. This page provides the complete specification, design philosophy, and syntax rules.

---

## Design Principles

The `.oxoflow` format is built on four core principles:

1.  **Declarative over Imperative** — Define *what* should happen (inputs, outputs, tools), not *how* to orchestrate it. The engine handles the execution logic.
2.  **Explicit is better than Implicit** — Every dependency and environment should be clearly visible. No hidden global state.
3.  **Composition over Inheritance** — Reuse logic through modular `include` directives and rule templates rather than complex inheritance hierarchies.
4.  **Traceability by Default** — The format structure directly supports generating provenance and audit trails (workflow checksums, per-rule input manifests, output hashes).

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
[config]            # Optional: user variables (plain or declarative form)
[defaults]          # Optional: rule defaults
[report]            # Optional: report configuration
[[include]]         # Optional: include external workflow files
[[references]]      # Optional: auto-built indexes and reference data
[[rules]]           # Required: one or more rules
[[pairs]]           # Optional: experiment-control pairs (WC-01)
[[sample_groups]]   # Optional: multi-sample groups (WC-02)
[resource_budget]   # Optional: resource limits
[env_groups]        # Optional: named reusable environment specs
[resource_groups]   # Optional: shared resource pools (API limits, DB connections)
[wildcard_constraints] # Optional: regex patterns to constrain wildcard values
[[execution_group]] # Optional: explicit sequential/parallel rule ordering
[cluster]           # Optional: HPC cluster profile (SLURM, PBS, SGE, LSF)
[[reference_db]]    # Optional: tracked reference database versions
[citation]          # Optional: citation metadata (DOI, authors, etc.)
[plugins]           # Optional: plugin configuration
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

### Interface Contracts

An `[[include]]` may declare a typed interface — what the host must wire
in, what the module exposes, and config defaults it reads (issue #112).
All checks run at **validation time**; execution semantics are unchanged,
and includes without a contract behave exactly as before.

```toml
# qc.oxoflow
[[rules]]
name = "fastqc"
input = ["raw/{sample}.fq.gz"]      # the module's NEED
output = ["qc/{sample}_fastqc.html"]
shell = "fastqc {input} -o {output}"

[[rules]]
name = "internal"
input = ["qc/{sample}_fastqc.html"]
output = ["qc/{sample}.bin"]        # internal — not declared below
shell = "true"

# main.oxoflow
[[include]]
path = "qc.oxoflow"
namespace = "qc"
inputs  = ["raw/{sample}.fq.gz"]    # host MUST produce these
outputs = ["qc/{sample}_fastqc.html"]  # module EXPOSES these
params  = { threads = "4" }         # config defaults (host values win)

[[rules]]
name = "download"
output = ["raw/{sample}.fq.gz"]     # wires the declared input
shell = "true"
```

| Field | Type | Description |
|---|---|---|
| `inputs` | Array of String | File patterns the host must wire into the module. A concrete declared input (no `{`-wildcard) that no rule produces fails validation with the gap named; wildcarded declared inputs are not statically checkable (a `{sample}` input forms no DAG edge — the placeholder only resolves at run time) — the host is responsible for wiring them |
| `outputs` | Array of String | Files the module exposes to the host. A declared output not produced by a module rule fails validation; a host rule reading a module-internal file that is **not** declared here produces an encapsulation warning. Wildcarded host inputs are matched structurally (identical literal prefix and suffix, e.g. `qc/{sample}.tmp`), so patterns that could address the same files warn too; patterns that resolve to different literals are not statically checkable |
| `params` | Table | Defaults for config keys the module reads, filled in profile-style (`or_insert` — explicit host values win). The module's own `[config]` entries fill any remaining gaps: a module declaring `[config] trim_quality = "20"` keeps that default when included without `params`, while any host value (its own `[config]` table or `[[include]]` `params`) still wins. A key referenced but defined nowhere remains an E005 error |
| `name` | String | Module identity for partial runs (`run --module <name>`). Defaults to the included file's stem (`rules/20_germline.oxoflow` → `20_germline`), so existing composed workflows are addressable without changes |
| `repo` + `ref` | String pair | Pin the included path to a git repository at a tag/branch/commit: `repo = "https://github.com/org/oxo-flow-modules"`, `ref = "v0.14.1"`, `path = "rules/qc.oxoflow"`. Any git URL works (https/ssh/file://); github.com clones fall back to China mirrors. Checkouts are cached under `~/.cache/oxo-flow/modules/<repo>@<ref>` (`OXO_FLOW_MODULE_CACHE` overrides) — versioned modules, reproducible composition |

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
| `genome_build` | String | No | — | Genome reference build identifier (e.g., `"GRCh38"`, `"hg38"`) |
| `min_version` | String | No | — | Minimum oxo-flow version required to run this workflow |
| `format_version` | String | No | — | Format specification version for compatibility |
| `pairs_file` | String | No | — | External TSV/CSV/JSON file defining experiment-control pairs |
| `sample_groups_file` | String | No | — | External TSV/JSON file defining sample groups |
| `pairs_pattern` | String | No | — | File glob pattern for auto-discovering pairs (e.g., `"aligned/{pair_id}/{exp}_vs_{ctrl}.bam"`) |
| `sample_pattern` | String | No | — | File glob pattern for auto-discovering samples (e.g., `"raw/{sample}_R1.fastq.gz"`) |

### Custom Interpreters (`interpreter_map`)

By default, oxo-flow auto-detects interpreters based on file extensions:

- `.py` → `python`
- `.py3` → `python3`
- `.R`, `.r` → `Rscript`
- `.sh`, `.bash` → `bash`
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

This mapping applies only to the [`script`](#script-execution-script) field.

---

## `[config]` — Configuration Variables

User-defined key-value pairs accessible in rules as `{config.<key>}`. Every
config key can be overridden from the CLI.

Two forms are supported:

```toml
[config]
reference = "/data/ref/hg38.fa"             # plain value (implicit default)
samples_dir = "raw_data"
results_dir = "results"
min_quality = "30"
```

Values are TOML strings, integers, booleans, or arrays. String interpolation
in rules uses `{config.key}` syntax; array values render as space-joined
lists in the shell.

Several keys are **engine-injected**:
- `config.samples_list` (all sample names, comma-joined) and
  `config.pairs_list` (all pair ids, comma-joined) — use them in
  `expand_inputs` to gather across samples or pairs without maintaining a
  parallel list by hand. A user-declared `config.samples_list` is
  union-merged with the discovered samples, and the two lists are also
  rewritten by `--samples` filtering.
- `config.samples_<group>` (one per `[[sample_groups]]` entry).
- `config.<reference name>` (each `[[references]]` entry's `output`) and,
  with `reference_dir`, the ten derived paths (`reference_fasta`,
  `gene_annotation`, `bwa_index`, `bwamem2_index`, `bowtie2_index`,
  `star_index`, `hisat2_index`, `minimap2_index`, `gatk_dict`,
  `samtools_faidx`). Explicit `[config]` entries with the same names are
  never overwritten.

### Declarative Form (inline table)

For user-facing parameters, use the declarative inline-table form. Declared
keys become automatic CLI flags (`--<key> <value>`), can be required, and
carry metadata:

```toml
[config]
database  = { required = true, help = "Path to the BLAST database" }
threshold = { default = "1e-5", help = "E-value cutoff" }
mode      = { default = "dna", choices = ["dna", "rna"], type = "string" }
samples   = { required = true, type = "path", must_exist = true }
```

### Fields

| Field | Type | Description |
|---|---|---|
| `default` | String | Default value when not provided via CLI |
| `required` | Bool | If `true`, the value must be provided at runtime (default: `false`) |
| `help` | String | Human-readable description shown in error messages |
| `sensitive` | Bool | Mask the value as `***` in captured rule output, recorded commands, checkpoint/report snapshots, and error summaries — before they reach the web UI or AI recovery (default: `false`). Values shorter than 4 characters are not masked, and structured or derived forms of the value (JSON/base64) pass through — exact-match masking only |
| `type` | String | Expected value type for validation: `"string"`, `"int"`, `"float"`, `"bool"`, `"path"` |
| `choices` | Array of String | Allowed values (requires `type = "string"`) |
| `range` | String | Numeric range `"min..max"` (requires `type = "int"` or `"float"`) |
| `must_exist` | Bool | Path must exist on disk (requires `type = "path"`) |

### Usage in Rules

Values are accessible as `{config.<name>}` in shell commands, input/output
paths, and `when` conditions:

```toml
[[rules]]
name = "blast"
shell = "blastn -db {config.database} -evalue {config.threshold} -query {input} -out {output}"
```

### CLI

```bash
# Provide a required value (direct flag form)
oxo-flow run pipeline.oxoflow --database refs/nt

# Attached form
oxo-flow run pipeline.oxoflow --database=refs/nt

# Key=value form directly after the workflow file
oxo-flow run pipeline.oxoflow database=refs/nt

# Override several values
oxo-flow run pipeline.oxoflow --database refs/nt --threshold 1e-3

# Legacy --arg form (still supported)
oxo-flow run pipeline.oxoflow --arg database=refs/nt --arg threshold=1e-3
```

### Precedence

CLI override > declared `default` > error if `required` and unset.

---

---

## `[[references]]` — Auto-Built Indexes & Reference Data

Declare reference artifacts (indexes, data files) that the engine auto-builds
when missing. Each `[[references]]` entry specifies a source, output, and build
command. The engine tracks built state in `.oxo-flow/reference-checkpoint.json`
and never rebuilds unnecessarily: each entry stores a fingerprint of the
definition plus the config values its build command references. The source
file's **content state** joins the fingerprint: size and mtime for every
source, plus a content hash for files up to 64 MiB — so for those files even
a timestamp-preserving same-path replacement (`cp -p`, `rsync -t`, `git
checkout`) triggers a rebuild; larger sources are guarded by size + mtime.
Editing the build/source/output, changing a referenced config value, or
touching the source file also triggers a rebuild, and rules that consume the
artifact through declared `input` paths (plus their downstream) are
invalidated so their outputs are regenerated.

When `reference_dir` is set, four standard indexes are auto-derived without
explicit `[[references]]` blocks: BWA, Bowtie2, STAR, and HISAT2.

Use `--skip-ref-build` to skip automatic reference building.

### Syntax

```toml
[[references]]
name = "bwa_index"
source = "{reference_dir}/genome.fa"
output = "{reference_dir}/bwa/genome.fa"
build = "mkdir -p {reference_dir}/bwa && bwa index -p {output} {source}"
threads = 8
memory = "8G"
description = "BWA index for alignment"

[references.environment]
conda = "envs/bwa.yaml"
```

### Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | Yes | Unique name (used for checkpoint tracking) |
| `source` | String | No | Source file for freshness checks |
| `output` | String | Yes | Path to the built artifact |
| `build` | String | Yes | Shell command to produce output from source |
| `threads` | Integer | No | CPU threads for the build command |
| `memory` | String | No | Memory limit (e.g., `"64G"`) |
| `description` | String | No | Human-readable description |
| `environment` | Table | No | Environment providing the build tool — same spec as `[rules.environment]` (conda/mamba/docker/…). Without it the build runs in the bare system shell, so builds that need workflow tools (`bowtie2-build`, `STAR --runMode genomeGenerate`, …) **must** declare one. The environment is created on first use and shared with rules through the same cache. |

### Built-in Builder Templates

Instead of a handwritten `build` shell, name one of the built-in
templates — the engine expands it into a canonical command (any string
containing whitespace or shell syntax is treated as a handwritten
command and passes through unchanged):

| Template | Produces |
|---|---|
| `samtools_faidx` | `.fai` index |
| `picard_dict` | `.dict` sequence dictionary |
| `bwa_index` | BWA's five index files (`.amb/.ann/.bwt/.pac/.sa`; the prefix is derived from `output`) |
| `star_index` | STAR genome directory (`output` = `--genomeDir`) |
| `bismark_index` | Bismark `Bisulfite_Genome` directory (`source` = FASTA directory) |

```toml
[[references]]
name = "genome_bwa"
source = "refs/genome.fa"
output = "refs/genome.fa.bwt"
build = "bwa_index"
threads = 8
```

Unknown template names, missing outputs, and duplicate reference names
are rejected at `validate`. Additionally, every reference injects a
keyed config value — `config.<name>` = its `output` — so rules consume
the artifact as `{config.genome_bwa}` without repeating paths.

### Auto-Derivation from `reference_dir`

When `[config]` contains `reference_dir` and no explicit `[[references]]` blocks
are declared, the engine automatically derives eight standard references:

| Name | Output |
|---|---|
| `samtools_faidx` | `{reference_dir}/genome.fa.fai` |
| `bwa_index` | `{reference_dir}/bwa/genome.fa.bwt` |
| `bwamem2_index` | `{reference_dir}/bwamem2/genome.fa.0123` |
| `bowtie2_index` | `{reference_dir}/bowtie2/genome.fa.1.bt2` |
| `minimap2_index` | `{reference_dir}/genome.fa.mmi` |
| `star_index` | `{reference_dir}/star/SAindex` |
| `hisat2_index` | `{reference_dir}/hisat2/genome.fa.1.ht2` |
| `gatk_dict` | `{reference_dir}/genome.dict` |

`reference_dir` also auto-derives config defaults such as
`reference_fasta` (`{reference_dir}/genome.fa`) and `gene_annotation`
(`{reference_dir}/genes.gtf`). Users can override any auto-derived reference
by declaring it explicitly.

### CLI

```bash
# Auto-build missing references
oxo-flow run pipeline.oxoflow -j 16

# Skip reference building (assume pre-built)
oxo-flow run pipeline.oxoflow --skip-ref-build
```

---

## `[defaults]` — Default Settings

Applied to all rules unless explicitly overridden:

```toml
[defaults]
threads = 4
memory = "8G"
environment = { conda = "envs/base.yaml" }
shell_prelude = "set -euo pipefail"
```

| Field | Type | Description |
|---|---|---|
| `threads` | Integer | Default CPU thread count |
| `memory` | String | Default memory allocation |
| `environment` | Table | Default environment specification |
| `shell_prelude` | String | Shell text prepended to every rule command, hook, reference build, and cluster job script on its own line (issue #92) — e.g. `set -euo pipefail` for fail-fast shells. **Opt-in**: empty by default, so existing workflows keep their exact command text. Applies inside environment wrappers (containers, conda) on every execution path. `pipefail` requires bash (the engine falls back to `sh` only when bash is absent) |

---

## `[report]` — Report Configuration

Configure report generation behavior. Reports are built by a pluggable section system — each section is produced by a `ReportSectionGenerator`. Use `sections` to control which generators run.

```toml
[report]
template = "report.html"
sections = ["universal", "workflow-info", "commands", "clinical-compliance"]
```

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template: the built-in name `"report.html"`, or a template file path (workflow directory first, then cwd). Applies to HTML output only; a render failure warns and falls back to the default renderer |
| `format` | Array | Parsed but **not supported yet** — setting it makes `report` warn (or fail under `--strict`); select the output format with `-f` instead |
| `sections` | Array | Report sections to include. If empty (or omitted), all applicable generators run. Available built-in IDs: `universal`, `execution-status`, `clinical-compliance`, `workflow-info`, `commands`, `file-manifest`, `environment`, `metrics`, `sample-matrix`, `provenance`, `task-summary` |

### How Sections Work

Each section ID maps to a registered `ReportSectionGenerator`:

| Section ID | Description | When Active |
|-----------|-------------|-------------|
| `universal` | Dashboard with QC indicators and task counts | Always |
| `workflow-info` | Name, version, author, config, sample/pair counts | Always |
| `commands` | Expanded shell commands for every rule | Always |
| `file-manifest` | Input and output file listings | Always |
| `environment` | Available backends and oxo-flow version | Always |
| `clinical-compliance` | ACMG/AMP classification, audit trail, biomarkers | Always |
| `execution-status` | Per-rule execution status and benchmark metrics | Only with checkpoint |

The domain (DNA-seq, RNA-seq, epigenomics, or generic) is auto-detected from tool names in the workflow. Custom generators can be registered programmatically.

---

## `[[rules]]` — Rule Definitions

Each `[[rules]]` entry defines a pipeline step. The double brackets indicate a TOML array of tables.

### Basic example

```toml
[[rules]]
name = "align"
input = ["{sample}_R1.fastq.gz", "{sample}_R2.fastq.gz"]
output = ["aligned/{sample}.bam"]
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"

[rules.resources]
threads = 16
memory = "32G"
```

### All fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | String | **Yes** | Unique rule identifier |
| `input` | Array of strings | **Yes** | Input file paths |
| `output` | Array of strings | **Yes** | Output file paths |
| `shell` | String | No | Shell command to execute |
| `script` | String | No | Script file path (auto-detects interpreter) |
| `description` | String | No | Human-readable description of what this rule does |
| `threads` | Integer | No | *(Deprecated)* CPU threads — use `resources.threads` instead. Still honored, but `oxo-flow lint` flags it with a W025 suggestion to move it under `[rules.resources]` |
| `memory` | String | No | *(Deprecated)* Memory allocation — use `resources.memory` instead. Still honored, but `oxo-flow lint` flags it with a W025 suggestion to move it under `[rules.resources]` |
| `resources` | Table | No | Full resource specification (threads, memory, gpu, disk, time_limit, partition, groups) |
| `environment` | Table | No | Environment specification |
| `transform` | Table | No | Unified scatter-gather operator (split → map → combine) |
| `when` | String | No | Conditional expression — skip rule when `false` |
| `envvars` | Table | No | Dictionary of environment variables to inject |
| `params` | Table | No | User-defined parameters for shell templates |
| `pre_exec` | String | No | Command to run *before* the main shell command |
| `on_success` | String | No | Command to run after rule succeeds |
| `on_failure` | String | No | Command to run after rule fails (all retries exhausted) |
| `retries` | Integer | No | Number of retry attempts on failure (default: 0) |
| `interpreter` | String | No | Explicit interpreter for script execution |
| `checkpoint` | Boolean | No | Enable checkpoint re-entry after this rule completes (requires `checkpoint_manifest`) |
| `checkpoint_manifest` | String | No | Path (relative to workdir, `{config.x}`-expanded) of the TOML manifest the checkpoint rule writes at runtime to declare new samples |
| `scatter` | Table | No | Fan-out parallel execution over a variable with optional gather |
| `expand_inputs` | Table | No | Cartesian product expansion of input patterns |
| `priority` | Integer | No | Execution priority (higher = runs first; default: 0) |
| `target` | Boolean | No | Mark as default target (built when no explicit `-t` given) |
| `required` | Boolean | No | Pipeline fails if this rule fails, even without downstream deps |
| `optional` | Boolean | No | Rule is skipped (no error) when its inputs don't exist — literal globs count as missing only when they match nothing |
| `benchmark` | String | No | Benchmark output path for performance data |
| `log` | String | No | Log file path for rule execution output |
| `group` | String | No | Job group label for cluster submission grouping |
| `cache_key` | String | No | Content-based cache key for output reuse |
| `input_function` | String | No | Dynamic input resolver function name |
| `rule_metadata` | Table | No | Arbitrary domain-specific metadata (assay, organism, etc.) |
| `env_group` | String | No | Reference to a named environment in `[env_groups]` |
| `depends_on` | Array | No | Explicit rule-level dependencies (by rule name) |
| `extends` | String | No | Inherit settings from a base rule |
| `retry_delay` | String | No | Delay between retries (e.g., `"5s"`, `"30s"`, `"2m"`) |
| `temp_output` | Array | No | Temporary outputs cleaned up after downstream rules complete |
| `temporary` | Boolean | No | Delete the rule's outputs after a fully successful run once every dependent has completed, recording a tombstone so a future run regenerates them on demand (leaf rules keep their outputs) |
| `scratch` | Boolean | No | Execute in an isolated per-instance scratch directory: inputs render as absolute paths, declared outputs written there move back to the main workdir and are verified, and the scratch is removed on success (preserved with a path note on failure). Use for tools that write fixed filenames or pollute the workdir |
| `protected_output` | Array | No | Outputs that must never be overwritten or deleted |
| `tags` | Array | No | Categorization tags (e.g., `["qc", "alignment"]`) |
| `shadow` | String | No | Shadow directory mode: `"minimal"`, `"shallow"`, or `"full"` |
| `ancient` | Array | No | Inputs that never trigger re-execution (e.g., reference files) |
| `localrule` | Boolean | No | Always run locally — never submit to a cluster scheduler |
| `format_hint` | Array | No | File format hints for I/O optimization (`"bam"`, `"vcf"`, `"fastq.gz"`) |
| `pipe` | Boolean | No | Enable FIFO streaming mode for input/output |
| `checksum` | String | No | Output integrity verification (`"md5"` or `"sha256"`) |
| `resource_hint` | Table | No | Resource estimation hints for dynamic scheduling |

**Note:** When a rule declares outputs, at least one of `shell`, `script`, or `transform` must be provided. If both `shell` and `script` are defined, they execute sequentially: shell first, then script.

### Environment specification

Exactly one backend per rule (uncomment the one you need):

```toml
[[rules]]
name = "example"
# Conda
environment = { conda = "envs/tools.yaml" }

# # Pixi
# environment = { pixi = "envs/pixi.toml" }

# # Docker
# environment = { docker = "biocontainers/bwa:0.7.17" }

# # Singularity
# environment = { singularity = "docker://biocontainers/bwa:0.7.17" }

# # Python venv
# environment = { venv = "envs/requirements.txt" }

# # HPC modules
# environment = { modules = ["gcc/11.2.0", "openmpi/4.1.1"] }

# # Conda with custom prefix
# environment = { conda = "envs/qc.yaml", conda_prefix = ".oxo-flow/envs" }
```

**Container shell:** `docker`/`singularity` rules run under `bash` inside
the container when the image provides it (falling back to `sh` otherwise).
nf-core-derived images ship bash, and their scripts rely on bash features
(`set -o pipefail`, `[[ ]]`) that the container default `sh` (often dash)
rejects — the re-exec shim keeps these scripts working unmodified.

**Singularity spec shapes:** a `singularity` spec is either a **pull URI**
(`docker://`, `library://`, `oras://`, `https://`) — the engine pulls and
converts it to a SIF in the workdir on first use, reusing the SIF when it
exists — or a **local SIF path** (e.g. a site image store's
`/data/images/bwa_0.7.17.sif`), which is used as-is with no pull step. A
local path that does not exist fails the rule with a clear diagnostic.

# # Mamba / micromamba (auto-detects binary, same YAML format as conda)
# environment = { mamba = "envs/qc.yaml", mamba_prefix = ".oxo-flow/envs" }

# # venv with custom requirements file
# environment = { venv = ".venv/", venv_requirements = "envs/dev-requirements.txt" }

# # Reference a named environment group (defined in [env_groups])
# env_group = "qc_env"
shell = "tool {input} -o {output}"
```

#### Named Environment Groups (`[env_groups]`)

Instead of repeating the same environment spec across multiple rules, define
named groups once in `[env_groups]` and reference them via `env_group`:

```toml
[env_groups.qc_env]
conda = "envs/qc.yaml"

[env_groups.align_env]
conda = "envs/alignment.yaml"

[[rules]]
name = "fastqc"
env_group = "qc_env"
input = ["raw/{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
shell = "fastqc {input} -o qc/"
```

Rules using `env_group` inherit the full environment specification from the
named group. The `env_group` spec takes precedence over an inline
`[rules.environment]`; the inline spec is only used when no `env_group` is set
(or the named group does not exist), followed by `[defaults] environment`.

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

Hook commands support the same `{config.x}` / `{input}` / `{output}` placeholder expansion as `shell`.

**Output validation:** After a rule's shell command exits successfully (exit code 0),
oxo-flow verifies that all declared `output` files exist on disk. If any declared output
is missing — for example, because a multi-step shell script's cleanup step masked an
earlier tool failure — the rule is treated as **failed** (with exit code sentinel `-1`)
and the `on_failure` hook runs instead of `on_success`. This prevents silently broken
rules from being checkpointed as "completed" and skipped on resume.

---

### Resources (extended)

For rules needing GPU, disk, or time limits, use the `resources` sub-table:

```toml
[[rules]]
name = "gpu_task"
input = ["data.h5"]
output = ["model.pt"]
shell = "python train.py"

[rules.resources]
threads = 8
memory = "64G"
gpu = 1
disk = "200G"
time_limit = "48h"
```

Declared `disk` requirements are checked against the working directory's
free space before a run starts — a shortfall prints a warning (the run
proceeds; the warning exists so long jobs don't fail mid-pipeline).

| Field | Type | Example | Description |
|---|---|---|---|
| `threads` | Integer | `8` | Number of CPU threads |
| `memory` | String | `"16G"` | Memory allocation |
| `gpu` | Integer | `1` | Number of GPUs |
| `disk` | String | `"200G"` | Local disk space |
| `time_limit` | String | `"48h"` | Wall-time limit |
| `partition` | String | `"gpu"` | HPC partition/queue to submit to |
| `groups` | Table | `{db_conn = 1}` | Resource group consumption tracking |

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
| Failure + declared `output` | Partial outputs created by the failed attempt are deleted; pre-existing files the attempt modified are moved aside as `<name>.oxo-failed`; untouched pre-existing files are preserved |
| Transform with `cleanup=true` | Chunk files cleaned after the whole run finishes successfully (kept on failed runs for debugging; re-runs recompute the map rules) |
| Success + `temporary = true` | Outputs deleted after the run once every dependent rule has completed; a tombstone is recorded in the checkpoint so a later run that needs the outputs regenerates the rule first (lazy cascade-up). Leaf rules (no dependents) keep their outputs. |

`temporary` is for whole intermediates (e.g. multi-GB per-sample BAMs kept
only until the queue-level callers finish): the deletion is checkpoint-aware,
so a plain re-run skips the rule and does NOT regenerate the file — it comes
back only when a dependent actually needs it again.

**Failed-rule output invalidation.** A rule that fails mid-write must never
leave partial outputs that the freshness gate would mistake for a completed
run. Before executing, the engine snapshots each declared output's existence
and modification time; when the rule fails (non-zero exit, timeout, or
missing outputs after an exit-0 shell), only what THIS attempt produced is
invalidated: files created during the attempt are deleted, pre-existing
files the attempt modified are moved aside as `<name>.oxo-failed` (the
failed content stays recoverable), and untouched pre-existing files are
left alone — a failure that never reached a file never destroys user data.
The next run then re-executes the rule instead of skipping it as
up-to-date. Note this covers failures the engine observes; a hard kill
(`SIGKILL`) mid-rule can still leave outputs behind, since no cleanup code
runs — the `--rerun` flag is the escape hatch for that case.

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

`shell` commands run through `bash` (falling back to `sh` when bash is
unavailable), so bashisms — brace expansion, `[[ ]]` conditionals, process
substitution, `$(( ))` arithmetic — work in rule shells.

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
| `temporary` | Boolean | Delete the rule's outputs after a fully successful run once every dependent has completed (tombstone + lazy regeneration; leaf rules keep outputs) |

```toml
[[rules]]
name = "align"
output = ["aligned/{sample}.bam", "aligned/{sample}.bam.bai"]
temp_output = ["aligned/{sample}.tmp.bam"]  # Cleaned after downstream use
temporary = true                             # Delete aligned/*.bam once all callers finish
```

### Execution Control

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `depends_on` | Array | — | Explicit rule dependencies (not inferred from files) |
| `localrule` | Boolean | `false` | Always run locally, never submit to cluster |
| `shadow` | String | — | Atomic execution mode: `"minimal"`, `"shallow"`, `"full"` |
| `checkpoint` | Boolean | `false` | Enable checkpoint re-entry (requires `checkpoint_manifest`; the rule must not use `{sample}`/`{group}`) |

**DAG edge inference** — edges form when a rule's `input` paths match
another rule's `output` paths, for every input form: template paths,
`expand_inputs` patterns (both the expanded concrete paths at run time
and the raw patterns in the template-level graph — multiqc-style
aggregators therefore connect to their contributors even in
`graph -f dot` without `--expanded`), glob patterns
(`raw/*.fastq.gz`), and directory inputs (every producer writing under
that directory). Rules that declare the SAME output string (shared
bins/staging directories — e.g. two assembler variants emitting into one
`GenomeBinning/<tool>/bins` dir) link ALL of their producers: every
consumer of the shared path is ordered after every writer.
`depends_on` remains available for ordering that files cannot express,
and duplicate edges are deduplicated.

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
tags = ["alignment", "production"]

[rules.resources]
threads = 8
memory = "32G"

[[rules]]
name = "align_fast"
extends = "align_default"  # Inherits threads, memory, tags

[rules.resources]
threads = 16  # Override inherited value
```

### Priority and Targeting

| Field | Type | Description |
|-------|------|-------------|
| `priority` | Integer | Execution priority (higher runs first; default: 0) |
| `target` | Boolean | Mark as default target — built when no explicit `-t` given |

```toml
[[rules]]
name = "critical_step"
priority = 10   # Runs ahead of lower-priority rules
target = true   # Included when running without -t
```

### Optional and Required Rules

| Field | Type | Description |
|-------|------|-------------|
| `optional` | Boolean | If `true`, missing inputs become warnings instead of errors |
| `required` | Boolean | If `false`, a failure of this rule does not fail the run: the failure is recorded and listed, its dependents are blocked, and the run exits 0 (best-effort / continue-on-error, e.g. QC steps). Defaults to `true` — required failures fail the run (or, with `--keep-going`, keep it running but still exit non-zero with the failures listed: keep-going changes scheduling, never the verdict) |

```toml
[[rules]]
name = "experimental"
optional = true     # Skip if input data is absent
required = false    # Failure is reported but does not fail the run
```

### Logging and Benchmarking

| Field | Type | Description |
|-------|------|-------------|
| `log` | String | File path for capturing rule stdout/stderr |
| `benchmark` | String | File path for performance metrics (wall-time, memory, CPU) |

```toml
[[rules]]
name = "align"
log = "logs/align_{sample}.log"
benchmark = "benchmarks/align_{sample}.tsv"
```

### Job Grouping and Caching

| Field | Type | Description |
|-------|------|-------------|
| `group` | String | Job group label for cluster submission grouping |
| `cache_key` | String | Content-based cache key for reusing previous outputs |

```toml
[[rules]]
name = "variant_call"
group = "variant_calling"       # Submit as a group on cluster
cache_key = "vc_v2.0"           # Cache key for output reuse
```

### Dynamic Input Resolution

| Field | Type | Description |
|-------|------|-------------|
| `input_function` | String | Name of a dynamic input resolver function called at runtime |

### Arbitrary Metadata

| Field | Type | Description |
|-------|------|-------------|
| `rule_metadata` | Table | Domain-specific metadata (assay type, organism, protocol, etc.) |

```toml
[[rules]]
name = "wgs_align"
[rules.rule_metadata]
assay = "WGS"
organism = "Homo sapiens"
protocol = "Illumina_NovaSeq_6000"
```

### Scatter-Gather (Legacy)

The `scatter` field provides fan-out parallelism over a variable with optional
gather. For new workflows, prefer the unified
[`transform`](#transform-unified-scatter-gather-operator) operator.

| Field | Type | Description |
|-------|------|-------------|
| `scatter.variable` | String | Variable to scatter over (e.g., `"chr"`) |
| `scatter.values` | Array | Values to scatter across |
| `scatter.values_from` | String | Config variable reference for values |
| `scatter.gather` | String | Name of the gather rule |

```toml
[[rules]]
name = "per_chr"
scatter = { variable = "chr", values = ["chr1", "chr2", "chr3"] }
```

### Expand Inputs

The `expand_inputs` field generates additional input combinations via Cartesian
product expansion.

| Field | Type | Description |
|-------|------|-------------|
| `expand_inputs[].pattern` | String | Input pattern with variables |
| `expand_inputs[].variables` | Table | Variable name → config reference (TOML array or comma-separated string) |

```toml
[[rules]]
name = "multi_ref_align"
expand_inputs = [
  { pattern = "refs/{ref_genome}.fa", variables = { ref_genome = "config.ref_genomes" } }
]
```

Variables may also reference the `[config]` section — either a TOML array
or a string:

```toml
[[sample_groups]]
name = "cohort"
samples = ["NA12878", "NA12879", "NA12880"]

[[rules]]
name = "combine_gvcfs"
input = []
expand_inputs = [
  { pattern = "variants/{sample}.g.vcf.gz", variables = { sample = "config.samples_list" } }
]
```

**Config reference semantics:**

| Config value | Expands to |
|---|---|
| `["a", "b"]` (array) | `a`, `b` |
| `"a"` (string, no comma) | `a` |
| `"a,b,c"` (comma-joined string) | `a`, `b`, `c` (split on commas, trimmed) |
| `["a,b"]` (single-element array) | `a,b` as one value — escape hatch for comma-containing strings |

Inline literals (a plain string written directly in `variables`, e.g.
`sample = "S1,S2"`) are always treated as one value and are never split —
the comma-splitting applies only to `config.*` references.

Comma-joined strings are how the engine injects merged sample lists
(`config.samples_list`, `config.samples_<group_name>` — see
[Merging Multiple Sample Sources](#merging-multiple-sample-sources)), so
they can be referenced directly here.

**The aggregation idiom** — multi-sample merge/collapse steps consume the
expanded inputs through `{input}`, which renders space-joined:

```toml
[[rules]]
name = "merge_bams"
expand_inputs = [
  { pattern = "aln/{sample}.bam", variables = { sample = "config.samples_list" } }
]
output = ["merged.bam"]
shell = "samtools merge {output[0]} {input}"   # aln/S1.bam aln/S2.bam ...
```

Shell loops over the same list work identically
(`for f in {input}; do ...; done` — the pattern used by the multiqc
aggregation rules across the community catalog). Note the deliberate
split: `config.samples_list` is **comma-joined** for expansion-time
fan-out, while `{input}` is **space-joined** for shell consumption. Do
not shell-loop over `{config.samples_list}` directly — a comma-joined
value iterates as a single word; use `tr ',' ' '` only if the expansion
form is genuinely unavailable. Chunk-level aggregation is the transform
operator's `combine` stage instead (gallery 10).

---

## Wildcards

Wildcards enable dynamic, pattern-based pipeline definitions. For a detailed guide on how they are discovered, expanded, and constrained, see the [Wildcards Reference](./wildcards.md).

### Basic Syntax

Use `{name}` in file paths for dynamic expansion:

```toml
input = ["raw/{sample}.fastq.gz"]
output = ["aligned/{sample}.bam"]
```

### Built-in Placeholders

Built-in placeholders use the same syntax but have reserved meanings:

| Placeholder | Expands to |
|---|---|
| `{input}` | Space-separated list of all input files |
| `{input[N]}` | The Nth input file (0-indexed) |
| `{input.name}` | The input file named `name` from `named_input` |
| `{output}` | Space-separated list of all output files |
| `{output[N]}` | The Nth output file (0-indexed) |
| `{output.name}` | The output file named `name` from `named_output` |
| `{threads}` | Thread count assigned to this rule |
| `{memory}` | Memory allocation assigned to this rule |
| `{effective_threads}` | Declared threads clamped to the machine's CPUs — the tool-facing concurrency. Use it for flags like `--threads`/`-t` when the declared value is an HPC-scale label (e.g. a rule declaring `threads = 12` renders `4` on a 4-core box). Unset threads render as `1` |
| `{effective_memory_mb}` | Declared memory clamped to the machine's total memory, as an integer number of MB — the tool-facing heap budget. Use it for JVM flags (`-Xmx{effective_memory_mb}m`, `-Xms…`) so tools size themselves to the box instead of OOM-ing on hardcoded HPC values (`-Xmx29491M` class). Unset memory renders as the machine's full total |
| `{log}` | Path of the rule's `log` field (every instance wildcard — `{sample}`, `{pair_id}`, `{assembler}` … — and `{config.x}` are expanded per instance; the parent directory is created automatically) |
| `{config.*}` | Value from the `[config]` section (plain value, declared default, or CLI override) |

**Why the effective pair exists** — `{threads}`/`{memory}` render the
*declared* values, which keep the pool semantics (an over-capacity rule
reserves the whole pool and runs alone). The `{effective_*}` pair renders
what the tool can *actually* get on this machine. Rules that embed
resource-sized flags should use the effective pair; rules that only pass
the pool declaration through can keep the plain pair.

**Container memory follows the same policy** — docker/singularity rules
get the declared memory as their `--memory` cgroup limit, clamped to the
machine's total the same way (a rule declaring `memory = "72G"` on a 4G
box runs with `--memory 4G`, not a limit the kernel can never back).
cgroup-aware tools (picard, STAR, Cell Ranger) therefore see an honest
limit on every machine size.

**The ceiling counts swap** — the machine total above is RAM + swap
(swap is backable memory the kernel uses under pressure; ignoring it
left real capacity unused on small boxes). Pass `--max-memory` to pin
the ceiling to RAM only when latency matters more than headroom.

**Existing environments are verified before setup** — when the
environment cache is cold (e.g. after `--rerun` wipes the checkpoint)
but the conda environment already exists, oxo-flow verifies it in place
and marks it ready instead of re-running the setup, whose `conda env
update` fallback re-resolves every dependency over the network.

**Environment creation is serialized per environment, not per machine**
— concurrent processes creating the *same* env would corrupt each
other's transaction, so setup holds an exclusive lock keyed by the env
cache key (`~/.oxo-flow/locks/env-<sha256>.lock`); *different* envs are
independent by conda/pixi semantics and never contend. The wait is
bounded (2h by default, `OXO_ENV_LOCK_TIMEOUT_SECS` to tune): the
waiter logs every 60s and a timeout fails the rule with the lock path
in the diagnostic instead of hanging the run. The holder's pid is
written into the lock file for diagnosis. On shared accounts, one
user's slow solve only blocks creators of that same environment.

**Array-valued `{config.*}`** — a `[config]` key holding an array renders
as a space-joined list in the shell (`["a", "b"]` → `a b`), matching the
`{input}` convention; iterate with `for x in {config.tools}`.

**`{input}` vs `{input[N]}`** — both forms are equivalent when the array has a single entry (`{input}` joins all inputs with spaces; `{input[0]}` takes the first). Practical guidance:

- **Single input/output**: either form works — `{input}` and `{output}` are the simplest
- **Multiple inputs/outputs**: use `{input[0]}`, `{input[1]}` … to select specific files, or `{input}` to pass all of them at once
- The indexed form communicates *intent* ("this rule expects exactly this file"), so it remains useful even for single-element arrays in multi-step pipelines

### Named Input & Output

For complex rules with many files, use `named_input` and `named_output` to improve readability:

```toml
[[rules]]
name = "align"

[rules.named_input]
reads1 = "raw/{sample}_R1.fastq.gz"
reads2 = "raw/{sample}_R2.fastq.gz"

[rules.named_output]
bam = "aligned/{sample}.bam"

shell = "bwa mem {input.reads1} {input.reads2} > {output.bam}"
```

### Custom Wildcards

Any `{name}` pattern not matching a built-in placeholder is treated as a wildcard. oxo-flow expands these based on:
1. **File discovery**: Scanning for matching files in the `input` path.
2. **Explicit lists**: Defined in [`[[pairs]]`](#pairs-experiment-control-pairing-wc-01) or [`[[sample_groups]]`](#sample_groups-multi-sample-cohorts-wc-02).
3. **Parameter lists**: Defined in [`[[values]]`](#values-parameter-fan-out-wc-03).

## `[[values]]` — Parameter Fan-out (WC-03)

`[[values]]` declares parameter lists that fan rules out the same way
sample groups do — the oxo-flow equivalent of an upstream
"for each assembler × binner" construct, without hardcoding one rule
per combination:

```toml
[[values]]
name = "assembler"
values = ["spades", "megahit"]

[[rules]]
name = "assemble"
input = ["reads/{sample}.fq"]
output = ["assemblies/{assembler}/{sample}/contigs.fa"]
shell = "{assembler} -o {output} {input}"
```

- Every rule referencing `{assembler}` (bare form) or `{values.assembler}`
  (namespaced form) expands into one instance per value.
- Multiple `[[values]]` tables form a cartesian product with each other
  and with sample groups / pairs (deterministic order: the first table
  varies slowest).
- Instance names follow the existing convention:
  `assemble_assembler_spades_batch_S1`.
- Value groups must have unique names and must not collide with the
  reserved wildcards (`sample`, `pair_id`, `experiment`, `control`,
  `group`, `tumor`, `normal`, `experiment_type`, `tumor_type`) or the
  executor placeholders (`input`, `output`, `log`, `threads`, `memory`) —
  a table named `input` would replace the placeholder in every rule's
  shell.

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
| `experiment` | String | **Yes** | Experiment sample name (alias: `tumor`) |
| `control` | String | **Yes** | Matched control sample name (alias: `normal`) |
| `experiment_type` | String | No | Optional cohort label (alias: `tumor_type`) |
| `metadata` | Table | No | Arbitrary key-value pairs (each key becomes a wildcard) |

Any rule that references `{experiment}`, `{control}`, or `{pair_id}` in its `input`, `output`, or `shell` fields is **automatically expanded** into one concrete rule instance per pair.  Rules that do not reference any pair wildcard are kept as-is.

**Expanded rule naming:** `{rule_name}_{pair_id}` (e.g., `mutect2_CASE_001`).

### Loading pairs from external file

For large cohort studies with hundreds or thousands of pairs, use `pairs_file` in `[workflow]`:

```toml
[workflow]
name = "somatic-calling"
pairs_file = "metadata/pairs.tsv"  # or .csv, .json
```

**TSV format** (tab-separated, header required):

```text
pair_id    experiment    control    experiment_type
CASE_001   EXP_01        CTRL_01    lung_adenocarcinoma
CASE_002   EXP_02        CTRL_02    colorectal
CASE_003   EXP_03        CTRL_03    breast_cancer
```

**CSV format** (comma-separated):

```text
pair_id,experiment,control,experiment_type
CASE_001,EXP_01,CTRL_01,lung_adenocarcinoma
CASE_002,EXP_02,CTRL_02,colorectal
```

**JSON format**:

```json
[
  {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
  {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
]
```

Inline `[[pairs]]` and `pairs_file` can be used together; entries from both sources are merged.

### Auto-discovery from file pattern

For workflows with existing paired files, use `pairs_pattern` in `[workflow]` to auto-discover pairs by scanning the filesystem:

```toml
[workflow]
name = "somatic-calling"
pairs_pattern = "aligned/{pair_id}/{experiment}_vs_{control}.bam"
```

oxo-flow scans files matching this pattern and extracts wildcards from paths. For a file:

```text
aligned/CASE_001/EXP_01_vs_CTRL_01.bam
```

Creates pair:

- `pair_id = CASE_001`
- `experiment = EXP_01`
- `control = CTRL_01`

**Pattern requirements:**
- Must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards
- Optional `{experiment_type}` wildcard also extracted
- Pattern is converted to glob (`*`) for filesystem scan

This eliminates the need for manual pair lists or external files when working with pre-organized directory structures.

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

See [`examples/gallery/15_paired_experiment_control_pairs.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/gallery/15_paired_experiment_control_pairs.oxoflow) for a full clinical somatic calling pipeline.

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

### Loading groups from external file

For large cohorts, use `sample_groups_file` in `[workflow]`:

```toml
[workflow]
name = "cohort-analysis"
sample_groups_file = "metadata/groups.tsv"  # or .csv, .json
```

**TSV format** (samples can be comma-separated within the field):

```text
name       samples
control    CTRL_001,CTRL_002,CTRL_003
case       CASE_001,CASE_002,CASE_003
treatment  TX_001,TX_002
```

**JSON format**:

```json
[
  {"name": "control", "samples": ["CTRL_001", "CTRL_002"]},
  {"name": "case", "samples": ["CASE_001", "CASE_002"]}
]
```

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

See [`examples/gallery/12_cohort_analysis.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/gallery/12_cohort_analysis.oxoflow) for a complete cohort study pipeline.

### Auto-Discovery with `sample_pattern`

The `[workflow]` `sample_pattern` auto-discovers samples from the filesystem:

```toml
[workflow]
# Exactly one pattern — uncomment the one that matches your data:
# Paired-end reads (Illumina standard)
sample_pattern = "raw/{sample}_R1.fastq.gz"

# # Paired-end reads (common variant)
# sample_pattern = "raw/{sample}_1.fq.gz"

# # Single-end reads
# sample_pattern = "raw/{sample}.fastq.gz"

# # With technical replicates
# sample_pattern = "raw/{sample}_rep{replicate}_R1.fastq.gz"
```

Supported wildcards in `sample_pattern`:
| Wildcard | Description | Example match |
|---|---|---|
| `{sample}` | Sample identifier | `SAMPLE_01` from `SAMPLE_01_R1.fastq.gz` |
| `{replicate}` | Technical replicate number | `1`, `2`, `3` |
| `{read}` | Read pair identifier | `1` or `2` (R1/R2) |

### Merging Multiple Sample Sources

All sample sources are merged into a single `{config.samples_list}`:

```bash
# Filesystem auto-discovery
sample_pattern = "raw/{sample}_R1.fastq.gz"

# CSV/TSV file
sample_groups_file = "metadata/samples.csv"

# Ad-hoc via CLI
oxo-flow run pipeline.oxoflow --sample EXTRA_01 --sample EXTRA_02
```

All sources deduplicate — the same sample from multiple sources appears once.
Per-group sample lists are available as `{config.samples_<group_name>}`.

These injected values are **comma-joined strings** (e.g. `"S001,S002"`):

- In `expand_inputs`, `scatter.values_from`, and `split.values_from` they
  resolve per value (comma-split) — see
  [Expand Inputs](#expand-inputs).
- In shell templates, `{config.samples_list}` renders as the comma-joined
  text, e.g. `for s in $(echo {config.samples_list} | tr ',' ' ')`.

### Partial Pair Tolerance & Tumor-Only Mode

Pairs with missing controls are supported — `control` is now optional:

```toml
# Tumor-only CNV calling (no matched normals)
[[pairs]]
pair_id = "T1"
experiment = "T1"
# control omitted → {control} = ""

# Pooled normal via config values
[[pairs]]
pair_id = "T2"
experiment = "T2"
# control = ""  # empty string also works
```

Rules using `{control}` receive an empty string when no control is specified.
Use shell conditionals or declarative `[config]` entries to handle this:

```toml
[[rules]]
name = "cnv_detect"
input = ["aligned/{experiment}.bam"]
shell = """
if [ -n "{control}" ]; then
    cnvkit.py batch {input} --normal aligned/{control}.bam
else
    cnvkit.py batch {input} --method cbs  # tumor-only mode
fi
"""
```

For pooled-normal scenarios, use declarative `[config]` entries:

```toml
[config]
normal_mode = { default = "pooled", help = "matched, pooled, or none" }
pooled_normal = { default = "results/pooled_normal.bam" }
```

Supported multi-omics pair patterns:

| Scenario | Pair Configuration | Control |
|---|---|---|
| Matched tumor-normal | `experiment = "T1", control = "N1"` | Required |
| Unmatched tumor vs pooled | `experiment = "T1"` | None |
| Tumor-only (CNV, somatic) | `experiment = "T1"` | None |
| Paired-end case-control | `experiment = "CASE", control = "CTRL"` | Required |
| Time-series (no control) | `experiment = "T0", experiment = "T6"` | None |

---

## `when` — Conditional Rule Execution (WF-01)

The optional `when` field on a rule contains an expression evaluated against `[config]` values. When the expression evaluates to **false** the rule is skipped at execution time (`JobStatus::Skipped`, "condition evaluated to false") — it remains in the DAG but does not run.

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

See [`examples/gallery/11_conditional_workflow.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/gallery/11_conditional_workflow.oxoflow) for a full example.

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
map = "gatk HaplotypeCaller -R {config.reference} -I {input} -L {chr} -O {output}"
cleanup = true

[rules.transform.combine]
shell = "gatk GatherVcfs $(for f in {chunks}; do echo \"-I $f \"; done) -O {output}"
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
| `{input}` | Chunk outputs — same as `{chunks}` (in combine) |
| `{output}` | Original rule output (in combine) |

### Modes

**Mode A: Split → Map → Combine**

Classic scatter-gather with explicit combine command:

```toml
[rules.transform.split]
by = "chr"
values_from = "config.chromosomes"

[rules.transform]
map = "gatk HaplotypeCaller -R {config.reference} -I {input} -L {chr} -O {output}"

[rules.transform.combine]
shell = "gatk GatherVcfs $(for f in {chunks}; do echo \"-I $f \"; done) -O {output}"
```

**Mode B: Split → Map (No Combine)**

Parallel processing without merging — each split produces independent output:

```toml
[rules.transform.split]
by = "chr"
values_from = "config.chromosomes"

[rules.transform]
map = "samtools flagstat {input} > {output}"
# No combine section
```

**Mode C: Split → Map → Aggregate**

Automatic aggregation (concat or json_merge):

```toml
[rules.transform.split]
by = "chunk"
n = "5"

[rules.transform]
map = "process {input} > {output}"

[rules.transform.combine]
aggregate = true
method = "concat"
```

### Cleanup

When `cleanup = true`, chunk files are automatically deleted after the whole run finishes successfully (empty chunk directories are removed too):

```toml
[rules.transform]
cleanup = true
```

Failed runs keep their chunks for debugging. Because chunk deletion makes the map outputs "missing", a re-run always recomputes the map rules — that is the disk-space trade-off of `cleanup = true`.

### Failure and Retry Logic

In a scatter-gather process, failures are handled at the chunk (map) level:

- If a single chunk fails, only that specific chunk is retried according to the rule's `retries` setting.
- Sibling chunks continue to process in parallel.
- The combine step will not execute until all chunks succeed. If any chunk fails exhaustively (after all retries), the combine step is cancelled.

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
author = "Shixiang Wang <w_shixiang@163.com>"

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
output = [
    "{config.results}/trimmed/{sample}_R1.fastq.gz",
    "{config.results}/trimmed/{sample}_R2.fastq.gz"
]
environment = { docker = "biocontainers/fastp:0.23.4" }
shell = "fastp --in1 {input[0]} --in2 {input[1]} --out1 {output[0]} --out2 {output[1]} --thread {threads}"

[[rules]]
name = "align"
input = [
    "{config.results}/trimmed/{sample}_R1.fastq.gz",
    "{config.results}/trimmed/{sample}_R2.fastq.gz"
]
output = ["{config.results}/aligned/{sample}.bam"]
environment = { conda = "envs/alignment.yaml" }
shell = "bwa mem -t {threads} -R '@RG\\tID:{sample}\\tSM:{sample}' {config.reference} {input[0]} {input[1]} | samtools sort -o {output}"

[rules.resources]
threads = 16
memory = "32G"
```

---

## JSON Schema

oxo-flow provides a comprehensive JSON Schema for the `.oxoflow` format. This can be used for automated validation in your CI/CD pipelines or for real-time autocompletion and error checking in your IDE (like VS Code or IntelliJ).

### Getting the Schema

You can output the schema directly from the CLI:

```bash
oxo-flow schema > oxoflow.schema.json
```

### IDE Configuration (VS Code)

To enable validation in VS Code, add the following to your `settings.json`:

```json
"yaml.schemas": {
    "https://traitome.github.io/oxo-flow/schema/oxoflow-v1.schema.json": "*.oxoflow"
}
```

(Note: Although `.oxoflow` is TOML, many VS Code extensions can apply JSON schemas to multiple formats).

---

## Checkpoint Re-entry

A `checkpoint = true` rule discovers new values at runtime (Snakemake-style
checkpoint re-entry): after it completes, the engine reads its
`checkpoint_manifest` — a TOML file the rule itself wrote — and re-expands
the workflow with the new values. Every round is still a static plan, so
previews stay deterministic and resumes reconstruct the same plan.

```toml
[[rules]]
name = "discover"
output = ["discover.toml"]
shell = "python discover.py > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"
```

The manifest declares new wildcard values — new samples and/or new
experiment-control pairs in the same round:

```toml
[reentry]
group = "batch"            # optional; defaults to the workflow's first sample group
sample = ["S4", "S5"]      # appended (dedup) to that group
pairs = [                  # optional; appended (dedup by pair_id) to [[pairs]]
  { pair_id = "CASE_007", tumor = "T7", normal = "N7", tumor_type = "tumor" },
]
```

Pair entries mirror `[[pairs]]` fields: `pair_id` (required), `experiment`
(required, alias `tumor`), `control` (alias `normal`), `experiment_type`
(alias `tumor_type`), and an optional `metadata` table. `tumor_type` and
`metadata` are optional.

Semantics:

- On success, the engine merges the samples and pairs, then re-expands the
  **rule templates**; newly created instances (e.g. `analyze_batch_S4`,
  `call_CASE_007`) execute in the same run. Existing instances are untouched.
- The checkpoint records each re-entry (`reentries` array: round, rule,
  group, samples, pairs). A resume **replays** the records whose checkpoint
  rule is still up-to-date — the plan reconstructs identically. If the
  checkpoint rule is invalidated (input/config change), its contribution is
  **revoked** (samples and pairs) until it re-runs and re-records.
- Pair identity is the `pair_id`: an existing id with identical content is a
  no-op; an existing id with different content is an error (E015) — silently
  superseding it would corrupt already-run pair outputs. The same sample
  appearing in several pairs is not an ambiguity: pair instances are keyed
  by `pair_id`, so each pair is its own instance.
- A discovered `pair_id` whose instance name collides with an existing
  instance (e.g. a sample-group instance `analyze_CASE_007`) is an error
  (E016).
- An empty manifest (`sample = []`, no `pairs`) is a valid no-op. A missing
  or unparsable manifest **fails the checkpoint rule** with a clear error,
  and its dependents do not run.
- Bounds: checkpoint rules are never re-expanded themselves (no
  `{sample}`/`{group}`/`{pair_id}`/`{experiment}` — validation error E014)
  and re-entry is capped at 32 rounds — a rule that keeps discovering values
  past that is a workflow bug, not an engine feature. Validation error E013
  requires `checkpoint_manifest` on every checkpoint rule.
- `dry-run` previews replay recorded re-entries (the preview shows the same
  static plan a run would execute) and mark checkpoint rules as possible
  re-entry points; `--json` includes a `reentry` section.

## See Also

- [Create a Workflow](../how-to/create-workflow.md) — practical authoring guide
- [DAG Engine](./dag-engine.md) — how dependencies are resolved
- [Environment System](./environment-system.md) — environment specification details
