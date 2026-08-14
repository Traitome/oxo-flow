# `oxo-flow dry-run`

Simulate execution without running any commands. Shows the execution plan, rule order, and expanded shell commands — and, when a checkpoint exists, predicts the **actual incremental plan**: which rules would re-run, which cascade downstream, and which stay protected.

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
| `--workdir <DIR>` | — | Resolve relative paths against this directory (default: the workflow file's directory) |
| `--profile <NAME>` | — | Execution profile loaded from `profiles/<NAME>.toml` — the SAME merge semantics as `run` |
| `--skip-ref-build` | — | Skip automatic reference/index building (assume pre-built) — the preview otherwise lists required builds |
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
- Relative paths resolve against the workflow file's directory — the same
  place rules run from — so the report is accurate even when you invoke
  `dry-run` from another directory.
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

## Checkpoint-Aware Rerun Preview

dry-run loads `.oxo-flow/checkpoint.json` **read-only** and classifies every
rule in the execution set exactly the way `run` would — same config-impact
fingerprints, same input manifests, same DAG downstream closure — so the
preview matches what an actual `run` will do. Without a checkpoint the same
classification still runs against an empty state (every rule "never
completed", `when` conditions still honored):

```console
$ oxo-flow dry-run pipeline.oxoflow --samples NA12891
DAG: (dry-run) 12 rules would execute
Checkpoint: ./.oxo-flow/checkpoint.json (modified 2026-08-10 14:32)
  completed: 705 | will run: 12 | will skip: 0 | protected (outside this run): 693
  rerun cascade: trim_cohort_NA12891 → align_cohort_NA12891 → combine_gvcfs → genotype_gvcfs → vqsr_snps
  1. trim_cohort_NA12891  [run: input changed]
  2. align_cohort_NA12891  [rerun: downstream of trim_cohort_NA12891]
  ...
  12. vqsr_snps  [rerun: downstream of trim_cohort_NA12891]
```

The other 99 samples' 693 completed rules are outside this execution set —
their work stays untouched, counted as `protected`.

Per-rule status markers:

| Marker | Meaning |
|---|---|
| `[run: never completed]` | No checkpoint entry — it will execute |
| `[run: input changed]` | Input files differ from the manifest recorded at completion |
| `[run: config changed]` | Config value or rule definition changed since completion |
| `[run: outputs missing]` | Declared outputs no longer exist |
| `[rerun: downstream of X]` | Was completed, but sits downstream of a rule that will execute (the cascade) |
| `[rerun: upstream of X]` | A completed producer regenerates first because rule X needs its (tombstoned or missing) outputs — lazy cascade-up |
| `[skip: up to date]` | Checkpoint hit — work stays protected |
| `[skip: when condition false]` | The rule's `when` condition evaluates to false against the merged config — `run` skips it regardless of invalidation state |

The summary line answers the two questions that matter before a targeted
re-run: **how much will actually execute** (`will run`, including the
cascade) and **how much prior work survives** (`protected`). The cascade
line makes the infection chain visible — one sample's data change
reaching the queue-level rules is exactly the part users cannot see from
the DAG alone.

`--profile <NAME>` applies the SAME merge `run` uses (profile values fill in
config keys the workflow does not set), so a preview computed with the
profile matches what a profiled `run` would invalidate — and a preview
without it flags exactly the drift. When references are declared and their
build outputs are missing, the preview lists them ("References: N reference
build(s) would run"); pass `--skip-ref-build` to assume they are pre-built,
mirroring the run flag.

Temporary rules (`temporary = true`) are modeled exactly like `run` treats
them: a tombstoned rule whose outputs were deleted by design shows
`[skip: up to date]` while no dependent needs them, and flips to
`[rerun: upstream of X]` the moment a dependent will execute again —
regenerating the intermediate is part of the predicted plan, so the preview
and the actual run stay identical. See
[`run`](run.md#temporary-rules-temporary-true) for the execution-side
semantics.

The preview is strictly read-only and never mutates the checkpoint; it is
orthogonal to `run --rerun` (which forces execution) — the preview only
**predicts**, it changes no execution semantics. With `--json` the same
prediction is machine-readable:

```json
"checkpoint_preview": {
  "path": ".oxo-flow/checkpoint.json",
  "modified": "2026-08-10 14:32:00",
  "completed_total": 705,
  "summary": {"will_run": 12, "will_skip": 0, "protected_outside": 693},
  "plan": [
    {"name": "trim_cohort_NA12891", "status": "run-input-changed", "cascaded_from": null},
    {"name": "combine_gvcfs", "status": "run-cascaded", "cascaded_from": "trim_cohort_NA12891"}
  ],
  "cascade_chains": [["trim_cohort_NA12891", "combine_gvcfs", "genotype_gvcfs", "vqsr_snps"]]
}
```

Top-level fields: `"profile"` (the `--profile` name, when given) and
`"reference_builds"` (reference names whose build outputs are missing —
`--skip-ref-build` empties the list).

Status values: `run-never-completed`, `run-input-changed`,
`run-config-changed`, `run-outputs-missing`, `run-cascaded`,
`run-cascaded-upstream`, `skip`, `skip-when-condition`.

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

### Checkpoint re-entry in previews

Recorded re-entries whose checkpoint rule is still up-to-date replay into the
preview: the preview shows the same static plan a real run would execute
(round-1 instances appear as up-to-date skips). Checkpoint rules that may add
instances at runtime are listed under the `reentry` section of `--json`
(`recorded` + `possible`). See
[Workflow Format](../reference/workflow-format.md#checkpoint-re-entry).

## Output

```
oxo-flow 0.11.0 — Bioinformatics Pipeline Engine
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
- The checkpoint is loaded **read-only** too — dry-run never saves, baselines
  nothing, and invalidates nothing on disk
- The preview mirrors `run`'s incremental semantics; it is orthogonal to
  `run --rerun` (which forces execution) and `run`'s config-change
  invalidation — see [Run](run.md) for those
- Shell commands are shown in full — they may span multiple lines
- The environment type (conda, docker, etc.) is shown for each rule
- Thread and resource settings are displayed per rule
- Use dry-run to verify your workflow before committing compute resources
- When `--target` is specified, only the named rules and all rules they depend on
  (transitively) are shown — downstream rules are excluded
