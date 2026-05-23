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

## Notes

- `watch` only validates — it does not automatically re-execute the workflow.
  Use `oxo-flow run` to execute after changes are validated.
- The polling interval is approximately 2 seconds.
- Press `Ctrl+C` to stop watching.

## Examples

```bash
# Watch a workflow during development
oxo-flow watch my_pipeline.oxoflow

# In another terminal, edit the workflow and save to trigger re-validation
```

## See Also

- [oxo-flow validate](validate.md) — one-time validation
- [oxo-flow test](test.md) — comprehensive pre-flight check
