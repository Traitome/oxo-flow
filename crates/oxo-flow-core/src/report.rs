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

    /// QC metric with pass/fail/warn status.
    QcStatus {
        /// Metric name.
        metric: String,
        /// Current value.
        value: String,
        /// Status: "pass", "warn", or "fail".
        status: String,
        /// Acceptable threshold description.
        threshold: String,
    },

    /// A group of QC status items rendered as colored indicators.
    QcIndicatorGroup { items: Vec<QcIndicator> },

    /// Hierarchical/nested data (for pathway, GO terms, taxonomy).
    Hierarchy {
        name: String,
        value: f64,
        children: Vec<HierarchyNode>,
    },

    /// Scatter plot data (for volcano plots, PCA, UMAP).
    ScatterPlot {
        title: String,
        x_label: String,
        y_label: String,
        points: Vec<ScatterPoint>,
    },
}

/// A single QC indicator with color-coded status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QcIndicator {
    pub label: String,
    pub value: String,
    pub status: QcStatusLevel,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QcStatusLevel {
    Pass,
    Warn,
    Fail,
    Info,
}

/// Hierarchical node for tree/treemap data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyNode {
    pub name: String,
    pub value: f64,
    #[serde(default)]
    pub children: Vec<HierarchyNode>,
}

/// A single point in a scatter plot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScatterPoint {
    pub x: f64,
    pub y: f64,
    pub label: Option<String>,
    pub group: Option<String>,
    pub size: Option<f64>,
}

/// Complete report document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Schema version of the report JSON (issue #83 WS3). Bumped on any
    /// breaking change to the model; consumers can gate on it.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// Command that produced this report — makes the JSON self-identifying,
    /// mirroring the `command` field of `status --json`.
    #[serde(default = "default_command")]
    pub command: String,

    /// Report title.
    pub title: String,

    /// Report generation timestamp. `None` under `--no-timestamps`;
    /// pinned via `SOURCE_DATE_EPOCH`/`--ci` for byte-reproducible output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<DateTime<Utc>>,

    /// Workflow name.
    pub workflow_name: String,

    /// Workflow version.
    pub workflow_version: String,

    /// Report sections.
    pub sections: Vec<ReportSection>,

    /// Report metadata (arbitrary key-value pairs).
    #[serde(default)]
    pub metadata: HashMap<String, String>,

    /// Provenance: path of the checkpoint this report was built from
    /// (issue #83 WS2). Absent for template-only reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_path: Option<String>,

    /// Provenance: path of the workflow file the report describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_path: Option<String>,

    /// Provenance: git HEAD SHA of the repository the workflow lives in,
    /// recorded by the run that produced the checkpoint (issue #115
    /// pillar 1). Absent when the workflow is not in a git repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_git_sha: Option<String>,
}

const REPORT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    REPORT_SCHEMA_VERSION
}

fn default_command() -> String {
    "report".to_string()
}

impl Report {
    /// Create a new empty report.
    pub fn new(title: &str, workflow_name: &str, workflow_version: &str) -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            command: "report".to_string(),
            title: title.to_string(),
            generated_at: Some(Utc::now()),
            workflow_name: workflow_name.to_string(),
            workflow_version: workflow_version.to_string(),
            sections: Vec::new(),
            metadata: HashMap::new(),
            checkpoint_path: None,
            workflow_path: None,
            workflow_git_sha: None,
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
    /// Includes dark mode support via `prefers-color-scheme` media query and
    /// print styles, so the same file prints cleanly (issue #83).
    ///
    /// Every user-controlled string (workflow/rules names, commands, file
    /// paths) is HTML-escaped — the report is safe to open and share.
    pub fn to_html(&self) -> String {
        let mut html = String::new();
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        html.push_str(&format!("  <title>{}</title>\n", escape_html(&self.title)));
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
        html.push_str("    .skip-link { position: absolute; left: -999px; top: 0; background: var(--primary); color: #fff; padding: 0.5rem 1rem; z-index: 100; }\n");
        html.push_str("    .skip-link:focus { left: 0; }\n");
        html.push_str("    header { border-bottom: 3px solid var(--primary); padding-bottom: 1rem; margin-bottom: 2rem; }\n");
        html.push_str("    header h1 { color: var(--primary); font-size: 1.8rem; }\n");
        html.push_str("    .meta { color: #4a5568; font-size: 0.85rem; margin-top: 0.25rem; }\n");
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
        html.push_str("    footer { margin-top: 3rem; border-top: 1px solid var(--border); padding-top: 0.5rem; color: #4a5568; font-size: 0.75rem; text-align: center; }\n");
        // Print styles — part of the default output so every report prints
        // cleanly (issue #83 P2-5).
        html.push_str("    @media print {\n");
        html.push_str("      @page { margin: 2cm; size: A4; }\n");
        html.push_str("      body { max-width: none; padding: 0; font-size: 10pt; }\n");
        html.push_str("      nav.toc, .skip-link { display: none; }\n");
        html.push_str(
            "      section { page-break-inside: avoid; margin-bottom: 1cm; border: none; }\n",
        );
        html.push_str("      h2 { page-break-after: avoid; }\n");
        html.push_str("      table { page-break-inside: avoid; }\n");
        html.push_str("      footer { position: fixed; bottom: 0; left: 0; right: 0; }\n");
        html.push_str("    }\n");
        html.push_str("  </style>\n</head>\n<body>\n");

        html.push_str("<a class=\"skip-link\" href=\"#main\">Skip to content</a>\n");

        // Header
        html.push_str("<header>\n");
        html.push_str(&format!("  <h1>{}</h1>\n", escape_html(&self.title)));
        let generated = match self.generated_at {
            Some(ts) => format!(" &middot; Generated: {ts}"),
            None => String::new(),
        };
        html.push_str(&format!(
            "  <p class=\"meta\">Workflow: {} v{}{}</p>\n",
            escape_html(&self.workflow_name),
            escape_html(&self.workflow_version),
            generated
        ));
        for (key, value) in &self.metadata {
            html.push_str(&format!(
                "  <p class=\"meta\">{}: {}</p>\n",
                escape_html(key),
                escape_html(value)
            ));
        }
        html.push_str("</header>\n\n");

        // Table of contents
        if !self.sections.is_empty() {
            html.push_str("<nav class=\"toc\">\n");
            html.push_str("  <h2>Contents</h2>\n  <ul>\n");
            for section in &self.sections {
                html.push_str(&format!(
                    "    <li><a href=\"#{}\">{}</a></li>\n",
                    escape_html(&section.id),
                    escape_html(&section.title)
                ));
            }
            html.push_str("  </ul>\n</nav>\n\n");
        }

        // Sections
        html.push_str("<main id=\"main\">\n");
        for section in &self.sections {
            render_section_html(&mut html, section, 2);
        }
        html.push_str("</main>\n");

        html.push_str(&format!(
            "<footer>Generated by oxo-flow v{} &middot; {} v{}</footer>\n",
            env!("CARGO_PKG_VERSION"),
            escape_html(&self.workflow_name),
            escape_html(&self.workflow_version)
        ));
        html.push_str("</body>\n</html>");
        html
    }

    /// Render the report as HTML optimized for PDF generation.
    ///
    /// Print styles are part of the default output (issue #83 P2-5), so this
    /// is an alias kept for API compatibility.
    pub fn to_printable_html(&self) -> String {
        self.to_html()
    }

    /// Generate a PDF using wkhtmltopdf command.
    ///
    /// Returns the command string to execute. Requires wkhtmltopdf to be installed.
    /// The output PDF will be saved to the specified path.
    ///
    /// # Arguments
    /// * `output_path` - Path to save the PDF file
    /// * `options` - Additional wkhtmltopdf options (e.g., "--enable-local-file-access")
    ///
    /// # Example
    /// ```rust,ignore
    /// let report = Report::new("Clinical Report", "venus", "1.0.0");
    /// let cmd = report.to_pdf_command("report.pdf", vec!["--enable-local-file-access"]);
    /// // Execute: std::process::Command::new("sh").arg("-c").arg(cmd).output()
    /// ```
    pub fn to_pdf_command(&self, output_path: &str, options: Vec<&str>) -> String {
        let html = self.to_printable_html();
        let opts = options.join(" ");
        format!(
            "wkhtmltopdf {} --encoding utf-8 \"{}\" \"{}\"",
            if opts.is_empty() { "" } else { &opts },
            html.replace('"', "\\\"").replace('\n', " "),
            output_path
        )
    }

    /// Generate PDF asynchronously using embedded HTML.
    ///
    /// Writes HTML to a temporary file and calls wkhtmltopdf.
    /// Requires wkhtmltopdf to be installed on the system.
    pub async fn to_pdf(&self, output_path: &std::path::Path) -> Result<()> {
        let html = self.to_printable_html();

        // Write HTML to temp file
        let temp_dir = tempfile::tempdir()?;
        let html_path = temp_dir.path().join("report.html");
        tokio::fs::write(&html_path, &html).await?;

        // Run wkhtmltopdf
        let output = tokio::process::Command::new("wkhtmltopdf")
            .arg("--enable-local-file-access")
            .arg("--encoding")
            .arg("utf-8")
            .arg("--page-size")
            .arg("A4")
            .arg("--margin-top")
            .arg("20mm")
            .arg("--margin-bottom")
            .arg("20mm")
            .arg("--margin-left")
            .arg("20mm")
            .arg("--margin-right")
            .arg("20mm")
            .arg(&html_path)
            .arg(output_path)
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                tracing::info!(path = %output_path.display(), "PDF generated successfully");
                Ok(())
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                Err(OxoFlowError::Validation {
                    message: format!("wkhtmltopdf failed: {}", stderr),
                    rule: None,
                    suggestion: Some("Ensure wkhtmltopdf is installed (brew install wkhtmltopdf on macOS)".to_string()),
                })
            }
            Err(e) => {
                Err(OxoFlowError::Validation {
                    message: format!("failed to run wkhtmltopdf: {}", e),
                    rule: None,
                    suggestion: Some("Install wkhtmltopdf: brew install wkhtmltopdf (macOS) or apt install wkhtmltopdf (Linux)".to_string()),
                })
            }
        }
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

    /// Render the report as Markdown with GFM tables — git-friendly and
    /// doc-embeddable (issue #83 P1-9). A projection of the same model as
    /// HTML/JSON, never a separate data path.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!("# {}\n\n", self.title));
        match self.generated_at {
            Some(ts) => md.push_str(&format!(
                "Workflow: {} v{} · Generated: {ts}\n\n",
                self.workflow_name, self.workflow_version
            )),
            None => md.push_str(&format!(
                "Workflow: {} v{}\n\n",
                self.workflow_name, self.workflow_version
            )),
        }
        for section in &self.sections {
            render_section_markdown(&mut md, section, 2);
        }
        md
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
// Alignment statistics section
// ---------------------------------------------------------------------------

/// Alignment statistics for a single sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignmentStats {
    pub sample: String,
    pub total_reads: u64,
    pub mapped_reads: u64,
    pub properly_paired: u64,
    pub singletons: u64,
    pub duplicates: u64,
    pub mapping_rate: f64,
    pub mean_coverage: Option<f64>,
    pub mean_insert_size: Option<f64>,
    pub gc_content: Option<f64>,
}

