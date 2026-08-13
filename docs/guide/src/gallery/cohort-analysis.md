# Cohort Analysis with Sample Groups

A population-scale study: multiple `[[sample_groups]]` expand per-sample instances, and a single multiqc aggregation step consumes the whole cohort's QC directory.

## What It Demonstrates

- `{sample}` rules expand once per sample across all groups (case/control here)
- The aggregation rule has no wildcard inputs → exactly one instance; `depends_on` keeps it behind every per-sample rule (directory inputs form no DAG edges)
- Per-rule conda environments keep tool versions isolated (fastp / bwa-mem2+samtools / gatk / multiqc)

## Workflow Definition

```toml
# examples/gallery/12_cohort_analysis.oxoflow
--8<-- "examples/gallery/12_cohort_analysis.oxoflow"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/12_cohort_analysis.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/12_cohort_analysis.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

