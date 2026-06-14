//! Integration tests for Phase 1 deterministic core endpoints.
//!
//! Tests the production router from `server::build_router("personal")`
//! to ensure all Phase 1 endpoints respond correctly.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn json_body(body: axum::body::Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ── POST /api/data/analyze ──

#[tokio::test]
async fn test_data_analyze_valid_paths() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/analyze")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"paths": ["/tmp/test.fastq.gz"], "max_depth": 1}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(body.get("files").is_some(), "should have files array");
    assert!(body.get("summary").is_some(), "should have summary");
}

#[tokio::test]
async fn test_data_analyze_empty_paths() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/analyze")
        .header("content-type", "application/json")
        .body(Body::from(json!({"paths": []}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Empty paths: returns 200 with empty results
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    let files = body["files"].as_array().unwrap();
    assert!(files.is_empty(), "empty paths should yield empty files");
}

#[tokio::test]
async fn test_data_analyze_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/analyze")
        .header("content-type", "application/json")
        .body(Body::from("not json"))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

// ── POST /api/data/reference ──

#[tokio::test]
async fn test_data_reference_known_genome() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/reference")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"genome": "hg38", "components": ["fasta", "gtf"]}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(body.get("found").is_some(), "should have found list");
    assert!(body.get("missing").is_some(), "should have missing list");
}

#[tokio::test]
async fn test_data_reference_empty_components() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/reference")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"genome": "mm10", "components": []}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_data_reference_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/data/reference")
        .header("content-type", "application/json")
        .body(Body::from("bad json"))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

// ── POST /api/plugins/validate ──

#[tokio::test]
async fn test_plugin_validate_valid_manifest() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins/validate")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "manifest": {
                    "name": "test-plugin",
                    "version": "1.0.0",
                    "plugin_type": "rule",
                    "description": "A test plugin",
                    "author": "tester"
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert_eq!(body["valid"], true);
    assert_eq!(body["name"], "test-plugin");
    assert!(body["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_plugin_validate_missing_fields() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins/validate")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "manifest": {
                    "name": "",
                    "version": "",
                    "plugin_type": "invalid_type"
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert_eq!(body["valid"], false);
    let errors = body["errors"].as_array().unwrap();
    assert!(!errors.is_empty(), "should have validation errors");
}

#[tokio::test]
async fn test_plugin_validate_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins/validate")
        .header("content-type", "application/json")
        .body(Body::from("garbage"))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

// ── POST /api/pipelines/diff ──

#[tokio::test]
async fn test_pipeline_diff_identical() {
    let toml = "[workflow]\nname = \"test\"\n[[rules]]\nname = \"step1\"\n";
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/diff")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"toml_a": toml, "toml_b": toml}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(body.get("diffs").is_some(), "should have diffs array");
    // Identical pipelines should have empty diffs
    let diffs = body["diffs"].as_array().unwrap();
    assert!(diffs.is_empty(), "identical pipelines should have no diffs");
}

#[tokio::test]
async fn test_pipeline_diff_different() {
    let toml_a = "[workflow]\nname = \"test-a\"\n[[rules]]\nname = \"step1\"\n";
    let toml_b = "[workflow]\nname = \"test-b\"\n[[rules]]\nname = \"step2\"\n";
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/diff")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"toml_a": toml_a, "toml_b": toml_b}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    let diffs = body["diffs"].as_array().unwrap();
    assert!(!diffs.is_empty(), "different pipelines should have diffs");
}

#[tokio::test]
async fn test_pipeline_diff_invalid_toml() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/diff")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"toml_a": "not valid toml [[[", "toml_b": "also bad [[["}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error() || resp.status() == StatusCode::OK);
    // If OK, should have parse errors in the diffs
    if resp.status() == StatusCode::OK {
        let body: Value = json_body(resp.into_body()).await;
        assert!(body.get("diffs").is_some());
    }
}

// ── Health check (Phase 1 requirement) ──

#[tokio::test]
async fn test_health_endpoint_returns_ok() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(body.get("status").is_some(), "health should have status");
    assert!(
        body.get("license").is_some(),
        "health should have license info"
    );
}
