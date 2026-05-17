# Wildcards

Wildcards are the mechanism by which oxo-flow enables dynamic, pattern-based pipeline definitions. Instead of writing a separate rule for every sample, you define a single rule with `{wildcard}` placeholders that oxo-flow expands into multiple concrete tasks.

---

## Pattern Syntax

Wildcards are denoted by curly braces `{}` containing a name (e.g., `{sample}`).

```toml
[[rules]]
name = "align"
input = ["raw/{sample}.fastq.gz"]
output = ["aligned/{sample}.bam"]
shell = "bwa mem ref.fa {input} > {output}"
```

In this example, `{sample}` is a wildcard. When oxo-flow runs this workflow, it scans for files matching `raw/*.fastq.gz`, extracts the matching portion as the `sample` value, and generates one `align` task for each sample found.

---

## Expansion Sources

oxo-flow determines the values for wildcards from three primary sources:

### 1. File Discovery (Automatic)

When a rule's `input` contains wildcards, oxo-flow scans the filesystem to find all files that match the pattern.

- **Example**: If `raw/` contains `S1.fastq.gz` and `S2.fastq.gz`.
- **Pattern**: `raw/{sample}.fastq.gz`
- **Values**: `sample` becomes `["S1", "S2"]`.

### 2. Experiment-Control Pairs (`[[pairs]]`)

For somatic variant calling and comparative analysis, you can define pairs in the `[[pairs]]` section.

```toml
[[pairs]]
pair_id = "CASE_001"
experiment = "TUMOR_01"
control = "NORMAL_01"
```

Any rule referencing `{pair_id}`, `{experiment}`, or `{control}` is expanded once per defined pair.

### 3. Sample Groups (`[[sample_groups]]`)

For cohort studies, define groups of samples in `[[sample_groups]]`.

```toml
[[sample_groups]]
name = "control"
samples = ["C1", "C2", "C3"]
```

Any rule referencing `{group}` or `{sample}` is expanded for every (group, sample) combination.

---

## Built-in Placeholders

While they use the same `{}` syntax, built-in placeholders are NOT wildcards; they are special variables managed by the engine:

| Placeholder | Expands to |
|---|---|
| `{input}` | Space-separated list of all input files |
| `{input[N]}` | The Nth input file (0-indexed) |
| `{input.name}` | The input file named `name` from `named_input` |
| `{output}` | Space-separated list of all output files |
| `{output[N]}` | The Nth output file (0-indexed) |
| `{output.name}` | The output file named `name` from `named_output` |
| `{threads}` | CPU thread count assigned to the task |
| `{memory}` | Memory allocation assigned to the task |
| `{config.X}` | Value of variable `X` from the `[config]` section |

---

## Multiple Wildcards & Cartesian Product

If a pattern contains multiple wildcards, oxo-flow generates the **Cartesian product** of all possible values.

- **Pattern**: `results/{sample}_R{read}.txt`
- **Values**: `sample=["A", "B"]`, `read=["1", "2"]`
- **Tasks**:
    - `results/A_R1.txt`
    - `results/A_R2.txt`
    - `results/B_R1.txt`
    - `results/B_R2.txt`

---

## Wildcard Constraints (Regex)

You can restrict what a wildcard can match using regular expressions in the `[workflow]` section. This is useful for preventing wildcards from matching across directory boundaries or ensuring specific naming conventions.

```toml
[workflow]
name = "constrained-pipeline"

[workflow.wildcard_constraints]
sample = "[A-Z0-9]+"
read = "[12]"
```

If a value discovered from the filesystem does not match its constraint, it is ignored.

---

## Rule Name Expansion

When a rule is expanded from wildcards, its unique name in the DAG is modified to include the wildcard values to avoid collisions:

- **Basic Wildcard**: `align` → `align_S1`, `align_S2`
- **Pairs**: `mutect2` → `mutect2_CASE_001`
- **Groups**: `fastqc` → `fastqc_control_C1`

---

## Advanced: The `transform` Operator

For complex scatter-gather patterns where you need to split data, process chunks in parallel, and then merge them, use the [`transform`](./workflow-format.md#transform-unified-scatter-gather-operator) operator. It provides more control over how wildcards are generated and aggregated than basic pattern matching.

---

## Best Practices

1. **Be specific**: Use suffixes like `.fastq.gz` instead of just `{sample}.*` to avoid matching unintended files.
2. **Use constraints**: Protect your pipeline from malformed filenames by using `wildcard_constraints`.
3. **Check with Dry-run**: Always run `oxo-flow dry-run` to see how your wildcards will expand before starting a large execution.
