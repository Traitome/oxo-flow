//! Modular report generation system.
//!
//! Provides a framework for generating structured reports (HTML, JSON)
//! from workflow execution results. Designed for clinical-grade reporting
//! with full traceability and provenance.

use crate::error::{OxoFlowError, Result};
use crate::executor::JobRecord;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A section in a report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSection {
    /// Section title.
    pub title: String,

    /// Section identifier (for linking and CSS).
    pub id: String,

    /// Section content (can be HTML, Markdown, or structured data).
    pub content: ReportContent,

    /// Subsections.
    #[serde(default)]
    pub subsections: Vec<ReportSection>,
}

/// Content types for report sections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ReportContent {
    /// Plain text content.
    Text { text: String },

    /// Markdown content.
    Markdown { markdown: String },

    /// HTML content.
    Html { html: String },

    /// Table data with headers and rows.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },

    /// Key-value pairs.
    KeyValue { pairs: Vec<(String, String)> },

    /// Raw JSON data.
    Json { data: serde_json::Value },

    /// Simple bar chart data for visualization.
    Chart {
        /// Chart title.
        title: String,
        /// Bar labels.
        labels: Vec<String>,
        /// Bar values.
        values: Vec<f64>,
        /// Unit label for values (e.g., "seconds", "MB").
        unit: String,
    },
}

