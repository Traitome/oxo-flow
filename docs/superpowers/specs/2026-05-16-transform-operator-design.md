# Transform Operator Design

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a unified `transform` operator that simplifies scatter-gather patterns into a single rule declaration.

**Architecture:** Extend the Rule struct with a `transform` field containing split/map/combine configurations. Internally expand to virtual rules during DAG construction.

**Tech Stack:** Rust, TOML parsing, existing scatter infrastructure

---

## Problem Statement

Current scatter-gather implementation requires **2+ rules**:

```toml
[[rules]]
name = "scatter_process"
scatter = { variable = "chr", values_from = "config.chromosomes", gather = "gather_results" }
shell = "process {chr}"

[[rules]]
name = "gather_results"
shell = "merge {input}"
```

This pattern (split → parallel process → combine) is extremely common in:
- dplyr: `group_by() %>% summarize()`
- pandas: `df.groupby().apply()`
- Spark: `map-reduce`
- Hadoop: distributed processing

Users want a **single-rule declarative syntax** for this ubiquitous pattern.

---

## Proposed Syntax

```toml
[[rules]]
name = "chromosome_processing"
input = ["aligned/sample.bam"]
output = ["variants/sample.vcf.gz"]
threads = 8

transform = {
    split = { by = "chr", values_from = "config.chromosomes" }
    map = "gatk HaplotypeCaller -R {config.reference} -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz"
    combine = "gatk GatherVcfs .oxo-flow/chunks/*.g.vcf.gz -O {output}"
    cleanup = true
}
```

---

## Three Processing Modes

### Mode A: Split → Map → Combine

Full scatter-gather with merge:

```toml
transform = {
    split = { by = "chr", values_from = "config.chromosomes" }
    map = "process {chr}"
    combine = "merge all chunks"
}
# Output: single merged file
```

### Mode B: Split → Map → Aggregate

Statistical aggregation per group:

```toml
transform = {
    split = { by = "sample", values_from = "config.samples" }
    map = "calculate_stats {sample}"
    combine = { aggregate = true, method = "concat", header = "sample,stat" }
}
# Output: summary file (CSV/JSON)
```

### Mode C: Split → Map (No Combine)

Parallel processing without merging:

```toml
transform = {
    split = { by = "sample", values_from = "config.samples" }
    map = "qc_fastqc {sample}"
    # No combine field
}
# Output: individual files per split
```

---

## Data Flow

```
input.bam
    │
    ▼ split (generates N parallel tasks)
    ├── map(chr=chr1) → temp/chr1.vcf.gz
    ├── map(chr=chr2) → temp/chr2.vcf.gz
    └── map(chr=chr3) → temp/chr3.vcf.gz
    │
    ▼ combine (optional)
output.vcf.gz (merged result)
```

---

## Field Specifications

### TransformConfig

| Field | Required | Description |
|-------|----------|-------------|
| `split` | Yes | How to partition the data |
| `map` | Yes | Processing command for each partition |
| `combine` | No | How to merge results |
| `cleanup` | No | Delete temporary chunk files after combine (default: false) |

### SplitConfig

| Field | Description |
|-------|-------------|
| `by` | Partition variable name (e.g., "chr", "sample", "chunk") |
| `values` | Direct list of values |
| `values_from` | Reference to config variable (e.g., "config.chromosomes") |
| `n` | Split into N chunks (numeric split) |
| `glob` | Split by file glob pattern |

### CombineConfig

Two forms supported:

**String form (shell command):**
```toml
combine = "gatk GatherVcfs {chunks} -O {output}"
```

**Object form (aggregation):**
```toml
combine = {
    aggregate = true,
    method = "concat",      # or "json_merge"
    header = "sample,stat"  # optional header line
}
```

---

## Built-in Variables

| Variable | Description |
|----------|-------------|
| `{split_var}` or `{by}` | Current partition value (e.g., `{chr}` = "chr1") |
| `{chunks}` | All chunk output files (space-separated list) |
| `{input}` | Original input file(s) |
| `{output}` | Final output file |
| `{n}` | Total number of chunks |
| `{config.xxx}` | Config variable reference |

---

## Struct Definitions

```rust
pub struct Rule {
    // ... existing fields ...

    /// Transform operator: scatter-gather in one rule
    pub transform: Option<TransformConfig>,
}

pub struct TransformConfig {
    pub split: SplitConfig,
    pub map: String,
    pub combine: Option<CombineConfig>,
    pub cleanup: Option<bool>,
}

pub struct SplitConfig {
    pub by: String,
    pub values: Option<Vec<String>>,
    pub values_from: Option<String>,
    pub n: Option<String>,
    pub glob: Option<String>,
}

pub struct CombineConfig {
    pub shell: Option<String>,
    pub aggregate: Option<bool>,
    pub method: Option<String>,
    pub header: Option<String>,
}
```

