# oxo-flow history

Show execution history from checkpoints.

## Usage

```
oxo-flow history [DIR] [-n <LIMIT>]
```

## Description

Displays execution history by reading the checkpoint file
(`.oxo-flow/checkpoint.json`) in the specified or current directory.
Shows the workflow name, completed and failed rule counts, total
execution time, and recent rules with per-rule durations.

## Options

| Option | Description |
|--------|-------------|
| `-n, --limit <LIMIT>` | Maximum number of recent rules to show (default: 10) |

## Examples

```bash
# Show history from the current directory
oxo-flow history

# Show history from a specific directory
oxo-flow history /path/to/project

# Show only the 5 most recent rules
oxo-flow history -n 5
```

## Output

```
History: /project/.oxo-flow/checkpoint.json
  Workflow: my_pipeline.oxoflow
  Completed: 3
  Failed:    0
  Total time: 45.2s

  Recent rules: (showing up to 10)
    ✓ fastp_trim (12.3s)
    ✓ bwa_align (30.1s)
    ✓ samtools_sort (2.8s)
```

## Notes

- The checkpoint is generated automatically by `oxo-flow run`.
- If no checkpoint exists, a helpful message is displayed.
- Long-running workflows benefit from `oxo-flow status` for live monitoring.

## See Also

- [oxo-flow status](status.md) — inspect live checkpoint state
- [oxo-flow resume](resume.md) — resume from a checkpoint
- [oxo-flow run](run.md) — execute a workflow