/// Create an alignment statistics section from per-sample alignment data.
pub fn alignment_stats_section(stats: &[AlignmentStats]) -> ReportSection {
    let headers = vec![
        "Sample",
        "Total Reads",
        "Mapped Reads",
        "Mapping Rate",
        "Properly Paired",
        "Duplicates",
        "Mean Coverage",
        "GC%",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let rows: Vec<Vec<String>> = stats
        .iter()
        .map(|s| {
            vec![
                s.sample.clone(),
                s.total_reads.to_string(),
                s.mapped_reads.to_string(),
                format!("{:.2}%", s.mapping_rate * 100.0),
                format!(
                    "{:.2}%",
                    s.properly_paired as f64 / s.total_reads as f64 * 100.0
                ),
                format!("{:.2}%", s.duplicates as f64 / s.total_reads as f64 * 100.0),
                s.mean_coverage
                    .map_or("N/A".into(), |c| format!("{:.1}x", c)),
                s.gc_content
                    .map_or("N/A".into(), |g| format!("{:.1}%", g * 100.0)),
            ]
        })
        .collect();

    ReportSection {
        title: "Alignment Statistics".to_string(),
        id: "alignment-stats".to_string(),
        content: ReportContent::Table { headers, rows },
        subsections: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// RNA-seq expression summary section
// ---------------------------------------------------------------------------

/// Expression data for a single gene across samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionRecord {
    pub gene: String,
    pub sample: String,
    pub tpm: f64,
    pub count: u64,
}

/// Create an RNA-seq expression summary with top expressed genes.
pub fn expression_summary_section(records: &[ExpressionRecord], top_n: usize) -> ReportSection {
    // Aggregate by gene: average TPM across samples
    let mut gene_tpm: HashMap<String, (f64, u64)> = HashMap::new();
    for r in records {
        let entry = gene_tpm.entry(r.gene.clone()).or_insert((0.0, 0));
        entry.0 += r.tpm;
        entry.1 += 1;
    }
    let mut genes: Vec<(String, f64)> = gene_tpm
        .into_iter()
        .map(|(g, (sum, n))| (g, sum / n as f64))
        .collect();
    genes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    genes.truncate(top_n);

    let headers = vec!["Rank", "Gene", "Mean TPM"]
        .into_iter()
        .map(String::from)
        .collect();
    let rows: Vec<Vec<String>> = genes
        .iter()
        .enumerate()
        .map(|(i, (gene, tpm))| vec![(i + 1).to_string(), gene.clone(), format!("{:.2}", tpm)])
        .collect();

    ReportSection {
        title: format!("Top {} Expressed Genes", top_n),
        id: "expression-summary".to_string(),
        content: ReportContent::Table { headers, rows },
        subsections: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Multi-sample comparison section
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Resource usage summary
// ---------------------------------------------------------------------------

/// Per-rule resource usage for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub rule: String,
    pub wall_time_secs: f64,
    pub max_memory_mb: Option<u64>,
    pub cpu_seconds: Option<f64>,
    pub threads: u32,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Dashboard / overview section
// ---------------------------------------------------------------------------

/// Key metrics for the dashboard overview at the top of reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardMetrics {
    pub pipeline_name: String,
    pub total_samples: usize,
    pub total_rules: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub total_reads_processed: Option<u64>,
    pub mean_mapping_rate: Option<f64>,
    pub variants_detected: Option<usize>,
    pub actionable_variants: Option<usize>,
    pub differentially_expressed_genes: Option<usize>,
    pub total_runtime_secs: Option<f64>,
}

/// Create a dashboard overview section with key QC indicators.
pub fn dashboard_section(metrics: &DashboardMetrics) -> ReportSection {
    let mut status_items = Vec::new();

    // Pipeline success status
    let status = if metrics.failed == 0 {
        QcStatusLevel::Pass
    } else {
        QcStatusLevel::Warn
    };
    let desc = if metrics.failed == 0 {
        "All rules completed successfully".to_string()
    } else {
        format!("{} rule(s) failed", metrics.failed)
    };
    status_items.push(QcIndicator {
        label: "Pipeline Status".into(),
        value: format!("{}/{} succeeded", metrics.succeeded, metrics.total_rules),
        status,
        description: desc,
    });

    // Sample count
    status_items.push(QcIndicator {
        label: "Samples Processed".into(),
        value: metrics.total_samples.to_string(),
        status: QcStatusLevel::Info,
        description: "Total samples in cohort".into(),
    });

    // Mapping rate
    if let Some(rate) = metrics.mean_mapping_rate {
        let (s, d) = if rate > 0.90 {
            (QcStatusLevel::Pass, "Excellent mapping rate")
        } else if rate > 0.70 {
            (QcStatusLevel::Warn, "Below expected mapping rate")
        } else {
            (
                QcStatusLevel::Fail,
                "Poor mapping rate — check sample quality",
            )
        };
        status_items.push(QcIndicator {
            label: "Mean Mapping Rate".into(),
            value: format!("{:.1}%", rate * 100.0),
            status: s,
            description: d.into(),
        });
    }

    // Variants
    if let Some(v) = metrics.variants_detected {
        status_items.push(QcIndicator {
            label: "Variants Detected".into(),
            value: v.to_string(),
            status: QcStatusLevel::Info,
            description: "Total variants called".into(),
        });
    }
    if let Some(av) = metrics.actionable_variants {
        status_items.push(QcIndicator {
            label: "Actionable Variants".into(),
            value: av.to_string(),
            status: if av > 0 {
                QcStatusLevel::Warn
            } else {
                QcStatusLevel::Pass
            },
            description: "Clinically actionable findings".into(),
        });
    }

    // DEGs
    if let Some(deg) = metrics.differentially_expressed_genes {
        status_items.push(QcIndicator {
            label: "Differentially Expressed Genes".into(),
            value: deg.to_string(),
            status: QcStatusLevel::Info,
            description: "Genes with |log2FC| > 1 and adj.p < 0.05".into(),
        });
    }

    // Runtime
    if let Some(runtime) = metrics.total_runtime_secs {
        status_items.push(QcIndicator {
            label: "Total Runtime".into(),
            value: if runtime > 3600.0 {
                format!("{:.1}h", runtime / 3600.0)
            } else if runtime > 60.0 {
                format!("{:.1}min", runtime / 60.0)
            } else {
                format!("{:.0}s", runtime)
            },
            status: QcStatusLevel::Info,
            description: "Total wall clock time".into(),
        });
    }

    ReportSection {
        title: "Dashboard".to_string(),
        id: "dashboard".to_string(),
        content: ReportContent::QcIndicatorGroup {
            items: status_items,
        },
        subsections: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Report builder (fluent API)
// ---------------------------------------------------------------------------

/// Fluent builder for constructing reports programmatically.
pub struct ReportBuilder {
    report: Report,
}

impl ReportBuilder {
    pub fn new(title: &str, workflow_name: &str, workflow_version: &str) -> Self {
        Self {
            report: Report::new(title, workflow_name, workflow_version),
        }
    }

    pub fn metadata(mut self, key: &str, value: &str) -> Self {
        self.report.add_metadata(key, value);
        self
    }

    pub fn section(mut self, section: ReportSection) -> Self {
        self.report.add_section(section);
        self
    }

    pub fn dashboard(mut self, metrics: &DashboardMetrics) -> Self {
        self.report.add_section(dashboard_section(metrics));
        self
    }

    pub fn qc_metrics(mut self, metrics: &[QcMetric]) -> Self {
        self.report.add_section(qc_metrics_section(metrics));
        self
    }

    pub fn alignment_stats(mut self, stats: &[AlignmentStats]) -> Self {
        self.report.add_section(alignment_stats_section(stats));
        self
    }

    pub fn expression(mut self, records: &[ExpressionRecord], top_n: usize) -> Self {
        self.report
            .add_section(expression_summary_section(records, top_n));
        self
    }

    pub fn variants(mut self, variants: &[VariantSummary]) -> Self {
        self.report.add_section(variant_summary_section(variants));
        self
    }

    pub fn provenance(
        mut self,
        wf_name: &str,
        wf_version: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        sw: &[(String, String)],
    ) -> Self {
        self.report
            .add_section(provenance_section(wf_name, wf_version, start, end, sw));
        self
    }

    /// Add a task summary section showing all rules with their shell commands.
    pub fn task_summary(mut self, rules: &[crate::rule::Rule]) -> Self {
        self.report.add_section(task_summary_section(rules));
        self
    }

    /// Provenance: the checkpoint path this report was built from
    /// (issue #83 WS2).
    pub fn checkpoint_path(mut self, path: Option<String>) -> Self {
        self.report.checkpoint_path = path;
        self
    }

    /// Provenance: the workflow file path the report describes.
    pub fn workflow_path(mut self, path: Option<String>) -> Self {
        self.report.workflow_path = path;
        self
    }

    /// Provenance: the workflow repository's git HEAD SHA (issue #115
    /// pillar 1) — which workflow version produced these results.
    pub fn workflow_git_sha(mut self, sha: Option<String>) -> Self {
        self.report.workflow_git_sha = sha;
        self
    }

    /// Pin the generation timestamp (`--ci` / `--no-timestamps`, issue #83
    /// P1-4). `None` omits the timestamp from the output entirely.
    pub fn generated_at(mut self, timestamp: Option<DateTime<Utc>>) -> Self {
        self.report.generated_at = timestamp;
        self
    }

    pub fn build(self) -> Report {
        self.report
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

/// The embedded default Tera report template — the scaffold source for
/// `oxo-flow report --init-template` (issue #83 WS5): users copy it to
/// `report-template.tera`, customize it, and wire it up via
/// `[report].template = "report-template.tera"`.
pub fn builtin_template() -> &'static str {
    DEFAULT_REPORT_TEMPLATE
}

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

/// Report language for internationalized sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportLanguage {
    English,
    Chinese,
}

impl ReportLanguage {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "zh" | "cn" | "chinese" | "zh-cn" | "zh_cn" => Self::Chinese,
            _ => Self::English,
        }
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

/// Escape special HTML characters in user-controlled text to prevent XSS.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Escape a table cell for Markdown (GFM: backslash-escape pipes).
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Render one section (and its subsections) as Markdown.
fn render_section_markdown(md: &mut String, section: &ReportSection, level: usize) {
    let hashes = "#".repeat(level.min(6));
    md.push_str(&format!("{hashes} {}\n\n", section.title));
    match &section.content {
        ReportContent::Text { text } => md.push_str(&format!("{text}\n\n")),
        ReportContent::Markdown { markdown } => {
            md.push_str("```text\n");
            md.push_str(markdown);
            md.push_str("\n```\n\n");
        }
        ReportContent::Html { .. } => {
            // Raw HTML has no Markdown projection.
        }
        ReportContent::Table { headers, rows } => {
            md.push_str(&format!(
                "| {} |\n",
                headers
                    .iter()
                    .map(|h| md_cell(h))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            md.push_str(&format!(
                "|{}|\n",
                headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
            ));
            for row in rows {
                md.push_str(&format!(
                    "| {} |\n",
                    row.iter()
                        .map(|c| md_cell(c))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            md.push('\n');
        }
        ReportContent::KeyValue { pairs } => {
            for (key, value) in pairs {
                md.push_str(&format!("- **{key}**: {value}\n"));
            }
            md.push('\n');
        }
        ReportContent::Json { data } => {
            md.push_str("```json\n");
            md.push_str(&serde_json::to_string_pretty(data).unwrap_or_default());
            md.push_str("\n```\n\n");
        }
        ReportContent::Chart {
            title,
            labels,
            values,
            unit,
        } => {
            md.push_str(&format!("*{title}*\n\n"));
            for (label, value) in labels.iter().zip(values.iter()) {
                md.push_str(&format!("- {label}: {value:.1} {unit}\n"));
            }
            md.push('\n');
        }
        ReportContent::QcStatus {
            metric,
            value,
            status,
            threshold,
        } => {
            md.push_str(&format!(
                "- **{metric}**: {value} ({status}; threshold: {threshold})\n\n"
            ));
        }
        ReportContent::QcIndicatorGroup { items } => {
            for item in items {
                let mark = match item.status {
                    QcStatusLevel::Pass => "\u{2705}",
                    QcStatusLevel::Warn => "\u{26A0}\u{FE0F}",
                    QcStatusLevel::Fail => "\u{274C}",
                    QcStatusLevel::Info => "\u{2139}\u{FE0F}",
                };
                md.push_str(&format!(
                    "- {mark} **{}**: {} — {}\n",
                    item.label, item.value, item.description
                ));
            }
            md.push('\n');
        }
        ReportContent::Hierarchy {
            name,
            value,
            children,
        } => {
            let root = HierarchyNode {
                name: name.clone(),
                value: *value,
                children: children.clone(),
            };
            fn flatten_md(nodes: &[HierarchyNode], md: &mut String, depth: usize) {
                for node in nodes {
                    md.push_str(&format!(
                        "{}- {}: {}\n",
                        "  ".repeat(depth),
                        node.name,
                        node.value
                    ));
                    flatten_md(&node.children, md, depth + 1);
                }
            }
            flatten_md(&[root], md, 0);
            md.push('\n');
        }
        ReportContent::ScatterPlot {
            title,
            x_label,
            y_label,
            points,
        } => {
            md.push_str(&format!("**{title}** ({x_label} vs {y_label})\n\n"));
            md.push_str(&format!("| {x_label} | {y_label} |\n|---|---|\n"));
            for point in points {
                md.push_str(&format!("| {} | {} |\n", point.x, point.y));
            }
            md.push('\n');
        }
    }
    for subsection in &section.subsections {
        render_section_markdown(md, subsection, level + 1);
    }
}

fn render_section_html(html: &mut String, section: &ReportSection, heading_level: u8) {
    let h = heading_level.min(6);
    html.push_str(&format!(
        "<h{h} id=\"{}\">{}</h{h}>\n",
        escape_html(&section.id),
        escape_html(&section.title)
    ));

    match &section.content {
        ReportContent::Text { text } => {
            html.push_str(&format!("<p>{}</p>\n", escape_html(text)));
        }
        ReportContent::Markdown { markdown } => {
            // Markdown is rendered as an escaped preformatted block — no
            // HTML interpretation (XSS-safe) and no fake formatting.
            html.push_str(&format!("<pre>{}</pre>\n", escape_html(markdown)));
        }
        ReportContent::Html { html: content } => {
            // Explicitly raw HTML — trusted content only, by contract.
            html.push_str(content);
            html.push('\n');
        }
        ReportContent::Table { headers, rows } => {
            html.push_str("<table>\n<thead><tr>\n");
            for header in headers {
                html.push_str(&format!(
                    "  <th scope=\"col\">{}</th>\n",
                    escape_html(header)
                ));
            }
            html.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                html.push_str("<tr>\n");
                for cell in row {
                    html.push_str(&format!("  <td>{}</td>\n", escape_html(cell)));
                }
                html.push_str("</tr>\n");
            }
            html.push_str("</tbody>\n</table>\n");
        }
        ReportContent::KeyValue { pairs } => {
            html.push_str("<dl>\n");
            for (key, value) in pairs {
                html.push_str(&format!(
                    "  <dt><strong>{}</strong></dt>\n",
                    escape_html(key)
                ));
                html.push_str(&format!("  <dd>{}</dd>\n", escape_html(value)));
            }
            html.push_str("</dl>\n");
        }
        ReportContent::Json { data } => {
            let json_str = serde_json::to_string_pretty(data).unwrap_or_default();
            html.push_str(&format!(
                "<pre><code>{}</code></pre>\n",
                escape_html(&json_str)
            ));
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

            // role="img" + aria-label make the chart readable by screen
            // readers; the title/labels are repeated in text form (issue #83
            // P1-16).
            html.push_str(&format!(
                "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" aria-label=\"{}\">\n",
                chart_width + label_width + 80,
                svg_height,
                escape_html(title)
            ));
            html.push_str(&format!(
                "  <text x=\"{}\" y=\"20\" font-size=\"14\" font-weight=\"bold\" fill=\"var(--text, #1a202c)\">{}</text>\n",
                (chart_width + label_width) / 2,
                escape_html(title)
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
                    escape_html(label)
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
                    escape_html(unit)
                ));
            }
            html.push_str("</svg>\n");
        }
        ReportContent::QcStatus {
            metric,
            value,
            status,
            threshold,
        } => {
            // Contrast-safe status colors (WCAG AA on white, issue #83 P1-16).
            let (icon, color) = match status.as_str() {
                "pass" => ("\u{2713}", "#047857"),
                "warn" => ("\u{26A0}", "#b45309"),
                "fail" => ("\u{2717}", "#b91c1c"),
                _ => ("\u{2139}", "#1d4ed8"),
            };
            html.push_str(&format!(
                "<div style=\"display:flex;align-items:center;gap:0.5rem;padding:0.5rem;border-left:4px solid {color};margin:0.5rem 0;background:var(--card-bg)\">\n"
            ));
            html.push_str(&format!(
                "  <span style=\"color:{color};font-size:1.2rem\">{icon}</span>\n"
            ));
            html.push_str(&format!(
                "  <strong>{}</strong> {} <span style=\"color:#4a5568\">(threshold: {})</span>\n",
                escape_html(metric),
                escape_html(value),
                escape_html(threshold)
            ));
            html.push_str("</div>\n");
        }
        ReportContent::QcIndicatorGroup { items } => {
            html.push_str("<div style=\"display:flex;flex-wrap:wrap;gap:1rem;margin:1rem 0\">\n");
            for item in items {
                let (icon, bg, border) = match item.status {
                    QcStatusLevel::Pass => ("\u{2713}", "#ecfdf5", "#047857"),
                    QcStatusLevel::Warn => ("\u{26A0}", "#fffbeb", "#b45309"),
                    QcStatusLevel::Fail => ("\u{2717}", "#fef2f2", "#b91c1c"),
                    QcStatusLevel::Info => ("\u{2139}", "#eff6ff", "#1d4ed8"),
                };
                html.push_str(&format!(
                    "<div style=\"flex:1;min-width:200px;background:{bg};border:1px solid {border};border-radius:8px;padding:1rem\">\n"
                ));
                html.push_str(&format!(
                    "  <div style=\"font-size:1.5rem;margin-bottom:0.25rem\">{icon}</div>\n"
                ));
                html.push_str(&format!(
                    "  <div style=\"font-size:1.8rem;font-weight:700\">{}</div>\n",
                    escape_html(&item.value)
                ));
                html.push_str(&format!(
                    "  <div style=\"color:#4b5563;font-size:0.85rem\">{}</div>\n",
                    escape_html(&item.label)
                ));
                html.push_str(&format!(
                    "  <div style=\"color:#6b7280;font-size:0.75rem;margin-top:0.25rem\">{}</div>\n",
                    escape_html(&item.description)
                ));
                html.push_str("</div>\n");
            }
            html.push_str("</div>\n");
        }
        ReportContent::Hierarchy {
            name,
            value,
            children,
        } => {
            // Flat table rendering — real data instead of a dead promise
            // about an interactive viewer (issue #83 P1-10).
            let mut rows: Vec<Vec<String>> = Vec::new();
            fn flatten(nodes: &[HierarchyNode], rows: &mut Vec<Vec<String>>) {
                for node in nodes {
                    rows.push(vec![
                        node.name.clone(),
                        node.value.to_string(),
                        node.children.len().to_string(),
                    ]);
                    flatten(&node.children, rows);
                }
            }
            flatten(
                &[HierarchyNode {
                    name: name.clone(),
                    value: *value,
                    children: children.clone(),
                }],
                &mut rows,
            );
            html.push_str("<table>\n<thead><tr>\n");
            html.push_str("  <th scope=\"col\">Node</th><th scope=\"col\">Value</th><th scope=\"col\">Children</th>\n");
            html.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                html.push_str("<tr>\n");
                for cell in row {
                    html.push_str(&format!("  <td>{}</td>\n", escape_html(&cell)));
                }
                html.push_str("</tr>\n");
            }
            html.push_str("</tbody>\n</table>\n");
        }
        ReportContent::ScatterPlot {
            title,
            x_label,
            y_label,
            points,
        } => {
            // Table rendering of the points — real data instead of a dead
            // promise about an interactive viewer (issue #83 P1-10).
            let rows: Vec<Vec<String>> = points
                .iter()
                .map(|p| vec![p.x.to_string(), p.y.to_string()])
                .collect();
            html.push_str(&format!(
                "<p><strong>{}</strong> — {} vs {}</p>\n",
                escape_html(title),
                escape_html(x_label),
                escape_html(y_label)
            ));
            html.push_str("<table>\n<thead><tr>\n");
            html.push_str(&format!(
                "  <th scope=\"col\">{}</th><th scope=\"col\">{}</th>\n",
                escape_html(x_label),
                escape_html(y_label)
            ));
            html.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                html.push_str("<tr>\n");
                for cell in row {
                    html.push_str(&format!("  <td>{}</td>\n", escape_html(&cell)));
                }
                html.push_str("</tr>\n");
            }
            html.push_str("</tbody>\n</table>\n");
        }
    }

    for subsection in &section.subsections {
        render_section_html(html, subsection, h + 1);
    }
}

// ===========================================================================
// Pluggable report section system
// ===========================================================================
//
// Report sections are generated by registered `ReportSectionGenerator`
// implementations — similar to how `ResultExtractor` plugins work for
// output file parsing.  Users can filter which sections appear via the
// `[report].sections` config in their .oxoflow file.
//
// Built-in generators cover: universal (dashboard, metadata), execution
// status, clinical compliance, I/O manifest, commands, and environment.
// Domain-specific generators can be added by implementing the trait and
// registering with `SectionRegistry`.

use crate::config::WorkflowConfig;
use crate::executor::CheckpointState;
use crate::report_metrics::{MetricsScanner, ParsedMetrics};

/// Classifies a workflow into a broad domain for report tailoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowDomain {
    DnaSequencing,
    RnaSequencing,
    Epigenomics,
    Clinical,
    Generic,
}

impl WorkflowDomain {
    /// Classify a workflow from the tools its shell commands reference.
    ///
    /// Ordering matters: DNA tool signals (GATK/Picard/variant callers)
    /// win over RNA aligner signals, because GATK-based variant calling
    /// pipelines legitimately contain `STAR`/`featureCounts` rules —
    /// the reverse (a pure RNA pipeline calling HaplotypeCaller) is rare
    /// (issue #83 P0-3). `Clinical` is never inferred from commands alone:
    /// no shell vocabulary reliably indicates clinical reporting, so it is
    /// only reachable through explicit configuration.
    pub fn detect(rules: &[crate::rule::Rule]) -> Self {
        let shells: Vec<&str> = rules.iter().filter_map(|r| r.shell.as_deref()).collect();
        let joined = shells.join(" ").to_lowercase();
        let has_dna_tools = joined.contains("haplotypecaller")
            || joined.contains("mutect2")
            || joined.contains("bwa mem")
            || joined.contains("bwa-mem")
            || joined.contains("gatk")
            || joined.contains("picard");
        let has_rna_tools = joined.contains("star ")
            || joined.contains("featurecounts")
            || joined.contains("salmon")
            || joined.contains("kallisto");
        let has_epi_tools = joined.contains("macs2")
            || joined.contains("macs3")
            || joined.contains("atac")
            || joined.contains("methylation");

        if has_dna_tools {
            if has_epi_tools {
                return Self::Epigenomics;
            }
            return Self::DnaSequencing;
        }
        if has_rna_tools {
            return Self::RnaSequencing;
        }
        if has_epi_tools {
            return Self::Epigenomics;
        }
        Self::Generic
    }
}

/// Context passed to each report section generator.
pub struct ReportContext<'a> {
    pub config: &'a WorkflowConfig,
    pub checkpoint: Option<&'a CheckpointState>,
    pub domain: WorkflowDomain,
    /// Path of the workflow file the report describes (issue #83 WS2).
    pub workflow_path: Option<&'a std::path::Path>,
    /// Path of the checkpoint the report was built from, when present.
    pub checkpoint_path: Option<&'a std::path::Path>,
}

/// Trait for pluggable report section generators.
pub trait ReportSectionGenerator: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn applicable(&self, ctx: &ReportContext) -> bool;
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection>;
}

/// Registry of report section generators — pluggable like ResultExtractorRegistry.
pub struct SectionRegistry {
    generators: Vec<Box<dyn ReportSectionGenerator>>,
}

impl SectionRegistry {
    pub fn new() -> Self {
        Self {
            generators: Vec::new(),
        }
    }

    pub fn register(&mut self, generator: Box<dyn ReportSectionGenerator>) {
        self.generators.push(generator);
    }

    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(UniversalGenerator));
        registry.register(Box::new(ExecutionStatusGenerator));
        registry.register(Box::new(FailureDiagnosisGenerator));
        registry.register(Box::new(ClinicalComplianceGenerator));
        registry.register(Box::new(WorkflowInfoGenerator));
        registry.register(Box::new(CommandManifestGenerator));
        registry.register(Box::new(IoManifestGenerator));
        registry.register(Box::new(EnvironmentInfoGenerator));
        registry.register(Box::new(MetricsGenerator));
        registry.register(Box::new(SampleMatrixGenerator));
        registry.register(Box::new(ProvenanceGenerator));
        registry.register(Box::new(TaskSummaryGenerator));
        registry
    }

    /// Generate sections. When a `filter` is given, generators listed in it
    /// always run (an explicit choice overrides `applicable()` — e.g. the
    /// clinical-compliance section is only otherwise shown for clinical
    /// workflows); unlisted generators are skipped. Without a filter, each
    /// generator's own `applicable()` decides (issue #83 P0-2/P2-1).
    pub fn generate(
        &self,
        ctx: &ReportContext,
        filter: Option<&std::collections::HashSet<String>>,
    ) -> Vec<ReportSection> {
        let mut sections = Vec::new();
        for generator in &self.generators {
            match filter {
                Some(filter_set) if !filter_set.contains(generator.name()) => continue,
                Some(_) => sections.extend(generator.generate(ctx)),
                None if generator.applicable(ctx) => sections.extend(generator.generate(ctx)),
                None => {}
            }
        }
        sections
    }

    /// Enumerate registered generators as `(name, description)` — the data
    /// behind `report --list-sections` (issue #83 P2-7).
    pub fn sections(&self) -> Vec<(&str, &str)> {
        self.generators
            .iter()
            .map(|g| (g.name(), g.description()))
            .collect()
    }
}

impl Default for SectionRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ── Built-in generators ──────────────────────────────────────────────────

struct UniversalGenerator;
impl ReportSectionGenerator for UniversalGenerator {
    fn name(&self) -> &str {
        "universal"
    }
    fn description(&self) -> &str {
        "Dashboard: pipeline status, task count, total runtime"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let total = ctx.config.rules.len();
        let completed = ctx.checkpoint.map(|c| c.completed_rules.len()).unwrap_or(0);
        let failed = ctx.checkpoint.map(|c| c.failed_rules.len()).unwrap_or(0);
        let total_runtime: Option<f64> = ctx.checkpoint.and_then(|c| {
            c.benchmarks
                .values()
                .map(|b| Some(b.wall_time_secs))
                .sum::<Option<f64>>()
        });

        // Honest status vocabulary (issue #83 P0-8): a report without a
        // checkpoint is "not run", not "all tasks completed". `completed`
        // counts rule INSTANCES (a scattered rule contributes one entry per
        // sample), so it must never be divided by the rule count.
        let (status_value, status, status_desc) = match ctx.checkpoint {
            None => (
                format!("{total} tasks, 0 executed"),
                QcStatusLevel::Info,
                "No execution data — run the workflow first".to_string(),
            ),
            Some(_) if failed > 0 => (
                format!("{failed} failed, {completed} succeeded"),
                QcStatusLevel::Warn,
                format!("{failed} task(s) failed"),
            ),
            // No failures and the completed set covers the plan exactly
            // (one instance per rule — the common single-sample case).
            Some(_) if completed == total => (
                format!("{completed}/{total} succeeded"),
                QcStatusLevel::Pass,
                "All tasks completed".to_string(),
            ),
            // Scattered rules complete as multiple instances; the checkpoint
            // records no pending set, so claim completion without a ratio.
            Some(_) => (
                format!("{completed} tasks succeeded"),
                QcStatusLevel::Info,
                "No failures recorded".to_string(),
            ),
        };

        vec![ReportSection {
            title: "Dashboard".into(),
            id: "dashboard".into(),
            content: ReportContent::QcIndicatorGroup {
                items: vec![
                    QcIndicator {
                        label: "Pipeline Status".into(),
                        value: status_value,
                        status,
                        description: status_desc,
                    },
                    QcIndicator {
                        label: "Total Tasks".into(),
                        value: total.to_string(),
                        status: QcStatusLevel::Info,
                        description: "Rules in workflow".into(),
                    },
                    QcIndicator {
                        label: "Total Runtime".into(),
                        value: total_runtime
                            .map(|t| format!("{t:.1}s"))
                            .unwrap_or_else(|| "-".into()),
                        status: QcStatusLevel::Info,
                        description: "Sum of completed rule wall times".into(),
                    },
                ],
            },
            subsections: vec![],
        }]
    }
}

struct ExecutionStatusGenerator;
impl ReportSectionGenerator for ExecutionStatusGenerator {
    fn name(&self) -> &str {
        "execution-status"
    }
    fn description(&self) -> &str {
        "Per-rule execution status and benchmark metrics"
    }
    fn applicable(&self, ctx: &ReportContext) -> bool {
        ctx.checkpoint.is_some()
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let cp = ctx.checkpoint.unwrap();
        let mut sections = Vec::new();

        // Deterministic order — checkpoint sets are HashSets, and reports
        // must be byte-stable for diffing (issue #83 P1-4).
        let mut completed: Vec<&String> = cp.completed_rules.iter().collect();
        completed.sort_unstable();
        let mut failed: Vec<&String> = cp.failed_rules.iter().collect();
        failed.sort_unstable();

        let mut rows: Vec<Vec<String>> = Vec::new();
        for name in &completed {
            let wall = cp
                .benchmarks
                .get(*name)
                .map(|b| format!("{:.1}s", b.wall_time_secs))
                .unwrap_or_else(|| "-".into());
            rows.push(vec![(*name).clone(), "success".into(), wall, "-".into()]);
        }
        for name in &failed {
            let exit = cp
                .rule_runs
                .get(*name)
                .and_then(|r| r.exit_code)
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into());
            rows.push(vec![(*name).clone(), "failed".into(), "-".into(), exit]);
        }
        if !rows.is_empty() {
            sections.push(ReportSection {
                title: "Execution Status".into(),
                id: "execution-status".into(),
                content: ReportContent::Table {
                    headers: vec![
                        "Rule".into(),
                        "Status".into(),
                        "Wall Time".into(),
                        "Exit Code".into(),
                    ],
                    rows,
                },
                subsections: vec![],
            });
        }

        // Sampled CPU seconds (issue #83 P1-13): measured per sampler tick
        // for local rules, so the column is honest now — `-` only where no
        // sampler ever ran (cluster executors, legacy checkpoints).
        let mut bench_names: Vec<&String> = cp.benchmarks.keys().collect();
        bench_names.sort_unstable();
        let mut bench_rows: Vec<Vec<String>> = Vec::new();
        for name in bench_names {
            let b = &cp.benchmarks[name];
            bench_rows.push(vec![
                name.clone(),
                format!("{:.2}s", b.wall_time_secs),
                b.max_memory_mb
                    .map(|m| format!("{}MB", m))
                    .unwrap_or_else(|| "-".into()),
                b.cpu_seconds
                    .map(|c| format!("{:.1}s", c))
                    .unwrap_or_else(|| "-".into()),
                b.retries.to_string(),
            ]);
        }
        if !bench_rows.is_empty() {
            sections.push(ReportSection {
                title: "Benchmarks".into(),
                id: "benchmarks".into(),
                content: ReportContent::Table {
                    headers: vec![
                        "Rule".into(),
                        "Wall Time".into(),
                        "Memory".into(),
                        "CPU".into(),
                        "Retries".into(),
                    ],
                    rows: bench_rows,
                },
                subsections: vec![],
            });
        }
        sections
    }
}

