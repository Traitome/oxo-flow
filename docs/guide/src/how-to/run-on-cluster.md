# Run on a Cluster

This guide explains how to execute oxo-flow workflows on HPC clusters using SLURM, PBS, SGE, and LSF backends.

---

## Overview

oxo-flow's cluster module translates each rule into a cluster job submission. Resource requirements declared in the `.oxoflow` file (`threads`, `memory`, `gpu`, `time_limit`) are mapped to the appropriate scheduler directives. The `disk` field is **not** mapped to any scheduler directive — it only produces a local warning during `oxo-flow run`.

There are two ways to reach a scheduler, and they suit different jobs:

| | What it does | Use when |
|---|---|---|
| `run --profile <NAME>` | Submits, tracks jobs to completion, and updates the checkpoint | Normal execution — you want the workflow to run |
| `cluster submit` | Writes job scripts and stops | You want to inspect or hand-edit scripts before submitting |

`run --profile` is the everyday path; it inherits everything `run` does — wildcard expansion, checkpoint/resume, `--samples`, `--rerun`, config-change invalidation. See [Cluster submission](../commands/run.md#cluster-submission) for the `[cluster]` profile block. `cluster submit` remains the escape hatch and is documented below.

**Environment wrapping is applied automatically** — conda, mamba, docker, singularity, pixi, venv, and modules environments are wrapped in the generated scripts. Each rule applies **one** backend (the first declared in the resolver order), so declaring e.g. both `singularity` and `modules` silently drops `modules`.

---

## Supported Schedulers

| Scheduler | Status | Directive prefix |
|---|---|---|
| **SLURM** | Supported | `#SBATCH` |
| **PBS/Torque** | Supported | `#PBS` |
| **SGE** | Supported | `#$` |
| **LSF** | Supported | `#BSUB` |

---

## Declaring Resources

Set resource requirements per rule:

```toml
[[rules]]
name = "align"
input = ["{sample}_R1.fastq.gz"]
output = ["aligned/{sample}.bam"]
environment = { singularity = "docker://biocontainers/bwa:0.7.17" }
shell = "bwa mem -t {threads} ref.fa {input} | samtools sort -o {output}"

[rules.resources]
threads = 16
memory = "32G"
time_limit = "24h"
```

> **Note on `gpu`:** declare `gpu = 1` or higher. `gpu = 0` is treated as a
> present-but-zero value and generates directives like `#SBATCH --gres=gpu:0`
> — omit the field instead.

### Resource fields

| Field | Type | Example | Description |
|---|---|---|---|
| `threads` | Integer | `16` | Number of CPU cores |
| `memory` | String | `"32G"` | RAM allocation |
| `gpu` | Integer | `1` | Number of GPUs (simple count) |
| `gpu_spec` | Table | See below | Detailed GPU specification |
| `disk` | String | `"100G"` | Local disk space — **local warning only**, never emitted as a scheduler directive |
| `time_limit` | String | `"24h"` | Wall-time limit |

### GPU Specification

For basic GPU requests, use the `gpu` field:

```toml
[rules.resources]
gpu = 2  # Request 2 GPUs
```

For advanced GPU configuration, use `gpu_spec`:

```toml
[rules.resources.gpu_spec]
count = 2
model = "a100"       # GPU model (optional — SLURM only)
memory_gb = 40       # Per-GPU memory in GB (optional — SLURM only)
```

`count` works on SLURM, PBS, and SGE; `model` and `memory_gb` only affect
SLURM directives (LSF ignores `gpu_spec` entirely).

Different schedulers handle GPU requests differently:

| Scheduler | GPU Directive | Notes |
|-----------|---------------|-------|
| **SLURM** | `--gres=gpu:2` or `--gres=gpu:a100:2:40g` | Full support for model and memory spec |
| **PBS** | `gpu=2` | Basic count only; model selection varies by site |
| **SGE** | `-l gpu=2` | Basic count only; requires queue configuration |
| **LSF** | `-gpu 2` | Basic count only |

---

## SLURM Example

oxo-flow generates SLURM job scripts automatically. For the `align` rule
above with a sample group `batch = ["S1", "S2", "S3"]`, the script generated
for the first instance is:

```bash
#!/bin/bash
#SBATCH --job-name=align_batch_S1
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --time=1-00:00:00
#SBATCH --output=logs/align_batch_S1.out
#SBATCH --error=logs/align_batch_S1.err

set -e

mkdir -p logs
singularity exec --bind .:. docker://biocontainers/bwa:0.7.17 sh -c 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

Note: one script is generated per **rule instance**. Wildcards expand before
the scripts are written, exactly as they do for `run`, so a 3-sample scatter
produces `align_batch_S1.sh`, `align_batch_S2.sh`, and `align_batch_S3.sh`
with concrete paths in each. The instance names match the ones `dry-run`
plans, so a script maps back to a planned rule by name.

---

## PBS Example

```bash
#!/bin/bash
#PBS -N align_batch_S1
#PBS -l nodes=1:ppn=16,mem=32G,walltime=1-00:00:00
#PBS -o logs/align_batch_S1.out
#PBS -e logs/align_batch_S1.err

set -e

mkdir -p logs
singularity exec --bind .:. docker://biocontainers/bwa:0.7.17 sh -c 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

---

## SGE Example

```bash
#!/bin/bash
#$ -N align_batch_S1
#$ -pe smp 16
#$ -l h_vmem=32G
#$ -l h_rt=1-00:00:00
#$ -o logs/align_batch_S1.out
#$ -e logs/align_batch_S1.err

set -e

mkdir -p logs
singularity exec --bind .:. docker://biocontainers/bwa:0.7.17 sh -c 'bwa mem -t 16 ref.fa S1_R1.fastq.gz | samtools sort -o aligned/S1.bam'
```

---

## Environment Wrapping

When generating cluster scripts, oxo-flow automatically wraps commands through the environment resolver:

| Backend | Wrapping |
|---|---|---|
| Conda / Mamba | `conda run -n <env> bash -c '<command>'` |
| Docker | `docker run --rm --user $(id -u):$(id -g) -v .:. -w . <image> sh -c '<command>'` |
| Singularity | `singularity exec --bind .:. <image> sh -c '<command>'` |
| Pixi | `pixi run -e <env> <command>` |
| Venv | `source <venv>/bin/activate && <command>` |
| Modules | `module load <mod1> <mod2> && <command>` |

### Environment Examples

**Conda with GPU for deep learning:**

```toml
[[rules]]
name = "train_model"
input = ["data/train.h5"]
output = ["models/trained.pt"]
environment = { conda = "envs/pytorch.yaml" }
shell = "python train.py --input {input} --output {output}"

[rules.resources]
threads = 8
memory = "64G"
gpu = 2
time_limit = "24h"
```

The `gpu` field controls the **scheduler allocation only** (e.g.
`#SBATCH --gres=gpu:2`) — there is no `{resources.gpu}` shell placeholder.

**Singularity (common on HPC):**

```toml
[[rules]]
name = "variant_call"
input = ["aligned/{sample}.bam"]
output = ["variants/{sample}.vcf"]
environment = { singularity = "docker://broadinstitute/gatk:4.4.0.0" }
shell = "gatk HaplotypeCaller -I {input} -O {output}"

[rules.resources]
threads = 16
memory = "32G"
```

Only **one** backend applies per rule, so combining e.g. `singularity` with
`modules` in the same rule would silently drop `modules` — use the
module-based example below for module-only setups.

**Pixi for reproducible environments:**

```toml
[[rules]]
name = "qc_check"
input = ["{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
environment = { pixi = "default" }  # environment name, not the pixi.toml path
shell = "fastqc -t {threads} -o qc/ {input}"

[rules.resources]
threads = 4
```

**Pure Module-based (traditional HPC):**

```toml
[[rules]]
name = "align"
input = ["reads/{sample}.fq"]
output = ["aligned/{sample}.bam"]
environment = { modules = ["bwa/0.7.17", "samtools/1.17", "gcc/11"] }
shell = "bwa mem -t {threads} ref.fa {input} | samtools sort -o {output}"

[rules.resources]
threads = 32
memory = "64G"
```

!!! tip "Pre-build environments on cluster nodes"
    Ensure your conda environments, docker images, or singularity containers are available on all cluster nodes before submitting jobs. Use `--skip-env-setup` when environments are pre-built.

---

## Resource Enforcement

### Local Execution

When running locally (`oxo-flow run`), resource constraints are enforced:

- **Check**: Before execution, verify resources are available
- **Reserve**: Reserve resources before starting the job
- **Release**: Release resources after completion (or on failure/timeout)

```bash
# Limit to 16 threads and 32GB memory for local execution
oxo-flow run pipeline.oxoflow --max-threads 16 --max-memory 32768
```

### Cluster Execution

On clusters, the scheduler enforces resources based on the generated directives. oxo-flow does not manage resources during cluster execution — the scheduler handles that.

---

## Best Practices

!!! tip "Use Singularity on clusters"
    Most HPC clusters do not allow Docker. Use Singularity instead — oxo-flow handles the conversion automatically when you specify `singularity = "docker://..."`.

!!! tip "Set realistic time limits"
    Generous wall-time limits prevent premature job termination but may lower scheduling priority. Profile your jobs first.

!!! tip "Use `--keep-going` for large batches"
    When running hundreds of samples, use `oxo-flow run -k` so that a single failure does not abort the entire run.

!!! tip "Check resource availability"
    Use `sinfo` (SLURM), `pbsnodes` (PBS), or `qhost` (SGE) to verify available resources before submitting.

!!! tip "Cache environment setup"
    Use `--cache-dir` to persist environment setup state across runs for faster startup.

---

## Web UI: Remote Execution over SSH

The web system can execute runs on a cluster login node while the server
itself lives elsewhere (your laptop, a lab server, a container):

1. **Register the connection** — Clusters page (or the platform config
   file's `[clusters]` section): SSH host, port, user, optional key path,
   scheduler hint, and a `remote_dir` for staged runs. Probe verifies
   connectivity and detects the scheduler (slurm/pbs/sge/lsf) from the
   remote's installed binaries.
2. **Run with `cluster_id`** — the Run dialog's *Execute on cluster*
   selector (or `cluster_id` in `POST /api/runs`).
3. **What happens** — the workdir is staged to
   `{remote_dir}/runs/{run_id}` over a tar stream (no rsync needed); a
   per-run wrapper launches `oxo-flow run` remotely under nohup; the
   server polls the remote `.exit-code` every 5s; on completion the whole
   workdir (logs, checkpoint, results) is pulled back, so the web logs,
   files, preview, and report views work exactly as for local runs.
   Cancel sends `pkill` for the per-run wrapper.

Requirements on the remote host: the `oxo-flow` CLI on `PATH` (any recent
release), `tar`, and non-interactive SSH access (key auth, `BatchMode`).
Cluster connections are admin-managed in team mode (they hold shared SSH
credentials); every run remains owned by the acting user.

## Monitoring Jobs

After submission, use your cluster's native tools:

```bash
# SLURM
squeue -u $USER

# PBS
qstat -u $USER

# SGE
qstat

# LSF
bjobs
```

Or use oxo-flow's status command with a checkpoint file:

```bash
oxo-flow status .oxo-flow/checkpoint.json
```

---

## See Also

- [Architecture: Cluster backends](../reference/architecture.md) — internal cluster module design
- [Environment System](../reference/environment-system.md) — Singularity and Docker on HPC
- [`run` command](../commands/run.md) — `--max-threads`, `--max-memory`, `--skip-env-setup`, `--cache-dir`
- [`cluster` command](../commands/cluster.md) — cluster submission reference
