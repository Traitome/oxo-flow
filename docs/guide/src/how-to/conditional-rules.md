# Conditional Rule Execution with `when`

This guide explains how to use the `when` field (WF-01) to skip rules based on configuration values.

## Problem

A single pipeline often needs to adapt to different inputs: WGS vs. WES sequencing modes, optional annotation steps, or analysis paths that depend on coverage thresholds.  Without conditional syntax, you either ship multiple pipeline files or rely on shell-level `if` statements that obscure the workflow structure.

## Solution: `when` expressions

Add a `when` field to any rule.  The expression is evaluated against your `[config]` section before the DAG is built.  When `when` evaluates to `false`, the rule is **removed from the DAG entirely** — its outputs are not expected and downstream rules that depend on them are handled accordingly.

```toml
[[rules]]
name  = "fastqc"
when  = "config.run_qc"
input = ["raw/sample_R1.fq.gz"]
output = ["qc/sample_fastqc.html"]
shell = "fastqc {input[0]} -o qc/"
```

## Expression Syntax

### Simple truthiness

```toml
when = "config.run_qc"        # true if run_qc is truthy (boolean true, non-zero, non-empty)
when = "!config.skip_step"    # true if skip_step is falsy
```

### Comparisons

```toml
when = 'config.mode == "WGS"'          # string equality
when = 'config.mode != "WES"'          # string inequality
when = "config.min_coverage >= 20"     # numeric (>=, >, <=, <)
when = "config.threads == 8"           # numeric equality
```

### Boolean equality

```toml
when = "config.run_annotation == true"
when = "config.dry_run == false"
```

### File existence

```toml
when = 'file_exists("panel_of_controls.vcf.gz")'
```

### Logical operators

```toml
when = "config.run_qc && config.min_coverage >= 20"
when = 'config.mode == "WGS" || config.mode == "WES"'
when = '(config.run_annotation && config.min_coverage >= 20) || config.force_annotate'
```

## Practical Example

```toml
[config]
sequencing_mode = "WGS"
run_qc          = true
min_coverage    = 35
target_bed      = ""

[[rules]]
name = "align"
# No `when` — always runs
input = ["raw/sample_R1.fq.gz"]
output = ["aligned/sample.bam"]
shell = "bwa mem ref.fa {input[0]} > {output[0]}"

[[rules]]
name  = "fastqc"
when  = "config.run_qc"
input = ["raw/sample_R1.fq.gz"]
output = ["qc/fastqc.html"]
shell = "fastqc {input[0]} -o qc/"

[[rules]]
name   = "wgs_coverage"
when   = 'config.sequencing_mode == "WGS"'
input  = ["aligned/sample.bam"]
output = ["qc/coverage.txt"]
shell  = "mosdepth qc/sample aligned/sample.bam"

[[rules]]
name   = "wes_coverage"
when   = 'config.sequencing_mode == "WES" && config.target_bed != ""'
input  = ["aligned/sample.bam"]
output = ["qc/coverage.txt"]
shell  = "mosdepth --by {config.target_bed} qc/sample aligned/sample.bam"

[[rules]]
name  = "annotate"
when  = "config.run_qc && config.min_coverage >= 20"
input = ["variants/sample.vcf.gz"]
output = ["annotated/sample.vcf.gz"]
shell = "vep --input_file {input[0]} --output_file {output[0]}"
```

With `sequencing_mode = "WGS"` and `run_qc = true` and `min_coverage = 35`:

- `align` — runs
- `fastqc` — runs (`run_qc` is true)
- `wgs_coverage` — runs (`sequencing_mode == "WGS"`)
- `wes_coverage` — **skipped** (`sequencing_mode != "WES"`)
- `annotate` — runs (`run_qc && min_coverage >= 20`)

## Checking Which Rules Will Run

Use `oxo-flow dry-run` to preview the effective DAG after condition evaluation:

```bash
oxo-flow dry-run my-pipeline.oxoflow
```

Skipped rules are listed separately in the dry-run output.

## Full Example

See [`examples/conditional_workflow.oxoflow`](../../../examples/conditional_workflow.oxoflow) for a complete WGS/WES adaptive pipeline that demonstrates all `when` expression types.
