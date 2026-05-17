# `oxo-flow cluster`

Manage cluster job submission and monitoring.

---

## Usage

```
oxo-flow cluster <ACTION> [OPTIONS] [WORKFLOW/JOB_IDS]
```

---

## Actions

| Action | Description |
|---|---|
| `submit` | Submit a workflow to a cluster scheduler |
| `status` | Show the status of submitted cluster jobs |
| `cancel` | Cancel submitted cluster jobs |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file (for `submit`) |
| `<JOB_ID>...` | One or more cluster job IDs (for `cancel`) |

---

## Options (Submit)

| Option | Short | Default | Description |
|---|---|---|---|
| `--backend` | `-b` | `slurm` | Cluster backend (`slurm`, `pbs`, `sge`, `lsf`) |
| `--queue` | `-q` | — | Partition / queue name |
| `--account` | `-a` | — | Account / project name |
| `--output-dir` | `-o` | `.oxo-flow/cluster` | Directory for generated scripts |
| `--pending-timeout` | — | — | Maximum time to wait for pending jobs (e.g., "30m", "1h") |

---

## Examples

### Submit to SLURM

```bash
oxo-flow cluster submit pipeline.oxoflow -b slurm -q work
```

### Submit to PBS/Torque

```bash
oxo-flow cluster submit pipeline.oxoflow -b pbs -q batch
```

### Submit to SGE (Sun Grid Engine)

```bash
oxo-flow cluster submit pipeline.oxoflow -b sge -q all.q
```

### Submit to LSF

```bash
oxo-flow cluster submit pipeline.oxoflow -b lsf -q normal
```

### Submit with pending timeout

```bash
# Abort submission if jobs stay in PENDING state for more than 1 hour
# Useful when cluster resources may be unavailable
oxo-flow cluster submit pipeline.oxoflow -b slurm -q work --pending-timeout 1h
```

### Submit with environment support

```bash
# If your workflow uses conda environments, the generated scripts
# will automatically include conda activation commands
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute
```

### Check job status

```bash
oxo-flow cluster status -b slurm
```

### Cancel specific jobs

```bash
oxo-flow cluster cancel -b slurm 12345 12346
```

---

## Output

```
oxo-flow 0.5.1 — Bioinformatics Pipeline Engine
Cluster: Generating slurm job scripts for 5 rules
  ✓ .oxo-flow/cluster/fastqc.sh
  ✓ .oxo-flow/cluster/trim_reads.sh
  ✓ .oxo-flow/cluster/bwa_align.sh
  ✓ .oxo-flow/cluster/sort_bam.sh
  ✓ .oxo-flow/cluster/call_variants.sh

Done: 5 scripts written to .oxo-flow/cluster
  Submit with: sbatch .oxo-flow/cluster/*.sh
```

---

## Generated Script Example

For a workflow rule with conda environment, different backends produce different scripts:

### SLURM Script

```bash
#!/bin/bash
#SBATCH --job-name=bwa_align
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --time=24:00:00
#SBATCH --partition=compute
#SBATCH --output=logs/bwa_align.out
#SBATCH --error=logs/bwa_align.err

# Environment wrapping (automatically added)
conda activate bwa_env

bwa mem -t 16 ref.fa reads.fq > aligned.sam
```

### PBS/Torque Script

```bash
#!/bin/bash
#PBS -N bwa_align
#PBS -l nodes=1:ppn=16
#PBS -l mem=32gb
#PBS -l walltime=24:00:00
#PBS -q compute
#PBS -o logs/bwa_align.out
#PBS -e logs/bwa_align.err

# Environment wrapping (automatically added)
conda activate bwa_env

bwa mem -t 16 ref.fa reads.fq > aligned.sam
```

### SGE Script

```bash
#!/bin/bash
#$ -N bwa_align
#$ -pe threaded 16
#$ -l mem_free=32G
#$ -l h_rt=24:00:00
#$ -q all.q
#$ -o logs/bwa_align.out
#$ -e logs/bwa_align.err

# Environment wrapping (automatically added)
conda activate bwa_env

bwa mem -t 16 ref.fa reads.fq > aligned.sam
```

### LSF Script

```bash
#!/bin/bash
#BSUB -J bwa_align
#BSUB -n 16
#BSUB -R "rusage[mem=32768]"
#BSUB -W 24:00
#BSUB -q normal
#BSUB -o logs/bwa_align.out
#BSUB -e logs/bwa_align.err

# Environment wrapping (automatically added)
conda activate bwa_env

bwa mem -t 16 ref.fa reads.fq > aligned.sam
```

---

## Notes

- `submit` generates shell scripts tailored for the specified cluster backend
- Resource requirements (threads, memory) from the workflow are automatically translated to cluster directives
- **Environment wrapping is applied automatically** — conda, docker, singularity, pixi, and venv environments are properly wrapped in the generated scripts
- `status` and `cancel` actively execute native cluster commands (like `squeue`, `scancel`) and print their outputs directly
- Ensure the required environments (conda envs, docker images, etc.) are available on cluster nodes before submitting
