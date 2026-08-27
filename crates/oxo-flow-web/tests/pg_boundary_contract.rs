//! Issue #207 phase-1 contract test.
//!
//! On a deployment where the embedded SQLite pool is never initialized
//! (PostgreSQL mode), EVERY `/api/runs*` endpoint must answer the same
//! structured 503 envelope, and non-run routes must stay outside the gate.
//! This process never calls `init_pool`, so `try_pool()` is absent — the
//! exact condition the router-layer gate keys on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server::build_router;
use tower::ServiceExt;

/// The full `/api/runs*` route contract (mirrors the `run_routes` table in
/// `server.rs`; update both together).
const RUN_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/runs"),
    ("GET", "/api/runs"),
    ("GET", "/api/runs/r1"),
    ("GET", "/api/runs/r1/status"),
    ("GET", "/api/runs/r1/dag-status"),
    ("GET", "/api/runs/r1/diagnostics"),
    ("GET", "/api/runs/r1/instances"),
    ("POST", "/api/runs/r1/clean"),
    ("POST", "/api/runs/r1/resume-checkpoint"),
    ("GET", "/api/runs/r1/logs"),
    ("GET", "/api/runs/r1/results"),
    ("POST", "/api/runs/r1/retry"),
    ("POST", "/api/runs/r1/cancel"),
    ("POST", "/api/runs/r1/pause"),
    ("POST", "/api/runs/r1/resume"),
    ("GET", "/api/runs/r1/preview"),
    ("GET", "/api/runs/r1/ai-status"),
    ("GET", "/api/runs/r1/report"),
    ("POST", "/api/runs/r1/report/ask"),
    ("POST", "/api/runs/r1/report/visualize"),
    ("GET", "/api/runs/r1/files"),
];

#[tokio::test]
async fn every_runs_route_answers_the_structured_sqlite_boundary() {
    let app = build_router("personal");
    for (method, path) in RUN_ROUTES {
        let req = Request::builder()
            .method(*method)
            .uri(*path)
            .body(Body::empty())
            .expect("request builds");
        let resp = app.clone().oneshot(req).await.expect("infallible service");
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path} must answer the structured runs boundary"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!("{method} {path}: envelope is not the ApiError JSON: {e} — {bytes:?}")
        });
        assert_eq!(
            envelope["code"], "RUNS_REQUIRE_SQLITE",
            "{method} {path}: wrong code in {envelope}"
        );
        assert!(
            envelope["message"].is_string() && envelope["suggestion"].is_string(),
            "{method} {path}: envelope must carry message + suggestion, got {envelope}"
        );
    }
}

#[tokio::test]
async fn non_run_routes_are_not_gated() {
    let app = build_router("personal");
    let req = Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(Body::empty())
        .expect("request builds");
    let resp = app.oneshot(req).await.expect("infallible service");
    assert_ne!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "health must stay outside the runs gate"
    );
}
