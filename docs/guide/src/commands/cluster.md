# `oxo-flow cluster`

Manage cluster job submission and monitoring.

> **Everyday path:** `run --profile <NAME>` submits to a scheduler and
> tracks jobs to completion when the profile carries a `[cluster]` block —
> see [Cluster submission](run.md#cluster-submission). The commands below
> remain the manual escape hatch: inspect scripts before submitting,
> or cancel/collect jobs after an interrupted run.

---

## Usage

```
oxo-flow cluster <ACTION> [OPTIONS]
```

---

## Actions

| Action | Description |
|---|---|
| `submit` | Submit a workflow to a cluster scheduler |
| `status` | Show the status of submitted cluster jobs |
| `cancel` | Cancel submitted cluster jobs |
| `logs` | Fetch the accounting record for a submitted cluster job |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file (for `submit`) |
| `[JOB_IDS]...` | Cluster job IDs from a submit/run output — required for `status` (an empty job list is rejected); optional for `cancel` |
| `<JOB_ID>` | Job ID (for `logs`) |

---

## Options (Submit)

| Option | Short | Default | Description |
|---|---|---|---|
| `--backend` | `-b` | *(required)* | Cluster backend (`slurm`, `pbs`, `sge`, `lsf`) |
| `--queue` | `-q` | — | Partition / queue name |
| `--account` | `-a` | — | Account / project name |
| `--walltime` | — | — | Wall-time limit for every job (`24h`, `2d`, or `24:00:00`) |
| `--extra-arg` | — | — | Extra scheduler argument, passed through verbatim (repeatable) |
| `--output` | `-o` | `cluster_scripts` | Directory for generated scripts |
| `--target` | `-t` | — | Target rule(s) to execute |
| `--module` | — | — | Run one include module plus the producers of its declared inputs (repeatable; unions with `--target`). Module names are the include's `name` field or its file stem |
| `--with-dependencies` | — | — | Generate dependency-aware submit script with job chains |
| `--dry-run` | — | — | Generate and write the scripts but submit nothing (and skip `submit.sh` even with `--with-dependencies`) |

One script is written per **rule instance**: wildcards expand first, so a
scatter rule over three samples yields three scripts whose names match the
instances `dry-run` plans.

A rule's own `time_limit` beats `--walltime`. `--extra-arg` values are
emitted as scheduler directives verbatim and are not validated — a typo
reaches the scheduler as written.

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

### Submit with queue and account

```bash
oxo-flow cluster submit pipeline.oxoflow -b slurm -q work -a lab-account
```

### Submit with environment support

```bash
# If your workflow uses conda environments, the generated scripts
# will automatically include conda activation commands
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute
```

### Submit with a wall-time limit and site-specific flags

```bash
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute \
  --walltime 24h --extra-arg --exclusive --extra-arg --constraint=haswell
```

### Submit with job dependencies

```bash
# Generate scripts with automatic dependency chain setup
# Creates a submit.sh wrapper script that handles job submission order
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute --with-dependencies

# Submit the generated wrapper script
bash cluster_scripts/submit.sh
```

The wrapper submits through an `oxo_submit` helper that captures the bare
scheduler job id — `sbatch --parsable` on SLURM, sentence parsing on SGE and
LSF — before chaining it into the next job's dependency flag. Dependencies
are wired per instance, so sample 2's `stats` waits on sample 2's `align`
rather than on every sample's.

### Submit specific target rules

```bash
# Only generate scripts for specific rules and their dependencies
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute -t align -t call_variants
```

### Dry run mode

```bash
# Write the scripts without submitting anything
oxo-flow cluster submit pipeline.oxoflow -b slurm -q compute --dry-run
```

`--dry-run` still writes every script to the output directory — it prints
`(dry-run) generating … job scripts … nothing is submitted` and ends with the
submission line to review, e.g. `sbatch cluster_scripts/*.sh`.

### Check job status

`status` needs at least one job id — an empty list is rejected, because
querying a scheduler for "everything" is not what this command is for:

```bash
oxo-flow cluster status -b slurm 12345 12346
```

Unknown backends are rejected rather than guessed: `-b slrm` fails with
`unknown cluster backend 'slrm' — expected slurm, pbs, sge, or lsf` instead of
silently querying SLURM.

### Cancel specific jobs

```bash
oxo-flow cluster cancel -b slurm 12345 12346
```

### Fetch a job's accounting record

```bash
oxo-flow cluster logs -b slurm 12345
```

SLURM prints the job's `sacct` record (`JobID|State|ExitCode|Elapsed|MaxRSS`);
PBS/SGE/LSF are best-effort (`qstat -f` / `qacct` / `bacct`). Requires the
scheduler's client commands on `PATH`.

---

## Output

### Basic Output

```
oxo-flow v0.16.0 — Rust-native bioinformatics pipeline engine
Cluster: Generating slurm job scripts for 5 rule instances
  ✓ cluster_scripts/fastqc.sh
  ✓ cluster_scripts/trim_reads.sh
  ✓ cluster_scripts/bwa_align_S1.sh
  ✓ cluster_scripts/bwa_align_S2.sh
  ✓ cluster_scripts/bwa_align_S3.sh

Done: 5 scripts written to cluster_scripts
  Submit with: sbatch cluster_scripts/*.sh
```

### With Dependencies Output

```
oxo-flow v0.16.0 — Rust-native bioinformatics pipeline engine
Cluster: Generating slurm job scripts for 5 rule instances
  ✓ cluster_scripts/fastqc.sh
  ✓ cluster_scripts/trim_reads.sh
  ✓ cluster_scripts/bwa_align_S1.sh
  ✓ cluster_scripts/bwa_align_S2.sh
  ✓ cluster_scripts/bwa_align_S3.sh
  ✓ cluster_scripts/submit.sh (dependency-aware submit script)

Done: 6 scripts written to cluster_scripts
  Submit with: bash cluster_scripts/submit.sh
```

---

## Generated Script Example

For a workflow rule with conda environment, different backends produce different scripts
(threads/memory come from the rule's resources; the `-q`/`-a` flags add queue/account
directives; `--walltime` — or a rule's `time_limit`, which wins — adds a time directive,
`#SBATCH --time=` on SLURM and `walltime=` on PBS):

### SLURM Script

```bash
#!/bin/bash
#SBATCH --job-name=bwa_align
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --partition=compute
#SBATCH --output=logs/bwa_align.out
#SBATCH --error=logs/bwa_align.err

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

### PBS/Torque Script

```bash
#!/bin/bash
#PBS -N bwa_align
#PBS -l nodes=1:ppn=16,mem=32G
#PBS -o logs/bwa_align.out
#PBS -e logs/bwa_align.err

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

### SGE Script

```bash
#!/bin/bash
#$ -N bwa_align
#$ -pe smp 16
#$ -l h_vmem=32G
#$ -o logs/bwa_align.out
#$ -e logs/bwa_align.err

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

### LSF Script

```bash
#!/bin/bash
#BSUB -J bwa_align
#BSUB -n 16
#BSUB -M 32G
#BSUB -o logs/bwa_align.out
#BSUB -e logs/bwa_align.err

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

Environment wrapping is applied automatically: conda rules are wrapped in
`conda run --no-capture-output -n <env> bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; ...'`, docker rules in
`docker run --rm --user $(id -u):$(id -g) ... <image> sh -c '<bash shim>' sh '...'`, singularity/apptainer rules in
`<apptainer|singularity> exec --bind <workdir>:<workdir> ... <image> sh -c '<bash shim>' sh '...'`, and rules
without an environment run the command directly. The shim re-execs the
command under `bash` when the image ships it — see
[Environment Wrapping](../how-to/run-on-cluster.md#environment-wrapping).

### Dependency-Aware Submit Script

When using `--with-dependencies`, oxo-flow generates a `submit.sh` wrapper that handles job submission order:

```bash
#!/bin/bash
# Auto-generated dependency-aware submit script
# Generated by oxo-flow

set -e

# Track job IDs
declare -A JOB_IDS

# Submit one script and echo its scheduler job id.
oxo_submit() {
  local out id
  out=$(sbatch --parsable "$@") || return $?
  id=${out%%;*}
  if [ -z "$id" ]; then
    echo "oxo-flow: cannot parse job id from: $out" >&2
    return 1
  fi
  printf '%s' "$id"
}

echo 'Submitting fastqc...'
JOB_IDS[fastqc]=$(oxo_submit cluster_scripts/fastqc.sh)

echo 'Submitting trim_reads...'
JOB_IDS[trim_reads]=$(oxo_submit --dependency=afterok:${JOB_IDS[fastqc]} cluster_scripts/trim_reads.sh)

echo 'Submitting bwa_align...'
JOB_IDS[bwa_align]=$(oxo_submit --dependency=afterok:${JOB_IDS[trim_reads]} cluster_scripts/bwa_align.sh)

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
- Script generation goes through the `ExecutorBackend` trait
  ([Execution Backends](../reference/execution-backends.md)) — the same
  render layer the live submission path uses, so generated scripts and
  submitted scripts can never drift apart
- The `logs/` directory referenced by the scripts' `--output` directives is
  created before the scripts are written (slurmd opens that file at job
  launch, before the script body runs)
- Resource requirements (threads, memory, gpu) from the workflow are automatically translated to cluster directives
- **Environment wrapping is applied automatically** — conda, docker, singularity, pixi, venv, and module environments are properly wrapped in the generated scripts
- `status` and `cancel` actively execute native cluster commands (like `squeue`, `scancel`) and print their outputs directly
- Ensure the required environments (conda envs, docker images, etc.) are available on cluster nodes before submitting
- Use `--with-dependencies` for workflows where rules depend on each other — this ensures proper execution order