struct ClinicalComplianceGenerator;
impl ReportSectionGenerator for ClinicalComplianceGenerator {
    fn name(&self) -> &str {
        "clinical-compliance"
    }
    fn description(&self) -> &str {
        "Static capability statement — clinical classification frameworks modeled by oxo-flow (no clinical data is generated)"
    }
    fn applicable(&self, ctx: &ReportContext) -> bool {
        // Only for clinical-domain workflows; explicitly listing the section
        // in [report].sections overrides this gate (issue #83 P0-2).
        ctx.domain == WorkflowDomain::Clinical
    }
    fn generate(&self, _ctx: &ReportContext) -> Vec<ReportSection> {
        vec![ReportSection {
            title: "Clinical Compliance".into(),
            id: "clinical-compliance".into(),
            content: ReportContent::Text {
                text: "Static capability statement: this section describes the \
                       classification frameworks modeled by oxo-flow's clinical \
                       module. It does not certify this run as clinically compliant, \
                       and no variant, biomarker, or audit data is generated by the \
                       report system."
                    .into(),
            },
            subsections: vec![ReportSection {
                title: "Modeled Frameworks".into(),
                id: "clinical-frameworks".into(),
                content: ReportContent::KeyValue {
                    pairs: vec![
                        (
                            "ACMG/AMP Framework".into(),
                            "Tier I-IV (somatic) + Pathogenic-Benign (germline)".into(),
                        ),
                        (
                            "Variant Classification".into(),
                            "VariantClassification enum with 9 tiers".into(),
                        ),
                        (
                            "Audit Trail".into(),
                            "ComplianceEvent: timestamp, actor, evidence hash".into(),
                        ),
                        (
                            "Gene Panel Support".into(),
                            "GenePanel: name, version, genes, BED file".into(),
                        ),
                        (
                            "Biomarker Tracking".into(),
                            "BiomarkerResult: value, units, reference range, interpretation".into(),
                        ),
                        (
                            "QC Thresholds".into(),
                            "QcThreshold: min/max, unit, passes(value) validator".into(),
                        ),
                    ],
                },
                subsections: vec![],
            }],
        }]
    }
}

