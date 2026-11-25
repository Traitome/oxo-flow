//! `OXO_FLOW_AI_FALLBACK=disabled` must limit web translate to ONE provider
//! (campaign Task 8: the live incident silently fell back from the configured
//! OpenAI provider to an ambient Claude credential).
//!
//! Process-global environment + the process-wide AI registry → one test owns
//! them in its own binary. The scripted backend fails immediately with an
//! auth-class error, so the only way the observed call count can exceed one
//! is a fallback attempt.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::scripted::{ScriptedBackend, ScriptedTurn};
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

fn set_test_env(key: &str, value: &str) {
    // SAFETY: this binary has one test; variables are set before any request
    // handler or provider client reads them.
    unsafe { std::env::set_var(key, value) };
}

/// Remove inherited AI credentials so only the scripted provider can run.
fn clear_inherited_ai_env() {
    for key in [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_API_KEY",
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
        "OXO_FLOW_AI_MAX_TOKENS",
        "OXO_FLOW_AI_CONTEXT_TOKENS",
        "OXO_FLOW_AI_FALLBACK",
    ] {
        // SAFETY: same single-threaded setup window as `set_test_env`.
        unsafe { std::env::remove_var(key) };
    }
}

async fn post_translate(intent: &str) -> (StatusCode, serde_json::Value) {
    let response = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/translate")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "intent": intent }).to_string()))
                .unwrap(),
        )
        .await
        .expect("router must answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn disabled_fallback_attempts_only_the_first_provider() {
    clear_inherited_ai_env();
    let scratch = tempfile::tempdir().expect("scratch session dir");
    set_test_env("OXO_FLOW_AI_SESSION_DIR", &scratch.path().to_string_lossy());
    set_test_env("OXO_FLOW_AI_FALLBACK", "disabled");

    // One scripted error turn; a fallback attempt would find the script
    // exhausted and be observable as a second call.
    let backend = ScriptedBackend::new(vec![ScriptedTurn::error("auth:fallback-test")]);
    oxo_flow_ai::AI.set_provider(AiProvider::Scripted(backend.clone()));

    let (status, body) = post_translate("fallback-disabled-sentinel").await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], json!("AI_TRANSLATE_ERROR"), "body: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("scripted"),
        "the structured failure must name the provider: {message}"
    );
    assert!(
        message.contains("authentication failed"),
        "the structured failure must keep the sanitized cause: {message}"
    );
    assert!(
        !message.contains("fallback-test"),
        "raw upstream error text must not leak into the message: {message}"
    );

    assert_eq!(
        backend.observed_calls().await.len(),
        1,
        "OXO_FLOW_AI_FALLBACK=disabled must not attempt another provider"
    );
}
