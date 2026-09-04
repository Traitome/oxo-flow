# Generate Reports

This guide explains how to use oxo-flow's reporting system to produce
structured HTML, JSON, Markdown, and PDF reports from workflow executions.

---

## Overview

Reports are built from **execution truth**: run a workflow first, then the
report shows what actually happened — expanded commands, per-rule exit
codes, real files with recorded checksums, and failure diagnosis for
failed runs. Without a checkpoint the report shows template-level data
only, and never claims a workflow ran when it didn't.

Reports are assembled from pluggable sections (key-value pairs, tables,
narrative text) — see
[reporting-system](../reference/reporting-system.md) for the architecture.

---

## Basic Report Generation

Generate a report from a workflow file:

```bash
# HTML report (default; format also inferred from the -o extension)
oxo-flow report pipeline.oxoflow

# JSON report (the versioned data model behind every view)
oxo-flow report pipeline.oxoflow -o report.json

# Markdown report (git-friendly GFM tables)
oxo-flow report pipeline.oxoflow -f md -o REPORT.md

# Write to a specific file
oxo-flow report pipeline.oxoflow -f html -o results/report.html
```

Reproducible CI output: `--ci` pins the generation timestamp
(`SOURCE_DATE_EPOCH` when set) and all lists are sorted, so identical
state yields byte-identical reports — safe to diff and commit.

---

## Export Declared Software Versions

For dependency auditing, export an nf-core-style `versions.yml` of what
the workflow **declares** and diff it in CI:

```bash
oxo-flow report pipeline.oxoflow --versions-yml versions.yml
```

Each rule records its declared backend — docker `image:tag`, module
`(tool, version)`, conda/mamba/pixi env files and `venv_requirements`
files with sha256 content hashes (a bare `venv` directory path is
recorded as declared, without a hash). This is a static declaration (the
engine never executes
anything to collect it), so resolved runtime package versions are not
included; every entry carries that caveat. `-` writes the YAML to
stdout.

---

## Report Contents

A generated report includes:

| Section | Contents |
|---|---|
| **Dashboard** | Pipeline status (honest: "No execution data" when nothing ran), task count, total runtime |
| **Execution Status** | Per-rule status, wall time, and exit codes (requires a checkpoint) |
| **Failure Diagnosis** | Failed rules: exit code, affected downstream rules, stderr excerpt, suggested next step (only when rules failed) |
| **Clinical Compliance** | Static capability statement — only for explicitly clinical workflows or when listed in `[report].sections`; generates no clinical data |
| **Workflow Information** | Name, version, total rules, detected domain, `[config]` variables |
| **Commands** | The commands that actually ran (expanded); declared templates with an explicit fallback label when no execution record exists |
| **File Manifest** | Real files: checkpoint-recorded inputs (path, size, mtime) and outputs (sha256 + on-disk size/mtime); transform chunk intermediates deleted by `cleanup = true` are listed with a "cleaned by design" marker instead of a missing status |
| **Metrics** | QC metrics parsed from real tool outputs under the workdir (fastp, flagstat, STAR, featureCounts, bcftools, kraken2), one subsection per tool × sample, with Pass/Warn/Fail flags — hidden when nothing parses |
| **Sample Matrix** | Rule × sample status grid from the checkpoint's expanded instance names — hidden without a checkpoint or sample definitions |
| **Resource Accounting** | Cluster accounting imported from an sacct CSV via `--acct` (Rule/State/Elapsed/CPU Time/Max RSS) |
| **Environment** | Engine version, platform, and the environments the workflow's rules declare |
| **Provenance** | Engine version, workflow file sha256, checkpoint location |
| **Software Versions** | Declared software per rule, nf-core-style: docker `image:tag`, module `tool/version`, env files with sha256 hashes — static declaration, runtime package versions deliberately not recorded |
| **Task Summary** | Per-rule table of tasks, types, inputs, outputs, environments, and resources |
| **Rule Captions** | Per-rule `report` annotations rendered as markdown (one subsection per executed instance) — set `report = "caption"` or `report = { file = "notes/qc.md", caption = "…" }` on a rule |
| **Aggregate Metrics** | MultiQC-style sample × metric matrix across all parsed tools, plus `*_mqc.json` custom content |

Sections adapt to available execution data — for example, **Execution
Status** and **Failure Diagnosis** only appear with a checkpoint — and
every section, including **Task Summary**, can be filtered via
`[report].sections`. List them all with `oxo-flow report WF --list-sections`.

---

## Report Configuration in `.oxoflow`

Add a `[report]` section to your workflow file to filter report sections:

```toml
[report]
sections = ["universal", "workflow-info", "commands", "failure-diagnosis"]
```

> **Note — partial support**: of the three fields, `sections` filters
> which registered sections the report includes, by generator **name**
> (`universal`, `execution-status`, `failure-diagnosis`,
> `clinical-compliance`, `workflow-info`, `commands`, `file-manifest`,
> `environment`, `metrics`, `sample-matrix`, `provenance`,
> `task-summary`, `software-versions`, `rule-captions`,
> `aggregate-metrics`). `template` is **consumed** (see below); `format` is
> parsed but still unsupported: setting it makes the command warn (or fail
> under `--strict`) instead of silently ignoring it — output formats are
> selected via the CLI `-f` flag.

