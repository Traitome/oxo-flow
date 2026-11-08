# `oxo-flow cluster`

Manage cluster job submission and monitoring.

> **Everyday path:** `run --profile <NAME>` submits to a scheduler and
> tracks jobs to completion when the profile carries a `[cluster]` block —
> see [Cluster submission](#cluster-submission-run-profile). The commands
> below remain the manual escape hatch: inspect scripts before submitting,
> or cancel/collect jobs after an interrupted run.

---

## Usage

```
oxo-flow cluster <COMMAND>
```

---

## Actions

| Action | Description |
|---|---|
| `submit` | Submit a workflow to a cluster scheduler |
| `status` | Show the status of submitted cluster jobs |
| `cancel` | Cancel submitted cluster jobs |
| `logs` | Fetch the scheduler's job record (SLURM: `sacct` accounting output) for a submitted cluster job |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file (for `submit`) |
| `[JOB_IDS]...` | Cluster job IDs from a submit/run output — for `status`: omitted, the command lists your queued jobs; given, it answers exactly those ids. Required for `cancel` |
| `<JOB_ID>` | Job ID (for `logs`) |

---

## Options

Every action accepts `--backend` / `-b`:

| Option | Short | Default | Description |
|---|---|---|---|
| `--backend` | `-b` | `$OXO_FLOW_CLUSTER_BACKEND`, then `slurm` | Cluster backend (`slurm`, `pbs`, `sge`, `lsf`, `kubernetes`/`k8s`, `aws-batch`, `gcp-batch`) — on a SLURM site you never need to type it |

An **explicit** value is always validated — `-b slrm` fails with
`unknown cluster backend 'slrm' — expected slurm, pbs, sge, lsf, kubernetes (k8s), aws-batch, or gcp-batch` rather
than silently querying SLURM. The default applies only when the flag is
omitted entirely. On a non-SLURM site set the default once in your shell
profile:

```bash
export OXO_FLOW_CLUSTER_BACKEND=pbs   # or sge / lsf / kubernetes / aws-batch / gcp-batch
```

---

## Options (Submit)

| Option | Short | Default | Description |
|---|---|---|---|
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

With no ids, `status` lists **your** queued jobs (all jobs the scheduler
reports for your user) — the natural first question after a submit:

```bash
$ oxo-flow cluster status
Cluster: Executing 'squeue -u alice --noheader -o %i|%T'...
Cluster: 2 queued jobs
  12345: running
  12346: pending
```

Pass the ids the submit/run step printed to answer exactly those — including
finished ones, which are settled from the scheduler's accounting store:

```bash
oxo-flow cluster status 12345 12346
```

### Cancel specific jobs

```bash
oxo-flow cluster cancel -b slurm 12345 12346
```

### Fetch a job's scheduler record

```bash
oxo-flow cluster logs -b slurm 12345
```

SLURM prints the job's `sacct` record
(`JobID,State,ExitCode,Elapsed,MaxRSS,TotalCPU`); PBS/SGE/LSF are
best-effort (`qstat -f` / `qacct` / `bacct`). Requires the
scheduler's client commands on `PATH`.

`cluster logs` on SLURM reads `sacct` directly and does NOT fall back to
`scontrol`: on clusters without slurmdbd (accounting storage disabled),
`sacct` exits non-zero and this command errors with
`'sacct' exited 1: Slurm accounting storage is disabled` (live-verified on
such a cluster). The `scontrol show job <id>` fallback mentioned below
belongs to run-time **settlement** inside `oxo-flow run`, not to this
command.

On SLURM clusters **without slurmdbd**, settlement in `oxo-flow run` falls
back to `scontrol show job <id>`, which reads the controller's in-memory
record (state + exit code; no Elapsed/RSS/CPU). A job that has left both the
live queue and every probe for longer than the blind-settlement window
settles as **failed with an unknown exit code** and a warning naming the
rule — the run ends instead of polling forever, and the operator verifies
via the rule's output files.

---

## Output

### Basic Output

```
oxo-flow v0.29.1 — Rust-native bioinformatics pipeline engine
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
oxo-flow v0.29.1 — Rust-native bioinformatics pipeline engine
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
`#SBATCH --time=` on SLURM and `walltime=` on PBS; every script is also pinned to the
run directory so relative output paths stay inside it — `#SBATCH --chdir` on SLURM,
a body `cd` on PBS, `#$ -wd` on SGE, `#BSUB -cwd` on LSF):

