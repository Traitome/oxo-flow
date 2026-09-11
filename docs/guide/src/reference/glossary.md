# Glossary

Key terms and concepts used in oxo-flow documentation.

---

## Core Concepts

### DAG (Directed Acyclic Graph)

A mathematical representation of a workflow where each node is a rule (task) and edges represent dependencies. "Acyclic" means there are no circular dependencies — a rule cannot depend on itself through a chain of other rules. oxo-flow automatically builds a DAG from input/output file relationships.

**Example**: If rule A produces `aligned.bam` and rule B needs `aligned.bam` as input, there's an edge A → B in the DAG.

---

### Rule

A single processing step in a workflow. Each rule defines:

- **Input**: Files needed to run the step
- **Output**: Files produced by the step
- **Shell/Script**: The command to execute
- **Environment**: The software environment to use

**Example**: A `bwa_align` rule takes FASTQ files as input and produces a BAM file as output.

---

### Workflow

The complete pipeline definition in a `.oxoflow` TOML file. A workflow contains:

- Metadata (name, version, author)
- Configuration variables
- Default settings
- One or more rules

---

### Wildcard

A placeholder pattern like `{sample}` that expands to concrete values based on input files or explicit configuration. Wildcards enable writing one rule template that processes many samples.

**Example**: `input = ["raw/{sample}.fastq.gz"]` expands to `raw/S001.fastq.gz`, `raw/S002.fastq.gz`, etc.

---

## Execution

### Topological Sort

An ordering of DAG nodes where every node appears before any nodes that depend on it. This determines the execution order — dependencies must run before the rules that need them.

**Example**: If A → B → C, execution order is A, B, C.

---

### Parallel Groups

Sets of rules that can run simultaneously because they have no dependencies on each other. oxo-flow's `-j` flag controls how many rules from a parallel group can run at once.

**Example**: `fastqc` and `fastp_trim` can run in parallel if neither depends on the other.

---

### Checkpoint

A persistent record of which rules have completed. Checkpoints enable resuming workflows after failures without re-running successful rules.

Besides rule status, a checkpoint stores config snapshots, rule fingerprints, and input file manifests — the engine compares them on every run to invalidate rules whose config, definition, or input file set changed. It also records **tombstones**: rules marked `temporary = true` whose outputs were deleted after a successful run, so later runs skip them unless a dependent needs the outputs again.

**File location**: `.oxo-flow/checkpoint.json`

---

## Environments

### Environment Backend

A system for managing software dependencies. oxo-flow supports:

| Backend | Use Case |
|---------|----------|
| **conda** | Bioinformatics tools with complex dependencies |
| **mamba** | Faster, C++ reimplementation of conda with parallel dependency solving |
| **pixi** | Fast, modern alternative to conda |
| **docker** | Full container isolation |
| **singularity** | HPC-friendly containers |
| **venv** | Python-only tools |
| **system** | Default system shell — no isolation (used when no backend is declared) |
| **modules** | HPC environment modules (Lmod/Environment Modules) |

---

### Environment Spec

A declaration in a rule that specifies which environment to use.

**Example**: `environment = { conda = "envs/bwa.yaml" }`

---

### Environment YAML

A conda environment definition file listing packages and versions.

**Example**:
```yaml
name: bwa_env
channels:
  - bioconda
  - conda-forge
dependencies:
  - bwa-mem2=2.2.1
  - samtools=1.20
```

---

## Workflow Format

### TOML (Tom's Obvious Minimal Language)

A configuration file format used for `.oxoflow` files. Key features:

- **Tables**: `[name]` defines a section
- **Arrays of Tables**: `[[name]]` defines multiple items (used for rules)
- **Key-Value**: `key = "value"`

