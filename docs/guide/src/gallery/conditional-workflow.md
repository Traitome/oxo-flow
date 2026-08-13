# Conditional Execution

Rule-level `when` conditions switch pipeline branches on config values: WGS vs WES coverage modes, optional QC gating, and downstream annotation gated on flags and thresholds.

## What It Demonstrates

- `when` supports bare boolean config keys (`config.run_qc`), string comparisons (`config.sequencing_mode == "WGS"`), and compound expressions with `&&` / `||` / `!`
- Condition-skipped rules count as satisfied for their dependents — safe to `depends_on` them
- Directory inputs form no file-based DAG edges; the report rule uses `depends_on` to stay behind its QC producers
- Read groups are set at alignment so every downstream tool (GATK included) sees proper SM tags

## Workflow Definition

```toml
# examples/gallery/11_conditional_workflow.oxoflow
--8<-- "examples/gallery/11_conditional_workflow.oxoflow"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/11_conditional_workflow.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/11_conditional_workflow.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