### SLURM Script

```bash
#!/bin/bash
#SBATCH --chdir='/abs/path/to/workdir'
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
cd '/abs/path/to/workdir' || exit 1

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

### SGE Script

```bash
#!/bin/bash
#$ -wd '/abs/path/to/workdir'
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
#BSUB -cwd '/abs/path/to/workdir'
#BSUB -J bwa_align
#BSUB -n 16
#BSUB -R 'rusage[mem=33554432] span[hosts=1]'
#BSUB -M 33554432
#BSUB -o logs/bwa_align.out
#BSUB -e logs/bwa_align.err

set -e

mkdir -p logs
conda run --no-capture-output -n bwa_env bash -c 'export PATH="$CONDA_PREFIX/bin:$PATH"; bwa mem -t 16 ref.fa reads.fq > aligned.sam'
```

Memory is emitted as an explicit KB figure (`32G` → `33554432`) in both
`-R 'rusage[mem=…]'` and `-M`: LSF's documented mem_spec unit set is
KB/MB/GB with KB as the default, and the bare `32G` form oxo-flow stores
internally is not a valid mem_spec. The `rusage` is what actually places
the job on a host with room; multi-threaded rules also carry
`span[hosts=1]` inside the same `-R` string.

Environment wrapping is applied automatically — conda, docker, and
singularity/apptainer rules (plus pixi, venv, and module environments) each
get their resolver's wrapper, and rules without an environment run the
command directly. The exact per-backend wrapping forms, the bash shim, and
the read-only mount rules are specified once in
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
echo "  Submitted fastqc as job ID: ${JOB_IDS[fastqc]}"

echo 'Submitting trim_reads...'
JOB_IDS[trim_reads]=$(oxo_submit --dependency=afterok:${JOB_IDS[fastqc]} cluster_scripts/trim_reads.sh)
echo "  Submitted trim_reads as job ID: ${JOB_IDS[trim_reads]}"

echo 'Submitting bwa_align...'
JOB_IDS[bwa_align]=$(oxo_submit --dependency=afterok:${JOB_IDS[trim_reads]} cluster_scripts/bwa_align.sh)
echo "  Submitted bwa_align as job ID: ${JOB_IDS[bwa_align]}"

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
| LSF | `-w "ended(jobid)"` |

### Element-wise array chaining (SLURM)

When a downstream rule has exactly one upstream dependency and BOTH scripts
declare the same straight `#SBATCH --array=START-END` range (typically via
`--extra-arg "--array=1-N"`), the wrapper chains them element-wise:

```bash
JOB_IDS[trim_reads]=$(oxo_submit --dependency=aftercorr:${JOB_IDS[fastqc]} cluster_scripts/trim_reads.sh)
```

`aftercorr` pairs array elements by index, so element 1 of the downstream
starts as soon as element 1 of the upstream finishes instead of waiting for
the whole array — a pipelining win on straight scatter chains (measured
1.33× makespan improvement on a real cluster). Any other shape keeps the
ready-batch `afterok` dependency: fan-in (multiple dependencies), mismatched
index ranges, scalar↔array mixes, or a non-array upstream. A `%throttle`
suffix does not block the match — it bounds concurrency, not the index set.

---

## Cluster submission (`run --profile`)

A profile that also carries a `[cluster]` block makes `run --profile` submit
to a scheduler instead of executing locally:

```toml
# profiles/slurm.toml
[cluster]
backend       = "slurm"      # required: slurm | pbs | sge | lsf | kubernetes (k8s) | aws-batch | gcp-batch
partition     = "compute"
account       = "lab01"
walltime      = "24h"        # a rule's own time_limit wins
max_submitted = 50           # jobs in flight at once
poll_interval = "30s"
extra_args    = ["--exclusive", "--constraint=haswell"]  # verbatim directives

[config]
reference = "/data/ref/hg38.fa"
```

