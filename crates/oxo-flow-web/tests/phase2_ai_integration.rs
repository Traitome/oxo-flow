//! Integration tests for Phase 2 AI Translation Layer endpoints.
//!
//! Tests the production router from `server::build_router("personal")`
//! for all 4 AI endpoints: translate, explain, interpret, optimize.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn json_body(body: axum::body::Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ── POST /api/ai/translate ──

#[tokio::test]
async fn test_ai_translate_rejects_a_missing_intent_field() {
    common::ensure_db().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate")
        .header("content-type", "application/json")
        .body(Body::from(json!({}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_client_error(),
        "a missing required intent must fail before any provider I/O"
    );
}

#[tokio::test]
async fn test_ai_translate_empty_intent() {
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/explain")
        .header("content-type", "application/json")
        .body(Body::from(json!({"run_id": "nonexistent-run"}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Ownership enforcement (#515): an unknown (or foreign) run id answers
    // 404 — the run's existence itself is private.
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "/api/ai/explain must 404 on an unknown run"
    );
}

#[tokio::test]
async fn test_ai_explain_bad_request() {
    common::ensure_db().await;
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
    common::ensure_db().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/interpret")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"run_id": "nonexistent-run", "result_type": "deg"}).to_string(),
        ))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    // Ownership enforcement (#515): an unknown (or foreign) run id answers
    // 404 — the run's existence itself is private.
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "/api/ai/interpret must 404 on an unknown run"
    );
}

// ── POST /api/ai/optimize ──

#[tokio::test]
async fn test_ai_optimize_endpoint_exists() {
    common::ensure_db().await;
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
    common::ensure_db().await;
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
async fn test_ai_translate_sse_rejects_an_empty_intent_without_provider_io() {
    common::ensure_db().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/translate/stream")
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(Body::from(json!({"intent": ""}).to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let is_sse = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").contains("text/event-stream"))
        .unwrap_or(false);
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(is_sse, "SSE response should be text/event-stream");
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("INVALID_INTENT"),
        "empty intent must produce the structured error event: {body}"
    );
}

// ── GET /api/ai/config ──

#[tokio::test]
async fn test_ai_config_get() {
    common::ensure_db().await;
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
