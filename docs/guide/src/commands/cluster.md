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
| `logs` | Fetch logs for a submitted cluster job |

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
| `--backend` | `-b` | *(required)* | Cluster backend (`slurm`, `pbs`, `sge`, `lsf`) |
| `--queue` | `-q` | — | Partition / queue name |
| `--account` | `-a` | — | Account / project name |
| `--output` | `-o` | `cluster_scripts` | Directory for generated scripts |
| `--target` | `-t` | — | Target rule(s) to execute |
| `--with-dependencies` | — | — | Generate dependency-aware submit script with job chains |
| `--dry-run` | `-n` | — | Preview scripts without generating files |

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
# Submit with queue and account
oxo-flow cluster submit pipeline.oxoflow -b slurm -q work -a lab-account
```

### Submit with environment support

```bash
# If your workflow uses conda environments, the generated scripts
# will automatically include conda activation commands
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute
```

### Submit with job dependencies

```bash
# Generate scripts with automatic dependency chain setup
# Creates a submit.sh wrapper script that handles job submission order
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute --with-dependencies

# Submit the generated wrapper script
bash cluster_scripts/submit.sh
```

### Submit specific target rules

```bash
# Only generate scripts for specific rules and their dependencies
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute -t align -t call_variants
```

### Dry run mode

```bash
# Preview what would be generated without creating files
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute --dry-run
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

### Basic Output

```
oxo-flow 0.7.0 — Bioinformatics Pipeline Engine
Cluster: Generating slurm job scripts for 5 rules
  ✓ cluster_scripts/fastqc.sh
  ✓ cluster_scripts/trim_reads.sh
  ✓ cluster_scripts/bwa_align.sh
  ✓ cluster_scripts/sort_bam.sh
  ✓ cluster_scripts/call_variants.sh

Done: 5 scripts written to cluster_scripts
  Submit with: sbatch cluster_scripts/*.sh
```

### With Dependencies Output

```
oxo-flow 0.7.0 — Bioinformatics Pipeline Engine
Cluster: Generating slurm job scripts for 5 rules
  ✓ cluster_scripts/fastqc.sh
  ✓ cluster_scripts/trim_reads.sh
  ✓ cluster_scripts/bwa_align.sh
  ✓ cluster_scripts/sort_bam.sh
  ✓ cluster_scripts/call_variants.sh
  ✓ cluster_scripts/submit.sh (dependency-aware submit script)

Done: 6 scripts written to cluster_scripts
  Submit with: bash cluster_scripts/submit.sh
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

### Dependency-Aware Submit Script

When using `--with-dependencies`, oxo-flow generates a `submit.sh` wrapper that handles job submission order:

```bash
#!/bin/bash
# Auto-generated dependency-aware submit script
# Generated by oxo-flow

set -e

# Track job IDs
declare -A JOB_IDS

echo 'Submitting fastqc...'
JOB_IDS[fastqc]=$(sbatch cluster_scripts/fastqc.sh)
echo '  Submitted fastqc as job ID: ${JOB_IDS[fastqc]}'

echo 'Submitting trim_reads...'
JOB_IDS[trim_reads]=$(sbatch --dependency=afterok:${JOB_IDS[fastqc]} cluster_scripts/trim_reads.sh)
echo '  Submitted trim_reads as job ID: ${JOB_IDS[trim_reads]}'

echo 'Submitting bwa_align...'
JOB_IDS[bwa_align]=$(sbatch --dependency=afterok:${JOB_IDS[trim_reads]} cluster_scripts/bwa_align.sh)
echo '  Submitted bwa_align as job ID: ${JOB_IDS[bwa_align]}'

echo 'All jobs submitted successfully!'
echo 'Job ID mapping:'
for name in "${!JOB_IDS[@]}"; do
  echo "  $name: ${JOB_IDS[$name]}"
done
```

Different backends use different dependency syntax:

| Backend | Dependency Flag |
|---------|-----------------|
| SLURM | `--dependency=afterok:jobid` |
| PBS | `-W depend=afterok:jobid` |
| SGE | `-hold_jid jobid` |
| LSF | `-w 'ended(jobid)'` |

---

## Notes

- `submit` generates shell scripts tailored for the specified cluster backend
- Resource requirements (threads, memory, gpu) from the workflow are automatically translated to cluster directives
- **Environment wrapping is applied automatically** — conda, docker, singularity, pixi, venv, and module environments are properly wrapped in the generated scripts
- `status` and `cancel` actively execute native cluster commands (like `squeue`, `scancel`) and print their outputs directly
- Ensure the required environments (conda envs, docker images, etc.) are available on cluster nodes before submitting
- Use `--with-dependencies` for workflows where rules depend on each other — this ensures proper execution order