Keys the profile omits fall back to the driver defaults: `max_submitted`
50, `max_array_size` 1001, `poll_interval` 5s — the authoritative
[`DriverConfig` table](../reference/execution-backends.md#backenddriver)
carries the complete knob list. `extra_args` entries are
emitted as scheduler directives verbatim and are not validated — a typo
reaches the scheduler as written.

```bash
oxo-flow run pipeline.oxoflow --profile slurm

# one-off override of the profile's queue cap
oxo-flow run pipeline.oxoflow --profile slurm --max-submitted 100
```

Profiles are looked up in the **workflow's own** `profiles/` directory —
there is no user- or system-level profile location, so a site profile shared
across a lab is copied into each workflow (or kept in the workflow
repository) rather than installed once per machine.

Both conditions are required: a `[cluster]` block must be in effect **and**
a profile must be named. A profile with only `[config]` keeps the local
path, and a workflow carrying its own `[cluster]` block still runs locally
until you pass `--profile`, so adding one never changes an existing run.

Rules become jobs after wildcard expansion, so a 100-sample scatter is 100
jobs, and dependencies are chained per instance. The run submits at most
`max_submitted` jobs at a time, tracks them to completion, and writes the
checkpoint exactly as a local run does — `status`, `resume`, and re-run
invalidation behave identically, and a second `run --profile slurm` with
nothing changed submits nothing.

The run's scratch directory (`[engine] tmp_dir` / `OXO_FLOW_TMPDIR`, issue
#907) travels onto the cluster too: every submitted job script exports
`TMPDIR`/`TMP`/`TEMP` to the resolved directory before the rule's command
runs (issue #1013). Cluster jobs start from the scheduler's clean
environment, so the exports are baked into each job's rendered command —
after the workdir `cd` guard and outside the environment wrapper, so
wrapped child processes (`conda run`, `pixi run`, venv activation) and
tools honoring `TMPDIR` all land in the scratch volume. The directory must
exist on the shared filesystem the nodes see; it is created on the submit
host at run start (the same fail-fast mkdir the local path performs) and
the `OXO_FLOW_TMPDIR` override stays verbatim-absolute there.

Like `localrule = true` and `checkpoint = true`, the `scratch = true`
directive is local-executor-only: workflows whose runnable rules declare it
are **rejected at plan time** on the cluster path (naming every offender)
instead of silently running those jobs in the shared workdir, where tools
writing fixed filenames collide across concurrent instances. See
[Scratch tier ladder](../reference/workflow-format.md#scratch-tier-ladder).

### Job arrays

Same-template instances that are ready together and agree on every
scheduler-visible directive (threads, memory, time limit, partition,
account, GPU spec, environment, workdir) are grouped into **one scheduler
array** automatically:

```toml
[cluster]
backend        = "slurm"
max_array_size = 1001   # optional — the scheduler's MaxArraySize; larger
                        # scatter groups are chunked into several arrays
```

- One submission instead of N `sbatch`/`qsub` calls — one queue entry that
  expands internally (SLURM `--array=1-N`, PBS `-J`, SGE `-t`,
  LSF `-J "name[1-N]"`).
- Array elements are tracked individually: per-instance job directories
  stay greppable (`jobs/<instance>/job.sh` + `job.id` + `status.json`),
  and `index.json` in the run directory maps each array index back to its
  instance, so the array never leaks into the human-facing layout.
- Instances that differ on any directive fall out of the group and submit
  as single jobs — heterogeneity degrades gracefully.
- Arrays are transport-level: the JobRecord set, checkpoint, and resume
  semantics are identical to per-job submission.

Element-wise `aftercorr` chaining is not part of the run-path slice:
downstream instances submit in ready batches as array elements finish,
which is correct but not as queue-efficient as true element-wise chaining.
The `--with-dependencies` wrapper above *does* chain straight scatter
arrays element-wise (see
[Element-wise array chaining (SLURM)](#element-wise-array-chaining-slurm)).

### Run directory layout

Each run leaves a directory you can navigate with ordinary tools:

```console
.oxo-flow/runs/2026-08-17T14-30-05/   # `latest` symlinks to the newest
  events.jsonl                        # append-only submit/complete/fail log
  index.json                          # array index → instance name
  jobs/<rule>/job.sh                  # the exact script submitted
  jobs/<rule>/job.id                  # scheduler job id
  jobs/<rule>/status.json
```

Every `events.jsonl` line carries an RFC 3339 `ts`, so the log reconstructs
a timeline rather than just an ordering:

```json
{"ts":"2026-08-17T14:30:05.114Z","t":"SUBMITTED","rule":"align_S1","job":"4812345"}
{"ts":"2026-08-17T15:02:44.907Z","t":"COMPLETED","rule":"align_S1","job":"4812345"}
```

### What a finished job records

When a job leaves the queue, oxo-flow reads the scheduler's accounting store
(`sacct`, `qstat -x -f`, `qacct`, `bacct`) and writes what it found into
`status.json`:

```json
{
  "state": "COMPLETED",
  "job_id": "4812345",
  "command": "bwa mem ref.fa data/S1.fq > aln/S1.bam",
  "submitted_at": "2026-08-17T14:30:05.114Z",
  "finished_at": "2026-08-17T15:02:44.907Z",
  "queue_wait_secs": 65,
  "exit_code": 0,
  "elapsed_secs": 1894,
  "max_rss_mb": 24680,
  "cpu_seconds": 14203
}
```

`queue_wait_secs` is submit-to-finish minus the scheduler's own elapsed
time — the driver never observes the moment a job starts, so the split
between waiting and working is only available by subtraction. The same
`elapsed_secs` is what `benchmarks.wall_time_secs` records in the
checkpoint, which therefore measures runtime rather than runtime plus queue
wait, and `max_rss_mb` / `cpu_seconds` populate the same benchmark fields a
local run fills from its own sampler.

How much of this appears depends on the site. Accounting is a deployment
choice: a SLURM cluster without `slurmdbd` answers `sacct` with nothing, and
LSF's `bacct` columns vary too much between versions to parse blind, so LSF
records state only. Fields the store did not report are omitted rather than
guessed — including `exit_code`, which stays absent for a failed job whose
real code could not be read, instead of defaulting to `1`.

Outputs, the working directory, and logs are assumed to live on storage
shared between the submitting host and the compute nodes.

Interrupting a cluster run cancels its in-flight scheduler jobs: on
Ctrl-C (or SIGTERM/SIGHUP) the driver calls the scheduler's cancel for
every job still in the queue or running, records each interrupted rule as
failed with an `interrupted by <SIG...>` skip reason, and exits with
`128 + <signal number>` — the same shape the local executor reports.
Signals are watched on the cluster path only; the local executor has its
own (pre-existing) signal handling.

What remains out of scope is *re-attaching*: a driver that died without a
signal (crash, OOM) cannot adopt the scheduler job ids of a previous
process. Those jobs keep running under the scheduler, and a subsequent
`resume` / `--resume-failed` re-queues their rules from the checkpoint's
RUNNING entries rather than reusing the live job ids — completed work on
shared storage is still honored by the invalidation analysis. Until a
re-attach path exists, use `oxo-flow cluster status` (no ids: lists your
queued jobs) and `oxo-flow cluster cancel <JOB_ID>...` to inspect or clean
up jobs orphaned by a crashed driver.

---

## Notes

- `submit` generates shell scripts tailored for the specified cluster backend
- Script generation goes through the `ExecutorBackend` trait
  ([Execution Backends](../reference/execution-backends.md)) — the same
  render layer the live submission path uses, so generated scripts and
  submitted scripts can never drift apart
- The `logs/` directory referenced by the scripts' `--output` directives is
  created at submit time (both by `oxo-flow run --profile` and by the
  engine's live submission path): schedulers open the script's `--output`
  file at job launch, before the script body runs, so a missing directory
  fails the job instantly. When you submit generated scripts yourself
  (`cluster submit --dry-run` output), create the directory first.
- Resource requirements (threads, memory, gpu) from the workflow are automatically translated to cluster directives
- **Environment wrapping is applied automatically** — conda, docker, singularity, pixi, venv, and module environments are properly wrapped in the generated scripts
- `status`, `cancel`, and `logs` actively execute native cluster commands
  (`squeue`, `scancel`, …). `status` captures the scheduler's output and
  parses it into one normalized state per job (pending / running /
  completed / failed / cancelled / unknown); for ids you request explicitly,
  jobs no longer in the queue fall back to the scheduler's accounting store
  (`sacct`, `qstat -x`, `qacct`, `bacct`) so finished jobs report their real
  final state instead of "not in the queue". The no-ids listing shows only
  live (queued or running) jobs — that is what the scheduler itself reports.
- Ensure the required environments (conda envs, docker images, etc.) are available on cluster nodes before submitting
- Use `--with-dependencies` for workflows where rules depend on each other — this ensures proper execution order
