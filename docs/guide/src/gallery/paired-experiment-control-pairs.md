# Paired Experiment-Control (Multiple Pairs)

The scalable version of gallery 14: the same somatic pipeline driven by `[[pairs]]` with `{experiment}` / `{control}` / `{pair_id}` wildcards, fanning out one branch per pair.

## What It Demonstrates

- `[[pairs]]` entries expand wildcard rules per pair; aggregation rules without pair wildcards stay single-instance
- `--germline-resource` / `--panel-of-normals` are noted in-rule for production hardening
- Read-group SM tags per branch make `Mutect2 -normal {control}` resolve correctly

## Workflow Definition

```toml
# examples/gallery/15_paired_experiment_control_pairs.oxoflow
--8<-- "examples/gallery/15_paired_experiment_control_pairs.oxoflow"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/15_paired_experiment_control_pairs.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/15_paired_experiment_control_pairs.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

