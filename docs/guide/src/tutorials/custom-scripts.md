# Writing Custom Scripts

This tutorial bridges the gap between running standard shell commands and writing complex logic within oxo-flow. Often, a single shell command isn't enough to process bioinformatics data, and you'll need to embed Python, R, or Bash scripts into your workflow.

---

## The `script` Directive

Instead of a `shell` command, a rule can declare a `script` field that points to an external script file. The interpreter is auto-detected from the file extension, and the script runs inside the rule's declared environment.

---

## A Complete Example

Here is a minimal, runnable workflow that calls a Python script:

### 1. The workflow (`count.oxoflow`)

```toml
[workflow]
name = "count-reads"
version = "0.1.0"

[config]
sample = "SAMPLE_01"

[[rules]]
name = "count"
input = ["raw/{config.sample}.fastq.gz"]
output = ["counts/{config.sample}.count.txt"]
script = "scripts/count_reads.py {input[0]} --min-quality {params.min_q} -o {output[0]}"

[rules.params]
min_q = 20   # ← defines {params.min_q} used above

[rules.environment]
conda = "envs/py.yaml"
```

### 2. The script (`scripts/count_reads.py`)

```python
#!/usr/bin/env python3
"""Count reads above a minimum quality threshold."""
import argparse

parser = argparse.ArgumentParser()
parser.add_argument("fastq", help="input FASTQ file")
parser.add_argument("--min-quality", type=int, default=20)
parser.add_argument("-o", dest="out", required=True, help="output file")
args = parser.parse_args()

count = 0
with open(args.fastq) as f:
    for line in f:
        if line.startswith("@"):
            count += 1

with open(args.out, "w") as f:
    f.write(f"{count}\n")
print(f"counted {count} reads")
```

### 3. Run it

```bash
oxo-flow run count.oxoflow
# ✓ count (0.1s)
cat counts/SAMPLE_01.count.txt
```

---

## How `{params.*}` Works

`{params.<key>}` placeholders refer to the rule's `[rules.params]` table — key-value pairs scoped to a **single rule** (unlike `{config.*}`, which is global):

```toml
[rules.params]
min_q = 20            # integer → "20"
genome = "hg38"       # string
```

```bash
script = "analyze.py --min-quality {params.min_q} --genome {params.genome} {input[0]}"
# Expands to: analyze.py --min-quality 20 --genome hg38 raw/SAMPLE_01.fastq.gz
```

| Placeholder | Scope | Defined in |
|-------------|-------|-----------|
| `{params.<key>}` | One rule | `[rules.params]` |
| `{config.<key>}` | Whole workflow | `[config]` |
| `{input[N]}` / `{input}` | One rule | `input` array |
| `{output[N]}` / `{output}` | One rule | `output` array |
| `{threads}` / `{memory}` | One rule | `threads` / `memory` fields |

All placeholders are expanded **before** the script is launched, so the script itself never sees `{...}` syntax — it receives concrete values as command-line arguments.

---

## Passing Files to Scripts

Scripts receive file paths as ordinary command-line arguments. Use `{input[N]}` / `{output[N]}` placeholders in the `script` string to pass them:

```toml
script = "scripts/align_stats.py {input[0]} --bam-out {output[0]} --threads {threads}"
```

For scripts that prefer stdin/stdout streams, wrap them in `shell` instead:

```toml
shell = "samtools view -h {input[0]} | python scripts/filter.py > {output[0]}"
```

---

## Interpreter Detection

The interpreter is detected automatically, in this order:

1. **Explicit `interpreter` field** on the rule
2. **Custom `[workflow.interpreter_map]`** in the workflow metadata
3. **Built-in defaults** based on file extension:

| Extension | Interpreter |
|-----------|-------------|
| `.py` / `.py3` | `python` / `python3` |
| `.R` / `.r` | `Rscript` |
| `.sh` / `.bash` | `bash` |
| `.jl` | `julia` |
| `.pl` | `perl` |
| `.rb` | `ruby` |
| `.qmd` / `.Rmd` | `quarto render` |
| `.ipynb` | `jupyter nbconvert --execute` |
| `.smk` | `snakemake` |
| `.nextflow` | `nextflow run` |
| `.wdl` | `miniwdl run` |

4. **Shebang line** (if the file is executable)

---

## Combining `shell` and `script`

When both are declared, `shell` runs first, then `script`:

```toml
[[rules]]
name = "qc_and_report"
shell = "mkdir -p results/"
script = "scripts/qc_report.R {input[0]} results/"
```

Outputs are verified **after both** complete.

---

## Scripts vs. Shell Commands

| | `shell` | `script` |
|---|---|---|
| Best for | Short commands, one-liners, pipes | Multi-step logic, complex programs |
| Language | Any shell (bash, sh) | Any language with an interpreter |
| Interpreter | Shell itself | Auto-detected from extension, `interpreter` field, or shebang |
| Dependency tracking | Same | Same |
| Output verification | Same — declared outputs must exist after completion | Same |

---

## See Also

- [Script Execution (workflow format)](../reference/workflow-format.md#script-execution-script) — full reference for the `script` field
- [Custom Interpreters (`interpreter_map`)](../reference/workflow-format.md#custom-interpreters-interpreter_map) — mapping extensions to interpreters
- [Command Reference](../commands/run.md) — running workflows
