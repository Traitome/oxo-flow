# Cohort Studies with Sample Groups

This guide explains how to use `[[sample_groups]]` (WC-02) to run per-sample and per-group analyses across an entire cohort.

## Problem

Population-scale studies require the same pipeline steps to run independently for every sample in every group.  Manually duplicating rules for 50+ samples is error-prone and unmaintainable.

## Solution: `[[sample_groups]]`

Define one `[[sample_groups]]` block per group.  Each block contains a list of sample IDs.  Rules that use `{sample}` or `{group}` placeholders are expanded once per `(group, sample)` combination.

```toml
[[sample_groups]]
name    = "control"
samples = ["CTRL_001", "CTRL_002", "CTRL_003"]

[[sample_groups]]
name    = "case"
samples = ["CASE_001", "CASE_002"]
```

## Wildcard Placeholders

| Placeholder | Replaced with |
|---|---|
| `{sample}` | Individual sample name |
| `{group}` | Group name for this sample |

## Expanded Rule Names

For a rule `align` across the groups above the engine produces:

- `align_control_CTRL_001`
- `align_control_CTRL_002`
- `align_control_CTRL_003`
- `align_case_CASE_001`
- `align_case_CASE_002`

Rules that do **not** reference `{sample}` or `{group}` (e.g., a `multiqc` step that takes the whole `qc/` directory) run once and are kept as-is.

## Group Metadata

Attach arbitrary metadata to each group for use in downstream reporting:

```toml
[[sample_groups]]
name    = "treatment_arm_A"
samples = ["PT_A001", "PT_A002"]
[sample_groups.metadata]
drug  = "compound_X"
dose  = "100mg"
```

## Minimal Example

```toml
[workflow]
name = "cohort-minimal"

[config]
reference = "/data/ref/hg38.fa"

[[sample_groups]]
name    = "healthy"
samples = ["H001", "H002"]

[[sample_groups]]
name    = "disease"
samples = ["D001", "D002", "D003"]

[[rules]]
name   = "align"
input  = ["raw/{sample}_R1.fq.gz", "raw/{sample}_R2.fq.gz"]
output = ["aligned/{sample}.bam"]
shell  = "bwa mem -t {threads} {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"
threads = 8

[[rules]]
name   = "haplotype_caller"
input  = ["aligned/{sample}.bam"]
output = ["gvcf/{sample}.g.vcf.gz"]
shell  = "gatk HaplotypeCaller -I {input[0]} -R {config.reference} -O {output[0]} -ERC GVCF"
threads = 4

# Aggregation step — runs ONCE for all samples
[[rules]]
name   = "multiqc"
input  = ["qc/"]
output = ["reports/multiqc_report.html"]
shell  = "multiqc qc/ -o reports/"
```

## Combining Groups and Pairs

You can use both `[[sample_groups]]` and `[[pairs]]` in the same workflow.  They expand independently: group-wildcard rules are expanded over samples, and pair-wildcard rules are expanded over pairs.

## Full Example

See [`examples/cohort_analysis.oxoflow`](../../../examples/cohort_analysis.oxoflow) for a complete population genomics pipeline including QC, alignment, deduplication, variant calling, and multi-QC aggregation.
