# `oxo-flow config`

Inspect and manage workflow configuration.

---

## Usage

```
oxo-flow config <ACTION> [WORKFLOW] [KEY] [VALUE]
```

---

## Actions

| Action | Description |
|---|---|
| `show` | Show all configuration variables from a workflow |
| `stats` | Show workflow statistics (rules, dependencies, etc.) |
| `get <KEY>` | Get a specific configuration variable value |
| `set <KEY> <VALUE>` | Set a configuration variable (modifies the workflow file) |

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |
| `<KEY>` | Configuration variable key (for `get`/`set`) |
| `<VALUE>` | Configuration variable value (for `set`) |

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

### Get a specific config value

```bash
oxo-flow config get pipeline.oxoflow reference
# Output: /data/references/GRCh38/genome.fa
```

### Set a config value

```bash
oxo-flow config set pipeline.oxoflow reference /data/references/GRCh38/genome.fa
# Output: ✓ Set reference = /data/references/GRCh38/genome.fa in pipeline.oxoflow
```

---

## Output

```
oxo-flow 0.5.1 — Bioinformatics Pipeline Engine
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

## Understanding Dependencies

The `config stats` command reports a **dependencies** count. Here's how to interpret it:

| Metric | Meaning |
|---|---|
| **Rules** | Total number of workflow rules (nodes in DAG) |
| **Dependencies** | Total number of edges between rules (connections in DAG) |
| **Max depth** | Longest path from start to end (critical path length) |

### Dependency Calculation Example

A workflow with **10 rules** and **11 dependencies**:

```
Rule A ──► Rule B ──► Rule C
     │              │
     └──► Rule D ───► Rule E
```

- Rule B has 1 incoming edge (from A)
- Rule C has 1 incoming edge (from B)
- Rule D has 1 incoming edge (from A)
- Rule E has 2 incoming edges (from C and D)
- Total edges = 1 + 1 + 1 + 2 = 5 dependencies

**Why can dependencies exceed rules?**

Some rules depend on multiple upstream outputs (e.g., a merge rule that combines results from parallel branches). Each input file creates an edge in the DAG.

---

## Notes

- `config show` displays metadata, configuration variables, includes, and execution groups
- `config stats` provides a high-level overview of the workflow's complexity
- `config get` exits with code 1 if the key is not found
- `config set` intelligently parses values as boolean, integer, float, or string
