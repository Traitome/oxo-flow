# `oxo-flow run`

Execute a workflow.

---

## Usage

```
oxo-flow run [OPTIONS] [WORKFLOW] [KEY=VALUE]...
```

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file. **Optional** — if not specified, auto-discovery searches for: (1) `main.oxoflow` in current directory, (2) alphabetically first `*.oxoflow` file in current directory. |
| `[KEY=VALUE]...` | Direct config overrides: `KEY=VALUE` and `--KEY=VALUE` work for any key; the `--KEY VALUE` space form requires the key to be **declared** in `[config]` (`key = { default = …, help = … }`) — an undeclared `--KEY VALUE` is a hard error (an unknown flag would otherwise be indistinguishable from a mistyped option; a typo'd `--key=val` after the workflow still sets a config var, so prefer the `=` forms for undeclared keys) |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--jobs` | `-j` | `1` | Maximum number of concurrent jobs |
| `--keep-going` | `-k` | — | Continue execution when a job fails |
| `--json` | — | — | Emit a machine-readable JSON summary on stdout after the run (`command`, `status`, `results` counts, `resources`). The document is emitted on **every** exit path — completed, failed, and aborted (preflight failures, budget breaches, cluster runs, plain-failure aborts) — so `--json` consumers never get zero bytes. `status` is `"completed"` only when the run finished with zero required failures; every other exit is `"failed"`. Human progress and result output on stderr is **not** suppressed — this flag appends the summary, it does not switch the run to machine-only mode. `resources` lists every benchmarked rule (`rule`, `status`, `wall_time_secs`, `peak_rss_mb`, `cpu_seconds`, `retries`) — the same data as the report's Benchmarks table; it is empty on aborted paths and on cluster runs (use the report's sacct-based Resource Accounting section there) |
| `--workdir` | `-d` | Workflow file's directory | Working directory for execution |
| `--log-file` | — | `.oxo-flow/logs/oxo-flow.log` in the workdir | Write the run log to a custom path instead (relative paths resolve against the workdir; previous logs rotate to `PATH.1` … `PATH.9`) |
| `--target` | `-t` | All rules | Run only specific target rules |
| `--module` | — | — | Run one include module and the producers of its declared inputs (repeatable; unions with `--target`). Module names are the include's `name` field or its file stem (see [Partial module runs](#partial-module-runs-module)) |
| `--retry` | `-r` | `0` | Number of times to retry failed jobs |
| `--timeout` | — | `0` (disabled) | Timeout per job in seconds |
| `--max-threads` | — | `0` (auto-detect) | Maximum CPU threads available for execution |
| `--max-memory` | — | `0` (auto-detect) | Maximum memory in MB available for execution |
| `--skip-env-setup` | — | — | Skip environment setup (assume environments are ready) |
| `--skip-ref-build` | — | — | Skip automatic reference/index building (assume pre-built) |
| `--cache-dir` | — | — | Directory for caching environment setup state (entries untouched for 90 days are cleaned up after each run; override with the `cache_max_age_days` config key, `0` disables aging) |
| `--resume-failed` | — | — | Resume only failed rules from a previous run |
| `--profile` | — | — | Execution profile name, loaded from `profiles/<NAME>.toml` or `profiles/<NAME>.oxoflow` (see [Execution profiles](#execution-profiles)) |
| `--max-submitted` | — | `N` | Cluster jobs in flight at once (overrides the profile's `max_submitted`) |
| `--provenance` | — | — | Track output file checksums for later verification |
| `--arg` | — | — | Legacy form: set a workflow config value (`KEY=VALUE`). Repeatable. See `[config]` in workflow-format |
| `--bundle` | — | — | Execute from a published bundle (`.tar.zst` or `.tar.gz`). Extracts, verifies checksums, shows resource requirements, and prompts for confirmation |
| `--yes` | — | — | Skip the confirmation prompt when running a bundle (required in non-interactive sessions: CI, scripts, redirected input, or `--json`) |
| `--ai-recover` | — | — | Enable AI error recovery on rule failure |
| `--ai-max-retries` | — | — | Maximum AI retries (overrides `[ai]` config) |
| `--samples` | — | `LIST` | Sample selection: `@path` **replaces** the workflow's samples from a samplesheet, `+@path` **appends** (same-name groups merge, new groups added); names **filter** (or **declare** when the workflow ships no samples), `first:N` (pilot) and `ready` (samples whose entry inputs are complete) **filter**. Repeatable, comma-separated |
| `--rerun` | — | — | Force re-execution of this run's rules (ignore up-to-date checks). Checkpoint records for rules outside this run are kept |
| `--no-report-snapshot` | — | — | Skip the automatic report snapshot written after the run (see [Report snapshots](#report-snapshots)); `resume` has the same flag |
| `--background` | — | — | Detach the run into a background process and exit 0 immediately (see [Background runs (--background)](#background-runs-background)) |
| `--verbose` | `-v` | — | Enable debug-level logging |

---

## Examples

### Run with auto-discovery (when only one .oxoflow file exists)

```bash
# No need to specify the workflow file
oxo-flow run
```

### Run with main.oxoflow (priority discovery)

```bash
# If main.oxoflow exists, it's automatically used
oxo-flow run
```

### Run with explicit workflow

```bash
oxo-flow run pipeline.oxoflow
```

### Run a workflow from a repository (nextflow-style)

A public git repository can be executed directly — no clone, no bundle:

```bash
# Default branch
oxo-flow run gh:owner/pipeline

# Pinned to a git tag or branch (@ref selects a git ref — unlike `pull`,
# where @tag means a GitHub Release bundle)
oxo-flow run gh:owner/pipeline@v0.15.0

# Any git URL or local repository directory
oxo-flow run https://example.com/team/pipeline.git
```

The repository is checked out into `.oxo-flow/repos/<name>` under the
current directory (reused on later runs — delete the directory to force a
fresh clone), and the workflow file is auto-discovered (`main.oxoflow`
first). Because the clone is a read-only cache, the working directory
defaults to the **current directory** for repository runs — outputs, the
checkpoint, and the workdir lock all land next to your data, not inside the
clone. All other run semantics apply unchanged: `--samples`, `--rerun`,
config overrides, `--workdir`, checkpoint-aware dry-run previews, etc.

For `github.com` clones the official URL is tried first, then the
`ghfast.top` / `gh-proxy.com` mirrors automatically (see
[China Mirrors](../how-to/china-mirrors.md)).

### Custom working directory

Relative paths in the workflow resolve against the workflow file's
directory by default, keeping a workflow self-contained. For a shared,
read-only workflow (a fixed reference resource), point `--workdir` at the
analysis directory instead:

```bash
# Workflow lives in a central, read-only location; data and results
# belong to the current analysis directory
oxo-flow run /opt/pipelines/wgs.oxoflow --workdir .

# Outputs, .oxo-flow checkpoint, and lock all land in the workdir;
# dry-run/clean/report/resume accept --workdir too, and resume re-uses
# the workdir recorded in the checkpoint
```

### Parallel execution

```bash
oxo-flow run pipeline.oxoflow -j 8
```

### Keep going on failure

```bash
oxo-flow run pipeline.oxoflow -j 4 -k
```

### Retry failed jobs

```bash
oxo-flow run pipeline.oxoflow -j 8 -r 2
```

### Run specific targets

```bash
oxo-flow run pipeline.oxoflow -t align -t sort
```

### Set a per-job timeout

```bash
oxo-flow run pipeline.oxoflow --timeout 3600
```

### Limit resource usage

```bash
# Use only 16 threads and 32GB memory
oxo-flow run pipeline.oxoflow --max-threads 16 --max-memory 32768
```

### Cache environment setup

```bash
# Cache environment setup state for faster subsequent runs
oxo-flow run pipeline.oxoflow --cache-dir .oxo-flow/cache
```

The cache would otherwise grow without bound, so after each run files
untouched for **90 days** are removed (the next run starts clean). The age
limit is configurable per workflow — set `cache_max_age_days = 0` to disable
aging entirely, or any other value to change the window:

```toml
[config]
cache_max_age_days = 30   # prune after 30 days instead of the 90-day default
```

### Skip environment setup (when environments are pre-built)

```bash
oxo-flow run pipeline.oxoflow --skip-env-setup
```

### Pass config values

Every key in the `[config]` section of the workflow (including declarative
`key = { default = …, required = …, … }` entries) can be overridden from the
CLI — no extra flags to declare:

```bash
# Direct flag form
oxo-flow run pipeline.oxoflow --database refs/nt

# Attached form
oxo-flow run pipeline.oxoflow --database=refs/nt

# Key=value form directly after the workflow file
oxo-flow run pipeline.oxoflow database=refs/nt threshold=1e-3

# Legacy --arg form (still supported)
oxo-flow run pipeline.oxoflow --arg database=refs/nt --arg threshold=1e-3
```

!!! note "Ordering: run flags before overrides"

    `run` flags (`--json`, `--rerun`, `--samples`, `-j`, …) must come
    **before** positional `KEY=VALUE` overrides — the override list is
    trailing, so a flag typed after it is reported with actionable guidance:

    ```console
    oxo-flow run pipeline.oxoflow min_quality=30 --json
    '--json' is a run flag, not a config override.
      Run flags must come before KEY=VALUE overrides, e.g.:
      oxo-flow run <workflow.oxoflow> --json min_quality=30
    ```

### Execution profiles

`--profile <NAME>` applies a reusable config supplement to a run. Profiles
are plain TOML (or `.oxoflow`) files placed in the workflow's own
`profiles/` directory; `.toml` is tried before `.oxoflow`:

```console
<workflow-dir>/profiles/<NAME>.toml     # preferred
<workflow-dir>/profiles/<NAME>.oxoflow
```

The name is just a filename — it carries no built-in meaning. What the
profile *contains* decides its effect: a `[config]`/`[defaults]` block
supplements config, a `[cluster]` block additionally routes execution to
a scheduler (see [Cluster submission](#cluster-submission)). Naming a
profile `conda` or `slurm` is a convention, not a mechanism — compute
environments are declared per rule via `[rules.environment]` and are
independent of profiles.

```toml
# profiles/cluster.toml
[config]
threads = "32"
memory_mb = "128000"
```

```bash
oxo-flow run pipeline.oxoflow --profile cluster
```

The profile's `[config]` table **fills in keys the workflow does not set
itself** by default — values already present in the workflow are never
overwritten. This lets one workflow carry per-environment fallback
defaults (laptop, cluster, cloud). Declare `profile_mode = "override"`
in the workflow's `[workflow]` section when a profile should instead
*replace* workflow values (deep-merged: nested tables merge recursively,
scalars and arrays are replaced) — the classic "cluster profile switches
threads/memory" case:

```toml
[workflow]
name = "rnaseq"
profile_mode = "override"   # fill | override (default: fill)
```

If the named profile file does not exist, `run` (and `dry-run`) **fail
loudly** with a nonzero exit and an error listing every available profile
in the `profiles/` directory — a typo'd `--profile` must never silently
run with the workflow's own config. The same hard error applies when the
profile file exists but its TOML fails to parse or merge.

#### Cluster submission

A profile that also carries a `[cluster]` block makes `run --profile` submit
to a scheduler instead of executing locally:

```toml
# profiles/slurm.toml
[cluster]
backend       = "slurm"      # slurm | pbs | sge | lsf — required
partition     = "compute"
account       = "lab01"
walltime      = "24h"
max_submitted = 50           # jobs in flight at once
poll_interval = "30s"

[config]
reference = "/data/ref/hg38.fa"
```

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

### Job arrays

Same-template instances that are ready together and agree on every
scheduler-visible directive (threads, memory, time limit, partition,
account, GPU spec, environment, workdir) are grouped into **one scheduler
array** automatically (issue #74):

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

Element-wise `aftercorr` chaining is not part of this slice: downstream
instances submit in ready batches as array elements finish, which is
correct but not as queue-efficient as true element-wise chaining.

### Partial module runs (`--module`)

A composed workflow (clinical pipelines like clindet: several
`[[include]]` modules chained together) can run just ONE module and the
producers of its declared inputs:

```bash
# run only the germline module (rules/20_germline.oxoflow) of a composed
# clinical pipeline — the module name defaults to the include's file stem
oxo-flow run main.oxoflow --module 20_germline

# multiple modules + rule targets union together
oxo-flow run main.oxoflow --module 20_germline --module 30_vcf_norm -t report
```

The closure is: the module's rules + every host rule producing one of its
declared concrete contract inputs; upstream DAG dependents come through
the regular target machinery. Unknown module names fail with the known
module list. `dry-run --module` previews the same set.

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

> **Not yet implemented.** The driver polls until every job reaches a
> terminal state; it does not yet re-attach to jobs still in flight if the
> driver exits, and Ctrl-C does not yet cancel submitted jobs. Until then,
> use `oxo-flow cluster cancel <JOB_ID>...` to clean up an interrupted run.

Cluster scheduler submission (SLURM, PBS, SGE, LSF) is configured
separately — see the [`cluster`](cluster.md) command.

### Execute a published bundle

```bash
# Run from a bundle (extracts, verifies checksums, shows resource requirements,
# then prompts for confirmation — requires an interactive terminal on both
# stdin and stderr)
oxo-flow run --bundle pipeline-bundle.tar.zst -j 16

# Skip confirmation for CI/scripts (also required when stdin is redirected
# or with --json, which is always non-interactive)
oxo-flow run --bundle pipeline-bundle.tar.zst -j 16 --yes

# .tar.gz format also supported
oxo-flow run --bundle pipeline-bundle.tar.gz -j 16 --yes

# Pull from remote and execute in one step
oxo-flow pull gh:user/repo@v1 && oxo-flow run --bundle repo-bundle.tar.zst -j 16 --yes
```

### Pilot runs and scale-up

For a large cohort, run a fast pilot on a subset first; the checkpoint
then skips the pilot samples when you scale up:

```bash
# Pilot: run the full pipeline on the first 2 samples only
oxo-flow run pipeline.oxoflow --samples first:2

# Or name the pilot samples explicitly (combines with first:N)
oxo-flow run pipeline.oxoflow --samples first:2,NA12899

# Scale up: no flag needed — completed samples are skipped automatically
oxo-flow run pipeline.oxoflow

# Preview the pilot plan without executing — with a checkpoint present,
# dry-run also predicts what will actually re-run (and what stays
# protected); see dry-run's "Checkpoint-Aware Rerun Preview"
oxo-flow dry-run pipeline.oxoflow --samples first:2

# Fix a config bug found by the pilot — only affected rules re-run
# automatically (see "Config changes" below); use --rerun to force everything
oxo-flow run pipeline.oxoflow --rerun
```

`--samples` filters every sample source (sample groups, `sample_pattern`
discovery, and experiment/control pairs). `--rerun` re-executes the rules
selected for *this* run even when outputs are up to date — combine it with
`--samples` to re-run just the pilot subset while keeping the other
samples' completed records.

After a `--samples` run, a **pilot summary** is printed: samples run,
wall time, per-sample time, and a linear projection for the full cohort.
When the workflow enables `[ai]`, a plain-language pilot report (health
assessment and scale-up advice) is appended automatically.

### Incremental data arrival: `--samples ready`

Sequencing centers deliver data in batches, so a cohort's fastq files
trickle in over days. Instead of waiting for every sample, run the analysis
as data arrives:

```bash
# Which samples can be processed right now, and what is still missing?
oxo-flow dry-run pipeline.oxoflow
#   Sample readiness: 87/100 complete, 13 waiting
#     ⏳ NA12891 (missing: data/NA12891_R2.fastq.gz), …

# Run only the samples whose entry inputs are complete
oxo-flow run pipeline.oxoflow --samples ready

# … more data arrives …
oxo-flow run pipeline.oxoflow --samples ready
#   Done: 5 succeeded, 87 skipped — the checkpoint skips completed samples
```

A sample is **ready** when every external input belonging to it exists —
that is, every rule input (after wildcard and `{config.x}` expansion) that
the workflow itself does not produce. Intermediate products are never
checked (producing them is the DAG's job), and `optional = true` rules do
not block readiness (the executor skips them when their inputs are absent).
Relative paths resolve against the workflow file's directory (or
`--workdir`) — the same place rules actually run from.

Semantics:

- `ready` is a special `--samples` value, not a new syntax: it resolves to
  the names of ready samples and combines with `first:N` and explicit names
  as a union. Because it is reserved, a sample literally named `ready`
  cannot be selected by name.
- With zero ready samples `run` aborts and lists the waiting samples;
  `dry-run --samples ready` instead reports the full cohort state.
- For `[[pairs]]` workflows, a pair is kept only when **both** experiment
  and control inputs are complete; otherwise it is skipped with a note.
  Missing files that belong to no specific sample (shared references) are
  reported as workflow-level inputs.
- The report is also available as JSON via `dry-run --json` (the `samples`
  block), and `--samples ready` works in `test --run` as well.

---

## Background runs (`--background`)

`run --background` (and `resume --background`) detaches execution into a
background process and returns to the shell immediately:

```bash
# Foreground prints one line and exits 0; the run continues in the background
oxo-flow run pipeline.oxoflow --background
#   started in background (pid 48291) · log: .oxo-flow/logs/oxo-flow.log ·
#   monitor: oxo-flow status .oxo-flow/checkpoint.json · stop: kill 48291

# Detach a resumed run the same way
oxo-flow resume .oxo-flow/checkpoint.json --background
```

Mechanics:

- The foreground process spawns a **detached child** that re-runs the same
  command with `--background` removed — every other flag (`-j`, `--profile`,
  `--samples`, `--rerun`, `--log-file`, …) passes through verbatim, so a
  background run behaves exactly like a foreground one: checkpoint, workdir
  lock, report snapshots, and resume semantics are unchanged.
- The child's stdout and stderr are redirected to the run log
  (`--log-file` if given, otherwise `.oxo-flow/logs/oxo-flow.log` in the
  workdir). In this mode the tracing tee is skipped on purpose — the
  redirect already captures everything, and a second writer would
  duplicate every line (issue #194 A3).
- The run log archives BOTH the structured tracing stream AND the
  user-facing progress narrative (`Running:` / `✓` / `Done:`) plus one
  JSON line per execution event (`workflow_started`, `rule_started`,
  `rule_completed`, `workflow_completed`) — the whole run is replayable
  from the single file (issue #194 B1/B3).
- The child's pid is written to `.oxo-flow/background.pid` in the workdir
  (also shown in the summary).
- The child gets its own process group, so it survives terminal close and
  Ctrl-C at the shell — it keeps running until it finishes or is killed.
- The foreground exits **0** once the child is spawned; it does not wait.
  If the spawn fails (for example because another run already holds the
  workdir lock), the foreground exits nonzero with the reason.

Which commands accept `--background`:

- `run` and `resume` — the only long-lived terminal-bound commands.
  Cluster runs go through `run --profile <name>` and therefore detach the
  same way: `run --profile slurm --background` submits and polls the
  cluster jobs entirely inside the background process.
- `dry-run` intentionally does not: a preview is fast and read-only —
  keep it in the foreground.
- Other commands (`pull`, `test`, `export`, `ai`, …) are either quick or
  one-shot; run them before the background run and keep them foreground.

Monitoring a background run works exactly like a foreground one: poll
`oxo-flow status .oxo-flow/checkpoint.json` (or `status --timing`), read
the run log, and check the report snapshots in `.oxo-flow/reports/`. Stop
a background run with `kill <pid>` (Unix) or `taskkill /PID <pid>` —
the pid is in the summary line and in `.oxo-flow/background.pid`.

Notes:

- Combined with the global `--json`, the foreground still prints its
  one-line summary to **stderr** and exits 0; stdout stays empty (the JSON
  run summary belongs to the actual run, which happens in the child).
- Bundle runs need an explicit `--workdir` (the bundle's extracted
  directory is created per-process, so it cannot be predicted from the
  foreground invocation) and `--yes` — a detached child cannot prompt.
- `--background` is a launcher flag, not a workflow setting: the child
  re-parses the remaining argv, so flag ordering rules (run flags before
  `KEY=VALUE` overrides) apply to the whole command line as usual.

---

## Checkpointing and Resuming

oxo-flow automatically persists execution state to a **checkpoint file** after every rule completion. This enables:

1.  **Resuming failed runs**: If a job fails or is interrupted, simply run `oxo-flow run` again. The engine will skip all rules already marked as `completed` and resume from the first pending task.
2.  **State Inspection**: Use the [`oxo-flow status`](./status.md) command to view the progress of a run from its checkpoint file.

### Metadata Directory

By default, checkpoints are saved in a hidden `.oxo-flow/` directory located in the same folder as the workflow file.

- **Filename**: `checkpoint.json` (the name is always the same regardless of workflow name)

### Workflow version in the checkpoint

When the workflow file lives inside a git repository, oxo-flow records the
repository's **HEAD commit SHA** in the checkpoint (`workflow_git_sha`) at
run start. Every result set is therefore auditable to the exact workflow
version that produced it: `oxo-flow provenance verify` prints the SHA, and
report snapshots carry it through the checkpoint they embed.

The lookup is best-effort — running outside a git repository simply omits
the field and never fails the run. Commit your workflow before running to
get fully version-audited results. See
[Workflow Versioning](../reference/versioning.md) for the full model.

### Run logs

Every run — and `resume`, which re-enters the same path — archives its own
log under the workdir:

- **`.oxo-flow/logs/oxo-flow.log`** is always the *latest* run; on each new
  run the previous log rotates to `.oxo-flow/logs/oxo-flow.log.1`, then
  `.2`, … up to `.9` (the oldest backup is deleted). `--log-file PATH`
  overrides the location; the same rotation applies to `PATH`.
- The log header names the exact workflow version that produced the record:
  timestamp, oxo-flow version, full command line, workflow path,
  `workflow_name`, `workflow_version`, `git_sha` (the repository HEAD
  recorded by the run — see [Workflow Versioning](../reference/versioning.md)),
  and workdir.
- The engine's tracing stream (rule start/finish verdicts, warnings, errors)
  is written into the log alongside stderr. Logging is best-effort: if the
  file cannot be opened or written, the run continues with stderr-only
  logging and a warning. Dry-run remains stderr-only.

### Report snapshots

After every run (and resume) — unless `--no-report-snapshot` is given —
oxo-flow writes a JSON report snapshot of the final checkpoint:

- `.oxo-flow/reports/report-<UTC timestamp>.json` — the full report data
  model, so a run leaves a machine-readable report behind with no
  reporting step needed (a `-N` suffix is used when two snapshots land in
  the same second)
- `.oxo-flow/reports/index.json` — a JSON array of
  `{generated_at, workflow, checkpoint, report}` entries, kept sorted by
  `generated_at`; the last entry is the newest snapshot

Snapshot failures are warnings — a reporting hiccup never fails a run.
`oxo-flow report` with no `-o` writes auto-discovered reports into the
same `.oxo-flow/reports/` directory.

### Config changes and precise invalidation

The checkpoint records a snapshot of the effective config values and a
structural fingerprint of every completed rule. On each run, oxo-flow
compares them against the current workflow:

- **Changed config keys** invalidate exactly the rules that reference them
  **plus their DAG downstream**; unrelated completed rules keep their
  checkpoint records. How a rule references a key decides the verdict:
  - A key **interpolated** into `shell`, `script`, `input`/`output` paths,
    `envvars`, or `params` (`{config.<key>}`) always invalidates — the new
    value bakes straight into the command or paths.
  - A key referenced only inside a **`when` condition** invalidates only
    when the condition's truth value actually flips under the new config.
    A toggle that leaves the gate true (or false) — e.g.
    `(config.refine_bins || config.run_checkm)` switching which term is
    true — reuses the completed rules instead of re-running hours-long
    chains whose inputs and commands are identical (issue #198). Flipping a
    gate in either direction still invalidates: the engine pre-marks
    completed rules without re-evaluating `when`, so only plan-time
    detection can retire a now-skipped producer or cascade a newly-activated
    one to its consumers.
- **Edited rule definitions** (shell, inputs, outputs, environment, …) are
  caught by the per-rule fingerprint and invalidate the same way.
- `samples_list` / `samples_<group>` are engine-injected and never trigger
  invalidation, so toggling `--samples` between runs stays cheap. This
  covers rules whose input list is baked from an injected key
  (`expand_inputs` over `config.samples_list`, a gather-rule pattern):
  their rule fingerprint and input manifest change with the selection, and
  oxo-flow treats that change as non-invalidating. When such a rule is
  skipped because of a selection change, `run` prints a warning naming the
  rule — **its outputs still reflect the previous run's full sample set**,
  and `--rerun` is the documented way to regenerate them under the new
  selection. The exclusion is narrow and verified: the rule's other
  definition fields must be byte-identical, and the current input file set
  must be exactly reproducible from the injected value with unchanged
  content — any genuine edit still invalidates. Checkpoints written by
  older binaries lack the input-excluded fingerprint, so the first subset
  run after an upgrade may re-run such a rule once (safe default).

```bash
# min_quality is interpolated by fastp_trim; only it and its downstream re-run.
# A flag toggled in a when-only rule whose gate stays true re-runs nothing —
# the run reports those rules as reused instead.
oxo-flow run pipeline.oxoflow min_quality=30
```

```console
Config change:
  min_quality: 20 → 30
  when-condition unchanged, reused: bin_classify
  → invalidated 3 (1 directly affected), re-running 3/10 this run, skipping 7
  ⊝ index_reference (already completed)
  Running: fastp_trim
  …
```

Rules changed by a transformed split set (`transform.split.values_from`)
are detected through their baked input lists, so chunked rules re-combine
correctly when the split values change.

**Limitations** (correctness-first, documented deliberately):

- Changing `threads`/`memory` (performance knobs) does **not** invalidate
  results; changing anything else in a rule — including shell comments —
  does.
- A checkpoint written before config tracking was introduced is adopted as
  a one-time baseline: the first post-upgrade run reuses everything and
  records the snapshot; changes made before that baseline cannot be
  detected. The same applies to the per-rule `when` verdicts behind
  gate-aware reuse: a pre-verdict checkpoint invalidates when-referencing
  rules once on the first changed-key run, records their verdicts, and
  reuses stably from then on.
- Config values declared `sensitive` are stored in the snapshot as SHA-256
  digests, never as plaintext.
- Concurrent `oxo-flow run` invocations on the same workdir are prevented
  by an exclusive lock on `.oxo-flow/lock` (held for the whole run). The
  second invocation fails with a clear error instead of silently racing on
  `checkpoint.json`; the lock releases automatically when the first process
  exits (even on a crash). `clean` refuses to delete while a run is active
  unless `--force` is given.

### Input changes and manifest invalidation

The checkpoint also records an **input manifest** for every completed rule:
the file set its inputs resolved to at completion time, with each file's
path, size, and modification time — plus a content hash for files up to
64 MiB. On every run, oxo-flow re-resolves the inputs and compares:

- **Literal glob inputs** (`data/*.txt`) detect added and removed files —
  a new file matching the glob rebuilds the rule (and its DAG downstream).
- **Directory inputs** (e.g. `input = ["results"]` — a path that resolves
  to a directory) are listed recursively, so files added or changed anywhere
  inside invalidate the rule. `Dir` inputs with a `pattern` filter track
  only matching files.
- **Plain file inputs** are content-addressed up to 64 MiB: a same-size
  rewrite is detected even when the mtime is preserved, and a mere `touch`
  no longer invalidates. Larger files compare size and mtime, closing the
  same stale-reuse hole for ordinary files.

```console
# a new file appears in data/ after the last run
oxo-flow run pipeline.oxoflow
```

```console
  ↻ input changes invalidated 2 rule(s): gather, report
  Running: gather
  Running: report
  …
```

Invalidated rules bypass the executor's freshness gate and re-execute even
when their outputs exist and look recent; downstream rules that consumed
their outputs rebuild in the same run.

**Limitations** (correctness-first, documented deliberately):

- Detection is a **hybrid policy**: files up to 64 MiB are content-hashed
  (`sha256`), so a same-size rewrite is detected even when the mtime is
  preserved — and a mere `touch` no longer invalidates. Larger files
  (multi-gigabyte intermediates) keep the size+mtime policy, like `make`
  and Snakemake's default mode: hashing every input on every run would cost
  O(total input bytes), prohibitive for bioinformatics-scale files. A large
  file rewritten with identical size and a deliberately preserved mtime is
  not detected; use `--rerun` to force. Legacy checkpoints (entries written
  before hashing) keep comparing size+mtime instead of invalidating
  everything once.
- Only one glob level is tracked per input pattern; brace-expansion
  patterns (`*.{txt,log}`) are not expanded by the manifest scanner, so
  their matched set is not tracked (the shell still expands them at
  runtime).
- Symlinked directories are recorded as single entries, never traversed
  (cycle-safe), so changes inside a symlinked directory are tracked at the
  symlink's own metadata level.
- Rules that clean up their inputs at the end of a successful run
  (`transform.cleanup = true` chunk consumers) are not manifest-tracked:
  their inputs are engine-managed intermediates governed by the
  upstream-cascade invalidation instead.
- A checkpoint written before input tracking was introduced adopts the
  current file set as a one-time baseline (the first post-upgrade run
  reuses everything); changes made before that baseline cannot be
  detected.

### Temporary rules (`temporary = true`)

Pipeline intermediates are usually the largest files in a run (per-sample
BAMs, unsorted alignments) and often useless once every downstream rule has
consumed them. Marking a rule `temporary = true` deletes its outputs after
a **fully successful** run — once every dependent has completed — and
records a **tombstone** in the checkpoint:

```toml
[[rules]]
name = "align"
output = ["aligned/{sample}.bam"]
temporary = true
```

This is checkpoint-aware, not a blind delete:

- A plain re-run **skips** the rule like any completed rule — the output is
  not regenerated just because it is missing.
- When a dependent actually needs the outputs again (its own outputs were
  deleted, its inputs changed, or the config changed), the producer is
  **regenerated first** (lazy cascade-up), then the dependent runs — the
  same order as the original run.
- Failed runs keep the outputs (the deletion only happens when nothing
  failed), so debugging a broken run never has to re-fetch intermediates.
- Leaf rules (no dependents) keep their outputs — there is no downstream
  work for them to enable.

```console
  ↻ temporary outputs needed again — re-running 1 producer rule(s): trim_S1
  …
  ⊘ temporary outputs deleted for 'trim_S1' (1 file(s), regenerated on demand)
```

Dry-run predicts exactly when a tombstoned rule will regenerate
(`[rerun: upstream of X]`) — see
[Checkpoint-aware rerun preview](./dry-run.md#checkpoint-aware-rerun-preview).

### Forcing Execution

To bypass checkpoints and re-execute rules that have already completed, use
[`oxo-flow clean`](./clean.md) to remove outputs and the checkpoint file
before running, or `--rerun` to re-execute this run's rules while keeping
checkpoint records for rules outside the run.

---

### Checkpoint re-entry (`checkpoint = true`)

A checkpoint rule discovers new values at runtime: after it completes, the
engine reads its `checkpoint_manifest`, merges the declared samples, and
executes the newly created rule instances in the same run. The checkpoint
records each re-entry (`reentries`), so a resume replays it and reconstructs
the same plan; invalidating the checkpoint rule revokes its samples until it
re-runs. A missing or unparsable manifest fails the checkpoint rule. See
[Workflow Format](../reference/workflow-format.md#checkpoint-re-entry) for
the config surface.

## Output

```
oxo-flow v0.15.0 — Rust-native bioinformatics pipeline engine
DAG: 5 rules in execution order
  1. fastqc
  2. trim_reads
  3. bwa_align
  4. sort_bam
  5. call_variants
⠋ [00:15] [████████████░░░░░░░░] 3/5 ETA:0:00:42 (executing sort_bam)
  ✓ fastqc (2.1s)
  ✓ trim_reads (15.0s)
  ✓ bwa_align (42.3s)
  ✓ sort_bam (10.2s)
  ✓ call_variants (8.4s)

Done: 5 succeeded, 0 skipped, 0 failed
✓ 5 output files verified (118.3MB total)
```

A progress bar shows execution progress with:

- Elapsed time
- Current position / total rules
- Estimated time remaining (ETA), plus the rule currently executing
- The final summary line reports succeeded / skipped / failed counts

---

## Notes

- The workflow file is optional; if not specified, auto-discovery searches for `main.oxoflow` first, then any `*.oxoflow` file alphabetically
- If no `.oxoflow` file is found, an error message suggests running `oxo-flow init` to create one
- The DAG is built and validated before any rules execute
- Rules are executed in topological order; independent rules may run in parallel up to the `-j` limit
- If `--keep-going` is not set, execution stops at the first failure
- **`--keep-going` changes scheduling, never the verdict:** it runs the
  remaining rules past a failure, but the run still exits non-zero when a
  `required` rule failed — scripts and the web UI (which classifies by
  exit code) see the truth. Only `required = false` failures leave the
  exit code at 0.
- The `--retry` flag re-runs failed jobs up to N times before marking them as failed
- A timeout of `0` means no timeout
- Resource constraints (`threads`, `memory`) in rules are checked against available resources before execution
- **Pre-flight budget check:** When `--max-threads` or `--max-memory` is explicitly set, the engine checks all rules *before* execution starts. Any rule whose requirements exceed the budget is reported immediately (fast-fail), preventing mid-pipeline failures. An explicit budget is a hard limit.
- **Auto-detected capacity is soft:** Rules whose declared requests exceed the machine's detected threads/memory are not rejected — the declared request is the tool's upper bound (often an upstream HPC label), not a scheduling requirement. The engine warns and clamps the pool reservation, so an over-capacity rule runs alone (serialized) instead of blocking the workflow.
- **Deadlock detection:** If pending rules remain while nothing is running and none can become ready (typically an upstream failure), the engine reports `Deadlock detected: N rules stuck` with the stuck rule names. Resource waits cannot deadlock: over-capacity requests are clamped, and explicit budget violations fail fast before any rule runs.
- **Target-aware execution:** The `-t` flag supports prefix matching — `-t al` matches all rules whose names start with "al". Use this to run a subset of the workflow, similar to `make <target>`. Only the named targets and their transitive upstream dependencies are executed; downstream rules are excluded.
- Setting `--max-threads 0` or `--max-memory 0` auto-detects system resources
- Environment setup is performed automatically before first use of each environment (conda, pixi, docker, singularity, venv)
- Use `--skip-env-setup` when environments are pre-built to avoid redundant setup
- Use `--cache-dir` to persist environment setup state across runs for faster startup