struct WorkflowInfoGenerator;
impl ReportSectionGenerator for WorkflowInfoGenerator {
    fn name(&self) -> &str {
        "workflow-info"
    }
    fn description(&self) -> &str {
        "Workflow name, version, author, config, sample/pair counts"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let mut pairs = vec![
            ("Name".into(), ctx.config.workflow.name.clone()),
            ("Version".into(), ctx.config.workflow.version.clone()),
            ("Total Rules".into(), ctx.config.rules.len().to_string()),
            ("Detected Domain".into(), format!("{:?}", ctx.domain)),
        ];
        if let Some(ref desc) = ctx.config.workflow.description {
            pairs.push(("Description".into(), desc.clone()));
        }
        if let Some(ref author) = ctx.config.workflow.author {
            pairs.push(("Author".into(), author.clone()));
        }
        if let Some(ref genome) = ctx.config.workflow.genome_build {
            pairs.push(("Genome Build".into(), genome.clone()));
        }
        if !ctx.config.config.is_empty() {
            // Deterministic order — HashMap iteration is arbitrary and
            // reports must be byte-stable (issue #83 P1-4).
            let mut keys: Vec<&String> = ctx.config.config.keys().collect();
            keys.sort_unstable();
            pairs.push((
                "Config Variables".into(),
                keys.iter()
                    .map(|k| (*k).clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let samples: usize = ctx
            .config
            .sample_groups
            .iter()
            .map(|g| g.samples.len())
            .sum();
        if samples > 0 {
            pairs.push(("Samples".into(), samples.to_string()));
        }
        if !ctx.config.pairs.is_empty() {
            pairs.push(("Pairs".into(), ctx.config.pairs.len().to_string()));
        }
        vec![ReportSection {
            title: "Workflow Information".into(),
            id: "workflow-info".into(),
            content: ReportContent::KeyValue { pairs },
            subsections: vec![],
        }]
    }
}

struct CommandManifestGenerator;
impl ReportSectionGenerator for CommandManifestGenerator {
    fn name(&self) -> &str {
        "commands"
    }
    fn description(&self) -> &str {
        "Executed commands — expanded from the checkpoint when available, declared templates otherwise"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let rows: Vec<Vec<String>> = ctx
            .config
            .rules
            .iter()
            .map(|r| {
                let declared = r.shell.clone().unwrap_or_else(|| "(none)".into());
                match ctx.checkpoint.and_then(|c| c.rule_runs.get(&r.name)) {
                    // The command that actually ran, with wildcards and
                    // {config.x} resolved (issue #83 P0-6).
                    Some(run) => vec![r.name.clone(), run.command.clone().unwrap_or(declared)],
                    None => vec![
                        r.name.clone(),
                        format!("{declared} (declared template — no execution record)"),
                    ],
                }
            })
            .collect();
        vec![ReportSection {
            title: "Commands".into(),
            id: "commands".into(),
            content: ReportContent::Table {
                headers: vec!["Task".into(), "Command".into()],
                rows,
            },
            subsections: vec![],
        }]
    }
}

struct IoManifestGenerator;
impl ReportSectionGenerator for IoManifestGenerator {
    fn name(&self) -> &str {
        "file-manifest"
    }
    fn description(&self) -> &str {
        "Real files on disk — checkpoint-recorded inputs and outputs (path, size, mtime, sha256)"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        match ctx.checkpoint {
            Some(cp) => {
                // Inputs: checkpoint input manifests — the actual file set
                // each rule's inputs resolved to when it completed, with
                // size + mtime recorded at snapshot time (issue #83 P0-6).
                let mut rules: Vec<&String> = cp.input_manifests.keys().collect();
                rules.sort_unstable();
                let mut inputs: Vec<Vec<String>> = Vec::new();
                for rule in rules {
                    for entry in &cp.input_manifests[rule] {
                        inputs.push(vec![
                            rule.clone(),
                            entry.path.clone(),
                            human_size(entry.size),
                            format_mtime_nanos(entry.mtime_nanos),
                        ]);
                    }
                }

                // Outputs: checkpoint checksums (sha256 recorded when the
                // rule completed), with current disk size/mtime where the
                // file still exists.
                let mut checksums: Vec<(&String, &String)> = cp.checksums.iter().collect();
                checksums.sort_unstable();
                let workdir = report_workdir(ctx);
                let mut outputs: Vec<Vec<String>> = Vec::new();
                for (path, sha) in checksums {
                    let (size, mtime) = workdir
                        .as_deref()
                        .and_then(|wd| std::fs::metadata(wd.join(path)).ok())
                        .map(|m| {
                            (
                                human_size(m.len()),
                                m.modified()
                                    .ok()
                                    .map(format_system_time)
                                    .unwrap_or_else(|| "-".into()),
                            )
                        })
                        .unwrap_or_else(|| ("-".into(), "-".into()));
                    outputs.push(vec![path.clone(), sha.clone(), size, mtime]);
                }

                let mut note = String::from(
                    "Files recorded in the checkpoint when each rule completed \
                     (input manifests + output checksums). Inputs are listed with \
                     snapshot size/mtime; outputs with their recorded sha256 and \
                     current on-disk size/mtime.",
                );
                if inputs.is_empty() {
                    note.push_str(
                        " No input manifests recorded — rules without inputs (or that \
                         never completed) record nothing.",
                    );
                }
                if outputs.is_empty() {
                    note.push_str(
                        " No output checksums recorded — run with --provenance to \
                         record sha256 for outputs.",
                    );
                }
                vec![ReportSection {
                    title: "File Manifest".into(),
                    id: "file-manifest".into(),
                    content: ReportContent::Text { text: note },
                    subsections: vec![
                        ReportSection {
                            title: "Input Files".into(),
                            id: "input-files".into(),
                            content: ReportContent::Table {
                                headers: vec![
                                    "Rule".into(),
                                    "Path".into(),
                                    "Size".into(),
                                    "Modified".into(),
                                ],
                                rows: inputs,
                            },
                            subsections: vec![],
                        },
                        ReportSection {
                            title: "Output Files".into(),
                            id: "output-files".into(),
                            content: ReportContent::Table {
                                headers: vec![
                                    "Path".into(),
                                    "SHA-256".into(),
                                    "Size".into(),
                                    "Modified".into(),
                                ],
                                rows: outputs,
                            },
                            subsections: vec![],
                        },
                    ],
                }]
            }
            None => {
                // Honest fallback: without a checkpoint there is no
                // execution data, so only declared patterns can be shown —
                // clearly labeled as such (issue #83 P0-6).
                let inputs: Vec<Vec<String>> = ctx
                    .config
                    .rules
                    .iter()
                    .flat_map(|r| r.input.iter().map(|i| vec![i.clone()]))
                    .collect();
                let outputs: Vec<Vec<String>> = ctx
                    .config
                    .rules
                    .iter()
                    .flat_map(|r| r.output.iter().map(|o| vec![o.clone()]))
                    .collect();
                vec![ReportSection {
                    title: "File Manifest".into(),
                    id: "file-manifest".into(),
                    content: ReportContent::Text {
                        text: "No execution data — showing declared patterns only. \
                                Run the workflow first for a real file listing."
                            .into(),
                    },
                    subsections: vec![
                        ReportSection {
                            title: "Declared Input Patterns".into(),
                            id: "input-files".into(),
                            content: ReportContent::Table {
                                headers: vec!["Pattern".into()],
                                rows: inputs,
                            },
                            subsections: vec![],
                        },
                        ReportSection {
                            title: "Declared Output Patterns".into(),
                            id: "output-files".into(),
                            content: ReportContent::Table {
                                headers: vec!["Pattern".into()],
                                rows: outputs,
                            },
                            subsections: vec![],
                        },
                    ],
                }]
            }
        }
    }
}

/// Working directory files resolve against: the checkpoint's recorded
/// workdir, falling back to the workflow file's directory.
fn report_workdir(ctx: &ReportContext) -> Option<std::path::PathBuf> {
    if let Some(cp) = ctx.checkpoint
        && let Some(wd) = cp.workdir.as_deref()
    {
        return Some(std::path::PathBuf::from(wd));
    }
    ctx.workflow_path
        .map(crate::parent_dir)
        .map(|p| p.to_path_buf())
}

/// Human-readable file size (binary units).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format a nanosecond Unix epoch timestamp as UTC, or "-" when invalid.
fn format_mtime_nanos(nanos: i128) -> String {
    // Realistic mtimes fit comfortably in i64 nanoseconds (the epoch is
    // ~1.77e18 ns in 2026; i64 max is 9.2e18).
    i64::try_from(nanos)
        .ok()
        .map(DateTime::from_timestamp_nanos)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "-".into())
}

fn format_system_time(t: std::time::SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

struct EnvironmentInfoGenerator;
impl ReportSectionGenerator for EnvironmentInfoGenerator {
    fn name(&self) -> &str {
        "environment"
    }
    fn description(&self) -> &str {
        "Engine version, platform, and the environments declared by the workflow's rules"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        // Real facts only (issue #83 P0-6): engine version, platform, and
        // the environments the workflow declares — no guessed list of
        // "available backends".
        let mut env_counts: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for rule in &ctx.config.rules {
            *env_counts.entry(rule.environment.kind()).or_default() += 1;
        }
        let declared = if env_counts.is_empty() {
            "(no rules)".to_string()
        } else {
            env_counts
                .iter()
                .map(|(kind, count)| format!("{kind} ({count} rule(s))"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        vec![ReportSection {
            title: "Environment".into(),
            id: "environment".into(),
            content: ReportContent::KeyValue {
                pairs: vec![
                    ("oxo-flow Version".into(), env!("CARGO_PKG_VERSION").into()),
                    (
                        "Platform".into(),
                        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                    ),
                    ("Declared Rule Environments".into(), declared),
                ],
            },
            subsections: vec![],
        }]
    }
}

/// QC metrics parsed from real tool outputs in the working directory (issue
/// #83 P1-5): fastp report.json, samtools flagstat, STAR Log.final.out,
/// featureCounts .summary, bcftools stats, kraken2 .report. One subsection
/// per (tool × sample); the section is hidden entirely when nothing parses
/// — a report never fabricates metrics.
struct MetricsGenerator;
impl ReportSectionGenerator for MetricsGenerator {
    fn name(&self) -> &str {
        "metrics"
    }
    fn description(&self) -> &str {
        "QC metrics parsed from tool outputs (fastp, flagstat, STAR, featureCounts, bcftools, kraken2)"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let Some(workdir) = report_workdir(ctx) else {
            return Vec::new();
        };
        let stats = MetricsScanner::new().scan_with_stats(&workdir);
        if stats.parsed.is_empty() {
            return Vec::new();
        }

        // Group by (tool, sample); BTreeMap keeps the keys sorted, and
        // within a group the scan order is already deterministic (sorted
        // directory entries) — the report stays byte-stable (issue #83
        // P1-4).
        let mut groups: std::collections::BTreeMap<(String, String), Vec<&ParsedMetrics>> =
            std::collections::BTreeMap::new();
        for parsed in &stats.parsed {
            groups
                .entry((
                    parsed.tool.clone(),
                    parsed.sample.clone().unwrap_or_default(),
                ))
                .or_default()
                .push(parsed);
        }

        let mut subsections = Vec::new();
        for ((tool, sample), entries) in groups {
            let mut rows: Vec<Vec<String>> = Vec::new();
            for entry in entries {
                for metric in &entry.metrics {
                    rows.push(vec![
                        metric.name.clone(),
                        format_metric_value(metric.value),
                        metric_status_word(metric.flag.clone()),
                    ]);
                }
            }
            // Deterministic row order.
            rows.sort_by(|a, b| a[0].cmp(&b[0]));
            let title = if sample.is_empty() {
                tool.clone()
            } else {
                format!("{tool} — {sample}")
            };
            subsections.push(ReportSection {
                title,
                id: sanitize_id(&format!("metrics-{tool}-{sample}")),
                content: ReportContent::Table {
                    headers: vec![
                        "Metric".to_string(),
                        "Value".to_string(),
                        "Status".to_string(),
                    ],
                    rows,
                },
                subsections: vec![],
            });
        }

        // A scanner that hid its gaps would look like full coverage — say
        // what could not be parsed (issue #83 P1-5 ruling).
        if stats.skipped > 0 {
            subsections.push(ReportSection {
                title: "Scan Notes".to_string(),
                id: "metrics-scan-notes".to_string(),
                content: ReportContent::Text {
                    text: format!(
                        "{} file(s) matched known tool patterns but failed to parse",
                        stats.skipped
                    ),
                },
                subsections: vec![],
            });
        }

        vec![ReportSection {
            title: "Metrics".to_string(),
            id: "metrics".to_string(),
            content: ReportContent::Text {
                text: "QC metrics parsed from tool output files in the working directory."
                    .to_string(),
            },
            subsections,
        }]
    }
}

/// Whole-number metric values below this magnitude print without decimals
/// (beyond it, `{:.0}` would hide float noise at the displayed precision).
const WHOLE_VALUE_DECIMAL_CAP: f64 = 1e15;
/// Decimal places for fractional metric values.
const METRIC_VALUE_DECIMALS: usize = 2;

/// Format a metric value for display: whole numbers stay whole, everything
/// else keeps two decimals.
fn format_metric_value(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < WHOLE_VALUE_DECIMAL_CAP {
        format!("{value:.0}")
    } else {
        format!("{value:.precision$}", precision = METRIC_VALUE_DECIMALS)
    }
}

/// Status cell text for a metric flag: emoji + word, matching the
/// QcIndicatorGroup marks used elsewhere; `None` = informational, no status.
fn metric_status_word(flag: Option<QcStatusLevel>) -> String {
    match flag {
        Some(QcStatusLevel::Pass) => "\u{2705} Pass".to_string(),
        Some(QcStatusLevel::Warn) => "\u{26A0}\u{FE0F} Warn".to_string(),
        Some(QcStatusLevel::Fail) => "\u{274C} Fail".to_string(),
        Some(QcStatusLevel::Info) => "\u{2139}\u{FE0F} Info".to_string(),
        None => "\u{2014}".to_string(),
    }
}

/// Rule × sample status matrix from the checkpoint's expanded instance
/// names (issue #83 P1-5). Samples come from sample_groups and pairs;
/// rows are the base (declared) rule names, sorted failed-first then by
/// name so failing samples surface at the top of the table.
struct SampleMatrixGenerator;
impl ReportSectionGenerator for SampleMatrixGenerator {
    fn name(&self) -> &str {
        "sample-matrix"
    }
    fn description(&self) -> &str {
        "Rule × sample status matrix (expanded instances from the checkpoint)"
    }
    fn applicable(&self, ctx: &ReportContext) -> bool {
        ctx.checkpoint.is_some()
            && (!ctx.config.sample_groups.is_empty() || !ctx.config.pairs.is_empty())
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        // The registry's section filter (`report.sections = [...]`) calls
        // generate() without consulting applicable() — guard here instead
        // of expecting (issue #83 P1-5 review).
        let Some(cp) = ctx.checkpoint else {
            return Vec::new();
        };

        // Sample universe: sample_groups[].samples + pairs[].experiment/
        // control, deduped and sorted (BTreeSet).
        let mut samples: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for group in &ctx.config.sample_groups {
            samples.extend(group.samples.iter().cloned());
        }
        for pair in &ctx.config.pairs {
            samples.insert(pair.experiment.clone());
            if let Some(control) = &pair.control {
                samples.insert(control.clone());
            }
        }

        // Rows: base rule names from the declared config; cell = checkpoint
        // state of the engine's real instance names. expand_wildcards names
        // named-group instances `{rule}_{group}_{sample}`, sample_pattern
        // discovery instances `{rule}_auto-discovered_{sample}`, and pair
        // instances `{rule}_{pair_id}` — a cell is success/failed if ANY of
        // the spellings for that sample claim the rule.
        let mut rows: Vec<(bool, String, Vec<String>)> = Vec::new();
        for rule in &ctx.config.rules {
            let mut has_failed = false;
            let mut cells = Vec::with_capacity(samples.len());
            for sample in &samples {
                let mut completed = false;
                let mut failed = false;
                for group in &ctx.config.sample_groups {
                    if group.samples.contains(sample) {
                        let name = format!("{}_{}_{}", rule.name, group.name, sample);
                        completed |= cp.completed_rules.contains(&name);
                        failed |= cp.failed_rules.contains(&name);
                    }
                }
                // sample_pattern discovery uses the engine's "auto-discovered"
                // group name (config.rs sample discovery).
                let auto = format!("{}_auto-discovered_{}", rule.name, sample);
                completed |= cp.completed_rules.contains(&auto);
                failed |= cp.failed_rules.contains(&auto);
                for pair in &ctx.config.pairs {
                    if pair.experiment == *sample
                        || pair.control.as_deref() == Some(sample.as_str())
                    {
                        let name = format!("{}_{}", rule.name, pair.pair_id);
                        completed |= cp.completed_rules.contains(&name);
                        failed |= cp.failed_rules.contains(&name);
                    }
                }
                let cell = if failed {
                    has_failed = true;
                    "failed"
                } else if completed {
                    "success"
                } else {
                    "-"
                };
                cells.push(cell.to_string());
            }
            rows.push((has_failed, rule.name.clone(), cells));
        }
        // Failed-first, then by rule name — deterministic, and failures
        // surface at the top of the table.
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let headers: Vec<String> = std::iter::once("Rule".to_string()).chain(samples).collect();
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(_, name, cells)| std::iter::once(name).chain(cells).collect())
            .collect();

        vec![ReportSection {
            title: "Sample Matrix".to_string(),
            id: "sample-matrix".to_string(),
            content: ReportContent::Table { headers, rows },
            subsections: vec![],
        }]
    }
}

/// Failure diagnosis — the first screen of a failed run's report: per failed
/// rule, the exit code, affected downstream rules, a stderr excerpt when
/// available, and a suggested next step (issue #83 P0-4).
struct FailureDiagnosisGenerator;
impl ReportSectionGenerator for FailureDiagnosisGenerator {
    fn name(&self) -> &str {
        "failure-diagnosis"
    }
    fn description(&self) -> &str {
        "Failed rules: exit code, cascade impact, stderr excerpt, suggested next steps"
    }
    fn applicable(&self, ctx: &ReportContext) -> bool {
        ctx.checkpoint
            .map(|c| !c.failed_rules.is_empty())
            .unwrap_or(false)
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let cp = ctx.checkpoint.unwrap();
        let mut failed: Vec<&String> = cp.failed_rules.iter().collect();
        failed.sort_unstable();

        let dag = crate::dag::WorkflowDag::from_rules(&ctx.config.rules).ok();
        let mut subsections = Vec::new();
        for rule in &failed {
            let run = cp.rule_runs.get(*rule);
            let exit = run.and_then(|r| r.exit_code);
            let tail = run.and_then(|r| r.stderr_tail.as_deref());

            let cascade = affected_downstream(rule, dag.as_ref());
            let cascade_text = if cascade.is_empty() {
                "none — no rules consume this rule's outputs".to_string()
            } else {
                format!("{} ({} rule(s))", cascade.join(", "), cascade.len())
            };

            let pairs = vec![
                (
                    "Exit Code".into(),
                    exit.map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown (engine-level failure)".into()),
                ),
                ("Affected Downstream".into(), cascade_text),
                ("Suggested Next Step".into(), failure_suggestion(exit, tail)),
            ];

            let mut details = vec![ReportSection {
                title: "Details".into(),
                id: format!("failure-{}-details", sanitize_id(rule)),
                content: ReportContent::KeyValue { pairs },
                subsections: vec![],
            }];
            if let Some(tail) = tail {
                details.push(ReportSection {
                    title: "Stderr Excerpt".into(),
                    id: format!("failure-{}-stderr", sanitize_id(rule)),
                    content: ReportContent::Markdown {
                        markdown: tail.to_string(),
                    },
                    subsections: vec![],
                });
            }
            subsections.push(ReportSection {
                title: (*rule).clone(),
                id: format!("failure-{}", sanitize_id(rule)),
                content: ReportContent::Text {
                    text: format!("Rule '{rule}' failed."),
                },
                subsections: details,
            });
        }
        vec![ReportSection {
            title: "Failure Diagnosis".into(),
            id: "failure-diagnosis".into(),
            content: ReportContent::Text {
                text: format!(
                    "{} rule(s) failed. For each: exit code, affected downstream rules, \
                     a stderr excerpt when available, and a suggested next step.",
                    failed.len()
                ),
            },
            subsections,
        }]
    }
}

/// All rules a failure propagates to: transitive dependents in the DAG.
fn affected_downstream(rule: &str, dag: Option<&crate::dag::WorkflowDag>) -> Vec<String> {
    let Some(dag) = dag else { return Vec::new() };
    let mut visited: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    visited.insert(rule.to_string());
    let mut queue = vec![rule.to_string()];
    let mut affected = Vec::new();
    while let Some(current) = queue.pop() {
        for dep in dag.dependents(&current).unwrap_or_default() {
            if visited.insert(dep.clone()) {
                affected.push(dep.clone());
                queue.push(dep);
            }
        }
    }
    affected
}

/// A concrete next step for the most common failure signatures, grounded in
/// POSIX exit-code conventions (issue #83 P0-4).
fn failure_suggestion(exit_code: Option<i32>, stderr_tail: Option<&str>) -> String {
    let stderr = stderr_tail.unwrap_or("").to_lowercase();
    if stderr.contains("no space left") {
        return "disk full — free space in the working directory, then re-run with --resume-failed"
            .into();
    }
    match exit_code {
        Some(127) => "command not found (exit 127) — verify the tool is installed in this \
                      rule's environment"
            .into(),
        Some(126) => {
            "not executable / permission denied (exit 126) — check file permissions".into()
        }
        Some(137) => {
            "killed (exit 137), likely out of memory — raise the rule's memory limit".into()
        }
        Some(124) => "timed out (exit 124) — raise the timeout or split the rule".into(),
        Some(-1) | None => "engine-level failure before the command completed — check resource \
                            limits and environment setup"
            .into(),
        Some(code) => format!(
            "inspect the stderr excerpt above, then re-run with --resume-failed (exit {code})"
        ),
    }
}

/// HTML-fragment-safe identifier for a rule name.
fn sanitize_id(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// Execution provenance: engine version, workflow file identity, and
/// checkpoint location — the audit-trail anchor of the report (issue #83
/// P1-8).
struct ProvenanceGenerator;
impl ReportSectionGenerator for ProvenanceGenerator {
    fn name(&self) -> &str {
        "provenance"
    }
    fn description(&self) -> &str {
        "Engine version, workflow file checksum, checkpoint location"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let mut pairs = vec![("oxo-flow Version".into(), env!("CARGO_PKG_VERSION").into())];
        if let Some(path) = ctx.workflow_path {
            pairs.push(("Workflow File".into(), path.display().to_string()));
            match crate::executor::checkpoint::compute_file_checksum(path) {
                Ok(checksum) => pairs.push(("Workflow Checksum".into(), checksum)),
                Err(_) => pairs.push(("Workflow Checksum".into(), "-".into())),
            }
        }
        if let Some(path) = ctx.checkpoint_path {
            pairs.push(("Checkpoint".into(), path.display().to_string()));
        }
        if let Some(sha) = ctx.checkpoint.and_then(|c| c.workflow_git_sha.as_deref()) {
            pairs.push(("Workflow git HEAD".into(), sha.into()));
        }
        vec![ReportSection {
            title: "Provenance".into(),
            id: "provenance".into(),
            content: ReportContent::KeyValue { pairs },
            subsections: vec![],
        }]
    }
}

/// Task summary — every rule with type, I/O counts, environment, and
/// resources. A filterable section like all others (issue #83 P2-1).
struct TaskSummaryGenerator;
impl ReportSectionGenerator for TaskSummaryGenerator {
    fn name(&self) -> &str {
        "task-summary"
    }
    fn description(&self) -> &str {
        "All rules with type, inputs/outputs, environment, and resources"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        vec![task_summary_section(&ctx.config.rules)]
    }
}

fn task_summary_section(rules: &[crate::rule::Rule]) -> ReportSection {
    let headers: Vec<String> = vec![
        "Task",
        "Type",
        "Inputs",
        "Outputs",
        "Environment",
        "Resources",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let rows: Vec<Vec<String>> = rules
        .iter()
        .map(|r| {
            let task_type = if r.shell.is_some() {
                "shell"
            } else if r.script.is_some() {
                "script"
            } else if r.transform.is_some() {
                "transform"
            } else {
                "other"
            };
            let env = r.environment.kind();
            let resources = format!(
                "t={} m={}",
                r.resources.threads,
                r.resources.memory.as_deref().unwrap_or("-")
            );
            vec![
                r.name.clone(),
                task_type.into(),
                r.input.len().to_string(),
                r.output.len().to_string(),
                env.to_string(),
                resources,
            ]
        })
        .collect();
    ReportSection {
        title: "Task Summary".into(),
        id: "task-summary".into(),
        content: ReportContent::Table { headers, rows },
        subsections: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_workflow_git_sha_provenance_roundtrip() {
        // The builder records the workflow version that produced the
        // results (issue #115 pillar 1); absent by default.
        let report = ReportBuilder::new("t", "wf", "1.0.0")
            .workflow_git_sha(Some("abc123".to_string()))
            .build();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"workflow_git_sha\":\"abc123\""));
        let plain = ReportBuilder::new("t", "wf", "1.0.0").build();
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("workflow_git_sha"));
    }

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
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
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
    fn builtin_template_exposes_default_scaffold() {
        // `report --init-template` writes exactly what TemplateEngine loads
        // as "report.html" (issue #83 P2-7).
        let template = builtin_template();
        assert!(template.contains("<!DOCTYPE html>"));
        assert!(template.contains("{{ title }}"));
        assert!(template.contains("{% for section in sections %}"));

        // Rendering with a Tera built from the exposed string must equal the
        // engine's own preloaded output — the scaffold cannot drift.
        let report = Report::new("T", "wf", "1.0");
        let expected = TemplateEngine::new()
            .unwrap()
            .render_report(&report)
            .unwrap();
        let mut tera = tera::Tera::default();
        tera.add_raw_template("report.html", template).unwrap();
        let context = tera::Context::from_value(serde_json::to_value(&report).unwrap()).unwrap();
        assert_eq!(tera.render("report.html", &context).unwrap(), expected);
    }

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
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
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

    // ── Issue #83: honesty, execution truth, determinism, XSS ──────────────

    /// Parse a workflow config from an inline TOML body.
    fn workflow_config(extra: &str) -> WorkflowConfig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(
            &path,
            format!("[workflow]\nname = \"test\"\nversion = \"0.1\"\n{extra}"),
        )
        .unwrap();
        WorkflowConfig::from_file(&path).unwrap()
    }

    fn fixture_checkpoint() -> CheckpointState {
        let mut cp = CheckpointState::new();
        cp.completed_rules.insert("align".to_string());
        cp.completed_rules.insert("bwa".to_string());
        cp.failed_rules.insert("call".to_string());
        cp.benchmarks.insert(
            "align".to_string(),
            crate::executor::checkpoint::BenchmarkRecord {
                rule: "align".to_string(),
                wall_time_secs: 12.5,
                max_memory_mb: Some(120),
                memory_limit_mb: Some(2048),
                cpu_seconds: Some(3.2),
                retries: 1,
            },
        );
        cp.rule_runs.insert(
            "call".to_string(),
            crate::executor::checkpoint::RuleRunRecord {
                exit_code: Some(127),
                command: Some("gatk HaplotypeCaller -I out.bam".to_string()),
                stderr_tail: Some("gatk: command not found".to_string()),
            },
        );
        cp.checksums
            .insert("out.bam".to_string(), "sha256:aaaa".to_string());
        cp.input_manifests.insert(
            "align".to_string(),
            vec![crate::executor::checkpoint::InputManifestEntry {
                path: "in.fastq".to_string(),
                size: 1024,
                mtime_nanos: 1_700_000_000_000_000_000,
                hash: Some("sha256:bbbb".to_string()),
                remote: None,
            }],
        );
        cp
    }

    fn ctx_for<'a>(
        config: &'a WorkflowConfig,
        checkpoint: Option<&'a CheckpointState>,
        workflow_path: Option<&'a std::path::Path>,
    ) -> ReportContext<'a> {
        ReportContext {
            config,
            checkpoint,
            domain: WorkflowDomain::detect(&config.rules),
            workflow_path,
            checkpoint_path: None,
        }
    }

    #[test]
    fn domain_detect_ordering() {
        // gatk + STAR is a variant-calling pipeline, not RNA-seq (issue #83 P0-3).
        let config = workflow_config(
            r#"[[rules]]
name = "align"
shell = "STAR --runThreadN 4"
[[rules]]
name = "call"
shell = "gatk HaplotypeCaller"
"#,
        );
        assert_eq!(
            WorkflowDomain::detect(&config.rules),
            WorkflowDomain::DnaSequencing
        );

        let config = workflow_config(
            r#"[[rules]]
name = "align"
shell = "STAR --runThreadN 4"
[[rules]]
name = "count"
shell = "featureCounts"
"#,
        );
        assert_eq!(
            WorkflowDomain::detect(&config.rules),
            WorkflowDomain::RnaSequencing
        );

        let config = workflow_config(
            r#"[[rules]]
name = "peak"
shell = "macs2 callpeak"
"#,
        );
        assert_eq!(
            WorkflowDomain::detect(&config.rules),
            WorkflowDomain::Epigenomics
        );

        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        assert_eq!(
            WorkflowDomain::detect(&config.rules),
            WorkflowDomain::Generic
        );
    }

    #[test]
    fn clinical_compliance_gated_by_domain_and_filter() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let ctx = ctx_for(&config, None, None);
        let registry = SectionRegistry::with_defaults();
        // Generic workflow: no clinical section by default (issue #83 P0-2).
        let sections = registry.generate(&ctx, None);
        assert!(!sections.iter().any(|s| s.id == "clinical-compliance"));
        // Explicit selection overrides the gate.
        let filter: std::collections::HashSet<String> = ["clinical-compliance".into()].into();
        let sections = registry.generate(&ctx, Some(&filter));
        assert!(sections.iter().any(|s| s.id == "clinical-compliance"));
    }

    #[test]
    fn dashboard_marks_unrun_without_checkpoint() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let ctx = ctx_for(&config, None, None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let dashboard = sections.iter().find(|s| s.id == "dashboard").unwrap();
        let mut html = String::new();
        render_section_html(&mut html, dashboard, 2);
        // Never claim completion for a run that never happened (issue #83 P0-8).
        assert!(!html.contains("All tasks completed"));
        assert!(html.contains("No execution data"));
    }

    #[test]
    fn html_escapes_user_controlled_strings() {
        let mut report = Report::new("T", "wf</title>", "1.0.0");
        report.add_section(ReportSection {
            title: "<img src=x onerror=alert(1)>".to_string(),
            id: "evil\" onclick=\"alert(1)".to_string(),
            content: ReportContent::Table {
                headers: vec!["<th>".to_string()],
                rows: vec![vec!["<script>alert(1)</script>".to_string()]],
            },
            subsections: vec![],
        });
        report.add_section(ReportSection {
            title: "kv".to_string(),
            id: "kv".to_string(),
            content: ReportContent::KeyValue {
                pairs: vec![("<script>".to_string(), "a&b <i>".to_string())],
            },
            subsections: vec![],
        });
        let html = report.to_html();
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<img src=x onerror"));
        assert!(!html.contains("</title><script>"));
        // The explicit-raw-Html variant stays raw by contract.
        report.add_section(ReportSection {
            title: "raw".to_string(),
            id: "raw".to_string(),
            content: ReportContent::Html {
                html: "<b>trusted</b>".to_string(),
            },
            subsections: vec![],
        });
        assert!(report.to_html().contains("<b>trusted</b>"));
    }

    #[test]
    fn execution_status_sorted_with_exit_codes() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let cp = fixture_checkpoint();
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let status = sections
            .iter()
            .find(|s| s.id == "execution-status")
            .unwrap();
        match &status.content {
            ReportContent::Table { headers, rows } => {
                assert!(headers.contains(&"Exit Code".to_string()));
                // Deterministic order (issue #83 P1-4): align, bwa, call.
                let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
                assert_eq!(names, vec!["align", "bwa", "call"]);
                let call_row = rows.iter().find(|r| r[0] == "call").unwrap();
                assert_eq!(call_row[3], "127");
            }
            _ => panic!("expected table"),
        }
        // Benchmarks table carries the measured CPU column (issue #83
        // P1-13) with the fixture's sampled value rendered.
        let bench = sections.iter().find(|s| s.id == "benchmarks").unwrap();
        match &bench.content {
            ReportContent::Table { headers, rows } => {
                assert!(headers.contains(&"CPU".to_string()));
                assert!(headers.contains(&"Memory".to_string()));
                let align_row = rows.iter().find(|r| r[0] == "align").unwrap();
                assert_eq!(align_row[3], "3.2s");
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn failure_diagnosis_includes_cascade_and_suggestion() {
        let config = workflow_config(
            r#"[[rules]]
name = "align"
shell = "touch out.bam"
output = ["out.bam"]
[[rules]]
name = "call"
shell = "gatk"
input = ["out.bam"]
"#,
        );
        let mut cp = fixture_checkpoint();
        cp.failed_rules.clear();
        cp.failed_rules.insert("align".to_string());
        cp.rule_runs.insert(
            "align".to_string(),
            crate::executor::checkpoint::RuleRunRecord {
                exit_code: Some(137),
                command: None,
                stderr_tail: Some("Killed".to_string()),
            },
        );
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let diag = sections
            .iter()
            .find(|s| s.id == "failure-diagnosis")
            .unwrap();
        let mut html = String::new();
        render_section_html(&mut html, diag, 2);
        // Downstream rule named in the cascade, 137 → memory suggestion.
        assert!(html.contains("call"));
        assert!(html.contains("memory"));
        assert!(html.contains("137"));
    }

    #[test]
    fn failure_suggestions_cover_common_exit_codes() {
        assert!(failure_suggestion(Some(127), None).contains("command not found"));
        assert!(failure_suggestion(Some(126), None).contains("permission"));
        assert!(failure_suggestion(Some(137), None).contains("memory"));
        assert!(failure_suggestion(Some(124), None).contains("timeout"));
        assert!(failure_suggestion(None, None).contains("engine-level"));
        assert!(failure_suggestion(Some(1), Some("no space left on device")).contains("disk"));
    }

    #[test]
    fn file_manifest_shows_recorded_files() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let cp = fixture_checkpoint();
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let manifest = sections.iter().find(|s| s.id == "file-manifest").unwrap();
        let mut html = String::new();
        render_section_html(&mut html, manifest, 2);
        assert!(html.contains("in.fastq"));
        assert!(html.contains("out.bam"));
        assert!(html.contains("sha256:aaaa"));
        // No pattern-only tables when execution data exists (issue #83 P0-6).
        assert!(!html.contains(">Pattern<"));
    }

    #[test]
    fn task_summary_respects_section_filter() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let ctx = ctx_for(&config, None, None);
        // Present by default...
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        assert!(sections.iter().any(|s| s.id == "task-summary"));
        // ...and removable via the filter (issue #83 P2-1).
        let filter: std::collections::HashSet<String> = ["universal".into()].into();
        let sections = SectionRegistry::with_defaults().generate(&ctx, Some(&filter));
        assert!(sections.iter().any(|s| s.id == "dashboard"));
        assert!(!sections.iter().any(|s| s.id == "task-summary"));
    }

    #[test]
    fn provenance_section_records_workflow_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(&path, "[workflow]\nname = \"t\"\nversion = \"0.1\"\n").unwrap();
        let config = WorkflowConfig::from_file(&path).unwrap();
        let ctx = ctx_for(&config, None, Some(&path));
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let prov = sections.iter().find(|s| s.id == "provenance").unwrap();
        match &prov.content {
            ReportContent::KeyValue { pairs } => {
                let checksum = pairs
                    .iter()
                    .find(|(k, _)| k == "Workflow Checksum")
                    .unwrap();
                assert!(checksum.1.starts_with("sha256:"));
                assert_eq!(checksum.1.len(), "sha256:".len() + 64);
            }
            _ => panic!("expected key-value pairs"),
        }
    }

    #[test]
    fn report_json_has_schema_and_command_fields() {
        let report = Report::new("T", "w", "0.1");
        let json = report.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "report");
    }