### Fields

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template: the built-in name `"report.html"`, or a template file path (resolved relative to the workflow file's directory first, then the current directory). Applies to HTML output only; a render failure warns and falls back to the default renderer (exit 2 under `--strict`) |
| `format` | Array | Parsed but not supported yet — setting it warns (or fails under `--strict`); use `-f` to select the output format |
| `sections` | Array | Sections to include, by generator name (see above) |

### Custom templates

`[report].template` wires a custom [Tera](https://tera.netlify.app/)
template into HTML rendering:

```toml
[report]
template = "report.html"        # the built-in template (the default)
# template = "my_report.tera"   # or a file: workflow dir first, then cwd
```

The scaffolded `report-template.tera` (from
`oxo-flow report --init-template`) is a good starting point. The built-in
template IS `report.html`; custom template files whose names do not end in
`.html`/`.htm`/`.xml` (like the scaffold) are registered under
`custom.html` so Tera HTML-escapes `{{ variables }}` exactly like the
built-in template. With `-f json`/`-f md`/`-f pdf` the template is skipped
with a note — it applies to HTML output only.

---

## HTML Reports

HTML reports are self-contained single-file documents with embedded CSS
(dark mode, print styles, accessible landmarks). User-controlled strings —
workflow and rule names, commands, file paths — are HTML-escaped, so
reports are safe to open and share. They can be opened in any browser:

```bash
oxo-flow report pipeline.oxoflow -f html -o report.html
open report.html   # macOS
xdg-open report.html   # Linux
```

---

## JSON Reports

JSON reports are the same data model, machine-readable, with
`schema_version`, a `command: "report"` tag, and provenance fields
(`workflow_path`, `checkpoint_path`):

```bash
oxo-flow report pipeline.oxoflow -f json -o report.json
# stdout stays pipe-safe: diagnostics and --ai output go to stderr
oxo-flow report pipeline.oxoflow -f json | jq .schema_version
```

Example output structure:

```json
{
  "schema_version": 1,
  "command": "report",
  "title": "my-pipeline Report",
  "generated_at": "2026-08-13T03:47:43Z",
  "workflow_name": "my-pipeline",
  "workflow_version": "1.0.0",
  "checkpoint_path": "/path/to/.oxo-flow/checkpoint.json",
  "workflow_path": "/path/to/my-pipeline.oxoflow",
  "sections": [
    {
      "title": "Workflow Information",
      "id": "workflow-info",
      "content": {
        "type": "KeyValue",
        "pairs": [
          ["Name", "my-pipeline"],
          ["Version", "1.0.0"],
          ["Total Rules", "7"],
          ["Detected Domain", "Generic"]
        ]
      },
      "subsections": []
    }
  ],
  "metadata": {}
}
```

---

## PDF Reports

PDF reports are converted from the HTML report via the external
`wkhtmltopdf` binary:

```bash
brew install wkhtmltopdf   # macOS
apt install wkhtmltopdf    # Linux
```

```bash
oxo-flow report pipeline.oxoflow -f pdf -o report.pdf
```

If `wkhtmltopdf` is not installed, the command degrades gracefully: it
writes a printable HTML file (`report.html`) and warns, instead of
failing. Note the wkhtmltopdf project upstream has been archived — HTML
output is the portable, supported-by-default format.

You can also output the raw `wkhtmltopdf` command for scripting or custom
processing:

```bash
oxo-flow report pipeline.oxoflow -f pdf-command
```

---

## R-friendly Export (`--r-data`)

`--r-data <DIR>` writes two TSV files for downstream R analysis (in
addition to the report itself):

```bash
oxo-flow report --run results/experiment1 --r-data analysis/
# analysis/sample_table.tsv   sample  group
# analysis/metrics.tsv        rule  wall_time_secs  max_memory_mb  status
```

`sample_table.tsv` maps every sample to its group (from `[[sample_groups]]`
and `[[pairs]]`); `metrics.tsv` has one row per rule with wall time, peak
memory, and status (`success` / `failed` / `-`). Without a checkpoint the
files carry headers only, with a note explaining why. Values are
TSV-sanitized (tabs and newlines replaced) so names can never break the
column layout.

---

## Automatic Snapshots

Every `oxo-flow run` / `oxo-flow resume` automatically writes a JSON
report snapshot after the run — no reporting step needed:

- `.oxo-flow/reports/report-<UTC timestamp>.json` — the full report data
  model (a `-N` suffix when two snapshots land in the same second)
- `.oxo-flow/reports/index.json` — a JSON array of
  `{generated_at, workflow, checkpoint, report}` entries, sorted by
  `generated_at`; the last entry is the newest snapshot

Disable the behavior with `--no-report-snapshot` (on `run` or `resume`).
Snapshot failures are warnings — a reporting hiccup never fails a run.
`report`'s own auto-discovered output lands in the same directory.

---

## Programmatic Report Generation

You can also generate reports programmatically using the core library:

```rust
use oxo_flow_core::report::{Report, ReportSection, ReportContent};

let mut report = Report::new("My Report", "pipeline", "1.0.0");

report.add_section(ReportSection {
    title: "Summary".to_string(),
    id: "summary".to_string(),
    content: ReportContent::KeyValue {
        pairs: vec![
            ("Total Samples".to_string(), "24".to_string()),
            ("Pass Rate".to_string(), "95.8%".to_string()),
        ],
    },
    subsections: vec![],
});

let html = report.to_html();
let json = report.to_json().unwrap();
let markdown = report.to_markdown();
```

---

## See Also

- [Reporting System](../reference/reporting-system.md) — architecture and section model
- [`report` command](../commands/report.md) — CLI reference
