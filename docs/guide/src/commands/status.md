# `oxo-flow status`

Show execution status from a checkpoint file. Displays which rules completed
successfully, which failed, and — with `--timing` — per-rule wall-clock times
and total runtime.

---

## Usage

```
oxo-flow status [OPTIONS] [CHECKPOINT]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[CHECKPOINT]` | Path to a checkpoint JSON file. Defaults to `.oxo-flow/checkpoint.json` in the current directory |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--timing` | | Show per-rule wall-clock times and total runtime, slowest first |
| `--limit <LIMIT>` | `-n` | Maximum number of rules in the `--timing` view (default: 10; requires `--timing`) |
| `--json` | | Output machine-readable JSON to stdout |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

```bash
# Status from the default checkpoint in the current directory
oxo-flow status

# Status from an explicit checkpoint
oxo-flow status .oxo-flow/checkpoint.json

# Per-rule timings, slowest 5 rules
oxo-flow status --timing -n 5

# Machine-readable output including timings
oxo-flow status --timing --json
```

---

## Output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
Status: Status for checkpoint: .oxo-flow/checkpoint.json
  Completed: 3
  Failed:    1

Completed rules:
  ✓ align
  ✓ sort_bam
  ✓ trim_reads

Failed rules:
  ✗ mark_duplicates
```

With `--timing`, the rule list is replaced by a wall-time view (slowest
first):

```
  Completed: 3
  Failed:    0

Rule timings: (top 3, total 45.2s)
  ✓ align (30.1s)
  ✓ sort_bam (12.3s)
  ✓ trim_reads (2.8s)
```

With `--json`, output goes to stdout:

```json
{
  "command": "status",
  "checkpoint": ".oxo-flow/checkpoint.json",
  "workflow": "pipeline.oxoflow",
  "completed": ["align", "sort_bam", "trim_reads"],
  "failed": [],
  "timings": {
    "align": 30.1,
    "sort_bam": 12.3,
    "trim_reads": 2.8
  },
  "total_time_secs": 45.2
}
```

`timings` and `total_time_secs` are only present with `--timing`.

---

## Checkpoint File Format

The checkpoint file is JSON with the following structure:

```json
{
  "completed_rules": ["trim_reads", "align", "sort_bam"],
  "failed_rules": ["mark_duplicates"],
  "benchmarks": {
    "trim_reads": {
      "rule": "trim_reads",
      "wall_time_secs": 42.5,
      "max_memory_mb": 1024,
      "cpu_seconds": 38.2,
      "retries": 0
    }
  },
  "workflow_path": "pipeline.oxoflow",
  "config_snapshot": {
    "min_quality": "20"
  },
  "rule_fingerprints": {
    "trim_reads": "sha256:1a2b3c…",
    "align": "sha256:4d5e6f…"
  }
}
```

`config_snapshot` records the effective config values (sensitive keys stored
as SHA-256 digests) and `rule_fingerprints` the structural fingerprints that
drive [precise invalidation](run.md#config-changes-and-precise-invalidation).

---

## Notes

- Checkpoint files are written automatically during `oxo-flow run` execution
- Use `status` to inspect progress of long-running pipelines, especially on clusters
- The checkpoint file is not updated after the pipeline completes — it reflects the state at the last write
- Exits with code `0` regardless of the pipeline's success or failure
- Rule order in `--timing` output is by wall-clock time descending, so the
  most expensive rules surface first
