//! Report generation handlers.
//!
//! Handles workflow report generation in HTML and JSON formats.

use axum::{extract::Json, http::StatusCode, response::IntoResponse};

use crate::{ApiError, ReportRequest};

/// `POST /api/reports/generate` — Generate a report from a workflow.
pub async fn generate_report(
    Json(req): Json<ReportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    // Build a report with workflow overview and rule details.
    let mut report = oxo_flow_core::report::Report::new(
        &format!("{} — Workflow Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );

    report.add_metadata("rules_count", &config.rules.len().to_string());
    report.add_metadata("edges_count", &dag.edge_count().to_string());

    // Overview section
    let overview = oxo_flow_core::report::ReportSection {
        title: "Workflow Overview".to_string(),
        id: "overview".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue {
            pairs: vec![
                ("Name".to_string(), config.workflow.name.clone()),
                ("Version".to_string(), config.workflow.version.clone()),
                (
                    "Description".to_string(),
                    config
                        .workflow
                        .description
                        .clone()
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
                ("Rules".to_string(), config.rules.len().to_string()),
                ("DAG edges".to_string(), dag.edge_count().to_string()),
            ],
        },
        subsections: vec![],
    };
    report.add_section(overview);

    // Execution order section
    let exec_section = oxo_flow_core::report::ReportSection {
        title: "Execution Order".to_string(),
        id: "execution-order".to_string(),
        content: oxo_flow_core::report::ReportContent::Table {
            headers: vec![
                "Step".to_string(),
                "Rule".to_string(),
                "Threads".to_string(),
                "Environment".to_string(),
            ],
            rows: order
                .iter()
                .enumerate()
                .filter_map(|(i, name)| {
                    config.get_rule(name).map(|r| {
                        vec![
                            (i + 1).to_string(),
                            r.name.clone(),
                            r.effective_threads().to_string(),
                            r.environment.kind().to_string(),
                        ]
                    })
                })
                .collect(),
        },
        subsections: vec![],
    };
    report.add_section(exec_section);

    let format = req.format.unwrap_or_else(|| "html".to_string());

    match format.as_str() {
        "json" => {
            let json = report.to_json().map_err(|e| {
                ApiError::unprocessable("Report generation failed", Some(e.to_string()))
            })?;
            Ok((StatusCode::OK, [("content-type", "application/json")], json))
        }
        _ => {
            let html = report.to_html();
            Ok((StatusCode::OK, [("content-type", "text/html")], html))
        }
    }
}
