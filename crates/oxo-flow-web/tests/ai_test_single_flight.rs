//! `/api/ai/test` must never fan out into a probe storm (issue #1040 B3).
//!
//! A settings page left open — or a scripted poller — used to fire several
//! concurrent provider round-trips per second, tripping the provider's rate
//! limiter and filling the session archive with stub errors. The handler is
//! now single-flight: concurrent callers share one outbound call, and a
//! short TTL reuses the result.
//!
//! The AI registry is process-wide, so this scenario lives in its own test
//! binary (a parallel test installing a different provider would race).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::scripted::{ScriptedBackend, ScriptedTurn};
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

fn install_scripted_provider_with(turns: Vec<ScriptedTurn>) -> ScriptedBackend {
    let backend = ScriptedBackend::new(turns);
    oxo_flow_ai::AI.set_provider(AiProvider::Scripted(backend.clone()));
    backend
}

async fn post_ai_test() -> (StatusCode, serde_json::Value) {
    let response = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/test")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn concurrent_probes_share_one_provider_call() {
    // Probes must never write session archives: point the archive at a
    // scratch dir and assert it stays empty (issue #1040 B3).
    let archive_dir = tempfile::tempdir().expect("archive tempdir");
    // SAFETY: this binary has one test, so the variable is set before any
    // provider call and no concurrent reader can race it.
    unsafe {
        std::env::set_var(
            "OXO_FLOW_AI_SESSION_DIR",
            archive_dir.path().to_string_lossy().as_ref(),
        );
    }
    // One scripted turn: a second outbound call would find the script
    // exhausted and fail with "script exhausted", so a storm is visible as
    // both a provider-call count > 1 and a failed response.
    let backend = install_scripted_provider_with(vec![ScriptedTurn::content("OK")]);

    let mut handles = Vec::new();
    for _ in 0..6 {
        handles.push(tokio::spawn(post_ai_test()));
    }
    for handle in handles {
        let (status, body) = handle.await.expect("probe task must not panic");
        assert_eq!(status, StatusCode::OK, "probe failed: {body}");
        assert_eq!(body["success"], json!(true), "probe body: {body}");
        assert_eq!(body["provider"], json!("scripted"), "probe body: {body}");
    }

    let calls = backend.observed_calls().await;
    assert_eq!(
        calls.len(),
        1,
        "six concurrent /api/ai/test calls must collapse into one provider \
         round-trip (single-flight), observed: {}",
        calls.len()
    );

    // A follow-up inside the TTL is served from the cache — still one call.
    let (status, body) = post_ai_test().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(
        backend.observed_calls().await.len(),
        1,
        "a probe within the TTL window must reuse the cached result"
    );

    let archives = std::fs::read_dir(archive_dir.path())
        .expect("archive dir")
        .count();
    assert_eq!(
        archives, 0,
        "connectivity probes must not write ai_sessions archives"
    );
}
