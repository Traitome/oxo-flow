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

**File location**: `.oxo-flow/checkpoint_<workflow-name>.json`

---

## Environments

### Environment Backend

A system for managing software dependencies. oxo-flow supports:

| Backend | Use Case |
|---------|----------|
| **conda** | Bioinformatics tools with complex dependencies |
| **pixi** | Fast, modern alternative to conda |
| **docker** | Full container isolation |
| **singularity** | HPC-friendly containers |
| **venv** | Python-only tools |

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

Named input file from `named_input` section.

**Example**: `{input.reads}` → the file named "reads" in `named_input`

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

A structured output document (HTML, JSON) summarizing workflow execution, QC metrics, and results. Clinical reports include variant classifications and audit trails.

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

## See Also

- [Workflow Format Reference](./workflow-format.md) — complete TOML specification
- [DAG Engine](./dag-engine.md) — how dependencies are resolved
- [Wildcards Reference](./wildcards.md) — pattern expansion details