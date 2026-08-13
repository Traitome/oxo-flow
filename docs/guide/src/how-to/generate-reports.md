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
| **Dashboard** | Workflow-level overview metrics |
| **Execution Status** | Per-rule status and metrics (requires execution data from the checkpoint) |
| **Clinical Compliance** | Compliance-oriented checks (clinical-domain workflows) |
| **Workflow Information** | Name, version, total rules, and all `[config]` variables |
| **Commands** | The shell commands the workflow will run |
| **File Manifest** | Input and output files per rule |
| **Environment** | Environment backends and specifications used |
| **Task Summary** | Always included — a per-rule table of tasks, types, inputs, outputs, environments, and resources |

Sections adapt to available execution data — for example, **Execution
Status** only appears when a checkpoint from a previous run is present —
and the **Task Summary** table is always appended.

---

## Report Configuration in `.oxoflow`

Add a `[report]` section to your workflow file to customize report output:

```toml
[report]
template = "clinical"
format = ["html", "json", "pdf"]
sections = ["universal", "commands", "environment"]
```

> **Note — partial support**: of the three fields, only `sections` is
> currently consumed — it filters which registered sections the report
> includes. The filter keys are the generator **names** (`universal`,
> `execution-status`, `clinical-compliance`, `workflow-info`, `commands`,
> `file-manifest`, `environment`), not the display titles in the table
> above. `template` and `format` are parsed but not yet consumed: there
> are no built-in templates such as `"clinical"` yet (the field is a
> free-form string reserved for future use), and output formats are
> selected via the CLI `-f` flag.

### Fields

| Field | Type | Description |
|---|---|---|
| `template` | String | Report template name (free-form; no built-in templates such as `"clinical"`/`"research"` exist yet) |
| `format` | Array | Output formats to generate (`"html"`, `"json"`, `"pdf"`) |
| `sections` | Array | Sections to include, by generator name (`universal`, `execution-status`, `clinical-compliance`, `workflow-info`, `commands`, `file-manifest`, `environment`) |

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
  "generated_at": "2026-08-13T03:47:43Z",
  "workflow_name": "my-pipeline",
  "workflow_version": "1.0.0",
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
          ["Config Variables", "reference, samples_dir"]
        ]
      },
      "subsections": []
    },
    {
      "title": "Task Summary",
      "id": "task-summary",
      "content": {
        "type": "Table",
        "headers": ["Task", "Type", "Inputs", "Outputs", "Environment", "Resources"],
        "rows": [
          ["align", "shell", "1", "1", "conda", "t=8 m=16G"]
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
