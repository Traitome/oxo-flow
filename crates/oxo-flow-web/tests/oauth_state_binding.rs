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
use axum::http::{HeaderMap, Request, StatusCode, header};
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

/// POST with a JSON body, returning the status, response headers, and body.
/// `cookie` is sent as the request `Cookie` header, modeling the browser
/// context the callback arrives from.
async fn post_json_full(
    uri: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = server::build_router("team")
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("router response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(json!(null)),
    )
}

async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
    let (status, _headers, body) = post_json_full(uri, body, None).await;
    (status, body)
}

/// All `Set-Cookie` values on a response.
fn set_cookies(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect()
}

/// The browser-binding cookie value issued by an authorize response.
fn binding_cookie(headers: &HeaderMap) -> String {
    set_cookies(headers)
        .iter()
        .find_map(|cookie| {
            cookie
                .strip_prefix("oxoflow_oauth_binding=")
                .and_then(|rest| rest.split(';').next())
                .map(str::to_string)
        })
        .expect("authorize must issue the binding cookie")
}

/// Issue a state through the real authorize endpoint. Returns the state and
/// the binding cookie value the same browser context now holds.
async fn issue_state(provider: &str) -> (String, String) {
    let (status, headers, body) = post_json_full(
        "/api/auth/oauth/authorize",
        json!({"provider": provider}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let cookie = binding_cookie(&headers);
    (body["state"].as_str().expect("state").to_string(), cookie)
}

#[tokio::test]
async fn state_issued_for_one_provider_is_rejected_on_another_providers_callback() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let (state, cookie) = issue_state("orcid").await;
    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "github", "code": "x", "state": state}),
        Some(&format!("oxoflow_oauth_binding={cookie}")),
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

    let (state, cookie) = issue_state("orcid").await;
    // Both targets are allowlisted; only the binding differs.
    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({
            "provider": "orcid",
            "code": "x",
            "state": state,
            "redirect_uri": OTHER_ALLOWED_ORIGIN_REDIRECT,
        }),
        Some(&format!("oxoflow_oauth_binding={cookie}")),
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
    service::store_pending_state("state-replayed-1141", "attacker", &redirect, "browser-a")
        .await
        .expect("store state");
    let cookie = "oxoflow_oauth_binding=browser-a";

    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-replayed-1141"}),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_CALLBACK_ERROR", "{body:?}");

    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-replayed-1141"}),
        Some(cookie),
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
        .store_oauth_state(
            "state-expired-1141",
            "attacker",
            &redirect,
            &service::hash_binding("browser-a"),
            &stale,
        )
        .await
        .expect("store stale state");

    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-expired-1141"}),
        Some("oxoflow_oauth_binding=browser-a"),
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

/// Issue #1197: the state carries the browser binding — a callback from a
/// different browser context (no cookie, or another browser's cookie) is
/// rejected with the same structured mismatch, without leaking which part
/// failed, and the state is consumed so the real browser cannot retry it.
#[tokio::test]
async fn state_issued_in_one_browser_is_rejected_from_another() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let (state, cookie) = issue_state("orcid").await;

    // No binding cookie at all (the callback arrived in another browser).
    let (status, headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "orcid", "code": "x", "state": state}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_STATE_MISMATCH", "{body:?}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("provider") && !message.contains("redirect"),
        "the binding rejection must not leak which part failed: {body:?}"
    );
    // The rejection clears the (absent) cookie too, so a stale value from a
    // previous flow cannot linger.
    assert!(
        set_cookies(&headers)
            .iter()
            .any(|c| c.starts_with("oxoflow_oauth_binding=;") && c.contains("Max-Age=0")),
        "the callback must clear the binding cookie: {headers:?}"
    );

    // The mismatch consumed the state: the initiating browser can no longer
    // complete it either.
    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "orcid", "code": "x", "state": state}),
        Some(&format!("oxoflow_oauth_binding={cookie}")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(
        body["code"], "OAUTH_INVALID_STATE",
        "a cross-browser callback must consume the state: {body:?}"
    );

    // Another browser's cookie value is just as invalid.
    let (state, _cookie) = issue_state("orcid").await;
    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "orcid", "code": "x", "state": state}),
        Some("oxoflow_oauth_binding=some-other-browser"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "OAUTH_STATE_MISMATCH", "{body:?}");
}

/// A row written before the browser-binding migration (empty digest) must
/// never verify, exactly like a state from another browser.
#[tokio::test]
async fn legacy_state_row_without_a_binding_is_rejected() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let backend = oxo_flow_web::infra::db::global_backend().expect("storage backend");
    let redirect = service::oauth_redirect_uri(None);
    backend
        .store_oauth_state(
            "state-legacy-1197",
            "attacker",
            &redirect,
            "",
            &chrono::Utc::now().to_rfc3339(),
        )
        .await
        .expect("store legacy state");

    let (status, _headers, body) = post_json_full(
        "/api/auth/oauth/callback",
        json!({"provider": "attacker", "code": "x", "state": "state-legacy-1197"}),
        Some("oxoflow_oauth_binding=browser-a"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(
        body["code"], "OAUTH_STATE_MISMATCH",
        "a legacy row with no binding must fail closed: {body:?}"
    );
}

/// The authorize cookie is HttpOnly, SameSite=Lax, scoped to the OAuth
/// routes, bounded to the 10-minute state TTL, and never exposes the
/// binding in the response body.
#[tokio::test]
async fn authorize_issues_a_scoped_httponly_binding_cookie() {
    let _guard = oauth_env().await;
    common::ensure_db().await;

    let (status, headers, body) = post_json_full(
        "/api/auth/oauth/authorize",
        json!({"provider": "orcid"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let cookie = set_cookies(&headers)
        .into_iter()
        .find(|c| c.starts_with("oxoflow_oauth_binding="))
        .expect("authorize must set the binding cookie");
    assert!(cookie.contains("; Path=/api/auth/oauth"), "{cookie}");
    assert!(cookie.contains("; Max-Age=600"), "{cookie}");
    assert!(cookie.contains("; HttpOnly"), "{cookie}");
    assert!(cookie.contains("; SameSite=Lax"), "{cookie}");
    // The env-configured redirect is https, so the cookie is Secure.
    assert!(cookie.contains("; Secure"), "{cookie}");
    // The secret rides only the cookie.
    assert!(body.get("binding").is_none(), "{body:?}");
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
