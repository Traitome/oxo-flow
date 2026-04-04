//! oxo-flow-web — Web interface for the oxo-flow pipeline engine.
//!
//! Provides a REST API and web UI for building, running, and monitoring
//! bioinformatics workflows.

use axum::{
    extract::Json,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Health check response.
#[derive(Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Workflow list response.
#[derive(Serialize, Deserialize)]
struct WorkflowListResponse {
    workflows: Vec<WorkflowSummary>,
}

/// Summary of a workflow.
#[derive(Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub version: String,
    pub rules_count: usize,
}

/// Full workflow detail including parsed rules.
#[derive(Serialize, Deserialize)]
pub struct WorkflowDetail {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub rules_count: usize,
    pub rules: Vec<RuleSummary>,
}

/// Summary of a single rule within a workflow.
#[derive(Serialize, Deserialize)]
pub struct RuleSummary {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment: String,
    pub threads: u32,
}

/// Environment backend info.
#[derive(Serialize, Deserialize)]
struct EnvInfo {
    available: Vec<String>,
}

/// Request body for endpoints that accept TOML workflow content.
#[derive(Serialize, Deserialize)]
pub struct ValidateRequest {
    pub toml_content: String,
}

/// Response from the validation endpoint.
#[derive(Serialize, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub errors: Vec<String>,
    pub rules_count: Option<usize>,
    pub edges_count: Option<usize>,
}

/// Optional run configuration parameters.
#[derive(Serialize, Deserialize)]
pub struct RunConfig {
    pub max_jobs: Option<usize>,
    pub dry_run: Option<bool>,
    pub keep_going: Option<bool>,
}

/// Status of a workflow run (used in dry-run response).
#[derive(Serialize, Deserialize)]
pub struct RunStatus {
    pub id: String,
    pub status: String,
    pub rules_total: usize,
    pub rules_completed: usize,
    pub started_at: Option<String>,
}

/// Request body for the dry-run endpoint.
#[derive(Serialize, Deserialize)]
struct DryRunRequest {
    toml_content: String,
    #[serde(default)]
    config: Option<RunConfig>,
}

/// DAG visualisation response.
#[derive(Serialize, Deserialize)]
pub struct DagResponse {
    pub dot: String,
    pub nodes: usize,
    pub edges: usize,
}

/// Request body for report generation.
#[derive(Serialize, Deserialize)]
pub struct ReportRequest {
    pub toml_content: String,
    pub format: Option<String>,
}

/// Uniform JSON error body.
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub detail: Option<String>,
}

/// Response from the run endpoint.
#[derive(Serialize, Deserialize)]
pub struct RunResponse {
    pub run_id: String,
    pub status: String,
    pub execution_order: Vec<String>,
    pub rules_total: usize,
}

/// Response from the version endpoint.
#[derive(Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
    pub crate_name: String,
    pub rust_version: String,
}

/// Response from the clean endpoint.
#[derive(Serialize, Deserialize)]
pub struct CleanResponse {
    pub workflow_name: String,
    pub files_to_clean: Vec<String>,
    pub total_files: usize,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

/// Wrap an `ErrorResponse` with an HTTP status code so it can be returned from
/// any handler via `Result<impl IntoResponse, ApiError>`.
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn bad_request(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }

    fn unprocessable(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Existing endpoints
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn list_workflows() -> Json<WorkflowListResponse> {
    // Placeholder — will scan working directory for .oxoflow files
    Json(WorkflowListResponse { workflows: vec![] })
}

async fn list_environments() -> Json<EnvInfo> {
    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
    Json(EnvInfo {
        available: resolver
            .available_backends()
            .into_iter()
            .map(String::from)
            .collect(),
    })
}

async fn not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "Not found".to_string(),
            detail: None,
        }),
    )
}

// ---------------------------------------------------------------------------
// New endpoints
// ---------------------------------------------------------------------------

