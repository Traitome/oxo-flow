//! Modular report generation system.
//!
//! Provides a framework for generating structured reports (HTML, JSON)
//! from workflow execution results. Designed for clinical-grade reporting
//! with full traceability and provenance.

use crate::error::Result;
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

    /// Render the report as a JSON string.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Render the report as a minimal HTML document.
    pub fn to_html(&self) -> String {
        let mut html = String::new();
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        html.push_str(&format!("  <title>{}</title>\n", self.title));
        html.push_str("  <meta charset=\"utf-8\">\n");
        html.push_str("  <style>\n");
        html.push_str("    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 900px; margin: 0 auto; padding: 20px; }\n");
        html.push_str("    table { border-collapse: collapse; width: 100%; margin: 1em 0; }\n");
        html.push_str("    th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }\n");
        html.push_str("    th { background-color: #4a90d9; color: white; }\n");
        html.push_str("    tr:nth-child(even) { background-color: #f2f2f2; }\n");
        html.push_str("    .metadata { color: #666; font-size: 0.9em; }\n");
        html.push_str("  </style>\n</head>\n<body>\n");

        html.push_str(&format!("<h1>{}</h1>\n", self.title));
        html.push_str(&format!(
            "<p class=\"metadata\">Workflow: {} v{} | Generated: {}</p>\n",
            self.workflow_name, self.workflow_version, self.generated_at
        ));

        for section in &self.sections {
            render_section_html(&mut html, section, 2);
        }

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
}
