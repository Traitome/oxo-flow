# Paired Experiment-Control (Single Pair)

Somatic variant calling for one tumor/control pair with hardcoded sample names — the readable version of the [[pairs]]-driven pattern shown next.

## What It Demonstrates

- Both tumor and control flow through identical QC → align → dedup → BQSR stages in parallel branches
- Mutect2 `-normal CTRL_01` resolves via the read-group SM tag set at alignment
- BQSR table and application run in one rule (intermediate table kept inside the shell)

## Workflow Definition

```toml
# examples/gallery/14_paired_experiment_control.oxoflow
--8<-- "examples/gallery/14_paired_experiment_control.oxoflow"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/14_paired_experiment_control.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/14_paired_experiment_control.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

