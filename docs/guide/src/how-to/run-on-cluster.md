# Run on a Cluster

This guide explains how to execute oxo-flow workflows on HPC clusters using SLURM, PBS, SGE, and LSF backends.

---

## Overview

oxo-flow's cluster module translates each rule into a cluster job submission. Resource requirements declared in the `.oxoflow` file (`threads`, `memory`, `gpu`, `disk`, `time_limit`) are mapped to the appropriate scheduler directives.

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
threads = 16
memory = "32G"

[rules.resources]
gpu = 0
disk = "100G"
time_limit = "24h"

environment = { singularity = "docker://biocontainers/bwa:0.7.17" }
shell = "bwa mem -t {threads} ref.fa {input} | samtools sort -o {output}"
```

### Resource fields

| Field | Type | Example | Description |
|---|---|---|---|
| `threads` | Integer | `16` | Number of CPU cores |
| `memory` | String | `"32G"` | RAM allocation |
| `gpu` | Integer | `1` | Number of GPUs |
| `disk` | String | `"100G"` | Local disk space |
| `time_limit` | String | `"24h"` | Wall-time limit |

---

## SLURM Example

oxo-flow generates SLURM job scripts automatically. For the `align` rule above, the generated script looks like:

```bash
#!/bin/bash
#SBATCH --job-name=align
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --time=24:00:00
#SBATCH --output=logs/align_%j.out
#SBATCH --error=logs/align_%j.err

# Environment activation
singularity exec docker://biocontainers/bwa:0.7.17 \
  bwa mem -t 16 ref.fa sample1_R1.fastq.gz | samtools sort -o aligned/sample1.bam
```

---

## PBS Example

```bash
#!/bin/bash
#PBS -N align
#PBS -l ncpus=16
#PBS -l mem=32gb
#PBS -l walltime=24:00:00
#PBS -o logs/align.out
#PBS -e logs/align.err

cd $PBS_O_WORKDIR
bwa mem -t 16 ref.fa sample1_R1.fastq.gz | samtools sort -o aligned/sample1.bam
```

---

## SGE Example

```bash
#!/bin/bash
#$ -N align
#$ -pe smp 16
#$ -l h_vmem=2G
#$ -l h_rt=24:00:00
#$ -o logs/align.out
#$ -e logs/align.err
#$ -cwd

bwa mem -t 16 ref.fa sample1_R1.fastq.gz | samtools sort -o aligned/sample1.bam
```

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

---

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
