# Wildcards

Wildcards are the mechanism by which oxo-flow enables dynamic, pattern-based pipeline definitions. Instead of writing a separate rule for every sample, you define a single rule with `{wildcard}` placeholders that oxo-flow expands into multiple concrete tasks.

---

## Pattern Syntax

Wildcards are denoted by curly braces `{}` containing a name (e.g., `{sample}`).

```toml
[workflow]
name = "align-pipeline"
sample_pattern = "raw/{sample}.fastq.gz"

[[rules]]
name = "align"
input = ["raw/{sample}.fastq.gz"]
output = ["aligned/{sample}.bam"]
shell = "bwa mem ref.fa {input} > {output}"
```

In this example, `{sample}` is a wildcard. When oxo-flow loads this workflow, `sample_pattern` scans the filesystem for matching files, extracts the matching portion as the `sample` value, and generates one `align` task for each sample found.

The same `{...}` syntax powers **three different mechanisms** with very different effects — [rule-level fan-out, input-level fan-in](#fan-out-vs-fan-in), and plain value substitution. Reading that section before writing your first multi-sample workflow will save you from the most common wildcard mistake.

---

## Expansion Sources

oxo-flow determines the values for wildcards from three primary sources:

### 1. File Discovery (Automatic)

Set [`[workflow] sample_pattern`](./workflow-format.md#auto-discovery-with-sample_pattern) to scan the filesystem at load time. oxo-flow collects the discovered samples into an `auto-discovered` sample group, and rules referencing `{sample}` expand once per discovered sample.

- **Example**: If `raw/` contains `S1.fastq.gz` and `S2.fastq.gz`.
- **Pattern**: `sample_pattern = "raw/{sample}.fastq.gz"`
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

## Fan-out vs Fan-in

The most important concept when writing multi-sample workflows: **where you put a wildcard decides whether the engine clones the rule or only fills its input list.**

### Rule-level fan-out: `{sample}` clones the rule

If `{sample}` (or `{group}`, or a pair wildcard such as `{experiment}`) appears anywhere in a rule's `input`, `output`, or `shell`, oxo-flow **clones the entire rule once per value**; inside each clone, the value also substitutes into `log`, `script`, and the hook fields (`pre_exec` / `on_success` / `on_failure`):

```toml
[[rules]]
name = "fastqc"
input = ["raw/{sample}.fastq.gz"]
output = ["qc/{sample}_fastqc.html"]
```

With samples `S1` and `S2`, this becomes **two tasks**: `fastqc_S1` (processing `S1`) and `fastqc_S2` (processing `S2`). This is what you want for per-sample steps — one definition, N tasks.

`script` and the hook fields substitute per clone but never start a fan-out by themselves: a wildcard that appears *only* there keeps the rule as a single task (and `${sample}`-style shell-variable spellings inside `script` are never treated as wildcards).

### Input-level fan-in: `expand_inputs` fills the input list

A gather step such as `CombineGVCFs` is the opposite: it must run **once**, with **all** per-sample files in its input list. Writing

```toml
[[rules]]
name = "combine_gvcfs"
input = ["variants/{sample}.g.vcf.gz"]   # WRONG — see below
output = ["variants/cohort.g.vcf.gz"]
```

does not do that. Because `{sample}` appears in the rule's paths, the rule itself is cloned once per sample: three separate `combine_gvcfs_*` tasks, each combining a single GVCF, all writing to the same `cohort.g.vcf.gz`. Instead, keep the rule un-cloned and let [`expand_inputs`](#gathering-inputs-with-expand_inputs) fill the input list:

```toml
[[rules]]
name = "combine_gvcfs"
input = []
expand_inputs = [
  { pattern = "variants/{sample}.g.vcf.gz", variables = { sample = "config.samples_list" } }
]
output = ["variants/cohort.g.vcf.gz"]
```

`combine_gvcfs` stays **one task** whose input list expands to `variants/S1.g.vcf.gz`, `variants/S2.g.vcf.gz`, … — and because expansion happens before the DAG is built, those inputs link up with the per-sample rules that produced them.

### Plain substitution: `{config.x}` replaces text only

`{config.x}` in any path is replaced by the value of `x` from `[config]` — a single text substitution. It never clones rules and never splits lists; `{config.samples_list}` in a path would literally insert the comma-joined sample list into one path. Use `expand_inputs` when you need to iterate over a list.

| Mechanism | Where | Effect | Use for |
|---|---|---|---|
| `{sample}` / `{group}` / pair wildcards | in `input`, `output`, or `shell` | clones the rule per value (fan-out); per-clone values substitute into `log`, `script`, and hooks | per-sample steps |
| `expand_inputs` | rule field | appends expanded paths to `input` (fan-in) | gather / combine steps |
| `{config.x}` | any path or shell | single-value text substitution | reusing config values |

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

If a pattern contains multiple wildcards with explicit value lists, oxo-flow generates the **Cartesian product** of all values — the semantics used by the [`expand_inputs`](#gathering-inputs-with-expand_inputs) field.

- **Pattern**: `results/{sample}_R{read}.txt`
- **Values**: `sample=["A", "B"]`, `read=["1", "2"]`
- **Tasks**:
    - `results/A_R1.txt`
    - `results/A_R2.txt`
    - `results/B_R1.txt`
    - `results/B_R2.txt`

---

## Gathering Inputs with `expand_inputs`

`expand_inputs` is a rule-level list of expansion specs. Each spec binds the `{variables}` in its `pattern` to values and appends the resulting paths to the rule's `input` list (after any static inputs). The rule itself is **not** cloned — this is the fan-in mechanism described above.

### Variable bindings

`variables` maps each placeholder name to one of three forms:

| Form | Example | Resolves to |
|---|---|---|
| Config reference | `{ sample = "config.samples_list" }` | the values of `samples_list` in `[config]` — a TOML array, or a comma-joined string that is split |
| Inline list | `{ chr = "[\"1\", \"2\"]" }` | the literal values `1`, `2` |
| Single literal | `{ ref = "GRCh38" }` | one value: `GRCh38` |

### `config.samples_list` is engine-injected

When the workflow defines `sample_pattern`, `[[pairs]]`, or `[[sample_groups]]`, oxo-flow merges every sample it finds into `config.samples_list` (a sorted, deduplicated, comma-joined string) before expansion. Referencing it in `expand_inputs` means the gather step always collects exactly the outputs of the per-sample rules — **no second copy of the sample list to maintain**:

```toml
[[sample_groups]]
name = "cohort"
samples = ["NA12878", "NA12879", "NA12880"]

[[rules]]
name = "combine_gvcfs"
input = []
expand_inputs = [
  { pattern = "variants/{sample}.g.vcf.gz", variables = { sample = "config.samples_list" } }
]
output = ["variants/cohort.g.vcf.gz"]
```

Expansion happens in the same pass as the per-sample rule cloning — first the per-sample rules are materialized, then `expand_inputs` resolves against the injected `samples_list` — so the expanded inputs participate in normal input→output dependency inference and the gather rule gains edges from every producer. (For a complete end-to-end example, see the [WGS germline gallery](../gallery/wgs-germline.md).)

Full field reference: [Expand Inputs](./workflow-format.md#expand-inputs) in the workflow format reference.

---

## Wildcard Constraints (Regex)

You can restrict what a wildcard can match using regular expressions in the top-level `[wildcard_constraints]` section. This is useful for preventing wildcards from matching across directory boundaries or ensuring specific naming conventions.

```toml
[workflow]
name = "constrained-pipeline"

[wildcard_constraints]
sample = "[A-Z0-9]+"
read = "[12]"
```

If a value discovered from the filesystem does not match its constraint, it is ignored.

### Safe Default Character Set

Because wildcard values are substituted into shell command strings, wildcards **without** a declared constraint may only carry values matching `[A-Za-z0-9._/-]+` (letters, digits, dot, underscore, dash, path separator). Values outside this set fail validation before any shell runs. Direct command substitution (`$(` and backticks) is rejected unconditionally — including for constrained wildcards.

If your data legitimately needs other characters, declare an explicit `[wildcard_constraints]` entry for that wildcard, or export `OXO_FLOW_UNSAFE_WILDCARDS=1` to relax the character-set layer for the process (a one-time warning is logged; the substitution floor remains enforced).

---

## Rule Name Expansion

When a rule is expanded from wildcards, its unique name in the DAG is modified to include the wildcard values to avoid collisions:

- **Sample/group wildcards**: `align` → `align_control_S1` — one expanded rule per (group, sample) combination
- **Group-less declared samples**: a flat sample list with no `[[sample_groups]]` collapses into a single implicit group named `samples`, so `align` → `align_samples_S1`. The same applies to `--samples` overrides of a workflow that declares no groups
- **Pairs**: `mutect2` → `mutect2_CASE_001`

---

## Advanced: The `transform` Operator

For complex scatter-gather patterns where you need to split data, process chunks in parallel, and then merge them, use the [`transform`](./workflow-format.md#transform-unified-scatter-gather-operator) operator. It provides more control over how wildcards are generated and aggregated than basic pattern matching.

---

## Best Practices

1. **Be specific**: Use suffixes like `.fastq.gz` instead of just `{sample}.*` to avoid matching unintended files.
2. **Use constraints**: Protect your pipeline from malformed filenames by using `wildcard_constraints`.
3. **Check with Dry-run**: Always run `oxo-flow dry-run` to see how your wildcards will expand before starting a large execution.
