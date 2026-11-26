# `oxo-flow report`

Generate reports from workflow execution.

The report is built from **execution truth**: whatever happened in a run is
what the report shows — expanded commands that actually ran, per-rule exit
codes, real files with recorded checksums, and failure diagnosis for failed
runs. Without a checkpoint the report honestly shows template-level data
only, and never claims a workflow ran when it didn't.

---

## Usage

```
oxo-flow report [OPTIONS] [WORKFLOW]
```

`WORKFLOW` is optional: when omitted, the workflow (and its checkpoint) are
auto-discovered — see [Workflow discovery](#workflow-discovery).

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file (optional — auto-discovered when omitted) |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | inferred | Output format: `html`, `json`, `md`, `pdf`, or `pdf-command`. Inferred from the `-o` extension when omitted, else from the first `[report].format` entry, else `html` |
| `--output` | `-o` | stdout | Output file path (`-` writes to stdout). For auto-discovered runs, defaults to `.oxo-flow/reports/report-<UTC timestamp>.<ext>` — the extension follows the requested format (`md`/`json`/`pdf`/`html`) |
| `--checkpoint` | — | `.oxo-flow/checkpoint.json` | Path to checkpoint file for execution metrics |
| `--run` | — | — | `DIR` — workdir of a previous run: the workflow and checkpoint are auto-discovered there. Conflicts with an explicit `WORKFLOW` |
| `--failed` | — | — | Failure-focused report: failure diagnosis is the first section (only when failures exist) |
| `--plan` | — | — | Template-only report — ignores execution data (no checkpoint required, no "no checkpoint" warning) |
| `--ai` | — | — | AI result interpretation — printed to stderr and included in the report as a marked AI-generated section |
| `--workdir` | `-d` | — | Working directory to look for `.oxo-flow` in (default: the workflow file's directory) |
| `--ci` | — | — | Reproducible output: pin the generation timestamp (`SOURCE_DATE_EPOCH` when set, else the Unix epoch) so identical state yields byte-identical reports |
| `--no-timestamps` | — | — | Omit the generation timestamp from the report |
| `--strict` | — | — | Fail (exit 2) when the checkpoint is missing or the configured template fails to render |
| `--diff` | — | — | `CHECKPOINT` — print a model-level diff of this report's checkpoint against another checkpoint (stderr, terminal-highlighted; the report still renders, exit code stays 0) |
| `--acct` | — | — | `PATH` — import sacct-style CSV accounting (JobID,JobName,State,Elapsed,CPUTime,MaxRSS) into a Resource Accounting section; with `--resource-ledger` it becomes the raw scheduler side of reconciliation, which requires `-f json` (CSV cannot carry scheduler totals, reconciliation, or conflicts — the combination is a usage error) |
| `--resource-ledger` | — | — | Emit the normalized `oxo-flow.resource-ledger/v1` document instead of a report: `-f json` (default) or `-f csv` (also inferred from a `.csv` `-o` extension). Requires a checkpoint. See [Resource ledger](#resource-ledger-resource-ledger) |
| `--r-data` | — | — | `DIR` — write R-friendly TSV files (`sample_table.tsv`, `metrics.tsv`) to DIR |
| `--versions-yml` | — | — | `PATH` — export an nf-core-style `versions.yml` of **declared** software per rule (docker `image:tag`, module `tool/version`, env files with sha256; `-` writes to stdout) and exit |
| `--list-sections` | — | — | List available report sections and exit (no workflow needed) |
| `--list-templates` | — | — | List available report templates and exit (no workflow needed) |
| `--init-template` | — | — | Write the built-in report template to `./report-template.tera` and exit (refuses to overwrite an existing file) |
| `--verbose` | `-v` | — | Enable debug-level logging |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Report generated (including template-only reports without a checkpoint; `--diff` always exits 0) |
| `1` | Rendering/usage error (e.g. unsupported format, multiple `[report].format` entries under `--strict`, `--diff` without both checkpoints, `--acct` with `--resource-ledger -f csv`) |
| `2` | Data source unavailable — under `--strict` when the checkpoint is missing or the configured template fails to render; when `--resource-ledger` finds no checkpoint and no usage error precedes it (an unsupported format or CSV + `--acct` exits `1` first; a ledger without execution records is not an empty report) |

The general `run`/`dry-run` exit-code contract (0 success, 1 failure) is
documented in [`run`](run.md#exit-codes).

---

## Workflow discovery

When `WORKFLOW` is omitted, the workflow and checkpoint are discovered in
this order:

1. **`--run DIR`** — the previous run's workdir (its `.oxo-flow/`
   directory anchors the checkpoint).
2. **`--workdir DIR`** — or the current directory for zero-arg discovery.

Inside the discovery directory, the checkpoint at
`.oxo-flow/checkpoint.json` is loaded first (an explicit `--checkpoint
PATH` replaces it — that file is loaded instead):

- If it records a `workflow_path` that exists, that workflow file is used.
- Otherwise (no checkpoint, or one without a usable `workflow_path`), the
  directory is scanned for a unique `*.oxoflow` file — `main.oxoflow`
  takes priority, then the alphabetically first file. **Multiple**
  candidates are an error: a report is a one-shot artifact, so the command
  asks you to pass `WORKFLOW` explicitly rather than silently picking one.
- No workflow found suggests passing `WORKFLOW` explicitly or using
  `--plan` for a template-only report.

With `--plan`, no checkpoint is loaded at all: discovery goes straight to
the workflow scan, and the report shows template-level data only, without
the usual "no checkpoint" warning.

An auto-discovered run writes the report to
`.oxo-flow/reports/report-<UTC timestamp>.<ext>` (path printed to stderr)
when no `-o` is given, where `<ext>` follows the requested format
(`-f md` → `.md`, `-f json` → `.json`, `-f pdf` → `.pdf`, else `.html`) —
the same directory that `run`'s auto-snapshots
fill (see [report snapshots](run.md#report-snapshots)).

---

## Examples

### Generate HTML report to stdout

```bash
oxo-flow report pipeline.oxoflow
```

### Discover and report on a previous run

```bash
# Zero-arg: workflow + checkpoint auto-discovered in the current directory
oxo-flow report

# Report on a specific previous run's workdir
oxo-flow report --run results/experiment1
```

### Write HTML to file

```bash
oxo-flow report pipeline.oxoflow -o report.html
```

### Generate JSON report (format inferred from the extension)

```bash
oxo-flow report pipeline.oxoflow -o report.json
```

The JSON is the versioned data model behind every view: it carries
`schema_version`, a `command: "report"` tag, and provenance fields
(`workflow_path`, `checkpoint_path`). All lists are sorted, and under
`--ci` the output is byte-reproducible — safe to diff, commit, and consume
with `jq`:

```bash
oxo-flow report pipeline.oxoflow --ci -o report.json && jq '.schema_version' report.json
```

### Generate Markdown report

```bash
# Git-friendly GFM tables — embed in docs or commit alongside the workflow
oxo-flow report pipeline.oxoflow -f md -o REPORT.md
```

### Generate PDF report

```bash
oxo-flow report pipeline.oxoflow -f pdf -o report.pdf
```

`-f pdf` requires the external `wkhtmltopdf` binary. If it is missing the
command degrades gracefully: it writes a printable HTML file
(`<workflow.name>_report.html`) and warns, instead of failing. Note that the
wkhtmltopdf project upstream has been archived — HTML output is the
portable, supported-by-default format.

### Output the PDF command (for scripting)

```bash
oxo-flow report pipeline.oxoflow -f pdf-command
```

### Failure-focused report

```bash
# Failure Diagnosis is the first section (when the run had failures)
oxo-flow report --run results/experiment1 --failed
```

### Template-only report

```bash
# Ignore execution data entirely — useful for previewing a workflow design
oxo-flow report pipeline.oxoflow --plan
```

### Enumerate available sections / templates

```bash
oxo-flow report --list-sections
oxo-flow report --list-templates
```

### Scaffold the built-in template

```bash
oxo-flow report --init-template   # writes ./report-template.tera
```

The built-in template covers every `ReportContent` variant (Text, Markdown,
Html, Table, KeyValue, Json, Chart, QcStatus, QcIndicatorGroup, Hierarchy,
ScatterPlot) in both top-level sections and subsections.

### Export R-friendly tables

```bash
oxo-flow report --run results/experiment1 --r-data analysis/
# writes analysis/sample_table.tsv (sample, group) and
#        analysis/metrics.tsv (rule, wall_time_secs, max_memory_mb, status)
```

### Diff two checkpoints

```bash
# Model-level diff on stderr (rules ±, status flips, benchmark deltas,
# checksum changes); the report still renders on stdout
oxo-flow report --run results/experiment1 --diff .oxo-flow/checkpoint.json
```

### Import cluster accounting

```bash
# sacct -o JobID,JobName,State,Elapsed,CPUTime,MaxRSS --parsable2
oxo-flow report --run results/experiment1 --acct sacct.csv
```

### Normalized resource ledger

```bash
# JSON document on stdout (the default format)
oxo-flow report --run results/experiment1 --resource-ledger -f json -o -

# CSV: one row per rule plus a TOTAL row (extension selects CSV)
oxo-flow report --run results/experiment1 --resource-ledger -o ledger.csv

# Reconcile the checkpoint's numbers against an independent sacct export
oxo-flow report --run results/experiment1 --resource-ledger --acct sacct.csv
```

See [Resource ledger](#resource-ledger-resource-ledger) for the schema,
the unknown-is-not-zero rule, and reconciliation semantics.

### Export declared software versions

```bash
# stdout
oxo-flow report pipeline.oxoflow --versions-yml -

# commit the file and diff it in CI to catch undeclared dependency drift
oxo-flow report pipeline.oxoflow --versions-yml versions.yml
```

Emits an nf-core-style `versions.yml` — the same data as the report's
[`software-versions` section](#software-versions-software-versions-versions-yml),
which documents the per-backend fields, the declared-not-executed caveat,
and the environment precedence (env_group > rule.environment >
defaults.environment).

---

## Data sources

The report reads three kinds of data, in this order of truth:

1. **Checkpoint** (`.oxo-flow/checkpoint.json`) — what actually happened:
   per-rule success/failure, exit codes, expanded commands, stderr excerpts
   (embedded verbatim, default 64 KiB per rule — override with the
   `OXO_FLOW_OUTPUT_TAIL_BYTES` environment variable), wall time / peak
   memory / sampled CPU benchmarks, input file manifests (path, size,
   mtime), and output checksums (`sha256:`). Generated by `oxo-flow run`.
2. **Workflow config** — declared rules, environments, and `[report]`
   settings. Only used directly where execution data does not exist, and
   labeled as such ("declared template — no execution record").
3. **Runtime facts** — oxo-flow engine version and platform.

If no checkpoint exists, the report shows template-level data only with a
clear "No execution data" indicator — it never claims a run happened.

### Failure diagnosis

A failed run's report opens with a **Failure Diagnosis** section: for each
failed rule, the exit code, the downstream rules affected (transitive DAG
dependents), a stderr excerpt when available, and a suggested next step
based on common failure signatures (exit 127 command-not-found, exit 137
OOM-kill, exit 124 timeout, disk-full, …).

### Provenance

Every report records the oxo-flow version, the workflow file and its
`sha256` checksum, and the checkpoint path it was built from — the report
itself is an audit-trail anchor. When the workflow lives in a git
repository the report also names its HEAD SHA, and each pinned git
`[[include]]` gets its own row (`Include <repo>@<ref> <path>` → the
checkout's resolved HEAD SHA at parse time) — comparable across a
run/resume pair to prove module content did not drift.

---

## Clinical Reporting

### Honest scope

The report system does **not** generate clinical data: no variant tables,
no biomarker summaries, no compliance audit records are produced. The
`clinical-compliance` section is a *static capability statement* — it
describes the classification frameworks modeled by oxo-flow's clinical
module, and says so explicitly. It only appears for explicitly
clinical-domain workflows or when listed in `[report].sections`; it is
never shown for generic workflows.

### ACMG/AMP Variant Classification (modeled, not generated)

**ACMG** (American College of Medical Genetics and Genomics) and **AMP**
(Association for Molecular Pathology) jointly published guidelines for the
interpretation of sequence variants. These guidelines define a five-tier
classification system:

| Tier | Classification | Clinical Meaning |
|------|---------------|-----------------|
| Tier I / Pathogenic | Strong clinical significance | Disease-causing; actionable for patient care |
| Tier II / Likely Pathogenic | Potential clinical significance | High confidence of disease association |
| Tier III / VUS | Uncertain significance | Insufficient evidence; re-evaluate periodically |
| Tier IV / Likely Benign | Probably not disease-causing | Strong evidence against pathogenicity |
| Benign | No clinical significance | Confirmed benign polymorphism |

oxo-flow's `VariantClassification` enum models both the somatic
(Tier I–IV, per AMP/ASCO/CAP) and germline (Pathogenic–Benign, per ACMG)
classification frameworks. Reporting on these classifications from
workflow data is future work; the report currently describes the modeled
frameworks only.

---

## Sections

Report sections are generated by a **pluggable system**; each generator
declares when it applies (e.g. `failure-diagnosis` only when rules failed,
`execution-status` only with a checkpoint). Filter them with
`[report].sections`:

```toml
[report]
sections = ["universal", "workflow-info", "commands"]
```

- Available built-in section IDs (generator names used by the filter):
  `universal`, `execution-status`, `failure-diagnosis`,
  `clinical-compliance`, `workflow-info`, `commands`, `file-manifest`,
  `environment`, `metrics`, `sample-matrix`, `provenance`, `task-summary`,
  `software-versions`, `rule-captions`, `aggregate-metrics` — run
  `oxo-flow report WF --list-sections` for the live list; the rendered
  HTML id can differ from the generator name: `universal`
  renders the `dashboard` section, and `execution-status` renders both
  `execution-status` and `benchmarks`
- The filter applies to **all** sections, including `task-summary`
- Explicitly listing `clinical-compliance` includes it regardless of the
  detected workflow domain
- `--ai` and `--acct` sections are appended after the build and are never
  filtered — they were explicitly requested
- Custom section generators can be added by implementing the
  `ReportSectionGenerator` trait (see [report.rs
  source](https://github.com/Traitome/oxo-flow/blob/main/crates/oxo-flow-core/src/report.rs))

### Metrics (parsed from tool outputs)

The `metrics` section parses **real tool output files** found under the
run's working directory (the checkpoint-recorded workdir, else the
workflow file's directory — scanned recursively, skipping hidden
directories and symlinks) and renders one subsection per tool × sample:

| Tool | File pattern (sample prefix) | Metrics | Flags |
|---|---|---|---|
| fastp | `*.fastp.json` (`S1.fastp.json` → sample `S1`) | `total_reads`, `q30_rate`, `gc_content`, `duplication_rate` | `q30_rate`: Pass ≥ 0.85, Warn ≥ 0.75, else Fail; `duplication_rate` ≥ 0.5 Warn, else Info |
| samtools flagstat | `*.flagstat`, `*.flagstat.txt` | `total_reads`, `mapped_rate`, `properly_paired_rate` | `mapped_rate`: Pass ≥ 0.90, Warn ≥ 0.80, else Fail |
| STAR | `*Log.final.out` | `uniquely_mapped_pct`, `multimapping_pct` | `uniquely_mapped_pct`: Pass ≥ 70, Warn ≥ 60, else Fail |
| featureCounts | `*.summary` | `total_count`, `assigned_rate` | `assigned_rate`: Pass ≥ 0.60, Warn ≥ 0.40, else Fail |
| bcftools | `*.bcftools.stats` | `snps`, `indels`, `ts_tv_ratio` | Informational only (no thresholds) |
| kraken2 | `*.kraken2.report`, `*.kraken.report` | `unclassified_rate` | Pass ≤ 20, Warn ≤ 40, else Fail (percentage scale) |

Matching is case-insensitive; a file whose name is exactly the pattern
(e.g. `fastp.json`) is parsed without a sample attribution. MultiQC-style
files are also picked up: `*_mqc.json` is parsed as JSON, while `*_mqc.yml`
is skipped with a Scan Note (no YAML parser shipped). Files that
match a known pattern but fail to parse are counted in a **Scan Notes**
subsection — a scanner that hid its gaps would look like full coverage.
The section is hidden entirely when nothing parses.

### Sample Matrix

With a checkpoint and sample definitions (`[[sample_groups]]` or
`[[pairs]]`), the `sample-matrix` section renders a rule × sample grid:
cells are `success` / `failed` / `-` per the engine's real expanded
instance names in the checkpoint (`{rule}_{group}_{sample}`,
`{rule}_auto-discovered_{sample}` for `sample_pattern` discovery, or
`{rule}_{pair_id}` for pairs). Rows are base rule names, sorted
failed-first so failing samples surface at the top.

### Software Versions (`software-versions` / `--versions-yml`)

The `software-versions` section is a static declaration extracted from
the workflow definition — the engine never executes anything to produce
it. Per rule it lists the declared backend: docker `image:tag` (registry
kept as its own field), module `(tool, version)` with a note when no
version segment exists, conda/mamba/pixi env files and
`venv_requirements` files with **sha256 content hashes** (a bare `venv`
directory path is recorded as declared, without a hash), or a "system
environment" note when no environment is
declared. Resolved runtime package versions depend on the execution
environment and are deliberately **not** recorded — every entry carries
that caveat. Export a machine-readable copy with
`oxo-flow report --versions-yml PATH` and diff it in CI to catch
undeclared dependency drift.

### Resource Accounting (`--acct`)

`--acct <FILE>` imports an sacct-style CSV into a **Resource Accounting**
section: a `Rule / State / Elapsed / CPU Time / Max RSS (MB)` table plus
import provenance and an honest coverage note listing rules without a
record. Column detection is header-based and case-insensitive; `JobName`,
`State`, `Elapsed`, `CPUTime` and `MaxRSS` are required (`JobID` optional,
used to prefer the batch row over step rows). `Elapsed`/`CPUTime` accept
`MM:SS`, `HH:MM:SS` or `[D-]HH:MM:SS` (with optional fractional seconds);
`MaxRSS` uses the same grammar as the scheduler adapters (one shared
`oxo_flow_core::backend::cluster::parse_rss_bytes` parser): `K`/`KB`,
`M`/`MB`, `G`/`GB`, `T`/`TB` suffixes (case-insensitive, base-1024), a
trailing `n` as SLURM's per-node qualifier, and a bare number meaning
kilobytes. The `c` suffix is positional: after a unit letter it is SLURM's
per-core qualifier and leaves the unit unchanged (`512Kc` = 512 KiB,
`512Mc` = 512 MiB), while directly after the number it is SLURM's byte unit
(`2048c` = 2,048 bytes, the value `sacct` prints for a non-whole-KiB
figure). A store that uses `c` for the other meaning will disagree with
this reading. The figure is rounded to the nearest whole MB, and a measured
sub-MiB peak reports as 1 MB — never a fabricated zero; `0` is the "not
reported" sentinel (unknown, not a measured zero), whatever its suffix. A
missing required column is a hard error — silently dropping a column would
fake a complete table.

### Resource ledger (`--resource-ledger`)

`--resource-ledger` replaces the rendered report with a normalized,
machine-readable accounting document — schema `oxo-flow.resource-ledger/v1`.
It is built only from the checkpoint (plus an optional raw `--acct` export),
never from AI or template data, so identical checkpoint state always renders
byte-identical JSON/CSV. The document spans the local, HPC, and cloud paths
with one set of canonical field names.

```bash
oxo-flow report --run results/experiment1 --resource-ledger -f json -o -
oxo-flow report --run results/experiment1 --resource-ledger -f csv -o ledger.csv
```

`-f json` is the default; `-f csv` (or a `.csv` output extension) selects
CSV. Other `-f` values are a usage error — the ledger is not a report
format. `-o -` writes to stdout; with no `-o` the document goes to stdout
too (unlike a rendered report, it is never auto-written under
`.oxo-flow/reports/`). `--resource-ledger` conflicts with the report-only
flags (`--ai`, `--plan`, `--failed`, `--r-data`, `--diff`,
`--versions-yml`, `--list-sections`, `--list-templates`,
`--init-template`). `--acct` reconciliation is available only with
`-f json`: CSV has no representation for scheduler totals, reconciliation,
or conflicts, so combining the two is a usage error (exit `1`) raised
before the export is read — never a clean table that silently dropped the
comparison. A missing checkpoint exits `2`: the ledger describes an
executed run, so "no execution records" is a data-source failure, not an
empty report.

The command still resolves a workflow file (explicit `WORKFLOW`, `--run`, or
directory discovery) before rendering, because that is how the checkpoint is
found — a bare copied `checkpoint.json` without a resolvable workflow cannot
be rendered yet. Pass the original workflow path (and `--checkpoint`) when
the run directory no longer contains one.

#### JSON schema

```json
{
  "schema": "oxo-flow.resource-ledger/v1",
  "run": {
    "workflow": "pipeline.oxoflow",
    "started_at": "2026-11-25T10:00:00Z",
    "finished_at": "2026-11-25T10:12:00Z",
    "totals": {
      "rules_total": 2,
      "rules_executed": 2,
      "wall_time_secs": 750.0,
      "wall_time_secs_complete": true,
      "cpu_seconds": 5700.0,
      "cpu_seconds_complete": false,
      "max_rss_bytes": 1073741824,
      "max_rss_bytes_complete": true,
      "io_read_bytes": 4000000000,
      "io_read_bytes_complete": false,
      "scheduler_wait_secs": 15,
      "scheduler_wait_secs_complete": false
    },
    "sources": [
      {"backend": "local", "tool": "sysinfo"},
      {"backend": "slurm", "tool": "sacct", "raw_fields": {"MaxRSS": "1G"}}
    ]
  },
  "budget": {
    "amount": 10.0,
    "currency": "USD",
    "cpu_second_rate": 0.0001,
    "gb_second_rate": 0.00002
  },
  "cost_accrual": {
    "currency": "USD",
    "limit": 10.0,
    "cpu_seconds": 5700.0,
    "gb_seconds": 772.8,
    "cpu_cost": 0.57,
    "gb_cost": 0.015456,
    "accrued": 0.585456,
    "unknown_rules": [],
    "method": "cpu_second_rate * cpu_seconds + gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)",
    "on_unknown": "refuse"
  },
  "rules": [
    {
      "rule": "align",
      "status": "completed",
      "wall_time_secs": 720.5,
      "cpu_seconds": 5700.0,
      "max_rss_bytes": 1073741824,
      "io_read_bytes": 4000000000,
      "scheduler_wait_secs": 15,
      "nodes": 1,
      "cores": 8,
      "source": {
        "backend": "slurm",
        "tool": "sacct",
        "raw_fields": {"MaxRSS": "1G", "TotalCPU": "01:35:00", "NNodes": "1"}
      }
    }
  ]
}
```

Unknown fields are omitted from JSON (`run.conflicts` is omitted when
empty; `run.scheduler_totals`, `run.scheduler_source`, and
`run.reconciliation` when no raw export was given); the example shows only
recorded values. `budget` and `cost_accrual` appear only when the
checkpoint's workflow declared a `[resource_budget] max_cost` table (see
[Cost accrual](#cost-accrual) below).

Per rule: `rule`, `status` (`completed`, `failed`, `cancelled`,
`timed_out`, `skipped`, or `unknown` for an interrupted attempt),
`wall_time_secs`, `cpu_seconds`, `user_seconds`, `system_seconds`,
`max_rss_bytes`, `peak_memory_bytes`, `io_read_bytes`, `io_write_bytes`,
`gpu_seconds`, `scheduler_wait_secs`, `nodes`, `cores`, `cost`,
`source`, and `recorded_as`.

#### Unknown is never zero

Every metric is optional. A backend that did not report it leaves the field
absent in JSON and an empty cell in CSV — never `0`, which would claim a
measurement nobody made. `source` is always present and says who reported
what: `backend` (execution backend), `tool` (the program that answered),
optional `version`, an optional `note` explaining an absence, and
`raw_fields` — the store's own raw values, key-ordered, for auditing the
conversion. Rules the checkpoint never measured (legacy checkpoints, a rule
that never executed) carry an `unknown` source with a note instead of
plausible-looking numbers. `recorded_as` marks synthetic
"outputs up-to-date" completions: the rule did not execute in this run, so
every metric is absent — including I/O a stale record from an earlier
attempt might carry — and the source note says so.

A checkpoint is shared across invocations, so the ledger draws an explicit
invocation boundary. Every `run` (local or cluster) begins by marking all
rules that already have a recorded attempt as **not executed in this
invocation** (`skipped_attempts`, kept beside — not inside — `rule_runs` so
the last real execution's exit code, command, tails, and timings survive).
A terminal attempt clears the marker; a skip rewrites it; a crash mid-flight
leaves it (the invocation did not complete that rule). This covers:

- the all-up-to-date re-run in the same workdir;
- a local subset run (`--target` / `--from-rule`), whose preserved
  completions outside the execution set did not run;
- a cluster re-run with nothing to submit, or a subset submission;
- rules blocked behind a failure.

Such rules read `status: "skipped"` with no metrics and a source note naming
the reason. Without a marker (legacy checkpoints), the pre-boundary behavior
applies: a completed rule reads as completed with its recorded metrics.

A benchmark only describes the latest attempt. A benchmark survives a
failure (`mark_failed` keeps it for history), and a synthetic completion can
leave a `recorded_as` placeholder behind, so the ledger uses a benchmark's
wall/CPU only when the latest `rule_runs` attempt is `success` — or when the
record predates attempt status (legacy checkpoints). A failed, cancelled, or
timed-out re-execution renders its own recorded start/finish duration and its
`resource_usage` (CPU, peak RSS, I/O, source), and counts in
`rules_executed` / the `--acct` executed set even if an earlier invocation
left a synthetic marker. An old success's wall/CPU never describes the
failed attempt, and the run totals sum only the current attempt.

No supported backend reports GPU-seconds, user/system CPU split, a
control-plane peak-memory metric, or a price, so `gpu_seconds`,
`user_seconds`, `system_seconds`, `peak_memory_bytes`, and the per-rule
`cost` are always absent. The schema reserves their shape; it never invents
an amount or a currency. A *declared* rate is a different thing: when a
workflow sets `[resource_budget] max_cost`, the ledger derives an
**accrued** amount from the recorded metrics using the operator's own rates
(see [Cost accrual](#cost-accrual)) — that is the run's own declaration
priced against its own measurements, not a vendor-billing figure.

#### Additive vs peak

`wall_time_secs`, `cpu_seconds`, `io_read_bytes`, `io_write_bytes`, and
`scheduler_wait_secs` sum across rules. Memory is a **peak**: a rule's
`max_rss_bytes` is the maximum resident set size observed, and the run
total is the maximum across rules — adding peaks would double-count the
same bytes. `rules_total` counts every rule the checkpoint mentions;
`rules_executed` counts the ones that actually ran in this run — synthetic
"outputs up-to-date" completions and rules whose latest attempt was an
up-to-date skip are excluded (completion bookkeeping, not execution). A
`*_complete` flag says whether every executed rule reported the metric, so a
partial sum is never presented as a full one.

#### Cost accrual

When the checkpoint's workflow declared a `[resource_budget] max_cost` table
(issue #1400), the ledger prices its own recorded metrics against the
operator's declared rates and adds two objects to the JSON document (and a
column group to the CSV — see [CSV format](#csv-format)):

- `budget` echoes the declaration: `amount`, `currency`,
  `cpu_second_rate` and/or `gb_second_rate`, and `on_unknown`.
- `cost_accrual` is the derived result: `currency`, `limit` (the `amount`),
  the summed `cpu_seconds` and `gb_seconds`, the per-rate contributions
  `cpu_cost`/`gb_cost`, the total `accrued`, `method` (a stable string
  naming the rates used), `on_unknown`, and `unknown_rules` — the names of
  executed rules whose metric a configured rate depends on but which did not
  report it.

The accrual formula is exactly:

```text
accrued = cpu_second_rate * sum(rule.cpu_seconds)
        + gb_second_rate   * sum(max_rss_bytes/1e9 * wall_time_secs)
```

`gb_seconds` needs both `max_rss_bytes` and `wall_time_secs` for every
executed rule; `cpu_seconds` needs the CPU metric. **Unknown is never
zero:** when a configured rate depends on a metric some executed rule did
not report, `accrued` is `null` (never `0`) and `unknown_rules` names the
offenders — the ledger reports the unbounded residual rather than a
fictitious number. `currency` is compared for equality only; a mismatch
between the budget and a rule's own reported cost is fail-closed, never a
converted figure. A legacy checkpoint without `resource_usage` has no
measured metrics at all, so accrual is unknown and `unknown_rules` lists
every executed rule.

#### Per-backend sources

| Backend | Tool | Fields reported | Unknown by design |
|---|---|---|---|
| local | `sysinfo` + Linux `/proc/<pid>/io` | wall time, CPU seconds, peak RSS bytes (sampled subtree), I/O bytes on Linux (every 200 ms tick, including successful attempts) | scheduler wait, nodes, cores, GPU; I/O off Linux; kernel writeback may under-count `write_bytes` |
| SLURM | `sacct` | `ElapsedRaw`/`Elapsed`, `MaxRSS`, `TotalCPU`, `Submit`→`Start` wait, `AllocCPUS`, `NNodes`, `AveDiskRead`/`AveDiskWrite` | fields the store omits (each stays absent) |
| PBS | `qstat -x` | `resources_used.walltime`, `.mem`, `.cput` | scheduler wait, I/O, allocation |
| SGE | `qacct` | `ru_wallclock`, `ru_maxrss`, `cpu` | scheduler wait, I/O, allocation |
| LSF | `bacct` | terminal state only (source note says so) | elapsed, memory, CPU, I/O, wait |
| SLURM without `slurmdbd` | `scontrol` | `JobState`, `ExitCode` | every measurement |
| Kubernetes / AWS Batch / GCP Batch / Tencent Batch | — | none (no accounting API in this adapter) | every measurement; the source note names the backend |

`MaxRSS` and `TotalCPU` come from the step rows, where SLURM records them;
the reported figure is the maximum across the job's rows (the allocation
and `.batch` rows describe the same work, so summing would double-count).
Memory figures accept SLURM's qualifiers positionally: `512Mn`/`512Kc`/
`512Mc` keep the preceding unit (per-node/per-core), while a bare
`<number>c` is the byte unit and a plain `<number>` is kilobytes; zero with
any suffix is unknown. A store that emits `c` with the other meaning falls
outside this mapping (see the `--acct` grammar above).
`AveDiskRead`/`AveDiskWrite` are per-task averages; the engine submits one
task per job, so the average is the rule's total.

#### Reconciliation with `--acct`

Passing `--acct <CSV>` to a ledger run treats that export as the raw
scheduler side of a reconciliation. This requires `-f json` (the default):
`-f csv` combined with `--acct` is a usage error (exit `1`) raised before
the file is read, because the CSV surface has no scheduler-total,
reconciliation, or conflict columns — silently dropping the comparison
would present a clean table for a check that never happened. Rows are first
**reduced to the rules that actually executed in this run** — a whole-account
export (`sacct -a`) may contain unrelated jobs, which are ignored rather
than allowed to inflate the raw side. The note reports them as job names **not matched to an
executed rule** (not "not executed": a site may truncate job names, or array
elements may share one base name), with the count and up to five names in
`run.scheduler_source.note`. The remaining merged batch rows are summed into
`run.scheduler_totals` (same shape as `run.totals`) and compared with the
checkpoint's per-rule sums.

The raw side always carries its own provenance in
`run.scheduler_source` (backend, tool, version/note/raw fields), present
exactly when `run.scheduler_totals` is. `run.reconciliation` records what the
comparison actually did: `covered_rules`, `compared_metrics`,
`skipped_metrics` (each with a reason such as an incomplete side or a
coverage gap), and a human `note` — so an agreeing comparison is
distinguishable from one that never happened.

A disagreement beyond the tolerance — 5% of the larger value, with a 1 s
floor for time and a 1 MiB floor for memory — is listed under
`run.conflicts`, naming both numbers and both sources; the ledger reports
the disagreement instead of picking a winner. A metric is compared only when
both sides are complete and the export covers every executed rule: a
coverage gap is not a numeric conflict. sacct's bare `MaxRSS=0` is SLURM's
"not reported" sentinel and is treated as unknown (never a measured zero),
matching the core parser.

#### CSV format

One row per rule plus a `TOTAL` row. Column names are stable:

```text
record_type,rule,status,wall_time_secs,cpu_seconds,user_seconds,system_seconds,
max_rss_bytes,peak_memory_bytes,io_read_bytes,io_write_bytes,
scheduler_wait_secs,nodes,cores,gpu_seconds,cost_amount,cost_currency,
cost_method,source_backend,source_tool,source_version,source_note,
source_raw_fields,recorded_as,budget_amount,budget_currency,accrued_cost,
cost_unknown_rules,incomplete_metrics
```

The four cost-budget columns (`budget_amount,budget_currency,accrued_cost,
cost_unknown_rules`, issue #1400) sit between `recorded_as` and
`incomplete_metrics`. Like the other run-level fields they are filled on the
`TOTAL` row and empty on per-rule rows. They are empty unless the workflow
declared a `[resource_budget] max_cost` table. `accrued_cost` is empty when
accrual is not computable (the JSON `accrued` is `null`; see
[Cost accrual](#cost-accrual)) and `cost_unknown_rules` is a compact list of
the offending rule names. A workflow without `max_cost` emits the same
columns, all four empty.

Unknown values are empty cells. Seconds render with millisecond precision
(`10.000`); bytes are integers. The reserved metric columns
(`user_seconds`, `system_seconds`, `peak_memory_bytes`) mirror the JSON
schema and stay empty until a backend reports them — they are carried now
so a future measurement cannot silently reach JSON consumers only.
`source_raw_fields` is the same key-ordered scheduler field map JSON
carries, serialized as compact JSON in one quoted cell (empty when the
source recorded none). The `TOTAL` row leaves rule-only columns empty and
lists any partially reported metrics in `incomplete_metrics` (the same
information as the JSON `*_complete` flags).

### Benchmarks CPU column

On **local** runs the **Benchmarks** table's CPU column shows **sampled**
CPU seconds: the executor's sampler reads each rule process's CPU time (all
its threads) at 200 ms ticks — child processes are not accumulated. Peak
memory is sampled the same way.

On **cluster** runs both columns come from the scheduler's accounting store
instead, read once as each job settles, and cover every step of the job.

`-` means neither source reported a number: very short local rules the
sampler never observed, LSF, a cluster job whose accounting row never
appeared, or legacy checkpoints written before sampling existed.

### Domain auto-detection

Workflow domains (DNA-seq, RNA-seq, epigenomics) are detected from the
tools a workflow's commands reference, and shown in the report's workflow
information. DNA tool signals (GATK/Picard/variant callers) take precedence
over RNA aligner signals, since variant-calling pipelines legitimately
contain `STAR`/`featureCounts` rules. `clinical` is never inferred from
commands — it is only reachable through explicit configuration.

---

## Notes

- If `--output` is not specified, the report is written to stdout
  (stderr carries diagnostics and the `--ai` interpretation — stdout stays
  pipe-safe: `oxo-flow report wf.oxoflow -f json | jq .` works). Exception:
  auto-discovered runs (zero-arg or `--run`) write to
  `.oxo-flow/reports/report-<UTC timestamp>.<ext>` — the extension follows
  the requested format (`-f md` → `.md`, `-f json` → `.json`,
  `-f pdf` → `.pdf`, else `.html`), so the default location is predictable
  and the file never masquerades as HTML
- `report` does not implement the global `--json` switch; select the JSON
  document with `-f json` (and the ledger, likewise, with
  `--resource-ledger -f json`). Passing `--json` is rejected with a usage
  error that names the right spelling instead of being silently ignored
- HTML reports are self-contained single files: embedded CSS, dark-mode
  support, print styles, semantic landmarks, and HTML-escaped user
  content (rule names, commands, paths are safe to open and share)
- `--ai` includes the interpretation as an AI-generated report section
  (with the model name and a "review before relying" note); without an AI
  provider configured it degrades to the standard report with a warning
- `--no-timestamps` / `--ci` control the generation timestamp; `--ci`
  honors `SOURCE_DATE_EPOCH` for reproducible builds
- PDF output uses `wkhtmltopdf` for conversion (upstream archived; absent
  binary degrades to printable HTML): `brew install wkhtmltopdf` (macOS)
  or `apt install wkhtmltopdf` (Linux)
