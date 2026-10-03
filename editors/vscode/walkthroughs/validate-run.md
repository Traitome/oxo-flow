# Validate, then run

Static gates first, compute second — the oxo-flow habit:

1. **oxo-flow: Validate Pipeline** (<kbd>Ctrl+Alt+V</kbd>) — structure,
   missing inputs, capacity. Green check = the DAG is well-formed.
2. **oxo-flow: Dry Run (plan only)** — previews the expansion and
   resource audit without executing anything.
3. **oxo-flow: Run Pipeline** (<kbd>Ctrl+Alt+R</kbd>) — executes the
   DAG as a task in a dedicated terminal panel.

Because run/dry-run are tasks (`type: "oxo-flow"`), you can pin
options in `tasks.json`: `jobs`, `keepGoing`, `target`, and
`extraArgs` (e.g. `["--profile", "slurm"]` for cluster submission).

A failed rule? **oxo-flow: Resume from Checkpoint** continues where
the run stopped without recomputing finished work.
