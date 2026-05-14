# Tumor-Normal Paired Somatic Variant Calling

This guide shows how to use the `[[pairs]]` feature (WC-01) to run a matched tumor-normal somatic variant calling pipeline.

## Problem

Clinical somatic calling requires processing tumor and normal samples _together_: each variant caller receives both BAMs and uses the normal to subtract germline variants.  Without first-class pair support, you would have to duplicate rules manually for each patient.

## Solution: `[[pairs]]`

Define one `[[pairs]]` entry per patient case.  Rules that use `{tumor}`, `{normal}`, or `{pair_id}` placeholders are automatically expanded into one concrete rule per pair.

```toml
[workflow]
name = "somatic-calling"

[[pairs]]
pair_id = "CASE_001"
tumor   = "TUMOR_01"
normal  = "NORMAL_01"

[[pairs]]
pair_id = "CASE_002"
tumor   = "TUMOR_02"
normal  = "NORMAL_02"
```

## Wildcard Placeholders

| Placeholder | Replaced with |
|---|---|
| `{tumor}` | Tumor sample name for this pair |
| `{normal}` | Normal sample name for this pair |
| `{pair_id}` | Unique pair identifier |

Placeholders can appear in `input`, `output`, and `shell` fields.

## Expanded Rule Names

A rule named `mutect2` with two pairs `CASE_001` and `CASE_002` produces:

- `mutect2_CASE_001`
- `mutect2_CASE_002`

Rules that do **not** reference any pair placeholder (e.g., a `multiqc` aggregation step) are kept as-is.

## Minimal Example

```toml
[workflow]
name = "somatic-minimal"

[config]
reference = "/data/ref/hg38.fa"

[[pairs]]
pair_id = "PT001"
tumor   = "PT001_Tumor"
normal  = "PT001_Normal"

[[rules]]
name   = "align_tumor"
input  = ["raw/{tumor}_R1.fq.gz", "raw/{tumor}_R2.fq.gz"]
output = ["aligned/{tumor}.bam"]
shell  = "bwa mem -t 8 {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"
threads = 8

[[rules]]
name   = "align_normal"
input  = ["raw/{normal}_R1.fq.gz", "raw/{normal}_R2.fq.gz"]
output = ["aligned/{normal}.bam"]
shell  = "bwa mem -t 8 {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"
threads = 8

[[rules]]
name   = "mutect2"
input  = ["aligned/{tumor}.bam", "aligned/{normal}.bam"]
output = ["variants/{pair_id}.vcf.gz"]
shell  = "gatk Mutect2 -R {config.reference} -I {input[0]} -I {input[1]} -normal {normal} -O {output[0]}"
threads = 4
```

Running this with `oxo-flow run somatic-minimal.oxoflow` internally expands to three rules: `align_tumor_PT001`, `align_normal_PT001`, and `mutect2_PT001`.

## Validate Expansion

Use `oxo-flow dry-run` to see the expanded rule set before execution:

```bash
oxo-flow dry-run somatic.oxoflow
```

## Full Example

See [`examples/paired_tumor_normal_pairs.oxoflow`](../../../examples/paired_tumor_normal_pairs.oxoflow) for a complete clinical WGS somatic calling pipeline with alignment, deduplication, variant calling, filtering, and HTML report generation.
