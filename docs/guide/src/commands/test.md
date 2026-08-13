# oxo-flow test

Run a workflow in test mode: validate + lint + dry-run (+ optional deep checks).

## Usage

```
oxo-flow test [OPTIONS] <WORKFLOW>
```

## Description

Performs a comprehensive pre-flight check on a workflow:

1. **Validate** — syntax and semantic correctness
2. **Lint** — best-practice checks (warnings for missing descriptions, logs, etc.)
3. **Dry-run** — preview the execution plan without running commands
4. **Deep checks** (with `--deep`) — script files, environment definition
   files, system-backend binaries, and reference data

This is the recommended command to run before executing a workflow to
catch issues early.

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--output` | — | — | Output file path to verify after the run (fails with exit code 1 if the file is not found) |
| `--run` | — | — | Execute the workflow after validation and lint (runs for real) |
| `--jobs` | `-j` | `1` | Number of parallel jobs (only with `--run`) |
| `--samples` | — | — | Test only a subset of samples: `first:N` (pilot), explicit names, or `ready` (complete entry inputs; repeatable, comma-separated) |
| `--workdir` | — | — | Working directory for the test run (default: the workflow file's directory) |
| `--deep` | — | — | Run deep health checks (script files, env YAML files, backend binaries, reference data) |

---

## Deep checks (`--deep`)

`--deep` detects **pipeline rot** — files and tools the workflow references
but that no longer exist. Without it, a half-year-old workflow fails hours
into a run; with it, the problem is found in seconds:

```
$ oxo-flow test pipeline.oxoflow --deep
  Scripts:
    ✗ script file not found: scripts/seurat_analysis.R (rule: clustering_analysis) [D001]
      hint: add the file to the repository or fix the path
  Environments:
    ⚠ conda environment YAML not found: envs/seurat.yaml (rule: clustering_analysis) [D002]
  Binaries:
    ✓ 2 command(s) found in PATH
  References:
    ⚠ reference path not found: /data/references/GRCh38/genome.fa (rule: bwa_mem2_align) [D004]

Deep check summary: 1 error(s), 3 warning(s)
```

### Check categories

| Code | Severity | Check | Rationale |
|---|---|---|---|
| `D001` | error | Script file exists (`script =` first token, plain interpreter invocations like `Rscript scripts/x.R`, explicit-path commands) | A missing script fails deterministically at run time |
| `D002` | warning | Environment definition exists (conda/mamba YAML, venv directory, `venv_requirements`, `pixi.toml`) | Environment files may be provisioned outside the repository |
| `D003` | warning | System-backend binaries are in `PATH` | PATH is machine-specific (HPC module systems, containers) |
| `D004` | warning | Reference data exists (path-like `[config]` values used in commands, `reference_dir`-derived tool indexes, `[[references]]` outputs) | Data can arrive later — see incremental arrival |

All relative paths resolve against the workflow file's directory (or
`--workdir`), the same base the executor runs rules from. `{config.x}`
placeholders are expanded; paths that still contain `{sample}` wildcards
are skipped and reported by `validate`/`dry-run` instead.

### Exit codes

- `0` — no error-severity findings (warnings do not fail)
- `1` — validation, lint, or a `D001` script-file finding

### Machine-readable output

With `--json`, each step emits its own JSON document on stdout; `--deep`
adds a fourth, so CI can gate on it:

```bash
oxo-flow test pipeline.oxoflow --deep --json | jq -s '.[-1]'
# { "command": "deep-check", "diagnostics": [...], "error_count": 1,
#   "warning_count": 3, "passed": false }
```

Each diagnostic has `severity`, `code`, `message`, `rule`, `suggestion`,
and `path` fields. Fail CI on `code == "D001"` — warnings are
machine-specific and should not gate.

### What `--deep` deliberately does not check

- Only the **first command of each line** is probed for binaries; no
  `&&`/`|`/`;` pipeline parsing.
- Script paths embedded in inline-code expressions
  (`Rscript -e "rmarkdown::render('templates/x.Rmd')"`) are not parsed —
  plain interpreter invocations are. Shell builtins and coreutils
  (`echo`, `mkdir`, `cat`, `grep`, …) are never probed.
- Container images (`docker`, `singularity`) and module names are not
  checked — they are not repository files.

---

## Examples

### Quick pre-flight check

```bash
oxo-flow test pipeline.oxoflow
```

### Run the full test suite including execution

```bash
oxo-flow test pipeline.oxoflow --run -j 4 --output results.txt
```

### Health-check a pipeline before piloting it

```bash
oxo-flow test pipeline.oxoflow --deep
oxo-flow test pipeline.oxoflow --deep --json   # CI preflight gate
```

## Exit Codes

- `0` — all checks passed
- `1` — validation, lint, or deep checks found issues

## See Also

- [oxo-flow validate](validate.md) — validate only
- [oxo-flow lint](lint.md) — lint only
- [oxo-flow dry-run](dry-run.md) — preview execution
- [oxo-flow env](env.md) — check environment backends