/// Complete report document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Report title.
    pub title: String,

    /// Report generation timestamp.
    pub generated_at: DateTime<Utc>,

    /// Workflow name.
    pub workflow_name: String,

    /// Workflow version.
    pub workflow_version: String,

    /// Report sections.
    pub sections: Vec<ReportSection>,

    /// Report metadata (arbitrary key-value pairs).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl Report {
    /// Create a new empty report.
    pub fn new(title: &str, workflow_name: &str, workflow_version: &str) -> Self {
        Self {
            title: title.to_string(),
            generated_at: Utc::now(),
            workflow_name: workflow_name.to_string(),
            workflow_version: workflow_version.to_string(),
            sections: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Add a section to the report.
    pub fn add_section(&mut self, section: ReportSection) {
        self.sections.push(section);
    }

    /// Add metadata to the report.
    pub fn add_metadata(&mut self, key: &str, value: &str) {
        self.metadata.insert(key.to_string(), value.to_string());
    }

    /// Add a provenance section to the report with execution metadata.
    pub fn add_provenance(&mut self, version: &str, checksum: &str, timestamp: &str) {
        let section = ReportSection {
            title: "Execution Provenance".to_string(),
            id: "provenance".to_string(),
            content: ReportContent::KeyValue {
                pairs: vec![
                    ("oxo-flow Version".to_string(), version.to_string()),
                    ("Config Checksum".to_string(), checksum.to_string()),
                    ("Execution Time".to_string(), timestamp.to_string()),
                ],
            },
            subsections: vec![],
        };
        self.add_section(section);
    }

    /// Render the report as a JSON string.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Render the report as a self-contained HTML document.
    ///
    /// All CSS is embedded inline so the report can be viewed offline.
    /// Includes dark mode support via `prefers-color-scheme` media query.
    pub fn to_html(&self) -> String {
        let mut html = String::new();
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        html.push_str(&format!("  <title>{}</title>\n", self.title));
        html.push_str("  <meta charset=\"utf-8\">\n");
        html.push_str(
            "  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
        );
        html.push_str("  <style>\n");
        // Light theme
        html.push_str("    :root { --bg: #f7fafc; --text: #1a202c; --primary: #2c5282; --border: #e2e8f0; --card-bg: #ffffff; --hover: #edf2f7; --code-bg: #edf2f7; }\n");
        // Dark theme
        html.push_str("    @media (prefers-color-scheme: dark) {\n");
        html.push_str("      :root { --bg: #1a202c; --text: #e2e8f0; --primary: #63b3ed; --border: #4a5568; --card-bg: #2d3748; --hover: #4a5568; --code-bg: #2d3748; }\n");
        html.push_str("    }\n");
        html.push_str("    * { box-sizing: border-box; margin: 0; padding: 0; }\n");
        html.push_str("    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; color: var(--text); background: var(--bg); max-width: 960px; margin: 0 auto; padding: 2rem; line-height: 1.6; }\n");
        html.push_str("    header { border-bottom: 3px solid var(--primary); padding-bottom: 1rem; margin-bottom: 2rem; }\n");
        html.push_str("    header h1 { color: var(--primary); font-size: 1.8rem; }\n");
        html.push_str("    .meta { color: #718096; font-size: 0.85rem; margin-top: 0.25rem; }\n");
        html.push_str("    nav.toc { background: var(--card-bg); border: 1px solid var(--border); border-radius: 8px; padding: 1rem 1.5rem; margin-bottom: 2rem; }\n");
        html.push_str(
            "    nav.toc h2 { font-size: 1rem; margin-bottom: 0.5rem; color: var(--primary); }\n",
        );
        html.push_str("    nav.toc ul { list-style: none; padding-left: 0; }\n");
        html.push_str("    nav.toc li { margin: 0.25rem 0; }\n");
        html.push_str("    nav.toc a { color: var(--primary); text-decoration: none; }\n");
        html.push_str("    nav.toc a:hover { text-decoration: underline; }\n");
        html.push_str("    section { margin-bottom: 2rem; background: var(--card-bg); border: 1px solid var(--border); border-radius: 8px; padding: 1.5rem; }\n");
        html.push_str("    h2 { color: var(--primary); font-size: 1.3rem; border-bottom: 1px solid var(--border); padding-bottom: 0.4rem; margin-bottom: 0.8rem; }\n");
        html.push_str("    table { border-collapse: collapse; width: 100%; margin: 0.5rem 0; font-size: 0.9rem; }\n");
        html.push_str("    th, td { border: 1px solid var(--border); padding: 0.5rem 0.75rem; text-align: left; }\n");
        html.push_str("    th { background: var(--primary); color: #fff; }\n");
        html.push_str("    tr:nth-child(even) { background: var(--hover); }\n");
        html.push_str(
            "    dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.3rem 1rem; }\n",
        );
        html.push_str("    dt { font-weight: 600; }\n");
        html.push_str("    dd { margin: 0; }\n");
        html.push_str("    pre { background: var(--code-bg); padding: 1rem; overflow-x: auto; border-radius: 4px; font-size: 0.85rem; }\n");
        html.push_str("    p { margin-bottom: 0.5rem; }\n");
        html.push_str("    .disclaimer { background: #fffbeb; border-left: 4px solid #f59e0b; padding: 1rem; border-radius: 4px; margin: 1rem 0; }\n");
        html.push_str("    footer { margin-top: 3rem; border-top: 1px solid var(--border); padding-top: 0.5rem; color: #a0aec0; font-size: 0.75rem; text-align: center; }\n");
        html.push_str("  </style>\n</head>\n<body>\n");

        // Header
        html.push_str("<header>\n");
        html.push_str(&format!("  <h1>{}</h1>\n", self.title));
        html.push_str(&format!(
            "  <p class=\"meta\">Workflow: {} v{} &middot; Generated: {}</p>\n",
            self.workflow_name, self.workflow_version, self.generated_at
        ));
        for (key, value) in &self.metadata {
            html.push_str(&format!("  <p class=\"meta\">{}: {}</p>\n", key, value));
        }
        html.push_str("</header>\n\n");

        // Table of contents
        if !self.sections.is_empty() {
            html.push_str("<nav class=\"toc\">\n");
            html.push_str("  <h2>Contents</h2>\n  <ul>\n");
            for section in &self.sections {
                html.push_str(&format!(
                    "    <li><a href=\"#{}\">{}</a></li>\n",
                    section.id, section.title
                ));
            }
            html.push_str("  </ul>\n</nav>\n\n");
        }

        // Sections
        for section in &self.sections {
            render_section_html(&mut html, section, 2);
        }

        html.push_str("<footer>Generated by oxo-flow</footer>\n");
        html.push_str("</body>\n</html>");
        html
    }

    /// Generate an execution summary section from job records.
    pub fn execution_summary(records: &HashMap<String, JobRecord>) -> ReportSection {
        let mut rows = Vec::new();
        for (name, record) in records {
            rows.push(vec![
                name.clone(),
                record.status.to_string(),
                record.exit_code.map(|c| c.to_string()).unwrap_or_default(),
                record.started_at.map(|t| t.to_string()).unwrap_or_default(),
                record
                    .finished_at
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
            ]);
        }

        ReportSection {
            title: "Execution Summary".to_string(),
            id: "execution-summary".to_string(),
            content: ReportContent::Table {
                headers: vec![
                    "Rule".to_string(),
                    "Status".to_string(),
                    "Exit Code".to_string(),
                    "Started".to_string(),
                    "Finished".to_string(),
                ],
                rows,
            },
            subsections: Vec::new(),
        }
    }
}

/// Quality-control metric for a single sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QcMetric {
    /// Sample identifier.
    pub sample: String,
    /// Total number of reads.
    pub total_reads: u64,
    /// Number of reads that mapped to the reference.
    pub mapped_reads: u64,
    /// Fraction of reads that mapped (0.0–1.0).
    pub mapping_rate: f64,
    /// Mean sequencing coverage depth.
    pub mean_coverage: f64,
    /// Fraction of reads marked as duplicates (0.0–1.0).
    pub duplicate_rate: f64,
}