---

## Complete Examples

### Example 1: WGS Chromosome Processing

```toml
[config]
reference = "/data/ref/GRCh38.fa"
chromosomes = ["chr1", "chr2", "chr3", "chr4", "chr5", "chrX"]

[[rules]]
name = "variant_calling"
input = ["aligned/sample.bam"]
output = ["variants/sample.vcf.gz"]
threads = 8

transform = {
    split = { by = "chr", values_from = "config.chromosomes" }
    map = "gatk HaplotypeCaller -R {config.reference} -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz -ERC GVCF"
    combine = "gatk GatherVcfs .oxo-flow/chunks/*.g.vcf.gz -O {output}"
    cleanup = true
}
```

### Example 2: Batch Sample QC (No Combine)

```toml
[config]
samples = ["S001", "S002", "S003"]

[[rules]]
name = "fastqc_all"
input = ["raw/{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]

transform = {
    split = { by = "sample", values_from = "config.samples" }
    map = "fastqc {input} -o qc/"
}
```

### Example 3: Coverage Statistics Aggregation

```toml
[[rules]]
name = "coverage_stats"
input = ["aligned/*.bam"]
output = ["reports/coverage_summary.tsv"]

transform = {
    split = { by = "bam", glob = "aligned/*.bam" }
    map = "samtools depth {bam} | awk '{sum+=$3} END {print \"{bam}\",sum/NR}' > .oxo-flow/chunks/{bam}.stats"
    combine = { aggregate = true, method = "concat", header = "sample,coverage" }
}
```

### Example 4: Chunk-Based Split

```toml
[config]
chunks = 10

[[rules]]
name = "parallel_process"
input = ["data/large_file.txt"]
output = ["results/processed.txt"]

transform = {
    split = { by = "chunk", n = "config.chunks" }
    map = "process_chunk {input} {chunk} {n}"
    combine = "cat .oxo-flow/chunks/*.txt > {output}"
    cleanup = true
}
```

---

## Implementation Strategy

### Phase 1: Core Struct Extension

1. Add `TransformConfig`, `SplitConfig`, `CombineConfig` structs to `rule.rs`
2. Add `transform: Option<TransformConfig>` field to `Rule`
3. Update TOML parsing in `config.rs`

### Phase 2: Expansion Logic

1. Implement `expand_transform()` function in `config.rs`
2. Generate virtual rules: `{rule_name}_{split_value}` for map tasks
3. Generate implicit combine rule if `combine` is specified
4. Build DAG dependencies: map rules → combine rule

### Phase 3: Execution Integration

1. Reuse existing `scatter` parallel execution infrastructure
2. Handle `cleanup` flag: delete `.oxo-flow/chunks/` after combine
3. Support `{chunks}` variable expansion in combine shell

### Phase 4: Validation & Linting

1. Add validation for transform configuration
2. Add lint warning W018: "transform with cleanup=true may delete intermediate outputs needed for debugging"
3. Ensure E003 wildcard exemption for `{split_var}`

### Phase 5: Documentation

1. Update scatter-gather gallery documentation
2. Add transform operator reference in workflow-format.md
3. Add examples showing all three modes

---

## Compatibility

### With Existing scatter Field

Both syntaxes coexist:

| Approach | Use Case |
|----------|----------|
| `scatter + gather rule` (existing) | Complex gather logic, multi-stage processing |
| `transform` (new) | Simple scatter-gather, one-line declarative |

### With Other Features

| Feature | Compatibility |
|---------|---------------|
| `wildcards` | `{split_var}` auto-expanded |
| `execution_group` | transform can be in group |
| `when` condition | Supported on transform rule |
| `retry`/`timeout` | Applies to map and combine |
| `environment` | Shared across map/combine |

---

## Success Criteria

1. User can express scatter-gather in **single rule**
2. All three modes (combine/aggregate/no-combine) work correctly
3. Existing scatter functionality unchanged
4. `{split_var}` exempt from E003 validation
5. Documentation updated with examples
6. All unit tests pass

---

## Open Questions

1. Should `cleanup` default to `true` or `false`? (Recommendation: `false` for debugging safety)
2. Should `combine.shell` and `combine.aggregate` be mutually exclusive? (Yes, validated)
3. How to handle partial combine failures when some map tasks succeed? (Retry combine, or fail entire rule)

---

## References

- Current scatter implementation: `crates/oxo-flow-core/src/rule.rs:ScatterConfig`
- Wildcard expansion: `crates/oxo-flow-core/src/config.rs:expand_wildcards()`
- DAG construction: `crates/oxo-flow-core/src/dag.rs`