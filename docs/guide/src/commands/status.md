# `oxo-flow status`

Show execution status from a checkpoint file. Displays which rules completed successfully and which failed.

---

## Usage

```
oxo-flow status <CHECKPOINT>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<CHECKPOINT>` | Path to a checkpoint JSON file (e.g., `.oxo-flow/checkpoint.json`) |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

```bash
oxo-flow status .oxo-flow/checkpoint.json
```

---

## Output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
Checkpoint Status:
  ✓ trim_reads
  ✓ align
  ✓ sort_bam
  ✗ mark_duplicates

Summary: 3 completed, 1 failed
```

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
      "cpu_seconds": 38.2
    }
  }
}
```

---

## Notes

- Checkpoint files are written automatically during `oxo-flow run` execution
- Use `status` to inspect progress of long-running pipelines, especially on clusters
- The checkpoint file is not updated after the pipeline completes — it reflects the state at the last write
- Exits with code `0` regardless of the pipeline's success or failure
