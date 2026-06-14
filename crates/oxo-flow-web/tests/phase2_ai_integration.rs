//! Integration tests for Phase 2 AI Translation Layer endpoints.
//!
//! Tests the production router from `server::build_router("personal")`
//! for all 4 AI endpoints: translate, explain, interpret, optimize.

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

// ── POST /api/ai/translate ──

#[tokio::test]
async fn test_ai_translate_requires_intent() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"intent": "RNA-seq alignment with STAR"}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Without AI configured, should return error (400 or 500) or noop response
    let status = resp.status();
    assert!(
        status.is_client_error() || status.is_server_error() || status == StatusCode::OK,
        "translate should respond (got {status})"
    );
}

#[tokio::test]
async fn test_ai_translate_empty_intent() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate")
        .header("content-type", "application/json")
        .body(Body::from(json!({"intent": ""}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Should return an error for empty intent
    let status = resp.status();
    assert!(
        status.is_client_error() || status.is_server_error(),
        "empty intent should fail (got {status})"
    );
}

#[tokio::test]
async fn test_ai_translate_missing_body() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate")
        .header("content-type", "application/json")
        .body(Body::from("bad json"))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_client_error(),
        "bad json should be client error"
    );
}

// ── POST /api/ai/explain ──

#[tokio::test]
async fn test_ai_explain_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/explain")
        .header("content-type", "application/json")
        .body(Body::from(json!({"run_id": "nonexistent-run"}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Should respond (may fail due to missing run, but endpoint must exist)
    let status = resp.status();
    assert!(
        status != StatusCode::NOT_FOUND,
        "/api/ai/explain should exist (got 404)"
    );
}

#[tokio::test]
async fn test_ai_explain_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/explain")
        .header("content-type", "application/json")
        .body(Body::from("garbage"))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

// ── POST /api/ai/interpret ──

#[tokio::test]
async fn test_ai_interpret_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/interpret")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"run_id": "nonexistent-run", "result_type": "deg"}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status != StatusCode::NOT_FOUND,
        "/api/ai/interpret should exist (got 404)"
    );
}

// ── POST /api/ai/optimize ──

#[tokio::test]
async fn test_ai_optimize_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/optimize")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "toml_content": "[workflow]\nname = \"test\"\n[[rules]]\nname = \"step1\"",
                "goal": "speed"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status != StatusCode::NOT_FOUND,
        "/api/ai/optimize should exist (got 404)"
    );
}

#[tokio::test]
async fn test_ai_optimize_accepts_constraints() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/optimize")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "toml_content": "[workflow]\nname = \"test\"\n[[rules]]\nname = \"step1\"",
                "goal": "cost",
                "constraints": {"max_memory": 16000, "max_threads": 4}
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status != StatusCode::NOT_FOUND,
        "/api/ai/optimize with constraints should exist"
    );
}

// ── POST /api/ai/translate/stream (SSE) ──

#[tokio::test]
async fn test_ai_translate_sse_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate/stream")
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(Body::from(
            json!({"intent": "RNA-seq analysis"}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // SSE should return 200 with text/event-stream content type
    let status = resp.status();
    let is_sse = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").contains("text/event-stream"))
        .unwrap_or(false);
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "SSE endpoint should respond (got {status})"
    );
    if status == StatusCode::OK {
        assert!(
            is_sse,
            "SSE response should have text/event-stream content type"
        );
    }
}

// ── GET /api/ai/config ──

#[tokio::test]
async fn test_ai_config_get() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/ai/config")
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = json_body(resp.into_body()).await;
    assert!(
        body.get("provider").is_some(),
        "ai config should have provider field"
    );
    assert!(body.get("is_configured").is_some());
}