/// `POST /api/workflows/validate` — Parse + validate a workflow TOML.
async fn validate_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    let config = match oxo_flow_core::WorkflowConfig::parse(&req.toml_content) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: None,
                edges_count: None,
            }));
        }
    };

    let dag = match oxo_flow_core::WorkflowDag::from_rules(&config.rules) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: Some(config.rules.len()),
                edges_count: None,
            }));
        }
    };

    if let Err(e) = dag.validate() {
        return Ok(Json(ValidateResponse {
            valid: false,
            errors: vec![e.to_string()],
            rules_count: Some(dag.node_count()),
            edges_count: Some(dag.edge_count()),
        }));
    }

    Ok(Json(ValidateResponse {
        valid: true,
        errors: vec![],
        rules_count: Some(dag.node_count()),
        edges_count: Some(dag.edge_count()),
    }))
}

/// `POST /api/workflows/parse` — Parse a workflow and return full detail.
async fn parse_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<WorkflowDetail>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let rules: Vec<RuleSummary> = config
        .rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.clone(),
            outputs: r.output.clone(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    Ok(Json(WorkflowDetail {
        name: config.workflow.name.clone(),
        version: config.workflow.version.clone(),
        description: config.workflow.description.clone(),
        author: config.workflow.author.clone(),
        rules_count: rules.len(),
        rules,
    }))
}

/// `POST /api/workflows/dag` — Build a DAG and return its DOT representation.
async fn build_dag(Json(req): Json<ValidateRequest>) -> Result<Json<DagResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    Ok(Json(DagResponse {
        dot: dag.to_dot(),
        nodes: dag.node_count(),
        edges: dag.edge_count(),
    }))
}

