//! OAuth state binding and redirect allowlist (issue #1141, audit WEB-16).
//!
//! A `state` is issued bound to `(provider, redirect URI)`, valid for ten
//! minutes, and single-use; the post-callback redirect target must have an
//! origin on the server-side allowlist derived from the origin config the
//! server already trusts.
//!
//! This file is its own test binary because every test mutates
//! process-global environment variables; the harness runs tests in parallel
//! threads, so [`ENV_LOCK`] serializes them.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::domains::auth::service;
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

/// The env-configured redirect URI: its origin is always allowlisted.
const REDIRECT: &str = "https://flows.example.com/oxo-flow/api/auth/oauth/callback";

/// A different origin that is also allowlisted by default (localhost:5173),
/// so a redirect-mismatch test cannot pass because of the allowlist.
const OTHER_ALLOWED_ORIGIN_REDIRECT: &str = "http://localhost:5173/api/auth/oauth/callback";

/// Serializes the env-mutating tests in this binary.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Deterministic OAuth environment for one test. Hold the returned guard for
/// the whole test body.
async fn oauth_env() -> tokio::sync::MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().await;
    // SAFETY: ENV_LOCK serializes every test in this binary that mutates or
    // reads these variables, so nothing observes a partial mutation.
    unsafe {
        std::env::set_var("ORCID_CLIENT_ID", "mock-client");
        std::env::set_var("ORCID_CLIENT_SECRET", "mock-secret");
        std::env::set_var("OXO_FLOW_OAUTH_REDIRECT_URI", REDIRECT);
        std::env::remove_var("OXO_FLOW_ALLOWED_ORIGINS");
        std::env::remove_var("OXO_FLOW_PUBLIC_URL");
        std::env::remove_var("ORCID_SANDBOX");
    }
    guard
}

async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
    let response = server::build_router("team")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(json!(null)),
    )
}

/// Issue a state through the real authorize endpoint and return it.
async fn issue_state(provider: &str) -> String {
    let (status, body) =
        post_json("/api/auth/oauth/authorize", json!({"provider": provider})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body["state"].as_str().expect("state").to_string()
}

#[tokio::test]
async fn state_issued_for_one_provider_is_rejected_on_another_providers_callback() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let state = issue_state("orcid").await;
    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({"provider": "github", "code": "x", "state": state}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_STATE_MISMATCH", "{body:?}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("provider"),
        "the provider mismatch must be reported: {body:?}"
    );
}

#[tokio::test]
async fn state_issued_with_one_redirect_is_rejected_when_callback_presents_another() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let state = issue_state("orcid").await;
    // Both targets are allowlisted; only the binding differs.
    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({
            "provider": "orcid",
            "code": "x",
            "state": state,
            "redirect_uri": OTHER_ALLOWED_ORIGIN_REDIRECT,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_STATE_MISMATCH", "{body:?}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("redirect"),
        "the redirect mismatch must be reported: {body:?}"
    );
}

#[tokio::test]
async fn replayed_state_is_rejected_on_the_callback() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    // Bind an unknown provider so the exchange fails offline: the first
    // callback proves the state gate passes, the second proves it is spent.
    let redirect = service::oauth_redirect_uri(None);
    service::store_pending_state("state-replayed-1141", "attacker", &redirect)
        .await
        .expect("store state");

    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-replayed-1141"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_CALLBACK_ERROR", "{body:?}");

    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-replayed-1141"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_INVALID_STATE", "{body:?}");
}

#[tokio::test]
async fn expired_state_is_rejected_even_when_bindings_match() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let backend = oxo_flow_web::infra::db::global_backend().expect("storage backend");
    let stale = (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
    let redirect = service::oauth_redirect_uri(None);
    backend
        .store_oauth_state("state-expired-1141", "attacker", &redirect, &stale)
        .await
        .expect("store stale state");

    let (status, body) = post_json(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-expired-1141"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_INVALID_STATE", "{body:?}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("expired"),
        "a stale state must be reported as expired: {body:?}"
    );
}

#[tokio::test]
async fn off_allowlist_redirect_target_is_rejected_with_400() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    for target in [
        "https://attacker.example/cb",
        // Userinfo must not smuggle the trusted origin into the authority.
        "https://flows.example.com@attacker.example/cb",
        // Non-http(s) schemes are never OAuth callback targets.
        "ftp://flows.example.com/cb",
    ] {
        let (status, body) = post_json(
            "/api/auth/oauth/authorize",
            json!({"provider": "orcid", "redirect_uri": target}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{target}: {body:?}");
        assert_eq!(
            body["code"], "OAUTH_REDIRECT_NOT_ALLOWED",
            "{target}: {body:?}"
        );
    }

    // The default (localhost) target and the env-configured target remain
    // usable: the allowlist constrains off-origin targets only.
    let (status, body) = post_json("/api/auth/oauth/authorize", json!({"provider": "orcid"})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

#[tokio::test]
async fn allowlisted_origins_from_env_and_public_url_are_accepted() {
    let _guard = oauth_env().await;
    common::ensure_db().await;
    // SAFETY: oauth_env holds ENV_LOCK for this whole test.
    unsafe {
        std::env::set_var("OXO_FLOW_ALLOWED_ORIGINS", "https://extra.example.com");
        std::env::set_var("OXO_FLOW_PUBLIC_URL", "https://public.example.com/oxo-flow");
    }

    for target in [
        "https://extra.example.com/cb",
        "https://public.example.com/oxo-flow/api/auth/oauth/callback",
    ] {
        let (status, body) = post_json(
            "/api/auth/oauth/authorize",
            json!({"provider": "orcid", "redirect_uri": target}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{target}: {body:?}");
    }
}
