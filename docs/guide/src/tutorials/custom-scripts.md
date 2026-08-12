# Writing Custom Scripts

This tutorial bridges the gap between running standard shell commands and writing complex logic within oxo-flow. Often, a single shell command isn't enough to process bioinformatics data, and you'll need to embed Python, R, or Bash scripts directly into your workflow.

---

## The `script` Directive

oxo-flow provides a clean way to execute inline or external scripts while still benefiting from dependency tracking and environment isolation.

Instead of a `shell` command, a rule can declare a `script` field that points to
a Python, R, or Bash script file:

```toml
[[rules]]
name = "analyze"
script = "scripts/analysis.py --min-quality {params.q}"
interpreter = "python3" # Optional: overrides auto-detection
```

The interpreter is detected automatically, in this order:

1. **Explicit `interpreter` field** on the rule
2. **Custom `[workflow.interpreter_map]`** in the workflow metadata
3. **Built-in defaults** based on file extension (`.py` → `python3`, `.R`/`.r` → `Rscript`, `.sh` → `bash`, etc.)
4. **Shebang line** (if the file is executable)

The script runs inside the rule's declared environment, so dependencies are
tracked and environment isolation still applies. Outputs declared in the rule
are only considered produced if the script completes successfully.

## Scripts vs. Shell Commands

| | `shell` | `script` |
|---|---|---|
| Best for | Short commands, one-liners, pipes | Multi-step logic, complex programs |
| Language | Any shell (bash, sh) | Any language with an interpreter |
| Interpreter | Shell itself | Auto-detected from extension, `interpreter` field, or shebang |
| Dependency tracking | Same | Same |

## See Also

- [Script Execution (workflow format)](../reference/workflow-format.md#script-execution-script) — full reference for the `script` field
- [Custom Interpreters (`interpreter_map`)](../reference/workflow-format.md#custom-interpreters-interpreter_map) — mapping extensions to interpreters
- [Command Reference](../commands/run.md) — running workflows