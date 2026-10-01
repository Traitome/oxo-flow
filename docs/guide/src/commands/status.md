# `oxo-flow status`

Show execution status from a checkpoint file. Displays which rules completed
successfully, which failed, and — with `--timing` — per-rule wall-clock times
and total runtime.

---

## Usage

```
oxo-flow status [OPTIONS] [CHECKPOINT]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[CHECKPOINT]` | Path to a checkpoint JSON file. Defaults to `.oxo-flow/checkpoint.json` in the current directory |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--workdir <DIR>` | | Resolve the default checkpoint under this run directory (`<DIR>/.oxo-flow/checkpoint.json`) — the same semantics as `run --workdir`; ignored when an explicit CHECKPOINT path is given |
| `--timing` | | Show per-rule wall-clock times, sampled peak RSS, and total runtime, slowest first |
| `--limit <LIMIT>` | `-n` | Maximum number of rules in the `--timing` view (default: 10; requires `--timing`) |
| `--json` | | Output machine-readable JSON to stdout |
| `--verbose` | `-v` | Enable debug-level logging (global) |
| `--quiet` | — | Suppress informational output, including the version banner (global) |
| `--no-color` | — | Disable colored output, also respects the `NO_COLOR` environment variable (global) |

---

## Examples

```bash
# Status from the default checkpoint in the current directory
oxo-flow status

# Status from an explicit checkpoint
oxo-flow status .oxo-flow/checkpoint.json

# Status for a run that executed with --workdir somewhere else
oxo-flow status --workdir runs/cohort-2026-09

# Per-rule timings, slowest 5 rules
oxo-flow status --timing -n 5

# Machine-readable output including timings
oxo-flow status --timing --json
```

---

## Output

```
oxo-flow v0.21.1 — Rust-native bioinformatics pipeline engine
Status: Status for checkpoint: .oxo-flow/checkpoint.json
  Completed: 3
  Failed:    1
  Skipped (when-gated off): 2

Completed rules:
  ✓ align
  ✓ sort_bam
  ✓ trim_reads

Failed rules:
  ✗ mark_duplicates

Skipped (when condition false):
  ⊘ filter_cohort_S2
  ⊘ filter_cohort_S1
```

### When-gated-off rules

A rule whose `when` condition evaluated to false is a **skip**, not a
failure. The executor never writes such a rule into `failed_rules` (the
verdict prunes any stale entry), and the display also reconciles legacy
checkpoints: an entry in `failed_rules` whose recorded verdict in
`when_verdicts` is `false` is reported as gated-off instead of failed —
under `Skipped (when condition false)` in the text view and in the
`skipped_by_when` JSON key instead of `failed` (issue #690). The same
reconciliation applies to the resume banner: rules whose recorded verdict is
`false` are counted as skips there, and the gate is re-judged on the resume
(see [Checkpointing and Resuming](run.md#checkpointing-and-resuming)).

```
  Completed: 2
  Failed:    0
  Skipped (when-gated off): 1

Completed rules:
  ✓ trim
  ✓ align

Skipped (when condition false):
  ⊘ qc_report — condition evaluated to false
```

```json
"failed": [],
"skipped_by_when": ["qc_report"]
```

### Staleness reasons

When the checkpoint's workflow file is still present and parseable, `status`
classifies every completed rule exactly the way `run` would (issue #432) and
explains **why each rule would re-run right now** — output deleted, input
manifest mismatch, config change, or a cascade from an upstream rule. The
section (and the `staleness` JSON key) appears only when **at least one
completed rule would re-run** — an all-up-to-date checkpoint shows just the
plain completed/failed listing:

```
Staleness reasons (as of now):
  why each completed rule would re-run under the current workflow+config
  ✓ align — input changed (recorded input manifest (paths/size/mtime) no longer matches disk)
  ✓ trim_reads — up to date
```

Rules shown as up to date would keep their checkpoint credit on the next
`run`. The same map is available in `--json` output:

```json
"staleness": {
  "align": {
    "status": "input changed",
    "detail": "recorded input manifest (paths/size/mtime) no longer matches disk"
  }
}
```

The classification is read-only: it never writes to the checkpoint or
records baselines on disk. When the workflow file is missing or no longer
parses, the section degrades to the plain completed/failed listing.

With `--timing`, the rule list is replaced by a wall-time view (slowest
first):

```
  Completed: 3
  Failed:    0

Rule timings: (top 3, total 45.2s)
  ✓ align (30.1s)  peak 29000/32000 MiB ⚠
  ✓ sort_bam (12.3s)  peak 8100/32000 MiB
  ✓ trim_reads (2.8s)