/// Summary of a single variant call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantSummary {
    /// Gene symbol.
    pub gene: String,
    /// HGVS or similar variant notation.
    pub variant: String,
    /// ACMG classification (e.g., "Pathogenic", "Likely Pathogenic", "VUS").
    pub classification: String,
    /// Variant allele frequency (0.0–1.0).
    pub allele_frequency: f64,
    /// Read depth at the variant site.
    pub depth: u32,
    /// Optional free-text clinical significance note.
    pub clinical_significance: Option<String>,
}

/// Create a QC metrics section with sample-level quality data.
pub fn qc_metrics_section(metrics: &[QcMetric]) -> ReportSection {
    let headers = vec![
        "Sample".to_string(),
        "Total Reads".to_string(),
        "Mapped Reads".to_string(),
        "Mapping Rate".to_string(),
        "Mean Coverage".to_string(),
        "Duplicate Rate".to_string(),
    ];

    let rows: Vec<Vec<String>> = metrics
        .iter()
        .map(|m| {
            vec![
                m.sample.clone(),
                m.total_reads.to_string(),
                m.mapped_reads.to_string(),
                format!("{:.2}%", m.mapping_rate * 100.0),
                format!("{:.1}x", m.mean_coverage),
                format!("{:.2}%", m.duplicate_rate * 100.0),
            ]
        })
        .collect();

    ReportSection {
        title: "QC Metrics".to_string(),
        id: "qc-metrics".to_string(),
        content: ReportContent::Table { headers, rows },
        subsections: Vec::new(),
    }
}

/// Create a variant summary section.
pub fn variant_summary_section(variants: &[VariantSummary]) -> ReportSection {
    let headers = vec![
        "Gene".to_string(),
        "Variant".to_string(),
        "Classification".to_string(),
        "Allele Frequency".to_string(),
        "Depth".to_string(),
        "Clinical Significance".to_string(),
    ];

    let rows: Vec<Vec<String>> = variants
        .iter()
        .map(|v| {
            vec![
                v.gene.clone(),
                v.variant.clone(),
                v.classification.clone(),
                format!("{:.4}", v.allele_frequency),
                v.depth.to_string(),
                v.clinical_significance.clone().unwrap_or_default(),
            ]
        })
        .collect();

    ReportSection {
        title: "Variant Summary".to_string(),
        id: "variant-summary".to_string(),
        content: ReportContent::Table { headers, rows },
        subsections: Vec::new(),
    }
}

/// Create a provenance section recording execution details.
pub fn provenance_section(
    workflow_name: &str,
    workflow_version: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    software_versions: &[(String, String)],
) -> ReportSection {
    let duration = end_time.signed_duration_since(start_time);
    let mut pairs = vec![
        ("Workflow".to_string(), workflow_name.to_string()),
        ("Version".to_string(), workflow_version.to_string()),
        ("Start Time".to_string(), start_time.to_rfc3339()),
        ("End Time".to_string(), end_time.to_rfc3339()),
        (
            "Duration".to_string(),
            format!("{}s", duration.num_seconds()),
        ),
    ];

    for (name, version) in software_versions {
        pairs.push((name.clone(), version.clone()));
    }

    ReportSection {
        title: "Provenance".to_string(),
        id: "provenance".to_string(),
        content: ReportContent::KeyValue { pairs },
        subsections: Vec::new(),
    }
}

