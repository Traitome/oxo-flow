# `oxo-flow run`

Execute a workflow.

---

## Usage

```
oxo-flow run [OPTIONS] [WORKFLOW]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file. **Optional** — if not specified, auto-discovery searches for: (1) `main.oxoflow` in current directory, (2) alphabetically first `*.oxoflow` file in current directory. |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--jobs` | `-j` | `1` | Maximum number of concurrent jobs |
| `--keep-going` | `-k` | — | Continue execution when a job fails |
| `--workdir` | `-d` | Current directory | Working directory for execution |
| `--target` | `-t` | All rules | Run only specific target rules |
| `--retry` | `-r` | `0` | Number of times to retry failed jobs |
| `--timeout` | — | `0` (disabled) | Timeout per job in seconds |
| `--max-threads` | — | `0` (auto-detect) | Maximum CPU threads available for execution |
| `--max-memory` | — | `0` (auto-detect) | Maximum memory in MB available for execution |
| `--skip-env-setup` | — | — | Skip environment setup (assume environments are ready) |
| `--cache-dir` | — | — | Directory for caching environment setup state |
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

---

## Output

```
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
DAG: 5 rules in execution order
  1. fastqc
  2. trim_reads
  3. bwa_align
  4. sort_bam
  5. call_variants
  ✓ fastqc
  ✓ trim_reads
  ✓ bwa_align
  ✓ sort_bam
  ✓ call_variants

Done: 5 succeeded, 0 failed
```

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
- Setting `--max-threads 0` or `--max-memory 0` auto-detects system resources
- Environment setup is performed automatically before first use of each environment (conda, pixi, docker, singularity, venv)
- Use `--skip-env-setup` when environments are pre-built to avoid redundant setup
- Use `--cache-dir` to persist environment setup state across runs for faster startup