```

With `--json`, output goes to stdout:

```json
{
  "command": "status",
  "checkpoint": ".oxo-flow/checkpoint.json",
  "workflow": "/abs/path/pipeline.oxoflow",
  "completed": ["align", "sort_bam", "trim_reads"],
  "failed": [],
  "skipped_by_when": ["qc_report"],
  "staleness": {
    "align": {
      "status": "input changed",
      "detail": "recorded input manifest (paths/size/mtime) no longer matches disk"
    }
  },
  "timings": {
    "align": 30.1,
    "sort_bam": 12.3,
    "trim_reads": 2.8
  },
  "total_time_secs": 45.2,
  "memory": {
    "align": { "max_memory_mb": 29000, "memory_limit_mb": 32000 },
    "sort_bam": { "max_memory_mb": 8100, "memory_limit_mb": 32000 }
  }
}
```

`timings`, `total_time_secs`, and `memory` are only present with
`--timing`; `memory` only lists rules with a sampled peak-RSS
measurement. `staleness` appears whenever the workflow file can be
loaded for classification **and** at least one completed rule would
re-run (see [Staleness reasons](#staleness-reasons)). `skipped_by_when`
appears only when at least one `failed_rules` entry is actually
when-gated-off; the verdicts themselves live in the checkpoint file's
`when_verdicts` map and are not duplicated into the JSON.

---

## Checkpoint File Format

The checkpoint file is JSON with the following structure:

```json
{
  "completed_rules": ["trim_reads", "align", "sort_bam"],
  "failed_rules": ["mark_duplicates"],
  "when_verdicts": {
    "filter_cohort_S1": true,
    "filter_cohort_S2": false
  },
  "benchmarks": {
    "trim_reads": {
      "rule": "trim_reads",
      "wall_time_secs": 42.5,
      "max_memory_mb": 1024,
      "memory_limit_mb": 4096,
      "cpu_seconds": 38.2,
      "retries": 0
    }
  },
  "workflow_path": "pipeline.oxoflow",
  "config_snapshot": {
    "min_quality": "20"
  },
  "rule_fingerprints": {
    "trim_reads": "sha256:1a2b3c…",
    "align": "sha256:4d5e6f…"
  },
  "input_manifests": {
    "align": [
      { "path": "trimmed/S1.fq.gz", "size": 48213, "mtime_nanos": 1790300000000000000,
        "hash": "sha256:9f2a…", "remote": null }
    ]
  },
  "tombstones": {
    "align": ["aligned/S1.bam"]
  },
  "reentries": [
    {
      "round": 1,
      "rule": "discover",
      "group": "batch",
      "samples": ["S4", "S5"],
      "pairs": []
    }
  ]
}
```

`config_snapshot` records the effective config values (sensitive keys stored
as SHA-256 digests) and `rule_fingerprints` the structural fingerprints that
drive [precise invalidation](run.md#config-changes-and-precise-invalidation).
`when_verdicts` records, per rule instance, the runtime verdict of its
`when` gate (`true`/`false`) — used for config-change replay and to
distinguish when-gated skips from real failures in `status` output (issue
#690). `input_manifests` records, per completed rule, every resolved input file
(path, size, mtime in nanoseconds, and a content hash for files up to
64 MiB) so later runs detect changed inputs (issue #72). `tombstones` lists
outputs of [`temporary`](run.md#temporary-rules-temporary-true)
rules that were deleted after a successful run — the rule stays skipped until
a dependent needs those outputs again. `reentries` records checkpoint
re-entry contributions (round, checkpoint rule, group, samples, pairs) so resumes
replay them deterministically and revoke them when the rule is invalidated
(see [Checkpoint re-entry](../reference/workflow-format.md#checkpoint-re-entry)).

Every key above is omitted from the JSON when empty — a minimal workflow
with no `[config]` section, no temporaries, no when-gates, and no
re-entries writes a much smaller checkpoint containing only
`completed_rules`, `failed_rules`, `benchmarks`, `workflow_path`,
`workdir`, `rule_fingerprints`, `rule_fingerprints_no_input`, and
`rule_runs`.

---

## Notes

- Checkpoint files are written automatically during `oxo-flow run` execution
- `--timing` reads the per-rule benchmarks recorded in the checkpoint by
  CLI runs (wall time, peak RSS) — no separate database is involved; web-
  initiated runs keep their own records in the web service's database, and
  their checkpoint timings are equally readable via `status` from the run's
  workdir
- Use `status` to inspect progress of long-running pipelines, especially on clusters
- The checkpoint file is not updated after the pipeline completes — it reflects the state at the last write
- Exits with code `0` regardless of the pipeline's success or failure
- Rule order in `--timing` output is by wall-clock time descending, so the
  most expensive rules surface first
