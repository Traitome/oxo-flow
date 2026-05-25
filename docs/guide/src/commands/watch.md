# oxo-flow watch

Watch a workflow file for changes and re-validate automatically.

## Usage

```
oxo-flow watch <WORKFLOW>
```

## Description

Monitors the specified `.oxoflow` file for modifications (via mtime polling).
When a change is detected, the workflow is re-validated and any errors or
warnings are displayed. This provides a fast edit-check cycle during
workflow development.

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--run` | — | — | Automatically execute the workflow on each detected change |
| `--jobs` | `-j` | `1` | Number of parallel jobs (only with `--run`) |

## Notes

- Without `--run`, `watch` only validates and dry-runs — it does not re-execute.
- The polling interval is approximately 2 seconds.
- Press `Ctrl+C` to stop watching.

## Examples

```bash
# Watch a workflow during development
oxo-flow watch my_pipeline.oxoflow

# Auto-execute on each change
oxo-flow watch my_pipeline.oxoflow --run -j 4

# In another terminal, edit the workflow and save to trigger re-validation
```

## See Also

- [oxo-flow validate](validate.md) — one-time validation
- [oxo-flow test](test.md) — comprehensive pre-flight check

## Auto-Run Mode

With `--run`, watch automatically re-executes the workflow when changes
are detected, enabling rapid development cycles:

```bash
# Edit-and-run loop: each save triggers validation + execution
oxo-flow watch my_pipeline.oxoflow --run -j 4
```

Without `--run` (default), watch validates and shows a dry-run preview
after each change.
