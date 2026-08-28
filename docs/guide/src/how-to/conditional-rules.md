# Conditional Rule Execution with `when`

This guide explains how to use the `when` field (WF-01) to skip rules based on configuration values.

## Problem

A single pipeline often needs to adapt to different inputs: WGS vs. WES sequencing modes, optional annotation steps, or analysis paths that depend on coverage thresholds.  Without conditional syntax, you either ship multiple pipeline files or rely on shell-level `if` statements that obscure the workflow structure.

## Solution: `when` expressions

Add a `when` field to any rule.  The expression is evaluated against your `[config]` section at execution time.  When `when` evaluates to `false`, the rule is **not removed from the DAG** — it stays in the graph but is **skipped at execution time** (reported as `Skipped` in the run output), so its outputs are not produced.

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
when = "config.run_qc"        # true if truthy: boolean true, non-zero number, or non-empty string other than "false"/"0"
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

Relative paths resolve against the **workflow root** (the workflow file's
directory) — the same root every other engine path uses — so the gate reads
the same no matter which directory you launch `oxo-flow` from. Absolute
paths pass through unchanged.

### Wildcard-scoped conditions (`wildcard.<key>`)

Conditions can reference the pair/group expansion context through the
`wildcard.` prefix — the same values that drive `{pair_id}`, `{experiment}`,
`{control}` and `{sample}` fan-out:

```toml
when = "wildcard.control != ''"     # only pairs with a control sample
when = "wildcard.control == ''"     # tumor-only pairs
when = "wildcard.group == 'case' && config.min_coverage >= 20"
```

Unlike `config.` conditions, `wildcard.` conditions are evaluated **per
expansion instance at DAG build time** — a non-matching instance never
enters the DAG (snakemake-style per-sample morphing), rather than being
planned and skipped at execution. A rule whose pair/group scope is
expressed only in `when` (no `{pair_id}`/`{sample}` in its paths) still
fans out per combo.

Notes:

- `control`/`normal`/`tumor` are always present — a pair without a control
  expands them to the empty string, so `wildcard.control != ''` exactly
  separates paired from tumor-only pairs.
- Optional wildcards (`experiment_type`, `tumor_type`) expand to the empty
  string when unset.
- `config.` conditions keep the execution-time flow (planned, then skipped
  when false); the two scopes compose with `&&`.
- `{key}` placeholders inside `when` are not substituted — use the
  `wildcard.<key>` form.

```toml
[workflow]
# samplesheet.csv: one row with Normal fastqs, one without
pairs_file = "samplesheet.csv"

[[rules]]
name   = "paired_mapping"
when   = "wildcard.control != ''"
input  = ["reads/{pair_id}.fq.gz"]
output = ["bam/{pair_id}.paired.bam"]
shell  = "mapper --paired {input[0]} -o {output[0]}"

[[rules]]
name   = "tumor_only_mapping"
when   = "wildcard.control == ''"
input  = ["reads/{pair_id}.fq.gz"]
output = ["bam/{pair_id}.tumor_only.bam"]
shell  = "mapper --tumor-only {input[0]} -o {output[0]}"
```

### Length checks (`len(...)`)

```toml
when = "len(config.gene_sets) > 0"   # list is non-empty
when = "len(config.samples) >= 2"    # at least two samples
when = "len(config.gene_sets)"       # truthy shorthand for len > 0
```

`len()` counts array items, string characters, or table keys. A missing
config key has length 0, so `len(config.x) == 0` covers both an empty list
and an undefined key. Note that a bare `config.gene_sets` is **true even
for an empty array** (the value exists) — use `len(...) > 0` when emptiness
must gate the rule.

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

Use `oxo-flow dry-run` to preview the execution plan:

```bash
oxo-flow dry-run my-pipeline.oxoflow
```

Note that dry-run does **not** evaluate `when` conditions — all rules appear in
the plan. `when` is evaluated at execution time: rules whose condition is false
are skipped and reported as `Skipped` in the run output.

## Full Example

See [`examples/gallery/11_conditional_workflow.oxoflow`](https://github.com/Traitome/oxo-flow/blob/main/examples/gallery/11_conditional_workflow.oxoflow) for a complete WGS/WES adaptive pipeline that demonstrates most `when` expression types (truthiness, string comparisons, `&&`, `>=`).
