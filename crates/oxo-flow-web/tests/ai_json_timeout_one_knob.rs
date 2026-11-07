//! The documented `504 AI_TIMEOUT` contract must hold under the PRODUCTION
//! one-knob wiring (issue #1040 P3-5, review wave 2 HIGH-1).
//!
//! `OXO_FLOW_AI_TIMEOUT_SECS` configures both the provider client's
//! per-request timeout and the non-streaming routes' total budget. With the
//! two deadlines equal the inner provider error used to win the race and
//! `/api/chat/send/json` answered `400 CHAT_ERROR` (4/5 runs at 2 s, 3/3 at
//! 5 s), while only `/api/ai/translate` returned the documented 504.
//!
//! This binary pins both orderings:
//!
//! 1. EQUAL — one knob constructs the provider and bounds the route (the
//!    shipped configuration). Both routes must answer 504 `AI_TIMEOUT`.
//! 2. SMALL PROVIDER BUDGET — the provider timer (1 s) is strictly shorter
//!    than the route budget (knob 3 s), so the inner timeout deterministically
//!    wins; the handler must still classify it as 504 `AI_TIMEOUT`.
//!
//! Environment variables are process-global: this binary is deliberately a
//! single test that owns them. Inherited AI credentials are cleared before
//! any client is built, so a developer shell (or CI runner) with real keys
//! exported can never turn this test into a paid provider call.

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
/// request work starts.
fn set_test_env(key: &str, value: &str) {
    // SAFETY: called once, before any provider/HTTP client is built and
    // before the test spawns request work.
    unsafe { std::env::set_var(key, value) };
}

/// Remove an inherited AI credential/endpoint variable so the only provider
/// this test can reach is the local blackhole.
fn clear_inherited_ai_env() {
    for key in [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "DEEPSEEK_API_KEY",
        "OLLAMA_HOST",
        "OXO_FLOW_AI_PROVIDER",
        "OXO_FLOW_AI_API_KEY",
        "OXO_FLOW_AI_API_URL",
        "OXO_FLOW_AI_MODEL",
    ] {
        // SAFETY: same single-threaded setup window as `set_test_env`.
        unsafe { std::env::remove_var(key) };
    }
}

/// Accept TCP connections and never answer them — a silently stalled
/// provider (a local mock, never a paid endpoint).
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

fn install_blackhole_provider(addr: std::net::SocketAddr) {
    let provider = create_provider(
        ProviderKind::OpenAi,
        Some("sk-test-key".to_string()),
        Some(format!("http://{addr}/v1/chat/completions")),
        Some("gpt-test".to_string()),
    );
    oxo_flow_ai::AI.set_provider(provider);
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

/// Both non-streaming AI JSON routes must answer the documented timeout.
async fn assert_both_routes_time_out(scenario: &str) {
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

        assert_eq!(
            status,
            StatusCode::GATEWAY_TIMEOUT,
            "[{scenario}] {uri}: {error}"
        );
        assert_eq!(
            error["code"],
            json!("AI_TIMEOUT"),
            "[{scenario}] {uri}: {error}"
        );
        assert!(
            error["suggestion"]
                .as_str()
                .unwrap_or_default()
                .contains("OXO_FLOW_AI_TIMEOUT_SECS"),
            "[{scenario}] {uri} must name the knob: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "[{scenario}] {uri} took {elapsed:?}; the budget must bound the route"
        );
    }
}

#[tokio::test]
async fn one_knob_and_small_provider_budgets_both_answer_504() {
    clear_inherited_ai_env();
    let scratch = std::env::temp_dir().join(format!("oxo-flow-one-knob-{}", std::process::id()));
    let sessions = scratch.join("sessions");
    std::fs::create_dir_all(&sessions).expect("scratch session dir");
    set_test_env("HOME", &scratch.to_string_lossy());
    set_test_env("OXO_FLOW_AI_SESSION_DIR", &sessions.to_string_lossy());

    let addr = blackhole_listener();

    // Scenario 1 — EQUAL budgets: the same knob builds the provider client
    // (2 s per request) and bounds the route (2 s minus the ordering margin).
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "2");
    install_blackhole_provider(addr);
    assert_both_routes_time_out("equal budgets").await;

    // Scenario 2 — SMALL provider budget: the provider client is built at
    // 1 s, then the knob is raised to 3 s (route budget 2 s). The provider
    // timer wins deterministically; the handler must still answer 504.
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "1");
    install_blackhole_provider(addr);
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "3");
    assert_both_routes_time_out("small provider budget").await;
}
