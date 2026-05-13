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
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
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
  "started_at": "2026-04-05T10:00:00Z",
  "updated_at": "2026-04-05T10:15:32Z"
}
```

---

## Notes

- Checkpoint files are written automatically during `oxo-flow run` execution
- Use `status` to inspect progress of long-running pipelines, especially on clusters
- The checkpoint file is not updated after the pipeline completes — it reflects the state at the last write
- Exits with code `0` regardless of the pipeline's success or failure
