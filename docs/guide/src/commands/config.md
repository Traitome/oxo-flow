# `oxo-flow config`

Inspect and manage workflow configuration.

---

## Usage

```
oxo-flow config <ACTION> [WORKFLOW]
```

---

## Actions

| Action | Description |
|---|---|
| `show` | Show all configuration variables from a workflow |
| `stats` | Show workflow statistics (rules, dependencies, etc.) |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Examples

### Show configuration

```bash
oxo-flow config show pipeline.oxoflow
```

### Show workflow statistics

```bash
oxo-flow config stats pipeline.oxoflow
```

---

## Output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
Workflow Configuration:
  Name:    germline-wgs
  Version: 1.0.0
  Desc:    Germline WGS variant calling pipeline

  Config Variables:
    genome = "hg38"
    threads = 16
    adapter = "AGATCGGAAGAG"
```

---

## Notes

- `config show` displays metadata, configuration variables, includes, and execution groups
- `config stats` provides a high-level overview of the workflow's complexity, including rule counts and resource requirements
