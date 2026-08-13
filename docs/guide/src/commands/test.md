# oxo-flow test

Run a workflow in test mode: validate + lint + dry-run.

## Usage

```
oxo-flow test [OPTIONS] <WORKFLOW>
```

## Description

Performs a comprehensive pre-flight check on a workflow:

1. **Validate** — syntax and semantic correctness
2. **Lint** — best-practice checks (warnings for missing descriptions, logs, etc.)
3. **Dry-run** — preview the execution plan without running commands

This is the recommended command to run before executing a workflow to
catch issues early.

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--output` | — | — | Output file path to verify after the run (fails with exit code 1 if the file is not found) |
| `--run` | — | — | Execute the workflow after validation and lint (runs for real) |
| `--jobs` | `-j` | `1` | Number of parallel jobs (only with `--run`) |

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

## Examples

```bash
# Test a workflow before running
oxo-flow test my_pipeline.oxoflow

# Fix any issues, then run
oxo-flow run my_pipeline.oxoflow -j 8
```

## Exit Codes

- `0` — all checks passed
- `1` — validation or lint found issues

## See Also

- [oxo-flow validate](validate.md) — validate only
- [oxo-flow lint](lint.md) — lint only
- [oxo-flow dry-run](dry-run.md) — preview execution
