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
| `--cache-dir` | — | — | Directory for caching environment setup state |
| `--resume-failed` | — | — | Resume only failed rules from a previous run |
| `--profile` | — | — | Execution profile name, loaded from `profiles/<NAME>.toml` (use `oxo-flow profile` to manage) |
| `--provenance` | — | — | Track output file checksums for later verification |
| `--arg` | — | — | Legacy form: set a workflow config value (`KEY=VALUE`). Repeatable. See `[config]` in workflow-format |
| `--sample` | — | — | Add a sample to the run. Repeatable. Merges with sample_pattern/CSV sources |
| `--bundle` | — | — | Execute from a published bundle (`.tar.zst` or `.tar.gz`). Extracts, verifies checksums, shows resource requirements, and prompts for confirmation |
| `--yes` | — | — | Skip the confirmation prompt when running a bundle (required in non-interactive sessions: CI, scripts, redirected input, or `--json`) |
| `--ai-recover` | — | — | Enable AI error recovery on rule failure |
| `--ai-max-retries` | — | — | Maximum AI retries (overrides `[ai]` config) |
| `--samples` | — | `LIST` | Run only a subset of samples: `first:N` (pilot) or explicit names. Repeatable, comma-separated. Filters `[[sample_groups]]`, `sample_pattern` discovery, and `[[pairs]]`. Mutually exclusive with `--sample` |
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

# Preview the pilot plan without executing
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
- Concurrent `oxo-flow run` invocations on the same workdir race on
  `checkpoint.json` (last writer wins) — run one workflow instance per
  workdir.

### Forcing Execution

To bypass checkpoints and re-execute rules that have already completed, use
[`oxo-flow clean`](./clean.md) to remove outputs and the checkpoint file
before running, or `--rerun` to re-execute this run's rules while keeping
checkpoint records for rules outside the run.

---

## Output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
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
