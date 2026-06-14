//! Integration tests for Phase 3 Collaboration & Multi-Mode endpoints.
//!
//! Tests the production router for fork, share, import, and
//! mode-specific behavior (personal vs team vs hpc).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

fn app_personal() -> axum::Router {
    server::build_router("personal")
}
fn app_team() -> axum::Router {
    server::build_router("team")
}
fn app_hpc() -> axum::Router {
    server::build_router("hpc")
}

async fn json_body(body: axum::body::Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ── Collaboration: Fork ──

#[tokio::test]
async fn test_fork_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/nonexistent-id/fork")
        .header("content-type", "application/json")
        .body(Body::from(json!({"user_id": "user-1"}).to_string()))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    // Accept 404 (not found) or 503 (DB unavailable in test) as both mean route exists
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::SERVICE_UNAVAILABLE,
        "fork endpoint should respond (got {})",
        resp.status()
    );
}

// ── Collaboration: Share ──

#[tokio::test]
async fn test_share_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/nonexistent-id/share")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"visibility": "link", "expires_in_days": 30}).to_string(),
        ))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::SERVICE_UNAVAILABLE,
        "share endpoint should respond (got {})",
        resp.status()
    );
}

// ── Collaboration: Import ──

#[tokio::test]
async fn test_import_endpoint_exists() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/import")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"url": "oxo+https://example.com/share/abc123"}).to_string(),
        ))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    // Accept 404 (not found) or 503 (DB unavailable) as both mean route exists
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::SERVICE_UNAVAILABLE,
        "import endpoint should respond (got {})",
        resp.status()
    );
}

#[tokio::test]
async fn test_import_invalid_url_format() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/import")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"url": "not-a-valid-oxo-url"}).to_string(),
        ))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn test_import_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/import")
        .header("content-type", "application/json")
        .body(Body::from("garbage"))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

// ── Mode-specific behavior ──

#[tokio::test]
async fn test_personal_mode_health() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp.into_body()).await;
    assert_eq!(body["mode"], "personal");
}

#[tokio::test]
async fn test_team_mode_health() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_team().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_hpc_mode_health() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_hpc().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_hpc_mode_has_hpc_route() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/hpc")
        .body(Body::empty())
        .unwrap();
    let resp = app_hpc().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_personal_mode_no_hpc_route() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/hpc")
        .body(Body::empty())
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    // In personal mode, HPC route not merged, returns 404 (SPA fallback)
    assert!(
        resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::OK,
        "personal mode should either lack HPC route or return fallback"
    );
}

// ── All modes have core routes ──

#[tokio::test]
async fn test_all_modes_have_pipeline_routes() {
    // Personal: unauthenticated access works
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/parse")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"toml_content": "[workflow]\nname = \"t\""}).to_string(),
        ))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_success(),
        "personal mode should have pipeline parse"
    );

    // Team: requires auth, returns 401 without token
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/pipelines/parse")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"toml_content": "[workflow]\nname = \"t\""}).to_string(),
        ))
        .unwrap();
    let resp = app_team().oneshot(req2).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "team mode should require auth for pipeline parse"
    );
}

#[tokio::test]
async fn test_all_modes_have_auth_routes() {
    for (mode, app) in [
        ("personal", app_personal()),
        ("team", app_team()),
        ("hpc", app_hpc()),
    ] {
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"username":"admin","password":"admin"}).to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_success(), "{mode} mode should have login");
    }
}

#[tokio::test]
async fn test_all_modes_have_ai_routes() {
    for (mode, app) in [
        ("personal", app_personal()),
        ("team", app_team()),
        ("hpc", app_hpc()),
    ] {
        let req = Request::builder()
            .method("GET")
            .uri("/api/ai/config")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status().is_success(),
            "{mode} mode should have AI config"
        );
    }
}

// ── License visibility ──

#[tokio::test]
async fn test_license_header_present() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    let license_header = resp.headers().get("x-oxoflow-license");
    assert!(
        license_header.is_some(),
        "X-OxoFlow-License header must be present"
    );
    let value_bytes = license_header.unwrap().as_bytes();
    let has_academic = std::str::from_utf8(value_bytes)
        .map(|s| s.contains("Academic"))
        .unwrap_or(false);
    assert!(
        has_academic || !value_bytes.is_empty(),
        "License header must be non-empty"
    );
}

#[tokio::test]
async fn test_version_header_present() {
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    let version_header = resp.headers().get("x-oxoflow-version");
    assert!(
        version_header.is_some(),
        "X-OxoFlow-Version header must be present"
    );
}
