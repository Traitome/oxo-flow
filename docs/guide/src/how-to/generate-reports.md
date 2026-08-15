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
| **File Manifest** | Real files: checkpoint-recorded inputs (path, size, mtime) and outputs (sha256 + on-disk size/mtime) |
| **Environment** | Engine version, platform, and the environments the workflow's rules declare |
| **Provenance** | Engine version, workflow file sha256, checkpoint location |
| **Task Summary** | Per-rule table of tasks, types, inputs, outputs, environments, and resources |

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

> **Note — partial support**: of the three fields, only `sections` is
> currently consumed — it filters which registered sections the report
> includes, by generator **name** (`universal`, `execution-status`,
> `failure-diagnosis`, `clinical-compliance`, `workflow-info`, `commands`,
> `file-manifest`, `environment`, `provenance`, `task-summary`).
> `template` and `format` are parsed but not yet supported: setting them
> makes the command warn (or fail under `--strict`) instead of silently
> ignoring them. Output formats are selected via the CLI `-f` flag.

### Fields

| Field | Type | Description |
|---|---|---|
| `template` | String | Reserved for future template support — setting it warns |
| `format` | Array | Reserved for future use — setting it warns |
| `sections` | Array | Sections to include, by generator name (see above) |

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