    #[test]
    fn report_rendering_is_deterministic_with_pinned_timestamp() {
        let config = workflow_config(
            r#"[[rules]]
name = "hello"
shell = "echo hi"
"#,
        );
        let cp = fixture_checkpoint();
        let ctx = ctx_for(&config, Some(&cp), None);
        let pinned = DateTime::from_timestamp(0, 0);

        // Sections are generated twice (registry generate is deterministic;
        // nothing is cloned) — both renders must be byte-identical.
        let render = |sections: &[ReportSection], generated_at: Option<DateTime<Utc>>| {
            let mut report = Report::new("T", "w", "0.1");
            report.generated_at = generated_at;
            for section in sections {
                report.add_section(section.clone());
            }
            report
        };
        let sections_a = SectionRegistry::with_defaults().generate(&ctx, None);
        let sections_b = SectionRegistry::with_defaults().generate(&ctx, None);
        let html_a = render(&sections_a, pinned).to_html();
        let html_b = render(&sections_b, pinned).to_html();
        assert_eq!(html_a, html_b);
        let json_a = render(&sections_a, pinned).to_json().unwrap();
        let json_b = render(&sections_b, pinned).to_json().unwrap();
        assert_eq!(json_a, json_b);
    }

    #[test]
    fn markdown_renders_gfm_tables() {
        let mut report = Report::new("T", "w", "0.1");
        report.add_section(ReportSection {
            title: "S".to_string(),
            id: "s".to_string(),
            content: ReportContent::Table {
                headers: vec!["A".to_string(), "B".to_string()],
                rows: vec![vec!["1".to_string(), "2|x".to_string()]],
            },
            subsections: vec![],
        });
        let md = report.to_markdown();
        assert!(md.contains("| A | B |"));
        assert!(md.contains("| 1 | 2\\|x |"));
    }

