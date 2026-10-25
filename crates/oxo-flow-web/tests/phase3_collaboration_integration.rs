//! Integration tests for Phase 3 Collaboration & Multi-Mode endpoints.
//!
//! Tests the production router for fork, share, import, and
//! mode-specific behavior (personal vs team vs hpc).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

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
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
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

// ── Share URL transport truth (#907 R16) ──

#[test]
fn share_url_scheme_follows_actual_transport() {
    // SAFETY: env::set_var/remove_var are process-wide and racy with any
    // concurrent getenv — all cases therefore run in ONE serial test, and no
    // other test in this binary touches OXO_FLOW_PUBLIC_URL/OXO_FLOW_HOST.
    // set_base_path/set_bound_port are first-call-wins OnceLocks; no other
    // test in this binary sets them, so the values chosen here hold.
    unsafe {
        // Default: the server has no native TLS, so without an explicit
        // public-URL override the minted scheme must be oxo+http:// — the
        // old hardcoded oxo+https:// was a transport lie on plain-HTTP
        // deployments.
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");
        let url = server::share_url_for("tok-r16");
        assert!(
            url.starts_with("oxo+http://") && !url.starts_with("oxo+https://"),
            "default share URL must be oxo+http:// (got {url})"
        );
        assert!(
            url.ends_with("/share/tok-r16"),
            "URL must end in the token path (got {url})"
        );

        // Override with an HTTPS public origin (reverse-proxy deployment):
        // scheme AND authority both come from the override.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "https://flows.example.com");
        assert_eq!(
            server::share_url_for("tok-pub"),
            "oxo+https://flows.example.com/share/tok-pub"
        );

        // Override with an HTTP public origin keeps the plain scheme.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "http://gw.internal:9000");
        assert_eq!(
            server::share_url_for("tok-gw"),
            "oxo+http://gw.internal:9000/share/tok-gw"
        );

        // Path-carrying override (deploy-modes.md reverse-proxy recipe):
        // the override IS the complete public origin — the local base path
        // must NOT be appended a second time (review B1: /oxo-flow twice).
        server::set_base_path("/oxo-flow");
        server::set_bound_port(18080);
        std::env::set_var(
            "OXO_FLOW_PUBLIC_URL",
            "https://flows.lab.example.edu/oxo-flow",
        );
        assert_eq!(
            server::share_url_for("tok-mount"),
            "oxo+https://flows.lab.example.edu/oxo-flow/share/tok-mount"
        );

        // Host-only override with a base path mounted: the local prefix
        // still applies.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "https://cdn.example.com");
        assert_eq!(
            server::share_url_for("tok-host"),
            "oxo+https://cdn.example.com/oxo-flow/share/tok-host"
        );
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");

        // An override that carries its own mount path also wins over a
        // DIFFERENT local base path — the override is the whole origin
        // (guards against a regression to double-prefixing).
        server::set_base_path("/other");
        std::env::set_var(
            "OXO_FLOW_PUBLIC_URL",
            "https://flows.lab.example.edu/oxo-flow",
        );
        assert_eq!(
            server::share_url_for("tok-mount2"),
            "oxo+https://flows.lab.example.edu/oxo-flow/share/tok-mount2"
        );
        server::set_base_path("/oxo-flow");
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");

        // Malformed overrides fall back to the local mint (review N4):
        // port-only authority, bare IPv6 literal.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "https://:8080");
        let url = server::share_url_for("tok-porthost");
        assert!(
            url.starts_with("oxo+http://localhost:18080/oxo-flow/share/tok-porthost"),
            "port-only override must be ignored for a local mint (got {url})"
        );
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "::1");
        let url = server::share_url_for("tok-v6-override");
        assert_eq!(
            url, "oxo+http://[::1]/oxo-flow/share/tok-v6-override",
            "scheme-less IPv6 override must be bracketed, no local port injected (got {url})"
        );
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");

        // Host-only IPv6 override keeps the bracketing too.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "http://[2001:db8::1]:8443");
        assert_eq!(
            server::share_url_for("tok-v6-br"),
            "oxo+http://[2001:db8::1]:8443/oxo-flow/share/tok-v6-br"
        );
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");

        // Default branch with a base path + bound port: prefix and port flow
        // through, and an IPv6 host literal gets bracketed (review N2).
        std::env::set_var("OXO_FLOW_HOST", "::1");
        let url = server::share_url_for("tok-v6");
        assert_eq!(
            url, "oxo+http://[::1]:18080/oxo-flow/share/tok-v6",
            "IPv6 literal must be bracketed in the default mint (got {url})"
        );
        std::env::remove_var("OXO_FLOW_HOST");
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");

        // A whitespace-only override behaves as unset — with a warning in
        // the logs (review N5) — and the default mint still applies.
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "   ");
        let url = server::share_url_for("tok-ws");
        assert!(
            url.starts_with("oxo+http://") && url.ends_with("/oxo-flow/share/tok-ws"),
            "whitespace-only override must fall through to the local mint (got {url})"
        );
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");
    }
}

#[tokio::test]
async fn test_import_accepts_oxo_http_scheme() {
    // The default minted scheme changed to oxo+http://; the import endpoint
    // must accept it (only a truly malformed URL is rejected).
    common::ensure_db().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines/import")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"url": "oxo+http://example.com/share/abc123"}).to_string(),
        ))
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    // 404/400 = the URL format was accepted and the token lookup ran;
    // 4xx for an unknown token is fine — a 400 INVALID_URL would NOT be.
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let code = body["code"].as_str().unwrap_or("");
    assert_ne!(code, "INVALID_URL", "oxo+http:// must pass URL validation");
}

#[tokio::test]
async fn test_import_bad_request() {
    common::ensure_db().await;
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
    common::ensure_db().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    let resp = app_personal().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp.into_body()).await;
    // The mode global is process-wide (first build_router wins — the
    // production contract: a server process has exactly one mode). The
    // invariant this test owns is health reporting EXACTLY the recorded
    // process mode, immune to which sibling test built its router first
    // (verified flake root cause, issue #268 item 2.1).
    assert_eq!(body["mode"], oxo_flow_web::server::running_mode());
}

#[tokio::test]
async fn test_team_mode_health() {
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/hpc")
        .body(Body::empty())
        .unwrap();
    let resp = app_hpc().oneshot(req).await.unwrap();
    // The route exists in hpc mode, but cluster internals are NOT
    // anonymous (issue #82 P0-5): without a session it must 401, not 200.
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "/api/hpc must require authentication in hpc mode"
    );
}

#[tokio::test]
async fn test_personal_mode_no_hpc_route() {
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
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
                json!({"username":"admin","password":"wrong"}).to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Should return a JSON response (401 for bad credentials), not a 404.
        // We check the route exists, not that credentials are accepted.
        assert!(
            resp.status() == axum::http::StatusCode::UNAUTHORIZED || resp.status().is_success(),
            "{mode} mode: login route should exist (expected 401 or 200, got {})",
            resp.status()
        );
    }
}

#[tokio::test]
async fn test_all_modes_have_ai_routes() {
    common::ensure_db().await;
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
    common::ensure_db().await;
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
    common::ensure_db().await;
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
