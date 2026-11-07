//! Non-streaming AI JSON routes must answer a structured `504 AI_TIMEOUT`
//! within the total budget instead of hanging behind a reverse proxy
//! (issue #1040 P3-5).
//!
//! The blackhole endpoint accepts TCP and never answers. The provider client
//! is constructed while `OXO_FLOW_AI_TIMEOUT_SECS` is 10 s, so its
//! per-request timeout cannot fire first; the knob is then lowered to 1 s,
//! which is the TOTAL budget the JSON route reads per request. Both
//! `/api/chat/send/json` and `/api/ai/translate` must short-circuit with the
//! shared `{code,message,detail,suggestion}` error, not block.
//!
//! Environment variables are process-global, so this scenario lives in its
//! own test binary.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::provider::{ProviderKind, create_provider};
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

/// Set a process environment variable for this test binary.
///
/// `std::env::set_var` is unsafe in edition 2024 because it races concurrent
/// readers; this binary has a single test that sets its variables before any
/// request handler reads them (the provider client is built first, under the
/// larger value).
fn set_test_env(key: &str, value: &str) {
    // SAFETY: called once, before any provider/HTTP client is built and
    // before the test spawns request work.
    unsafe { std::env::set_var(key, value) };
}

/// Accept TCP connections and never answer them — a silent proxy.
fn blackhole_listener() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blackhole");
    let addr = listener.local_addr().expect("blackhole addr");
    std::thread::spawn(move || {
        // Keep every accepted socket open for the life of the process:
        // dropping it would send FIN and turn the stall into a fast EOF.
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
    });
    addr
}

/// POST a JSON body through the app router, returning status, parsed body,
/// and wall time. The 5 s watchdog turns a regression (indefinite hang) into
/// a clear test failure instead of a stuck suite.
async fn post_json(
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Duration) {
    let started = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        server::build_router("personal").oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        ),
    )
    .await
    .expect("the JSON route must answer instead of hanging")
    .expect("router must answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("error body");
    let parsed = serde_json::from_slice(&bytes).expect("structured error body");
    (status, parsed, started.elapsed())
}

#[tokio::test]
async fn slow_provider_json_routes_return_a_structured_timeout() {
    // Provider client first: 10 s per-request budget.
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "10");
    let addr = blackhole_listener();
    let provider = create_provider(
        ProviderKind::OpenAi,
        Some("sk-test-key".to_string()),
        Some(format!("http://{addr}/v1/chat/completions")),
        Some("gpt-test".to_string()),
    );
    oxo_flow_ai::AI.set_provider(provider);

    // The route-level TOTAL budget reads the knob per request: 1 s.
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "1");

    for (uri, body) in [
        (
            "/api/chat/send/json",
            json!({ "message": "generate an RNA-seq pipeline" }),
        ),
        (
            "/api/ai/translate",
            json!({ "intent": "generate an RNA-seq pipeline" }),
        ),
    ] {
        let (status, error, elapsed) = post_json(uri, body).await;

        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{uri}: {error}");
        assert_eq!(error["code"], json!("AI_TIMEOUT"), "{uri}: {error}");
        assert!(
            error["message"]
                .as_str()
                .unwrap_or_default()
                .contains("1-second"),
            "{uri} must report the expired budget: {error}"
        );
        assert!(
            error["suggestion"]
                .as_str()
                .unwrap_or_default()
                .contains("OXO_FLOW_AI_TIMEOUT_SECS"),
            "{uri} must name the knob: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "{uri} took {elapsed:?}; the 1 s budget must bound the route"
        );
    }
}
