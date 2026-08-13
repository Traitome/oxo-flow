# `oxo-flow dry-run`

Simulate execution without running any commands. Shows the execution plan, rule order, and expanded shell commands.

---

## Usage

```
oxo-flow dry-run [OPTIONS] [WORKFLOW]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file. **Optional** — if not specified, auto-discovery searches for: (1) `main.oxoflow` in current directory, (2) alphabetically first `*.oxoflow` file in current directory. |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--target` | `-t` | Run only specific target rules and their dependencies (repeatable, prefix matching) |
| `--samples <LIST>` | — | Preview only a subset of samples: `first:N` (pilot), explicit names, or `ready` (samples whose entry inputs are complete; repeatable, comma-separated) |
| `--ai` | — | Enable AI-powered analysis of the workflow |
| `--ai-max-retries <N>` | — | Maximum AI analysis rounds (overrides `[ai]` config) |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Scientific Preflight

Every dry-run runs deterministic, evidence-backed scientific checks on the
workflow design and prints findings (e.g. `SCI-VQSR-COHORT`,
`SCI-MUTECT2-TUMOR-ONLY`). With `--ai`, the findings are also passed to the
model for a plain-language explanation.

```bash
# A 2-sample pilot of a VQSR workflow fails for scientific reasons —
# the preflight says so before any compute is spent
oxo-flow dry-run pipeline.oxoflow --samples first:2
```

## Sample Readiness

Every dry-run on a sample-scoped workflow reports **which samples have
complete entry inputs** and which are still waiting for data — designed for
incremental data arrival, when a sequencing center delivers fastq files in
batches:

```console
$ oxo-flow dry-run pipeline.oxoflow
DAG: (dry-run) 100 rules would execute
Sample readiness: 87/100 complete, 13 waiting
    ⏳ NA12891 (missing: data/NA12891_R2.fastq.gz)
    ⏳ NA12892 (missing: data/NA12892_R2.fastq.gz)
    … and 11 more waiting
```

Rules are judged per sample:

- A sample is **ready** when every external input belonging to it exists.
  External inputs are rule inputs (after wildcard and `{config.x}`
  expansion) that the workflow itself does not produce; intermediate
  products are the DAG's job, so they are never checked.
- `optional = true` rules do not block readiness — the executor skips them
  when their inputs are absent.
- Missing files that belong to no specific sample (shared references) are
  reported as workflow-level inputs that block every sample.
- `--samples ready` previews only the ready samples, but the readiness
  section still covers the whole cohort so you can see what was left out.

With `--json` the same report is machine-readable:

```json
"samples": {
  "total": 100, "ready": 87, "waiting_count": 13,
  "ready_names": ["NA12878", "…"],
  "waiting": [{"name": "NA12891", "missing": ["data/NA12891_R2.fastq.gz"]}],
  "missing_global": []
}
```

See [`run`](run.md#incremental-data-arrival-samples-ready) for the matching
`--samples ready` execution mode.

## Examples

### Preview with auto-discovery

```bash
# Auto-discover workflow in current directory
oxo-flow dry-run
```

### Preview a specific workflow

```bash
oxo-flow dry-run pipeline.oxoflow
```

### Preview a specific target rule and its dependencies

```bash
oxo-flow dry-run pipeline.oxoflow -t align
```

### Preview multiple target rules

```bash
oxo-flow dry-run pipeline.oxoflow -t align -t sort_bam
```

### With verbose output

```bash
oxo-flow dry-run pipeline.oxoflow -v
```

---

## Output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
DAG: (dry-run) 3 rules would execute
  1. generate_data
     threads=1
     outputs: ["data/raw.csv"]
     command: mkdir -p data
echo 'id,name,value' > data/raw.csv

  2. transform
     threads=1
     outputs: ["data/filtered.csv"]
     command: head -1 data/raw.csv > data/filtered.csv
awk -F',' 'NR>1 && $3 > 500' data/raw.csv >> data/filtered.csv

     input ✗: data/raw.csv
  3. summarize
     threads=2
     memory=4G
     env=conda
     outputs: ["results/summary.txt"]
     command: mkdir -p results
echo "Filtered records: $total" > results/summary.txt

Summary: 3 rules, total 4 threads declared, max 2 threads/rule
         1 rule(s) with memory requirements

To execute:  oxo-flow run pipeline.oxoflow -j 1
```

---

## Notes

- The workflow file is optional; if not specified, auto-discovery searches for `main.oxoflow` first, then any `*.oxoflow` file alphabetically
- If no `.oxoflow` file is found, an error message suggests running `oxo-flow init` to create one
- No shell commands are executed — the dry-run is read-only
- Shell commands are shown in full — they may span multiple lines
- The environment type (conda, docker, etc.) is shown for each rule
- Thread and resource settings are displayed per rule
- Use dry-run to verify your workflow before committing compute resources
- When `--target` is specified, only the named rules and all rules they depend on
  (transitively) are shown — downstream rules are excluded