/// `POST /api/workflows/dry-run` — Simulate execution and return the plan.
async fn dry_run(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let rules: Vec<RuleSummary> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.clone(),
            outputs: r.output.clone(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    let run_config = req.config.unwrap_or(RunConfig {
        max_jobs: None,
        dry_run: None,
        keep_going: None,
    });

    let status = RunStatus {
        id: uuid::Uuid::new_v4().to_string(),
        status: "dry-run".to_string(),
        rules_total: rules.len(),
        rules_completed: 0,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
    };

    #[derive(Serialize)]
    struct DryRunResponse {
        status: RunStatus,
        execution_order: Vec<String>,
        rules: Vec<RuleSummary>,
        config: RunConfig,
    }

    Ok(Json(DryRunResponse {
        status,
        execution_order: order,
        rules,
        config: run_config,
    }))
}

/// `POST /api/reports/generate` — Generate a report from a workflow.
async fn generate_report(Json(req): Json<ReportRequest>) -> Result<impl IntoResponse, ApiError> {
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

/// `POST /api/workflows/run` — Validate and return an execution plan as if starting a run.
async fn run_workflow(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    Ok(Json(RunResponse {
        run_id: uuid::Uuid::new_v4().to_string(),
        status: "started".to_string(),
        execution_order: order.clone(),
        rules_total: order.len(),
    }))
}

/// `GET /api/version` — Return crate version and build info.
async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        crate_name: env!("CARGO_PKG_NAME").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// `POST /api/workflows/clean` — List output files that would be cleaned.
async fn clean_workflow(Json(req): Json<ValidateRequest>) -> Result<Json<CleanResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let files_to_clean: Vec<String> = config.rules.iter().flat_map(|r| r.output.clone()).collect();

    Ok(Json(CleanResponse {
        workflow_name: config.workflow.name.clone(),
        total_files: files_to_clean.len(),
        files_to_clean,
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the web application router.
pub fn build_router() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/validate", post(validate_workflow))
        .route("/api/workflows/parse", post(parse_workflow))
        .route("/api/workflows/dag", post(build_dag))
        .route("/api/workflows/dry-run", post(dry_run))
        .route("/api/workflows/run", post(run_workflow))
        .route("/api/workflows/clean", post(clean_workflow))
        .route("/api/environments", get(list_environments))
        .route("/api/reports/generate", post(generate_report))
        .fallback(not_found)
        .layer(cors)
}

/// Start the web server.
pub async fn start_server(host: &str, port: u16) -> anyhow::Result<()> {
    let app = build_router();
    let addr = format!("{host}:{port}");
    tracing::info!("Starting oxo-flow web server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A minimal valid workflow TOML used across tests.
    const VALID_TOML: &str = r#"
[workflow]
name = "test-pipeline"
version = "1.0.0"
description = "A test workflow"
author = "Test Author"

[[rules]]
name = "step_a"
input = ["raw/{sample}.fastq"]
output = ["trimmed/{sample}.fastq"]
shell = "trim {input} > {output}"
threads = 4

[[rules]]
name = "step_b"
input = ["trimmed/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell = "align {input} > {output}"
threads = 8
[rules.environment]
docker = "biocontainers/bwa:0.7.17"
"#;

    /// Helper: send a POST request with a JSON body and return the response.
    async fn post_json(uri: &str, body: impl Serialize) -> axum::http::Response<Body> {
        let app = build_router();
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    // -- Existing endpoint tests -------------------------------------------------

    #[tokio::test]
    async fn health_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "ok");
    }

    #[tokio::test]
    async fn workflows_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/workflows")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn environments_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/environments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn not_found_fallback() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.error, "Not found");
    }

    // -- Validate endpoint -------------------------------------------------------

    #[tokio::test]
    async fn validate_valid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.valid);
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.rules_count, Some(2));
        assert!(parsed.edges_count.is_some());
    }

    #[tokio::test]
    async fn validate_invalid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: "this is not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.valid);
        assert!(!parsed.errors.is_empty());
    }

    // -- Parse endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn parse_valid_workflow() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: WorkflowDetail = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail.name, "test-pipeline");
        assert_eq!(detail.version, "1.0.0");
        assert_eq!(detail.description.as_deref(), Some("A test workflow"));
        assert_eq!(detail.author.as_deref(), Some("Test Author"));
        assert_eq!(detail.rules_count, 2);
        assert_eq!(detail.rules[0].name, "step_a");
        assert_eq!(detail.rules[0].threads, 4);
        assert_eq!(detail.rules[1].environment, "docker");
    }

    #[tokio::test]
    async fn parse_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: "not valid".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    // -- DAG endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn dag_generation() {
        let resp = post_json(
            "/api/workflows/dag",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dag: DagResponse = serde_json::from_slice(&body).unwrap();
        assert!(dag.dot.contains("digraph"));
        assert_eq!(dag.nodes, 2);
        assert!(dag.edges >= 1);
    }

    // -- Dry-run endpoint --------------------------------------------------------

    #[tokio::test]
    async fn dry_run_endpoint() {
        let resp = post_json(
            "/api/workflows/dry-run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: Some(RunConfig {
                    max_jobs: Some(4),
                    dry_run: Some(true),
                    keep_going: Some(false),
                }),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let status = &value["status"];
        assert_eq!(status["status"], "dry-run");
        assert_eq!(status["rules_total"], 2);
        assert_eq!(status["rules_completed"], 0);

        let order = value["execution_order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        // step_a must come before step_b (dependency)
        assert_eq!(order[0], "step_a");
        assert_eq!(order[1], "step_b");

        let rules = value["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
    }

    // -- Report generation endpoint ----------------------------------------------

    #[tokio::test]
    async fn report_generate_html() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: None, // default → html
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-pipeline"));
    }

    #[tokio::test]
    async fn report_generate_json() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: Some("json".to_string()),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/json");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["workflow_name"], "test-pipeline");
        assert!(value["sections"].as_array().unwrap().len() >= 2);
    }

    // -- Run endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn run_workflow_endpoint() {
        let resp = post_json(
            "/api/workflows/run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: None,
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: RunResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "started");
        assert_eq!(parsed.rules_total, 2);
        assert!(!parsed.run_id.is_empty());
        assert_eq!(parsed.execution_order.len(), 2);
        assert_eq!(parsed.execution_order[0], "step_a");
        assert_eq!(parsed.execution_order[1], "step_b");
    }

    // -- Version endpoint --------------------------------------------------------

    #[tokio::test]
    async fn version_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: VersionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(parsed.crate_name, "oxo-flow-web");
    }

    // -- Clean endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn clean_workflow_endpoint() {
        let resp = post_json(
            "/api/workflows/clean",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: CleanResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.workflow_name, "test-pipeline");
        assert_eq!(parsed.total_files, 2);
        assert!(parsed
            .files_to_clean
            .contains(&"trimmed/{sample}.fastq".to_string()));
        assert!(parsed
            .files_to_clean
            .contains(&"aligned/{sample}.bam".to_string()));
    }
}
