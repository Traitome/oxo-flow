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

---

## Checkpointing and Resuming

oxo-flow automatically persists execution state to a **checkpoint file** after every rule completion. This enables:

1.  **Resuming failed runs**: If a job fails or is interrupted, simply run `oxo-flow run` again. The engine will skip all rules already marked as `completed` and resume from the first pending task.
2.  **State Inspection**: Use the [`oxo-flow status`](./status.md) command to view the progress of a run from its checkpoint file.

### Metadata Directory

By default, checkpoints are saved in a hidden `.oxo-flow/` directory located in the same folder as the workflow file.

- **Filename**: `checkpoint.json` (the name is always the same regardless of workflow name)

### Forcing Execution

To bypass checkpoints and re-execute rules that have already completed, use the [`oxo-flow clean`](./clean.md) command to remove outputs and the checkpoint file before running.

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