/// Create an execution time chart section from job records.
///
/// Generates an inline SVG bar chart showing the wall-clock time
/// for each rule, sorted by duration (longest first).
pub fn execution_time_chart(records: &HashMap<String, JobRecord>) -> ReportSection {
    let mut entries: Vec<(String, f64)> = records
        .iter()
        .filter_map(
            |(name, record)| match (record.started_at, record.finished_at) {
                (Some(start), Some(end)) => {
                    let duration = end.signed_duration_since(start);
                    Some((name.clone(), duration.num_milliseconds() as f64 / 1000.0))
                }
                _ => None,
            },
        )
        .collect();

    // Sort by duration descending
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let labels: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    let values: Vec<f64> = entries.iter().map(|(_, dur)| *dur).collect();

    ReportSection {
        title: "Execution Time".to_string(),
        id: "execution-time-chart".to_string(),
        content: ReportContent::Chart {
            title: "Rule Execution Time".to_string(),
            labels,
            values,
            unit: "s".to_string(),
        },
        subsections: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Default Tera template (embedded as a constant)
// ---------------------------------------------------------------------------

const DEFAULT_REPORT_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{{ title }}</title>
  <style>
    :root { --primary: #2c5282; --bg: #f7fafc; --text: #1a202c; }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      color: var(--text); background: var(--bg); max-width: 960px;
      margin: 0 auto; padding: 2rem; line-height: 1.6;
    }
    header { border-bottom: 3px solid var(--primary); padding-bottom: 1rem; margin-bottom: 2rem; }
    header h1 { color: var(--primary); font-size: 1.8rem; }
    .meta { color: #718096; font-size: 0.85rem; margin-top: 0.25rem; }
    section { margin-bottom: 2rem; }
    h2 { color: var(--primary); font-size: 1.3rem; border-bottom: 1px solid #e2e8f0; padding-bottom: 0.4rem; margin-bottom: 0.8rem; }
    table { border-collapse: collapse; width: 100%; margin: 0.5rem 0; font-size: 0.9rem; }
    th, td { border: 1px solid #cbd5e0; padding: 0.5rem 0.75rem; text-align: left; }
    th { background: var(--primary); color: #fff; }
    tr:nth-child(even) { background: #edf2f7; }
    dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.3rem 1rem; }
    dt { font-weight: 600; }
    dd { margin: 0; }
    pre { background: #edf2f7; padding: 1rem; overflow-x: auto; border-radius: 4px; font-size: 0.85rem; }
    p { margin-bottom: 0.5rem; }
    footer { margin-top: 3rem; border-top: 1px solid #e2e8f0; padding-top: 0.5rem; color: #a0aec0; font-size: 0.75rem; }
  </style>
</head>
<body>
  <header>
    <h1>{{ title }}</h1>
    <p class="meta">Workflow: {{ workflow_name }} v{{ workflow_version }} &middot; Generated: {{ generated_at }}</p>
    {% for key, value in metadata %}
    <p class="meta">{{ key }}: {{ value }}</p>
    {% endfor %}
  </header>

  {% for section in sections %}
  <section id="{{ section.id }}">
    <h2>{{ section.title }}</h2>
    {% if section.content.type == "Text" %}
      <p>{{ section.content.text }}</p>
    {% elif section.content.type == "Markdown" %}
      <pre>{{ section.content.markdown }}</pre>
    {% elif section.content.type == "Html" %}
      {{ section.content.html }}
    {% elif section.content.type == "Table" %}
      <table>
        <thead><tr>
          {% for h in section.content.headers %}<th>{{ h }}</th>{% endfor %}
        </tr></thead>
        <tbody>
          {% for row in section.content.rows %}
          <tr>{% for cell in row %}<td>{{ cell }}</td>{% endfor %}</tr>
          {% endfor %}
        </tbody>
      </table>
    {% elif section.content.type == "KeyValue" %}
      <dl>
        {% for pair in section.content.pairs %}
          <dt>{{ pair.0 }}</dt><dd>{{ pair.1 }}</dd>
        {% endfor %}
      </dl>
    {% elif section.content.type == "Json" %}
      <pre><code>{{ section.content.data }}</code></pre>
    {% endif %}

    {% for sub in section.subsections %}
    <section id="{{ sub.id }}">
      <h2>{{ sub.title }}</h2>
      {% if sub.content.type == "Text" %}
        <p>{{ sub.content.text }}</p>
      {% elif sub.content.type == "Table" %}
        <table>
          <thead><tr>
            {% for h in sub.content.headers %}<th>{{ h }}</th>{% endfor %}
          </tr></thead>
          <tbody>
            {% for row in sub.content.rows %}
            <tr>{% for cell in row %}<td>{{ cell }}</td>{% endfor %}</tr>
            {% endfor %}
          </tbody>
        </table>
      {% elif sub.content.type == "KeyValue" %}
        <dl>
          {% for pair in sub.content.pairs %}
            <dt>{{ pair.0 }}</dt><dd>{{ pair.1 }}</dd>
          {% endfor %}
        </dl>
      {% endif %}
    </section>
    {% endfor %}
  </section>
  {% endfor %}

  <footer>Generated by oxo-flow</footer>
</body>
</html>"#;

/// Template engine using Tera for report rendering.
pub struct TemplateEngine {
    tera: tera::Tera,
}

impl TemplateEngine {
    /// Create a new engine pre-loaded with the built-in default templates.
    pub fn new() -> Result<Self> {
        let mut tera = tera::Tera::default();
        tera.add_raw_template("report.html", DEFAULT_REPORT_TEMPLATE)?;
        Ok(Self { tera })
    }

    /// Register a custom template under the given name.
    pub fn add_template(&mut self, name: &str, content: &str) -> Result<()> {
        self.tera.add_raw_template(name, content)?;
        Ok(())
    }

    /// Render a report using the default `"report.html"` template.
    pub fn render_report(&self, report: &Report) -> Result<String> {
        self.render_with_template("report.html", report)
    }

    /// Render a report using a named template.
    pub fn render_with_template(&self, template_name: &str, report: &Report) -> Result<String> {
        let context = self.build_context(report)?;
        self.tera
            .render(template_name, &context)
            .map_err(|e| OxoFlowError::Report {
                message: format!("template render failed: {e}"),
            })
    }

    fn build_context(&self, report: &Report) -> Result<tera::Context> {
        let value = serde_json::to_value(report)?;
        let context = tera::Context::from_value(value).map_err(|e| OxoFlowError::Report {
            message: format!("failed to build template context: {e}"),
        })?;
        Ok(context)
    }
}

/// Generate a standard clinical disclaimer section for regulatory compliance.
///
/// This section should be included in all clinical reports to clarify
/// that results require professional interpretation.
pub fn clinical_disclaimer_section() -> ReportSection {
    ReportSection {
        title: "Clinical Disclaimer".to_string(),
        id: "clinical-disclaimer".to_string(),
        content: ReportContent::Html {
            html: "<div class=\"disclaimer\">\
                <p><strong>IMPORTANT:</strong> This report is generated by an automated bioinformatics pipeline \
                and is intended for research and clinical decision support only. All findings should be \
                reviewed and interpreted by qualified medical professionals. Variant classifications are \
                based on current knowledge and databases and may be updated as new evidence becomes available.</p>\
                <p>This report does not constitute a medical diagnosis. Clinical correlation and confirmatory \
                testing (e.g., Sanger sequencing) may be required before making treatment decisions.</p>\
                </div>"
                .to_string(),
        },
        subsections: Vec::new(),
    }
}

/// Sample metadata for clinical report headers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleInfo {
    /// Sample identifier.
    pub sample_id: String,
    /// Patient identifier (anonymized).
    pub patient_id: Option<String>,
    /// Sample type (e.g., "Experiment", "Control", "Tumor", "Normal", "Blood").
    pub sample_type: String,
    /// Collection date.
    pub collection_date: Option<String>,
    /// Sequencing platform (e.g., "Illumina NovaSeq 6000").
    pub platform: Option<String>,
    /// Sequencing type (e.g., "WGS", "WES", "Panel").
    pub seq_type: Option<String>,
}

/// Create a sample information section for clinical reports.
pub fn sample_info_section(info: &SampleInfo) -> ReportSection {
    let mut pairs = vec![
        ("Sample ID".to_string(), info.sample_id.clone()),
        ("Sample Type".to_string(), info.sample_type.clone()),
    ];
    if let Some(ref pid) = info.patient_id {
        pairs.push(("Patient ID".to_string(), pid.clone()));
    }
    if let Some(ref date) = info.collection_date {
        pairs.push(("Collection Date".to_string(), date.clone()));
    }
    if let Some(ref platform) = info.platform {
        pairs.push(("Platform".to_string(), platform.clone()));
    }
    if let Some(ref st) = info.seq_type {
        pairs.push(("Sequencing Type".to_string(), st.clone()));
    }
    ReportSection {
        title: "Sample Information".to_string(),
        id: "sample-info".to_string(),
        content: ReportContent::KeyValue { pairs },
        subsections: Vec::new(),
    }
}

fn render_section_html(html: &mut String, section: &ReportSection, heading_level: u8) {
    let h = heading_level.min(6);
    html.push_str(&format!(
        "<h{h} id=\"{}\">{}</h{h}>\n",
        section.id, section.title
    ));

    match &section.content {
        ReportContent::Text { text } => {
            html.push_str(&format!("<p>{text}</p>\n"));
        }
        ReportContent::Markdown { markdown } => {
            // Simple markdown rendering (just wrap in pre for now)
            html.push_str(&format!("<pre>{markdown}</pre>\n"));
        }
        ReportContent::Html { html: content } => {
            html.push_str(content);
            html.push('\n');
        }
        ReportContent::Table { headers, rows } => {
            html.push_str("<table>\n<thead><tr>\n");
            for header in headers {
                html.push_str(&format!("  <th>{header}</th>\n"));
            }
            html.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                html.push_str("<tr>\n");
                for cell in row {
                    html.push_str(&format!("  <td>{cell}</td>\n"));
                }
                html.push_str("</tr>\n");
            }
            html.push_str("</tbody>\n</table>\n");
        }
        ReportContent::KeyValue { pairs } => {
            html.push_str("<dl>\n");
            for (key, value) in pairs {
                html.push_str(&format!("  <dt><strong>{key}</strong></dt>\n"));
                html.push_str(&format!("  <dd>{value}</dd>\n"));
            }
            html.push_str("</dl>\n");
        }
        ReportContent::Json { data } => {
            let json_str = serde_json::to_string_pretty(data).unwrap_or_default();
            html.push_str(&format!("<pre><code>{json_str}</code></pre>\n"));
        }
        ReportContent::Chart {
            title,
            labels,
            values,
            unit,
        } => {
            let max_val = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
            let bar_height = 24;
            let bar_gap = 4;
            let label_width = 120;
            let chart_width = 600;
            let svg_height = (bar_height + bar_gap) * values.len() + 40;

            html.push_str(&format!(
                "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\">\n",
                chart_width + label_width + 80,
                svg_height
            ));
            html.push_str(&format!(
                "  <text x=\"{}\" y=\"20\" font-size=\"14\" font-weight=\"bold\" fill=\"var(--text, #1a202c)\">{}</text>\n",
                (chart_width + label_width) / 2,
                title
            ));
            for (i, (label, &value)) in labels.iter().zip(values.iter()).enumerate() {
                let y = 30 + i * (bar_height + bar_gap);
                let bar_w = if max_val > 0.0 {
                    (value / max_val * chart_width as f64) as usize
                } else {
                    0
                };
                // Label
                html.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-size=\"12\" text-anchor=\"end\" fill=\"var(--text, #1a202c)\">{}</text>\n",
                    label_width - 5,
                    y + bar_height / 2 + 4,
                    label
                ));
                // Bar
                html.push_str(&format!(
                    "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#4a90d9\" rx=\"3\"/>\n",
                    label_width, y, bar_w, bar_height
                ));
                // Value label
                html.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"var(--text, #1a202c)\">{:.1} {}</text>\n",
                    label_width + bar_w + 5,
                    y + bar_height / 2 + 4,
                    value,
                    unit
                ));
            }
            html.push_str("</svg>\n");
        }
    }

    for subsection in &section.subsections {
        render_section_html(html, subsection, h + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_report() {
        let report = Report::new("Test Report", "test-pipeline", "1.0.0");
        assert_eq!(report.title, "Test Report");
        assert_eq!(report.workflow_name, "test-pipeline");
        assert!(report.sections.is_empty());
    }

    #[test]
    fn report_add_section() {
        let mut report = Report::new("Test", "test", "1.0.0");
        report.add_section(ReportSection {
            title: "Summary".to_string(),
            id: "summary".to_string(),
            content: ReportContent::Text {
                text: "All steps completed.".to_string(),
            },
            subsections: vec![],
        });

        assert_eq!(report.sections.len(), 1);
    }

    #[test]
    fn report_to_json() {
        let report = Report::new("Test", "test", "1.0.0");
        let json = report.to_json().unwrap();
        assert!(json.contains("Test"));
    }

    #[test]
    fn report_to_html() {
        let mut report = Report::new("Test Report", "pipeline", "1.0.0");
        report.add_section(ReportSection {
            title: "QC".to_string(),
            id: "qc".to_string(),
            content: ReportContent::Table {
                headers: vec!["Sample".to_string(), "Pass".to_string()],
                rows: vec![vec!["S1".to_string(), "Yes".to_string()]],
            },
            subsections: vec![],
        });

        let html = report.to_html();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Test Report"));
        assert!(html.contains("<table>"));
    }

    #[test]
    fn execution_summary_section() {
        let mut records = HashMap::new();
        records.insert(
            "step1".to_string(),
            JobRecord {
                rule: "step1".to_string(),
                status: crate::executor::JobStatus::Success,
                started_at: Some(Utc::now()),
                finished_at: Some(Utc::now()),
                exit_code: Some(0),
                stdout: None,
                stderr: None,
                command: Some("echo hello".to_string()),
                retries: 0,
                timeout: None,
            },
        );

        let section = Report::execution_summary(&records);
        assert_eq!(section.title, "Execution Summary");
        if let ReportContent::Table { headers, rows } = &section.content {
            assert_eq!(headers.len(), 5);
            assert_eq!(rows.len(), 1);
        } else {
            panic!("Expected Table content");
        }
    }

    // --- TemplateEngine tests ---

    #[test]
    fn template_engine_creation() {
        let engine = TemplateEngine::new().unwrap();
        // The default template should be registered
        let report = Report::new("Init", "wf", "0.1.0");
        let html = engine.render_report(&report).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn template_engine_add_and_render() {
        let mut engine = TemplateEngine::new().unwrap();
        engine
            .add_template("custom.html", "<h1>{{ title }}</h1>")
            .unwrap();

        let report = Report::new("Custom Title", "wf", "1.0.0");
        let html = engine.render_with_template("custom.html", &report).unwrap();
        assert!(html.contains("Custom Title"));
    }

    #[test]
    fn template_engine_render_report_with_sections() {
        let engine = TemplateEngine::new().unwrap();
        let mut report = Report::new("Full Report", "pipeline", "2.0.0");
        report.add_section(ReportSection {
            title: "Summary".to_string(),
            id: "summary".to_string(),
            content: ReportContent::Text {
                text: "Everything passed.".to_string(),
            },
            subsections: vec![],
        });
        report.add_section(ReportSection {
            title: "Data".to_string(),
            id: "data".to_string(),
            content: ReportContent::Table {
                headers: vec!["A".to_string(), "B".to_string()],
                rows: vec![vec!["1".to_string(), "2".to_string()]],
            },
            subsections: vec![],
        });

        let html = engine.render_report(&report).unwrap();
        assert!(html.contains("Full Report"));
        assert!(html.contains("Everything passed."));
        assert!(html.contains("<table>"));
    }

    #[test]
    fn template_engine_missing_template_error() {
        let engine = TemplateEngine::new().unwrap();
        let report = Report::new("Test", "wf", "1.0.0");
        let result = engine.render_with_template("nonexistent.html", &report);
        assert!(result.is_err());
    }

    // --- Clinical section tests ---

    #[test]
    fn qc_metrics_section_generation() {
        let metrics = vec![
            QcMetric {
                sample: "S1".to_string(),
                total_reads: 1_000_000,
                mapped_reads: 950_000,
                mapping_rate: 0.95,
                mean_coverage: 30.5,
                duplicate_rate: 0.12,
            },
            QcMetric {
                sample: "S2".to_string(),
                total_reads: 2_000_000,
                mapped_reads: 1_800_000,
                mapping_rate: 0.90,
                mean_coverage: 45.0,
                duplicate_rate: 0.08,
            },
        ];

        let section = qc_metrics_section(&metrics);
        assert_eq!(section.title, "QC Metrics");
        assert_eq!(section.id, "qc-metrics");
        if let ReportContent::Table { headers, rows } = &section.content {
            assert_eq!(headers.len(), 6);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], "S1");
            assert!(rows[0][3].contains("95.00%"));
            assert!(rows[0][4].contains("30.5x"));
        } else {
            panic!("Expected Table content");
        }
    }

    #[test]
    fn variant_summary_section_generation() {
        let variants = vec![VariantSummary {
            gene: "BRCA1".to_string(),
            variant: "c.5266dupC".to_string(),
            classification: "Pathogenic".to_string(),
            allele_frequency: 0.4532,
            depth: 250,
            clinical_significance: Some("Associated with breast cancer".to_string()),
        }];

        let section = variant_summary_section(&variants);
        assert_eq!(section.title, "Variant Summary");
        assert_eq!(section.id, "variant-summary");
        if let ReportContent::Table { headers, rows } = &section.content {
            assert_eq!(headers.len(), 6);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "BRCA1");
            assert_eq!(rows[0][2], "Pathogenic");
            assert!(rows[0][5].contains("breast cancer"));
        } else {
            panic!("Expected Table content");
        }
    }

    #[test]
    fn variant_summary_none_significance() {
        let variants = vec![VariantSummary {
            gene: "TP53".to_string(),
            variant: "p.R175H".to_string(),
            classification: "VUS".to_string(),
            allele_frequency: 0.12,
            depth: 100,
            clinical_significance: None,
        }];

        let section = variant_summary_section(&variants);
        if let ReportContent::Table { rows, .. } = &section.content {
            assert_eq!(rows[0][5], "");
        } else {
            panic!("Expected Table content");
        }
    }

    #[test]
    fn provenance_section_generation() {
        let start = Utc::now() - chrono::Duration::seconds(120);
        let end = Utc::now();
        let sw = vec![
            ("bwa".to_string(), "0.7.17".to_string()),
            ("samtools".to_string(), "1.18".to_string()),
        ];

        let section = provenance_section("venus", "2.0.0", start, end, &sw);
        assert_eq!(section.title, "Provenance");
        assert_eq!(section.id, "provenance");
        if let ReportContent::KeyValue { pairs } = &section.content {
            assert!(pairs.len() >= 7); // 5 base + 2 software
            assert_eq!(pairs[0].0, "Workflow");
            assert_eq!(pairs[0].1, "venus");
            assert_eq!(pairs[1].1, "2.0.0");
            assert_eq!(pairs[5].0, "bwa");
        } else {
            panic!("Expected KeyValue content");
        }
    }

    #[test]
    fn qc_metric_serialization() {
        let metric = QcMetric {
            sample: "S1".to_string(),
            total_reads: 500_000,
            mapped_reads: 480_000,
            mapping_rate: 0.96,
            mean_coverage: 25.0,
            duplicate_rate: 0.05,
        };

        let json = serde_json::to_string(&metric).unwrap();
        assert!(json.contains("\"sample\":\"S1\""));
        assert!(json.contains("\"total_reads\":500000"));

        let deser: QcMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.sample, "S1");
        assert_eq!(deser.total_reads, 500_000);
    }

    #[test]
    fn variant_summary_serialization() {
        let variant = VariantSummary {
            gene: "EGFR".to_string(),
            variant: "p.T790M".to_string(),
            classification: "Pathogenic".to_string(),
            allele_frequency: 0.35,
            depth: 300,
            clinical_significance: Some("Resistance mutation".to_string()),
        };

        let json = serde_json::to_string(&variant).unwrap();
        assert!(json.contains("\"gene\":\"EGFR\""));
        assert!(json.contains("Resistance mutation"));

        let deser: VariantSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.gene, "EGFR");
        assert_eq!(deser.depth, 300);
        assert_eq!(
            deser.clinical_significance.as_deref(),
            Some("Resistance mutation")
        );
    }

    #[test]
    fn clinical_disclaimer_section_generation() {
        let section = clinical_disclaimer_section();
        assert_eq!(section.title, "Clinical Disclaimer");
        assert_eq!(section.id, "clinical-disclaimer");
        if let ReportContent::Html { html } = &section.content {
            assert!(html.contains("IMPORTANT"));
            assert!(html.contains("automated bioinformatics pipeline"));
            assert!(html.contains("medical diagnosis"));
        } else {
            panic!("Expected Html content");
        }
    }

    #[test]
    fn sample_info_section_generation() {
        let info = SampleInfo {
            sample_id: "S001".to_string(),
            patient_id: Some("P001".to_string()),
            sample_type: "Experiment".to_string(),
            collection_date: Some("2024-01-15".to_string()),
            platform: Some("Illumina NovaSeq 6000".to_string()),
            seq_type: Some("WGS".to_string()),
        };
        let section = sample_info_section(&info);
        assert_eq!(section.title, "Sample Information");
        assert_eq!(section.id, "sample-info");
        if let ReportContent::KeyValue { pairs } = &section.content {
            assert_eq!(pairs[0], ("Sample ID".to_string(), "S001".to_string()));
            assert_eq!(
                pairs[1],
                ("Sample Type".to_string(), "Experiment".to_string())
            );
            assert!(pairs.iter().any(|(k, v)| k == "Patient ID" && v == "P001"));
            assert!(
                pairs
                    .iter()
                    .any(|(k, v)| k == "Platform" && v == "Illumina NovaSeq 6000")
            );
            assert!(pairs.len() >= 6);
        } else {
            panic!("Expected KeyValue content");
        }
    }

    #[test]
    fn sample_info_serialization() {
        let info = SampleInfo {
            sample_id: "S001".to_string(),
            patient_id: Some("P001".to_string()),
            sample_type: "Experiment".to_string(),
            collection_date: None,
            platform: None,
            seq_type: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"sample_id\":\"S001\""));
        assert!(json.contains("\"sample_type\":\"Experiment\""));
        let deser: SampleInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.sample_id, "S001");
        assert_eq!(deser.patient_id.as_deref(), Some("P001"));
        assert!(deser.collection_date.is_none());
    }

    #[test]
    fn report_provenance_section() {
        let mut report = Report::new("Test Report", "test-pipeline", "1.0.0");
        report.add_provenance("0.1.0", "abc123", "2026-04-05T10:00:00Z");
        let html = report.to_html();
        assert!(html.contains("Execution Provenance"));
        assert!(html.contains("abc123"));
    }

    #[test]
    fn report_html_has_toc() {
        let mut report = Report::new("Test", "test", "1.0.0");
        report.add_section(ReportSection {
            title: "Section One".to_string(),
            id: "section-one".to_string(),
            content: ReportContent::Text {
                text: "hello".to_string(),
            },
            subsections: vec![],
        });
        let html = report.to_html();
        assert!(
            html.contains("class=\"toc\""),
            "should have table of contents"
        );
        assert!(html.contains("href=\"#section-one\""));
        assert!(html.contains("Section One"));
    }

    #[test]
    fn report_html_has_dark_mode() {
        let report = Report::new("Test", "test", "1.0.0");
        let html = report.to_html();
        assert!(html.contains("prefers-color-scheme: dark"));
    }

    #[test]
    fn chart_section_renders_svg() {
        let section = ReportSection {
            title: "Times".to_string(),
            id: "times".to_string(),
            content: ReportContent::Chart {
                title: "Execution".to_string(),
                labels: vec!["step1".to_string(), "step2".to_string()],
                values: vec![10.5, 3.2],
                unit: "s".to_string(),
            },
            subsections: vec![],
        };
        let mut html = String::new();
        render_section_html(&mut html, &section, 2);
        assert!(html.contains("<svg"));
        assert!(html.contains("step1"));
        assert!(html.contains("step2"));
    }

    #[test]
    fn execution_time_chart_from_records() {
        let mut records = std::collections::HashMap::new();
        records.insert(
            "align".to_string(),
            crate::executor::JobRecord {
                rule: "align".to_string(),
                status: crate::executor::JobStatus::Success,
                started_at: Some(chrono::Utc::now() - chrono::Duration::seconds(60)),
                finished_at: Some(chrono::Utc::now()),
                exit_code: Some(0),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
            },
        );
        let section = execution_time_chart(&records);
        assert_eq!(section.id, "execution-time-chart");
        match &section.content {
            ReportContent::Chart { labels, values, .. } => {
                assert_eq!(labels.len(), 1);
                assert!(values[0] > 0.0);
            }
            _ => panic!("expected Chart content"),
        }
    }
}
