//! A stalled AI provider must surface an SSE `error` within the configured
//! budget — and leave a failure archive behind (issue #1040 B1).
//!
//! The browser test saw a generation sit on "planning" for 14+ minutes with
//! no error and no retry entry, while the server had quietly finished and
//! written an archive that was later pruned away. This test pins both
//! halves of the server contract that the client relies on:
//!
//! - an unresponsive endpoint (TCP accepted, never answered) produces a
//!   terminal `error` event within `OXO_FLOW_AI_TIMEOUT_SECS`, and
//! - the aborted loop persists a `Failed` session record (the post-mortem
//!   artifact) instead of leaving nothing behind.
//!
//! Environment variables are process-global, so this scenario lives in its
//! own test binary and sets them before any provider is constructed.

use std::time::Duration;

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
/// thread reads them (the requirement is documented at the call site).
fn set_test_env(key: &str, value: &str) {
    // SAFETY: called once, before any provider/HTTP client is built and
    // before the test spawns tokio work.
    unsafe { std::env::set_var(key, value) };
}

/// Accept TCP connections and never answer them — a silent proxy.
fn blackhole_listener() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blackhole");
    let addr = listener.local_addr().expect("blackhole addr");
    std::thread::spawn(move || {
        // Keep every accepted socket open for the life of the process: dropping
        // it would send FIN and turn the stall into a fast EOF error.
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

#[tokio::test]
async fn stalled_provider_surfaces_an_error_and_keeps_a_failure_archive() {
    let archive_dir = tempfile::tempdir().expect("archive tempdir");
    set_test_env(
        "OXO_FLOW_AI_SESSION_DIR",
        &archive_dir.path().to_string_lossy(),
    );
    // One second is the documented per-request budget knob; the test proves
    // the chat surface honors it instead of hanging on the default 300 s.
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "1");

    let addr = blackhole_listener();
    let provider = create_provider(
        ProviderKind::OpenAi,
        Some("sk-test-key".to_string()),
        Some(format!("http://{addr}/v1/chat/completions")),
        Some("gpt-test".to_string()),
    );
    oxo_flow_ai::AI.set_provider(provider);

    let request = server::build_router("personal").oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/chat/send")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"message": "generate an RNA-seq pipeline"}).to_string(),
            ))
            .unwrap(),
    );

    // The whole exchange must finish well inside the provider budget plus
    // orchestration overhead — a hang here is the reported bug.
    let response = tokio::time::timeout(Duration::from_secs(15), request)
        .await
        .expect("the stalled stream must terminate within the configured budget")
        .expect("router must answer");
    assert_eq!(response.status(), StatusCode::OK);

    let started = std::time::Instant::now();
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024),
    )
    .await
    .expect("reading the SSE body must not hang")
    .expect("body collect");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the error must arrive shortly after the 1 s provider budget, took {:?}",
        started.elapsed()
    );

    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(text.contains("event: ready"), "missing handshake: {text}");
    assert!(
        text.contains("event: error"),
        "a stalled provider must produce an error event, got: {text}"
    );
    assert!(
        text.to_lowercase().contains("timed out"),
        "the error must name the timeout, got: {text}"
    );

    // The aborted loop persists its transcript with outcome=error — the
    // artifact the browser test found missing.
    let mut archives = std::fs::read_dir(archive_dir.path())
        .expect("archive dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    archives.sort();
    assert_eq!(
        archives.len(),
        1,
        "exactly one failure archive expected, got {archives:?}"
    );
    let raw = std::fs::read_to_string(&archives[0]).expect("read archive");
    let saved: serde_json::Value = serde_json::from_str(&raw).expect("archive must be valid JSON");
    assert_eq!(saved["command"], json!("web-chat"), "archive: {saved}");
    assert_eq!(saved["outcome"], json!("Failed"), "archive: {saved}");
    assert!(
        saved["error"]
            .as_str()
            .is_some_and(|error| error.to_lowercase().contains("timed out")),
        "the archive must record the abort reason: {saved}"
    );
}
