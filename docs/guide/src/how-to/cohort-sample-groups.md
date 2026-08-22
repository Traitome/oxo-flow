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
shell  = "bwa mem -t {threads} -R '@RG\tID:{sample}\tSM:{sample}\tPL:ILLUMINA' {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"

[rules.resources]
threads = 8

[[rules]]
name   = "haplotype_caller"
input  = ["aligned/{sample}.bam"]
output = ["gvcf/{sample}.g.vcf.gz"]
shell  = "gatk HaplotypeCaller -I {input[0]} -R {config.reference} -O {output[0]} -ERC GVCF"

[rules.resources]
threads = 4

# Aggregation step — runs ONCE for all samples.
# Directory inputs form no file-based DAG edges, so depends_on keeps the
# aggregation behind every rule that writes into qc/ (declared names
# expand to all sample instances).
[[rules]]
name   = "multiqc"
input  = ["qc/"]
output = ["reports/multiqc_report.html"]
shell  = "multiqc qc/ -o reports/"
depends_on = ["align"]
```

## Combining Groups and Pairs

You can use both `[[sample_groups]]` and `[[pairs]]` in the same workflow.  They expand independently: group-wildcard rules are expanded over samples, and pair-wildcard rules are expanded over pairs.

## Full Example

See [`examples/gallery/12_cohort_analysis.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/gallery/12_cohort_analysis.oxoflow) for a complete population genomics pipeline including QC, alignment, deduplication, variant calling, and multi-QC aggregation.

---

## Loading Groups from External File

For large cohorts with many groups and samples, use `sample_groups_file` in `[workflow]` instead of inline `[[sample_groups]]`:

```toml
[workflow]
name = "cohort-analysis"
sample_groups_file = "metadata/groups.tsv"  # or .csv, .json
```

Supported formats:

### TSV Format

One row per group; the `samples` column is comma-separated. The same
layout works for CSV (with `,` as delimiter):

```tsv
name	samples
healthy	H001,H002
disease	D001,D002
```

For groups with metadata, use JSON format.

### JSON Format

```json
[
  {
    "name": "treatment_arm_A",
    "samples": ["PT_A001", "PT_A002"],
    "metadata": {
      "drug": "compound_X",
      "dose": "100mg"
    }
  },
  {
    "name": "treatment_arm_B",
    "samples": ["PT_B001", "PT_B002"],
    "metadata": {
      "drug": "compound_Y",
      "dose": "50mg"
    }
  }
]
```

You can combine inline `[[sample_groups]]` with `sample_groups_file` — entries from both sources are merged.

!!! tip "External file benefits"
    - Easily manage large cohort definitions
    - Share group definitions across multiple workflows
    - Update sample lists without modifying workflow files
    - Supports metadata for downstream reporting

## Pilot Subsets

For a large cohort, test the full pipeline on a subset before scaling up —
no workflow edits needed:

```bash
# Pilot: first 2 samples of the cohort, full pipeline
oxo-flow run cohort.oxoflow --samples first:2

# Scale up: completed pilot samples are skipped automatically
oxo-flow run cohort.oxoflow
```

`--samples` filters every source declared here (`[[sample_groups]]`,
`sample_groups_file`, `sample_pattern`) and prunes `[[pairs]]` whose
samples were filtered out. See
[Pilot runs](../commands/run.md#pilot-runs-and-scale-up) for details.

When the cohort's data arrives in batches (a sequencing center delivering
fastq files over days), check readiness with
[`dry-run`](../commands/dry-run.md#sample-readiness) and run what is
complete with
[`--samples ready`](../commands/run.md#incremental-data-arrival-samples-ready).

## Selecting Samples on the Command Line

`--samples` is the single selection entry point. One parameter, four
orthogonal semantics — **replace**, **append**, and **filter** (by name,
pilot size, or readiness):

| Spec | Semantics |
| --- | --- |
| `--samples @sheet.tsv` | **Replace** — the sheet's groups become the run set, overwriting the workflow's inline / auto-discovered / file-loaded groups |
| `--samples +@sheet.tsv` | **Append** — same-name groups merge (union, deduplicated); new group names are added. `[[pairs]]` are untouched: appending can only add samples |
| `--samples S1,S2` | **Filter** on workflows with declared samples (unknown names fail — no phantom samples); **declare** on workflows that ship with no samples at all (the template-workflow invocation pattern) |
| `--samples first:3` / `--samples ready` | **Filter** — narrow the (possibly replaced, appended, or declared) set to a subset / to samples whose entry inputs are complete (see [Incremental data arrival](../commands/run.md#incremental-data-arrival-samples-ready)) |

Sheet specs apply in order and can combine with filters:
`--samples @real.tsv,first:2` replaces with `real.tsv` then runs the first
two. A later `@path` resets earlier `+@path` appends (the last replace
wins).

### Replacing fixture samples with real identifiers

The core invocation-side use case: a workflow ships with fixture names
(e.g. `S1`/`S2`) and the caller runs real identifiers without editing the
file:

```bash
# samplesheet.tsv: one group per row (TSV / CSV / JSON)
#   name     samples
#   cohort   SRR6357072,SRR6357076

oxo-flow run cohort.oxoflow --samples @samplesheet.tsv
```

The sheet's groups **replace** the inline `[[sample_groups]]` (and any
`sample_pattern` / `sample_groups_file` sources). Samples the workflow never
declared are **added** as new samples, so `{group}`/`{sample}` expansion
binds to the override. `[[pairs]]` whose experiment/control are no longer
selected are dropped, and stale injected `samples_<group>` config keys for
dropped groups are pruned. A sheet with no data rows (or rows with empty
samples) fails the run loudly — it never silently falls back to the
workflow's own samples.

### Appending to a cohort

```bash
# Adds S3 to the declared cohort (S2 is deduplicated):
#   name     samples
#   cohort   S2,S3
oxo-flow run cohort.oxoflow --samples +@more.tsv
```

Same-name groups merge with the declared samples (union, deduplicated,
original order preserved); a new group name adds a whole new group.
`[[pairs]]` stay as declared — appending never removes a pair's side.

