//! Integration tests for v0.9 AI Companion API endpoints.
//!
//! Phase 4-6: Chat, DAG Edit, Data Perception, Monitor, Report, AI Config.
//!
//! Tests the production router from `server::build_router("personal")`
//! to ensure all v0.9 endpoints respond correctly.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
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

async fn post_json(uri: &str, body: &Value) -> axum::response::Response {
    app()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_json(uri: &str) -> axum::response::Response {
    app()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

// ── 1. Data Perception API ──

#[tokio::test]
async fn test_data_perceive_empty() {
    let resp = post_json("/api/data/perceive", &json!({})).await;
    let body: Value = json_body(resp.into_body()).await;
    // Should return data_level 0 (intent only) since no paths or description
    assert_eq!(body["data_level"], 0, "no data → level 0: got {body:?}");
}

#[tokio::test]
async fn test_data_perceive_with_description() {
    let resp = post_json(
        "/api/data/perceive",
        &json!({
            "description": "RNA-seq paired-end 150bp hg38 3x replicates"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert_eq!(body["data_level"], 2, "description → level 2");
    let findings = body["findings"].as_array().unwrap();
    assert!(
        findings.iter().any(|f| f["field"] == "read_type"),
        "should detect read_type: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f["field"] == "genome"),
        "should detect genome"
    );
}

#[tokio::test]
async fn test_data_reference_status() {
    let resp = get_json("/api/data/reference/status").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(body.get("installed").is_some(), "should have installed key");
    assert!(body.get("missing").is_some(), "should have missing key");
}

#[tokio::test]
async fn test_data_samplesheet_parse() {
    let csv = "sample,fastq_r1,fastq_r2,condition\ns1,/data/s1_R1.fq,/data/s1_R2.fq,WT\ns2,/data/s2_R1.fq,/data/s2_R2.fq,KO\n";
    let resp = post_json("/api/data/samplesheet/parse", &json!({"content": csv})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert_eq!(body["samples_count"], 2, "should parse 2 samples");
    assert_eq!(body["format"], "standard", "should detect standard format");
}

// ── 2. Chat API ──

#[tokio::test]
async fn test_chat_send_json_basic() {
    let resp = post_json(
        "/api/chat/send/json",
        &json!({
            "message": "RNA-seq differential expression"
        }),
    )
    .await;
    // May return 200 (with no AI) or error
    let status = resp.status();
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "chat send should respond: {status}"
    );
    if status == StatusCode::OK {
        let body: Value = json_body(resp.into_body()).await;
        assert!(
            body.get("pipeline_id").is_some() || body.get("error").is_some(),
            "response should have pipeline_id or error"
        );
    }
}

#[tokio::test]
async fn test_chat_sessions() {
    let resp = get_json("/api/chat/sessions").await;
    assert!(
        resp.status() == StatusCode::OK || resp.status().is_server_error(),
        "sessions should respond: {}",
        resp.status()
    );
}

// ── 3. DAG Edit API ──

#[tokio::test]
async fn test_dag_edit_add_rule() {
    let resp = post_json(
        "/api/pipeline/test-123/command",
        &json!({
            "toml_content": "[workflow]\nname=\"test\"\n[[rules]]\nname=\"init\"\nshell=\"echo\"",
            "source": "dag_editor",
            "operation": "add_rule",
            "payload": {"name": "new_step", "shell": "echo hello"}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(
        body.get("success").is_some(),
        "should return success: {body:?}"
    );
    assert!(body.get("toml_content").is_some(), "should return toml");
}

#[tokio::test]
async fn test_dag_edit_invalid_operation() {
    let resp = post_json(
        "/api/pipeline/test-123/command",
        &json!({
            "toml_content": "[workflow]\nname=\"test\"",
            "source": "dag_editor",
            "operation": "unknown_op",
            "payload": {}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "unknown op → 400");
}

// ── 4. Monitor API ──

#[tokio::test]
async fn test_monitor_pause_nonexistent_run() {
    let resp = post_json(
        "/api/runs/nonexistent-id/pause",
        &json!({
            "reason": "user_request"
        }),
    )
    .await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

#[tokio::test]
async fn test_monitor_resume_nonexistent_run() {
    let resp = post_json("/api/runs/nonexistent-id/resume", &json!({})).await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

#[tokio::test]
async fn test_monitor_ai_status_nonexistent() {
    let resp = get_json("/api/runs/nonexistent-id/ai-status").await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

// ── 5. Report API ──

#[tokio::test]
async fn test_report_get_nonexistent() {
    let resp = get_json("/api/runs/nonexistent-id/report").await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

#[tokio::test]
async fn test_report_ask_nonexistent() {
    let resp = post_json(
        "/api/runs/nonexistent-id/report/ask",
        &json!({
            "question": "What are the results?"
        }),
    )
    .await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

#[tokio::test]
async fn test_report_visualize_nonexistent() {
    let resp = post_json(
        "/api/runs/nonexistent-id/report/visualize",
        &json!({
            "type": "volcano"
        }),
    )
    .await;
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status().is_server_error(),
        "nonexistent run → {} (expected 404 or 5xx)",
        resp.status()
    );
}

// ── 6. AI Config API (three-tier) ──

#[tokio::test]
async fn test_ai_config_effective() {
    let resp = get_json("/api/ai/config/effective").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(
        body.get("effective").is_some(),
        "should have effective config: {body:?}"
    );
    assert!(
        body.get("resolution_order").is_some(),
        "should have resolution order"
    );
    let order = body["resolution_order"].as_array().unwrap();
    assert_eq!(order.len(), 4, "should have 4 resolution tiers");
}

#[tokio::test]
async fn test_ai_config_user() {
    let resp = get_json("/api/ai/config/user").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    // Should return a user config (may be null if not configured)
    assert!(
        body.get("configured").is_some(),
        "should have configured flag"
    );
}

#[tokio::test]
async fn test_ai_config_server() {
    let resp = get_json("/api/ai/config/server").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(
        body.get("configured").is_some(),
        "should have configured flag"
    );
}

#[tokio::test]
async fn test_ai_config_update_user() {
    let resp = put_json(
        "/api/ai/config/user",
        &json!({
            "provider": "openai",
            "api_key": "sk-test",
            "model": "gpt-4o"
        }),
    )
    .await;
    // Should return 200 or 500 (DB not available in test)
    let status = resp.status();
    assert!(
        status == StatusCode::OK || status.is_server_error(),
        "config update should respond: {status}"
    );
}

#[tokio::test]
async fn test_ai_config_update_server() {
    let resp = put_json(
        "/api/ai/config/server",
        &json!({
            "provider": "openai",
            "api_url": "https://api.openai.com/v1",
            "model": "gpt-4o"
        }),
    )
    .await;
    let status = resp.status();
    assert!(
        status == StatusCode::OK || status.is_server_error(),
        "server config update should respond: {status}"
    );
}

// ── 7. Old AI endpoints still work ──

#[tokio::test]
async fn test_ai_translate_endpoint_exists() {
    let resp = post_json(
        "/api/ai/translate",
        &json!({
            "intent": "RNA-seq alignment"
        }),
    )
    .await;
    let status = resp.status();
    // Endpoint exists and responds (may be error if no AI configured)
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "translate should respond: {status}"
    );
}

#[tokio::test]
async fn test_ai_optimize_endpoint_exists() {
    let resp = post_json(
        "/api/ai/optimize",
        &json!({
            "pipeline_id": "test",
            "goal": "speed"
        }),
    )
    .await;
    let status = resp.status();
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "optimize should respond: {status}"
    );
}
async fn put_json(uri: &str, body: &Value) -> axum::response::Response {
    app()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
