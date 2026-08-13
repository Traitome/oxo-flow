# Germline Variant Calling (Simple Chain)

The per-sample GATK germline chain: FastQC + fastp QC, BWA-MEM2 alignment with read groups, MarkDuplicates, BQSR, and HaplotypeCaller GVCF — the simpler sibling of gallery 07, which adds joint calling, VQSR, and annotation.

## What It Demonstrates

- Read-group string `-R '@RG\tID:{sample}\tSM:{sample}\tPL:ILLUMINA'` renders real TAB separators
- Each stage declares its own environment (conda for QC tools, singularity for GATK)
- Mix of environment backends per rule is first-class

## Workflow Definition

```toml
# examples/gallery/13_simple_variant_calling.oxoflow
--8<-- "examples/gallery/13_simple_variant_calling.oxoflow"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/13_simple_variant_calling.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/13_simple_variant_calling.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

