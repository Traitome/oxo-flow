# Experiment-Control Paired Variant Calling

This guide shows how to use the `[[pairs]]` feature (WC-01) to run an experiment-control paired variant pipeline.

## Problem

Comparative calling workflows often require processing experiment and control samples _together_: each caller receives both BAMs and uses the control to remove background or baseline signal. Without first-class pair support, you would have to duplicate rules manually for each case.

## Solution: `[[pairs]]`

Define one `[[pairs]]` entry per patient case.  Rules that use `{experiment}`, `{control}`, or `{pair_id}` placeholders are automatically expanded into one concrete rule per pair.

```toml
[workflow]
name = "somatic-calling"

[[pairs]]
pair_id = "CASE_001"
experiment = "EXP_01"
control    = "CTRL_01"

[[pairs]]
pair_id = "CASE_002"
experiment = "EXP_02"
control    = "CTRL_02"
```

## Wildcard Placeholders

| Placeholder | Replaced with |
|---|---|
| `{experiment}` | Experiment sample name for this pair |
| `{control}` | Control sample name for this pair |
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
experiment = "PT001_EXP"
control    = "PT001_CTRL"

[[rules]]
name   = "align_experiment"
input  = ["raw/{experiment}_R1.fq.gz", "raw/{experiment}_R2.fq.gz"]
output = ["aligned/{experiment}.bam"]
shell  = "bwa mem -t 8 {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"
threads = 8

[[rules]]
name   = "align_control"
input  = ["raw/{control}_R1.fq.gz", "raw/{control}_R2.fq.gz"]
output = ["aligned/{control}.bam"]
shell  = "bwa mem -t 8 {config.reference} {input[0]} {input[1]} | samtools sort -o {output[0]}"
threads = 8

[[rules]]
name   = "mutect2"
input  = ["aligned/{experiment}.bam", "aligned/{control}.bam"]
output = ["variants/{pair_id}.vcf.gz"]
shell  = "gatk Mutect2 -R {config.reference} -I {input[0]} -I {input[1]} -normal {control} -O {output[0]}"
threads = 4
```

Running this with `oxo-flow run somatic-minimal.oxoflow` internally expands to three rules: `align_experiment_PT001`, `align_control_PT001`, and `mutect2_PT001`.

## Validate Expansion

Use `oxo-flow dry-run` to see the expanded rule set before execution:

```bash
oxo-flow dry-run somatic.oxoflow
```

## Full Example

See [`examples/paired_experiment_control_pairs.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/paired_experiment_control_pairs.oxoflow) for a complete clinical WGS somatic calling pipeline with alignment, deduplication, variant calling, filtering, and HTML report generation.

---

## Loading Pairs from External File

For large cohort studies with hundreds or thousands of pairs, use `pairs_file` in `[workflow]` instead of inline `[[pairs]]`:

```toml
[workflow]
name = "somatic-calling"
pairs_file = "metadata/pairs.tsv"  # or .csv, .json
```

Supported formats:

### TSV Format

```tsv
pair_id	experiment	control
CASE_001	EXP_01	CTRL_01
CASE_002	EXP_02	CTRL_02
```

### CSV Format

```csv
pair_id,experiment,control
CASE_001,EXP_01,CTRL_01
CASE_002,EXP_02,CTRL_02
```

### JSON Format

```json
[
  {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
  {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
]
```

You can combine inline `[[pairs]]` with `pairs_file` — entries from both sources are merged.

---

## Auto-Discovery from File Pattern

For workflows where paired BAM files already exist, use `pairs_pattern` to auto-discover pairs by scanning the filesystem:

```toml
[workflow]
name = "somatic-calling"
pairs_pattern = "aligned/{pair_id}/{experiment}_vs_{control}.bam"
```

For a file `aligned/CASE_001/EXP_01_vs_CTRL_01.bam`, this creates:
- `pair_id = CASE_001`
- `experiment = EXP_01`
- `control = CTRL_01`

The pattern must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards. Optional `{experiment_type}` is also supported.

!!! tip "Auto-discovery benefits"
    - No manual pair definitions needed
    - Automatically adapts when new paired files are added
    - Works with any file naming convention that includes the required wildcards