    #[test]
    fn hierarchy_and_scatter_render_as_tables() {
        let mut report = Report::new("T", "w", "0.1");
        report.add_section(ReportSection {
            title: "S".to_string(),
            id: "s".to_string(),
            content: ReportContent::ScatterPlot {
                title: "t".to_string(),
                x_label: "x".to_string(),
                y_label: "y".to_string(),
                points: vec![ScatterPoint {
                    x: 1.0,
                    y: 2.0,
                    label: None,
                    group: None,
                    size: None,
                }],
            },
            subsections: vec![],
        });
        let html = report.to_html();
        // Real table rendering, not a dead promise (issue #83 P1-10).
        assert!(!html.contains("interactive"));
        assert!(html.contains("<table>"));
    }

    // ── Issue #83 P1-5: metrics adapters + sample matrix ─────────────────

    /// A minimal valid fastp report.json for generator fixtures.
    fn fastp_fixture(total_reads: u64) -> String {
        format!(
            r#"{{"summary": {{"before_filtering": {{"total_reads": {total_reads}}}, "after_filtering": {{"q30_rate": 0.92, "gc_content": 0.451}}, "duplication": {{"rate": 0.11}}}}}}"#
        )
    }

    #[test]
    fn metrics_section_parses_tool_outputs() {
        let workdir = tempfile::tempdir().unwrap();
        std::fs::write(workdir.path().join("S1.fastp.json"), fastp_fixture(1234)).unwrap();
        // A second matching file that fails to parse → Scan Notes.
        std::fs::write(workdir.path().join("S2.fastp.json"), "garbage").unwrap();

        let config = workflow_config("[[rules]]\nname = \"hello\"\nshell = \"echo hi\"\n");
        let mut cp = fixture_checkpoint();
        cp.workdir = Some(workdir.path().display().to_string());
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let metrics = sections.iter().find(|s| s.id == "metrics").unwrap();

        assert_eq!(metrics.title, "Metrics");
        // One subsection per (tool × sample), then Scan Notes.
        assert_eq!(metrics.subsections[0].title, "fastp — S1");
        match &metrics.subsections[0].content {
            ReportContent::Table { headers, rows } => {
                assert_eq!(headers, &["Metric", "Value", "Status"]);
                // Rows sorted by metric name.
                let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
                assert_eq!(
                    names,
                    ["duplication_rate", "gc_content", "q30_rate", "total_reads"]
                );
                let total = rows.iter().find(|r| r[0] == "total_reads").unwrap();
                assert_eq!(total[1], "1234");
                assert_eq!(total[2], "—");
                let q30 = rows.iter().find(|r| r[0] == "q30_rate").unwrap();
                assert_eq!(q30[1], "0.92");
                assert_eq!(q30[2], "✅ Pass");
                let dup = rows.iter().find(|r| r[0] == "duplication_rate").unwrap();
                assert_eq!(dup[1], "0.11");
                assert_eq!(dup[2], "ℹ️ Info");
            }
            _ => panic!("expected table subsection"),
        }
        // Scan Notes reports the parse failure.
        let notes = &metrics.subsections[1];
        assert_eq!(notes.title, "Scan Notes");
        match &notes.content {
            ReportContent::Text { text } => {
                assert!(text.contains("1 file(s) matched known tool patterns"));
            }
            _ => panic!("expected text subsection"),
        }

        // Deterministic output (issue #83 P1-4).
        let sections_b = SectionRegistry::with_defaults().generate(&ctx, None);
        let report = |sections: &[ReportSection]| {
            let mut r = Report::new("T", "w", "0.1");
            r.generated_at = None;
            for section in sections {
                r.add_section(section.clone());
            }
            r.to_json().unwrap()
        };
        assert_eq!(report(&sections), report(&sections_b));
    }

