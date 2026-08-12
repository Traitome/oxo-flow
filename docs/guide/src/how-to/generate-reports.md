# Generate Reports

This guide explains how to use oxo-flow's reporting system to produce structured HTML, JSON, and PDF reports from workflow executions.

---

## Overview

oxo-flow includes a modular report generation system designed for both research and clinical use. Reports are built from sections that contain key-value pairs, tables, and narrative text.

---

## Basic Report Generation

Generate a report from a workflow file:

```bash
# HTML report (default)
oxo-flow report pipeline.oxoflow

# JSON report
oxo-flow report pipeline.oxoflow -f json

# Write to a specific file
oxo-flow report pipeline.oxoflow -f html -o results/report.html
```

---

## Report Contents

A generated report includes:

| Section | Contents |
|---|---|
| **Workflow Information** | Name, version, author, number of rules |
| **Execution Summary** | Rules executed, success/failure counts, duration |
| **Rule Details** | Per-rule inputs, outputs, resource usage, environment |
| **Configuration** | All `[config]` variables used |

---

## Report Configuration in `.oxoflow`

Add a `[report]` section to your workflow file to customize report output:

```toml
[report]
template = "clinical"
format = ["html", "json", "pdf"]
sections = ["summary", "variants", "quality"]
```

> **Note — planned, not yet active**: the `[report]` section is parsed by the
> core library but **not yet consumed by any code path**. It is planned
> functionality; report output is currently controlled entirely via the
> `oxo-flow report` CLI flags (`-f`, `-o`). Likewise, there are no built-in
> report templates such as `"clinical"` or `"research"` yet — the `template`
> field is a free-form string reserved for future use.

### Fields

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template name (free-form; no built-in templates such as `"clinical"`/`"research"` exist yet) |
| `format` | Array | Output formats to generate (`"html"`, `"json"`, `"pdf"`) |
| `sections` | Array | Sections to include in the report |

---

## HTML Reports

HTML reports are self-contained single-file documents with embedded CSS. They can be opened in any web browser and shared without a web server.

```bash
oxo-flow report pipeline.oxoflow -f html -o report.html
open report.html   # macOS
xdg-open report.html   # Linux
```

---

## JSON Reports

JSON reports contain the same information in a machine-readable format suitable for downstream processing:

```bash
oxo-flow report pipeline.oxoflow -f json -o report.json
```

Example output structure:

```json
{
  "title": "my-pipeline Report",
  "workflow": "my-pipeline",
  "version": "0.10.2",
  "generated_at": "2026-04-05T12:00:00Z",
  "sections": [
    {
      "title": "Workflow Information",
      "id": "workflow-info",
      "content": {
        "type": "key_value",
        "pairs": [
          ["Name", "my-pipeline"],
          ["Version", "1.0.0"],
          ["Rules", "7"]
        ]
      }
    }
  ]
}
```

---

## PDF Reports

PDF reports are converted from the HTML report via `wkhtmltopdf`. Install it first:

```bash
brew install wkhtmltopdf   # macOS
apt install wkhtmltopdf    # Linux
```

```bash
oxo-flow report pipeline.oxoflow -f pdf -o report.pdf
```

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
```

---

## See Also

- [Reporting System](../reference/reporting-system.md) — architecture and template system
- [`report` command](../commands/report.md) — CLI reference
