//! A JSON-route budget expiry must leave a failure archive behind.
//!
//! The documented contract is "every agent-loop run — successful or aborted
//! — is persisted". The orchestrator archives its own error paths, but when
//! the ROUTE budget fires first the orchestrator future is dropped mid-flight
//! and never reaches them, so the 504 path used to leave zero archives
//! (review wave 3 LOW-3). The handlers now write a synthetic `Failed` session
//! carrying the timeout reason, matching the SSE/abort behavior.
//!
//! Environment variables are process-global, so this binary is deliberately
//! a single test that owns them. Inherited AI credentials are cleared before
//! any client is built, so a developer shell with real keys exported can
//! never turn this test into a paid provider call.

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

/// POST a JSON body through the app router and return the status + body.
/// The 5 s watchdog turns a regression (indefinite hang) into a clear test
/// failure instead of a stuck suite.
async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
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
        .expect("body");
    let parsed = serde_json::from_slice(&bytes).expect("structured JSON body");
    (status, parsed)
}

/// All session archives currently on disk, sorted by path.
fn archives(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("archive dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
}

#[tokio::test]
async fn route_budget_expiry_archives_a_failed_run_on_both_json_routes() {
    clear_inherited_ai_env();
    let scratch =
        std::env::temp_dir().join(format!("oxo-flow-timeout-archive-{}", std::process::id()));
    // A reused PID must not inherit a previous run's archives.
    let _ = std::fs::remove_dir_all(&scratch);
    let sessions = scratch.join("sessions");
    std::fs::create_dir_all(&sessions).expect("scratch session dir");
    set_test_env("HOME", &scratch.to_string_lossy());
    set_test_env("OXO_FLOW_AI_SESSION_DIR", &sessions.to_string_lossy());

    // Provider timeout 2 s, route budget 1 s: the route timer wins, so both
    // requests answer 504 through the `with_budget` expiry branch — the path
    // that used to drop the orchestrator future without archiving anything.
    set_test_env("OXO_FLOW_AI_TIMEOUT_SECS", "2");
    let addr = blackhole_listener();
    install_blackhole_provider(addr);

    for (uri, body) in [
        (
            "/api/chat/send/json",
            json!({ "message": "generate an RNA-seq pipeline" }),
        ),
        (
            "/api/ai/translate",
            json!({ "intent": "generate a variant-calling pipeline" }),
        ),
    ] {
        let (status, error) = post_json(uri, body).await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{uri}: {error}");
        assert_eq!(error["code"], json!("AI_TIMEOUT"), "{uri}: {error}");
    }

    let found = archives(&sessions);
    assert_eq!(
        found.len(),
        2,
        "each aborted route must archive one failure, got {found:?}"
    );

    let mut commands = Vec::new();
    for path in &found {
        let raw = std::fs::read_to_string(path).expect("read archive");
        let saved: serde_json::Value =
            serde_json::from_str(&raw).expect("archive must be valid JSON");
        assert_eq!(saved["outcome"], json!("Failed"), "{path:?}: {saved}");
        assert!(saved["ended_at"].is_string(), "{path:?}: {saved}");
        let reason = saved["error"].as_str().unwrap_or_default();
        assert!(
            reason.contains("budget"),
            "the archive must record the timeout reason: {path:?}: {saved}"
        );
        commands.push(saved["command"].as_str().unwrap_or_default().to_string());
    }
    commands.sort();
    assert_eq!(
        commands,
        vec!["web-chat", "web-translate"],
        "one archive per JSON generation surface: {found:?}"
    );
}
