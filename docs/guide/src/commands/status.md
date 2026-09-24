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
| `--verbose` | `-v` | Enable debug-level logging |

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
oxo-flow v0.20.1 — Rust-native bioinformatics pipeline engine
Status: Status for checkpoint: .oxo-flow/checkpoint.json
  Completed: 3
  Failed:    1

Completed rules:
  ✓ align
  ✓ sort_bam
  ✓ trim_reads

Failed rules:
  ✗ mark_duplicates
```

### Staleness reasons

When the checkpoint's workflow file is still present and parseable, `status`
also classifies every completed rule exactly the way `run` would (issue
#432) and explains **why each rule would re-run right now** — output
deleted, input manifest mismatch, config change, or a cascade from an
upstream rule:

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
  "workflow": "pipeline.oxoflow",
  "completed": ["align", "sort_bam", "trim_reads"],
  "failed": [],
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
measurement. `staleness` is present whenever the workflow file can be
loaded for classification (see [Staleness reasons](#staleness-reasons)).

---

## Checkpoint File Format

The checkpoint file is JSON with the following structure:

```json
{
  "completed_rules": ["trim_reads", "align", "sort_bam"],
  "failed_rules": ["mark_duplicates"],
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
`tombstones` lists outputs of [`temporary`](run.md#temporary-rules-temporary-true)
rules that were deleted after a successful run — the rule stays skipped until
a dependent needs those outputs again. `reentries` records checkpoint
re-entry contributions (round, checkpoint rule, group, samples, pairs) so resumes
replay them deterministically and revoke them when the rule is invalidated
(see [Checkpoint re-entry](../reference/workflow-format.md#checkpoint-re-entry)).

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
