# `oxo-flow lint`

Run best-practice linting checks on a `.oxoflow` file.

---

## Usage

```
oxo-flow lint [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--strict` | — | — | Treat warnings as errors (non-zero exit) |

---

## Examples

### Run standard linting

```bash
oxo-flow lint pipeline.oxoflow
```

### Run strict linting

```bash
oxo-flow lint pipeline.oxoflow --strict
```

---

## Output

```
oxo-flow 0.6.1 — Bioinformatics Pipeline Engine
  warning [W003]: rule has no description (rule: bwa_align)
  warning [W004]: rule has a shell command but no log file specified (rule: bwa_align)
  info [W007]: leaf rule (no dependents) could be marked as target = true (rule: fastqc)

Summary: 0 error(s), 2 warning(s), 1 info
```

---

## Notes

- Linting checks for common mistakes, missing metadata, and potential performance issues
- Rules are checked for valid input/output patterns and environment declarations
- Use `--strict` to ensure high-quality workflow definitions in production environments
