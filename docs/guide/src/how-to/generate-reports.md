# Generate Reports

This guide explains how to use oxo-flow's reporting system to produce structured HTML and JSON reports from workflow executions.

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
format = ["html", "json"]
sections = ["summary", "variants", "quality"]
```

### Fields

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template name (e.g., `"clinical"`, `"research"`) |
| `format` | Array | Output formats to generate (`"html"`, `"json"`) |
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
  "version": "1.0.0",
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

## Clinical Reports

The Venus pipeline uses the reporting system to generate clinical-grade reports for tumor variant calling results. See the [Venus Pipeline](../reference/venus-pipeline.md) reference for details.

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
- [Venus Pipeline](../reference/venus-pipeline.md) — clinical reporting example
