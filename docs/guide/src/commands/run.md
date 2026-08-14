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
| `[KEY=VALUE]...` | Direct config overrides: `KEY=VALUE`, `--KEY=VALUE`, or `--KEY VALUE` |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--jobs` | `-j` | `1` | Maximum number of concurrent jobs |
| `--keep-going` | `-k` | — | Continue execution when a job fails |
| `--workdir` | `-d` | Workflow file's directory | Working directory for execution |
| `--target` | `-t` | All rules | Run only specific target rules |
| `--retry` | `-r` | `0` | Number of times to retry failed jobs |
| `--timeout` | — | `0` (disabled) | Timeout per job in seconds |
| `--max-threads` | — | `0` (auto-detect) | Maximum CPU threads available for execution |
| `--max-memory` | — | `0` (auto-detect) | Maximum memory in MB available for execution |
| `--skip-env-setup` | — | — | Skip environment setup (assume environments are ready) |
| `--skip-ref-build` | — | — | Skip automatic reference/index building (assume pre-built) |
| `--cache-dir` | — | — | Directory for caching environment setup state (entries untouched for 90 days are cleaned up after each run; override with the `cache_max_age_days` config key, `0` disables aging) |
| `--resume-failed` | — | — | Resume only failed rules from a previous run |
| `--profile` | — | — | Execution profile name, loaded from `profiles/<NAME>.toml` or `profiles/<NAME>.oxoflow` (see [Execution profiles](#execution-profiles)) |
| `--provenance` | — | — | Track output file checksums for later verification |
| `--arg` | — | — | Legacy form: set a workflow config value (`KEY=VALUE`). Repeatable. See `[config]` in workflow-format |
| `--sample` | — | — | Add a sample to the run. Repeatable. Merges with sample_pattern/CSV sources |
| `--bundle` | — | — | Execute from a published bundle (`.tar.zst` or `.tar.gz`). Extracts, verifies checksums, shows resource requirements, and prompts for confirmation |
| `--yes` | — | — | Skip the confirmation prompt when running a bundle (required in non-interactive sessions: CI, scripts, redirected input, or `--json`) |
| `--ai-recover` | — | — | Enable AI error recovery on rule failure |
| `--ai-max-retries` | — | — | Maximum AI retries (overrides `[ai]` config) |
| `--samples` | — | `LIST` | Run only a subset of samples: `first:N` (pilot), explicit names, or `ready` (samples whose entry inputs are complete). Repeatable, comma-separated. Filters `[[sample_groups]]`, `sample_pattern` discovery, and `[[pairs]]`. Mutually exclusive with `--sample` |
| `--rerun` | — | — | Force re-execution of this run's rules (ignore up-to-date checks). Checkpoint records for rules outside this run are kept |
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
oxo-flow run gh:owner/pipeline@v0.11.0

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
itself** — values already present in the workflow are never overwritten.
This lets one workflow carry per-environment fallback defaults (laptop,
cluster, cloud). If the named profile file does not exist, `run` prints a
warning and proceeds with the workflow's own config.

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

## Checkpointing and Resuming

oxo-flow automatically persists execution state to a **checkpoint file** after every rule completion. This enables:

1.  **Resuming failed runs**: If a job fails or is interrupted, simply run `oxo-flow run` again. The engine will skip all rules already marked as `completed` and resume from the first pending task.
2.  **State Inspection**: Use the [`oxo-flow status`](./status.md) command to view the progress of a run from its checkpoint file.

### Metadata Directory

By default, checkpoints are saved in a hidden `.oxo-flow/` directory located in the same folder as the workflow file.

- **Filename**: `checkpoint.json` (the name is always the same regardless of workflow name)

### Config changes and precise invalidation

The checkpoint records a snapshot of the effective config values and a
structural fingerprint of every completed rule. On each run, oxo-flow
compares them against the current workflow:

- **Changed config keys** invalidate exactly the rules that reference them
  (in `shell`, `script`, `input`/`output` paths, `envvars`, `params`, or
  `when` conditions) **plus their DAG downstream**. Unrelated completed
  rules keep their checkpoint records.
- **Edited rule definitions** (shell, inputs, outputs, environment, …) are
  caught by the per-rule fingerprint and invalidate the same way.
- `samples_list` / `samples_<group>` are engine-injected and never trigger
  invalidation, so toggling `--samples` between runs stays cheap.

```bash
# min_quality is referenced by fastp_trim; only it and its downstream re-run
oxo-flow run pipeline.oxoflow min_quality=30
```

```console
Config change:
  min_quality: 20 → 30
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
  detected.
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

## Output

```
oxo-flow 0.11.0 — Bioinformatics Pipeline Engine
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
- The `--retry` flag re-runs failed jobs up to N times before marking them as failed
- A timeout of `0` means no timeout
- Resource constraints (`threads`, `memory`) in rules are checked against available resources before execution
- **Pre-flight budget check:** When `--max-threads` or `--max-memory` is explicitly set, the engine checks all rules *before* execution starts. Any rule whose requirements exceed the budget is reported immediately (fast-fail), preventing mid-pipeline failures. Rules that exceed auto-detected system resources produce warnings but don't block execution.
- **Deadlock detection:** If pending rules can never fit in the available resource pool (e.g., every pending rule requires 64 threads but `--max-threads=32`), the engine reports `Deadlock detected: N rules stuck` with the stuck rule names and guidance on resolution.
- **Target-aware execution:** The `-t` flag supports prefix matching — `-t al` matches all rules whose names start with "al". Use this to run a subset of the workflow, similar to `make <target>`. Only the named targets and their transitive upstream dependencies are executed; downstream rules are excluded.
- Setting `--max-threads 0` or `--max-memory 0` auto-detects system resources
- Environment setup is performed automatically before first use of each environment (conda, pixi, docker, singularity, venv)
- Use `--skip-env-setup` when environments are pre-built to avoid redundant setup
- Use `--cache-dir` to persist environment setup state across runs for faster startup