See [TOML Specification](https://toml.io/) for full syntax.

---

### .oxoflow

The file extension for oxo-flow workflow definitions. These files are TOML format and define the complete pipeline.

---

### `[defaults]`

A section that sets default values (threads, memory, environment) for all rules unless overridden.

---

### `[[pairs]]`

A section defining experiment-control sample pairs for comparative analyses like tumor-normal variant calling.

**Fields**:
- `pair_id`: Unique identifier for the pair
- `experiment`: Sample name (e.g., tumor)
- `control`: Matched control sample (e.g., normal)

---

### `[[sample_groups]]`

A section organizing samples into named groups for cohort studies.

**Fields**:
- `name`: Group name (e.g., "treatment", "control")
- `samples`: Array of sample identifiers

---

### `transform`

An operator that unifies scatter (split) → map (process) → gather (combine) patterns in a single rule declaration.

**Example**: Split by chromosome, call variants per chromosome, then combine VCFs.

---

## Placeholders

### `{input}`

Expands to all input files, space-separated.

**Example**: `{input}` → `sample_R1.fastq.gz sample_R2.fastq.gz`

---

### `{input[N]}`

The Nth input file (0-indexed).

**Example**: `{input[0]}` → `sample_R1.fastq.gz`

---

### `{input.name}`

Named input file — `input` given as an inline map (`input = { reads = "..." }`).

**Example**: `{input.reads}` → the value of the `reads` entry in the rule's `input` map

---

### `{output}`

Expands to all output files, space-separated.

---

### `{threads}`

The thread count for the current rule.

---

### `{config.key}`

A configuration variable from the `[config]` section.

**Example**: `{config.reference}` → `/data/ref/hg38.fa`

---

## Output

### Report

A structured output document (HTML, JSON, PDF) summarizing workflow execution, QC metrics, and results. Clinical reports include variant classifications and audit trails.

---

### Provenance

A record of how each output was produced: which inputs, which tool, which version, which parameters. Essential for reproducibility.

---

### Benchmark

Timing and resource usage data for each rule (wall time, memory, CPU).

---

## Cluster/HPC

### Backend

A cluster scheduler system. oxo-flow supports:

| Backend | Scheduler |
|---------|-----------|
| `slurm` | SLURM (SLURM Workload Manager) |
| `pbs` | PBS/Torque |
| `sge` | Sun Grid Engine / Oracle Grid Engine |
| `lsf` | IBM Spectrum LSF |

---

### Partition/Queue

A group of cluster nodes with specific characteristics. Jobs are submitted to a partition/queue.

**Example**: `-q compute` submits to the "compute" partition.

---

### Wall-time

The maximum time a job can run before being terminated by the scheduler.

**Example**: `time_limit = "24h"` → 24-hour wall-time limit.

---

### Executor backend

A pluggable execution adapter ([`ExecutorBackend`](./execution-backends.md))
that maps the engine's static plan onto a scheduler API — submit, poll,
cancel, logs. The first implementation targets SLURM/PBS/SGE/LSF; the plan
itself stays executor-agnostic, so the dry-run preview is valid for every
backend.

### Static plan

The fully determined execution plan computed before any execution:
topological order, parallel groups, per-rule resource declarations, and the
invalidation set. Executors consume the plan without re-deriving it.

### Checkpoint re-entry

A bounded dynamic-DAG mechanism ([Workflow Format](./workflow-format.md#checkpoint-re-entry)):
a `checkpoint = true` rule writes a manifest at runtime declaring new samples;
the engine merges them, re-expands the rule templates, and executes the new
instances in the same run. Every round is still a static plan; the checkpoint
records the rounds so resumes replay (and revoke) them deterministically.

### Remote manifest entry

The content identity of an `s3://` / `gs://` input recorded in a rule's
input manifest: `(scheme, key, size, etag)` — S3 ETag or GCS `md5Hash`.
Same-size remote rewrites invalidate the rule exactly as local content
changes do ([Cloud Storage](./cloud-storage.md#content-addressed-invalidation)).

---

## See Also

- [Workflow Format Reference](./workflow-format.md) — complete TOML specification
- [DAG Engine](./dag-engine.md) — how dependencies are resolved
- [Wildcards Reference](./wildcards.md) — pattern expansion details
---

## Diagnostic Codes

Every static check in oxo-flow tags its findings with a stable code so
tools (and you) can branch on them programmatically:

- **`Exx`** — an **error**: the workflow is rejected (`validate` fails) or
  flagged as not runnable. Example: `E017` — an unknown key in a rule, the
  engine's way of saying "this TOML uses a dialect the schema does not
  have".
- **`Wxx`** — a **warning**: the workflow runs, but the check found a
  likely mistake worth fixing (see `oxo-flow lint`).

Codes are three characters: the letter, then a zero-padded number. The
authoritative, machine-readable source is the `--json` output of each
command (`validate --json`, `dry-run --json`, `lint --json`); the tables
below cover the codes referenced across this documentation.

### Validation errors (`validate` / `dry-run`)

| Code | Meaning |
|---|---|
| `E001` | Workflow name is empty |
| `E003` | A wildcard appears in a rule's output but not in its input — the engine cannot key instances |
| `E005` | A `{config.key}` reference points at a key absent from `[config]` |
| `E006` | DAG construction error (e.g. a cycle) |
| `E007` | `depends_on` names a rule that does not exist |
| `E008` | `extends` names a rule that does not exist |
| `E009` | An input path contains `..` and may escape the working directory |
| `E010` | A rule references an undefined `env_group` |
| `E011` | A rule command matches a dangerous shell pattern (destructive command class) |
| `E013` | A `checkpoint = true` rule lacks `checkpoint_manifest` |
| `E014` | A checkpoint rule is parameterized by `{sample}`/`{group}`/`{pair_id}` (not allowed) |
| `E015` | Re-entry declares a `pair_id` with conflicting content |
| `E016` | An environment field contains a shell-unsafe character — the spec would not render as safe argv |
| `E017` | Unknown key in a rule — usually a foreign workflow dialect (e.g. `foreach`, `inputs = {...}`) |

### Lint warnings (`lint`)

| Code | Meaning |
|---|---|
| `W001`/`W002` | Workflow missing `description` / `author` |
| `W003` | Rule missing a description |
| `W004` | Rule has a shell command but no `log` file specified |
| `W005` | Rule uses >8 threads with no `memory` |
| `W007` | Rule declares no environment — runs in the bare system shell |
| `W011` | Rule has `retries` but no `retry_delay` |
| `W014` | `depends_on` references an unknown rule |
| `W016` | Conda/pixi spec is not a lockfile — builds may not be reproducible |
| `W017` | Input path is absolute |
| `W018` | Input path references a home directory |
| `W019` | Rule executes a command but declares no outputs |
| `W020`/`W021`/`W022` | `pre_exec`/`on_success`/`on_failure` contain a risky pattern |
| `W024` | A wildcard has no declared source — it will stay literal at run time |
| `W025` | Legacy `threads =`/`memory =` keys under the rule body (use `resources.`) |
| `W027` | A `when` condition references a wildcard nothing can bind — evaluates false |
| `W028` | Docker image is not fully qualified (resolves against docker.io) |
| `W029` | `len(config.x)` compared against a non-numeric value |
| `W030` | Malformed `regex_extract` in a `when` condition |
| `W031` | Producer is when-gated but its consumer expands the output unconditionally |
| `W032` | A config key looks like a secret but is not declared `sensitive` |

The full, current list with suggestions is best read from the commands:
`oxo-flow lint --json` prints every code with its message and suggestion
(see [lint](../commands/lint.md)). AI-generated drafts that fail a gate
have their errors fed back to the model for correction
([AI CLI](../commands/ai.md)); deterministic repairs for purely mechanical
classes run first, before any model round is spent.

### Benchmark terms

- **pass@1 / pass@k** — the probability that a single generation (or at
  least one of k attempts) passes all gates. Used in
  [AI evaluation](./ai-eval.md) and `eval/frontier/`.
- **fidelity** — a deterministic check that the tools an intent names
  actually appear in the generated pipeline. Gates alone cannot see a
  "wrong but valid" pipeline; the frontier benchmark reports
  **success = gates ∧ fidelity**.
- **seed** — one repeat of the same intent under identical settings;
  multiple seeds measure sampling variance instead of quoting a single
  lucky/unlucky run.