    #[test]
    fn metrics_section_hidden_when_nothing_parses() {
        let config = workflow_config("[[rules]]\nname = \"hello\"\nshell = \"echo hi\"\n");

        // (a) Workdir resolvable but contains no tool outputs → hidden,
        // never a fabricated empty table.
        let empty = tempfile::tempdir().unwrap();
        let mut cp = fixture_checkpoint();
        cp.workdir = Some(empty.path().display().to_string());
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        assert!(!sections.iter().any(|s| s.id == "metrics"));

        // (b) No workdir resolvable at all (no checkpoint workdir, no
        // workflow path) → hidden.
        let cp = fixture_checkpoint();
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        assert!(!sections.iter().any(|s| s.id == "metrics"));
    }

    #[test]
    fn sample_matrix_cells_from_checkpoint() {
        let config = workflow_config(
            r#"[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "align"
shell = "echo hi"

[[rules]]
name = "call"
shell = "echo hi"
"#,
        );
        let mut cp = CheckpointState::new();
        // The engine names named-group instances `{rule}_{group}_{sample}`
        // (config.rs expand_wildcards) — exactly what a real run records.
        cp.completed_rules.insert("align_cohort_S1".to_string());
        cp.completed_rules.insert("align_cohort_S2".to_string());
        cp.failed_rules.insert("call_cohort_S1".to_string());

        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let matrix = sections.iter().find(|s| s.id == "sample-matrix").unwrap();
        match &matrix.content {
            ReportContent::Table { headers, rows } => {
                assert_eq!(headers, &["Rule", "S1", "S2"]);
                // Failed rows first, then by rule name.
                assert_eq!(rows[0], &["call", "failed", "-"]);
                assert_eq!(rows[1], &["align", "success", "success"]);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn sample_matrix_pair_cells_map_to_pair_id_instances() {
        let config = workflow_config(
            r#"[[pairs]]
pair_id = "CASE_001"
experiment = "EXP_01"
control = "CTRL_01"

[[pairs]]
pair_id = "CASE_002"
experiment = "EXP_02"
control = "CTRL_02"

[[rules]]
name = "call"
shell = "echo hi"
"#,
        );
        let mut cp = CheckpointState::new();
        // The engine names pair instances `{rule}_{pair_id}`.
        cp.completed_rules.insert("call_CASE_001".to_string());
        cp.failed_rules.insert("call_CASE_002".to_string());

        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let matrix = sections.iter().find(|s| s.id == "sample-matrix").unwrap();
        match &matrix.content {
            ReportContent::Table { headers, rows } => {
                assert_eq!(headers, &["Rule", "CTRL_01", "CTRL_02", "EXP_01", "EXP_02"]);
                // Both samples of a pair share the pair's instance state:
                // CASE_001 completed, CASE_002 failed.
                assert_eq!(rows[0], &["call", "success", "failed", "success", "failed"]);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn sample_matrix_matches_auto_discovered_instances() {
        let config = workflow_config(
            r#"[[sample_groups]]
name = "cohort"
samples = ["S1"]

[[rules]]
name = "count"
shell = "echo hi"
"#,
        );
        let mut cp = CheckpointState::new();
        // The engine expands {sample} rules against the "auto-discovered"
        // group when the group name is not part of the instance.
        cp.completed_rules
            .insert("count_auto-discovered_S1".to_string());
        cp.failed_rules
            .insert("count_auto-discovered_S1".to_string());

        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let matrix = sections.iter().find(|s| s.id == "sample-matrix").unwrap();
        match &matrix.content {
            ReportContent::Table { rows, .. } => {
                // Both instance spellings match; failed wins when both
                // sets claim the rule (defensive).
                assert_eq!(rows[0][1], "failed");
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn sample_matrix_gated_on_checkpoint_and_samples() {
        let config = workflow_config("[[rules]]\nname = \"hello\"\nshell = \"echo hi\"\n");
        // No checkpoint → hidden.
        let ctx = ctx_for(&config, None, None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        assert!(!sections.iter().any(|s| s.id == "sample-matrix"));
        // Checkpoint but no sample_groups/pairs → hidden.
        let cp = fixture_checkpoint();
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        assert!(!sections.iter().any(|s| s.id == "sample-matrix"));

        // Pairs contribute experiment + control samples; a completed pair
        // instance marks both of its samples success.
        let config = workflow_config(
            r#"[[pairs]]
pair_id = "CASE_001"
experiment = "EXP_01"
control = "CTRL_01"

[[rules]]
name = "call"
shell = "echo hi"
"#,
        );
        let mut cp = fixture_checkpoint();
        cp.completed_rules.insert("call_CASE_001".to_string());
        let ctx = ctx_for(&config, Some(&cp), None);
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let matrix = sections.iter().find(|s| s.id == "sample-matrix").unwrap();
        match &matrix.content {
            ReportContent::Table { headers, rows } => {
                assert_eq!(headers, &["Rule", "CTRL_01", "EXP_01"]);
                assert_eq!(rows[0], &["call", "success", "success"]);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn sample_matrix_filter_path_without_checkpoint_does_not_panic() {
        // `report.sections = ["sample-matrix"]` bypasses applicable() — the
        // generator must yield nothing instead of panicking on the missing
        // checkpoint.
        let config = workflow_config(
            r#"[[sample_groups]]
name = "cohort"
samples = ["S1"]

[[rules]]
name = "align"
shell = "echo hi"
"#,
        );
        let ctx = ctx_for(&config, None, None);
        let mut filter = std::collections::HashSet::new();
        filter.insert("sample-matrix".to_string());
        let filtered = SectionRegistry::with_defaults().generate(&ctx, Some(&filter));
        assert!(filtered.is_empty());
    }

    #[test]
    fn html_includes_a11y_landmarks() {
        let mut report = Report::new("T", "w", "0.1");
        report.add_section(ReportSection {
            title: "S".to_string(),
            id: "s".to_string(),
            content: ReportContent::Table {
                headers: vec!["A".to_string()],
                rows: vec![vec!["1".to_string()]],
            },
            subsections: vec![],
        });
        let html = report.to_html();
        assert!(html.contains("role=\"img\"") || html.contains("skip-link"));
        assert!(html.contains("<main id=\"main\">"));
        assert!(html.contains("scope=\"col\""));
        assert!(html.contains("@media print"));
    }
}

#[cfg(test)]
mod dashboard_status_tests {
    use super::*;
    use crate::executor::checkpoint::CheckpointState;

    fn workflow_config(extra: &str) -> WorkflowConfig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(
            &path,
            format!("[workflow]\nname = \"test\"\nversion = \"0.1\"\n{extra}"),
        )
        .unwrap();
        WorkflowConfig::from_file(&path).unwrap()
    }

    fn checkpoint_with(completed: &[&str], failed: &[&str]) -> CheckpointState {
        let mut ck = CheckpointState::new();
        for r in completed {
            ck.mark_completed(
                r,
                crate::executor::checkpoint::BenchmarkRecord {
                    rule: (*r).to_string(),
                    wall_time_secs: 1.0,
                    max_memory_mb: None,
                    memory_limit_mb: None,
                    cpu_seconds: None,
                    retries: 0,
                },
            );
        }
        for r in failed {
            ck.mark_failed(r);
        }
        ck
    }

    fn dashboard_html(config: &WorkflowConfig, ck: Option<&CheckpointState>) -> String {
        let ctx = ReportContext {
            config,
            checkpoint: ck,
            domain: WorkflowDomain::Generic,
            workflow_path: None,
            checkpoint_path: None,
        };
        let sections = SectionRegistry::with_defaults().generate(&ctx, None);
        let dashboard = sections.iter().find(|s| s.id == "dashboard").unwrap();
        let mut html = String::new();
        render_section_html(&mut html, dashboard, 2);
        html
    }

    #[test]
    fn dashboard_never_divides_instances_by_rule_count() {
        // Arrange — a scattered 1-rule workflow completing 3 instances:
        // 3 completed vs 1 rule must not read "3/1 succeeded".
        let config = workflow_config(
            r#"[[rules]]
name = "qc"
shell = "echo qc"
"#,
        );
        let ck = checkpoint_with(&["qc_batch_S1", "qc_batch_S2", "qc_batch_S3"], &[]);

        // Act
        let html = dashboard_html(&config, Some(&ck));

        // Assert — absolute count, no impossible ratio, no false partiality.
        assert!(html.contains("3 tasks succeeded"), "got: {html}");
        assert!(!html.contains("3/1"), "got: {html}");
        assert!(!html.contains("Partially complete"), "got: {html}");
    }

    #[test]
    fn dashboard_passes_when_completed_set_matches_rule_count() {
        // Arrange — single-instance rules: completed == total.
        let config = workflow_config(
            r#"[[rules]]
name = "a"
shell = "echo a"
[[rules]]
name = "b"
shell = "echo b"
"#,
        );
        let ck = checkpoint_with(&["a", "b"], &[]);

        // Act
        let html = dashboard_html(&config, Some(&ck));

        // Assert
        assert!(html.contains("2/2 succeeded"), "got: {html}");
        assert!(html.contains("All tasks completed"), "got: {html}");
    }

    #[test]
    fn dashboard_warns_with_absolute_counts_on_failure() {
        // Arrange
        let config = workflow_config(
            r#"[[rules]]
name = "a"
shell = "echo a"
[[rules]]
name = "b"
shell = "false"
"#,
        );
        let ck = checkpoint_with(&["a"], &["b"]);

        // Act
        let html = dashboard_html(&config, Some(&ck));

        // Assert — "1 failed, 1 succeeded", never a ratio against 2 rules.
        assert!(html.contains("1 failed, 1 succeeded"), "got: {html}");
        assert!(html.contains("1 task(s) failed"), "got: {html}");
    }
}
