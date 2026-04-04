//! oxo-flow-web — Web interface for the oxo-flow pipeline engine.
//!
//! Provides a REST API and web UI for building, running, and monitoring
//! bioinformatics workflows.

use axum::{extract::Json, http::StatusCode, routing::get, Router};
use serde::{Deserialize, Serialize};

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Workflow list response.
#[derive(Serialize)]
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

/// Environment backend info.
#[derive(Serialize)]
struct EnvInfo {
    available: Vec<String>,
}

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

async fn not_found() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "Not found")
}

/// Build the web application router.
pub fn build_router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/workflows", get(list_workflows))
        .route("/api/environments", get(list_environments))
        .fallback(not_found)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

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
    }
}
