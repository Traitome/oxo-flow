//! OAuth authorize URL must carry the same redirect URI the callback will
//! use for the token exchange.
//!
//! Documented contract (`docs/guide/src/reference/web-api.md`): the
//! redirect URI is `OXO_FLOW_OAUTH_REDIRECT_URI` when set, with a fixed
//! fallback. Providers reject the code exchange when the two differ, so the
//! authorize endpoint must not ignore the env var.
//!
//! This file is its own test binary with a single test because it mutates
//! process-global environment variables.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const REDIRECT_URI: &str = "https://flows.example.com/oxo-flow/api/auth/oauth/callback";

async fn post_authorize(body: Value) -> (StatusCode, HeaderMap, Value) {
    let response = server::build_router("team")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/oauth/authorize")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
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

/// The browser-binding value the authorize response set as a cookie.
fn binding_cookie(headers: &HeaderMap) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| {
            cookie
                .strip_prefix("oxoflow_oauth_binding=")
                .and_then(|rest| rest.split(';').next())
                .map(str::to_string)
        })
        .expect("authorize must issue the browser-binding cookie")
}

#[tokio::test]
async fn authorize_honors_the_env_redirect_uri() {
    common::ensure_db().await;
    // SAFETY: this test binary contains a single test, so no other thread
    // reads the process environment while it is mutated. The variables are
    // removed again at the end.
    unsafe {
        std::env::set_var("ORCID_CLIENT_ID", "mock-client");
        std::env::set_var("ORCID_CLIENT_SECRET", "mock-secret");
        std::env::set_var("OXO_FLOW_OAUTH_REDIRECT_URI", REDIRECT_URI);
        // The explicit-override half below targets this origin; it must be
        // on the server-side OAuth redirect allowlist (#1141).
        std::env::set_var("OXO_FLOW_ALLOWED_ORIGINS", "https://other.example.com");
        // Pin the production host so the assertion is deterministic.
        std::env::remove_var("ORCID_SANDBOX");
    }

    // No explicit redirect_uri: the env-configured value must be used, not
    // the fixed localhost fallback.
    let (status, headers, body) = post_authorize(json!({"provider": "orcid"})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let authorize_url = body["authorize_url"].as_str().expect("authorize_url");
    assert!(
        authorize_url.starts_with("https://orcid.org/oauth/authorize?"),
        "{authorize_url}"
    );
    assert!(
        authorize_url.contains(&format!(
            "redirect_uri={}",
            urlencoding::encode(REDIRECT_URI)
        )),
        "authorize URL must carry the env redirect URI: {authorize_url}"
    );
    assert!(
        !authorize_url.contains("localhost%3A3000"),
        "the fixed fallback must not shadow the env override: {authorize_url}"
    );

    // The minted CSRF state was persisted server-side, bound to this
    // provider + redirect URI + browser cookie, and is single-use.
    let state = body["state"].as_str().expect("state");
    let binding = binding_cookie(&headers);
    oxo_flow_web::domains::auth::service::verify_and_consume_state(
        state,
        "orcid",
        REDIRECT_URI,
        Some(&binding),
    )
    .await
    .expect("issued state verifies once");

    // An explicit request value still wins over the env default — provided
    // its origin is on the allowlist (`OXO_FLOW_ALLOWED_ORIGINS` above).
    let explicit = "https://other.example.com/cb";
    let (status, headers, body) = post_authorize(json!({
        "provider": "orcid",
        "redirect_uri": explicit,
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let authorize_url = body["authorize_url"].as_str().expect("authorize_url");
    assert!(
        authorize_url.contains(&format!("redirect_uri={}", urlencoding::encode(explicit))),
        "{authorize_url}"
    );
    let state = body["state"].as_str().expect("state");
    let binding = binding_cookie(&headers);
    oxo_flow_web::domains::auth::service::verify_and_consume_state(
        state,
        "orcid",
        explicit,
        Some(&binding),
    )
    .await
    .expect("the state is bound to the explicit redirect URI");

    // SAFETY: same single-threaded mutation as above.
    unsafe {
        std::env::remove_var("OXO_FLOW_OAUTH_REDIRECT_URI");
        std::env::remove_var("OXO_FLOW_ALLOWED_ORIGINS");
        std::env::remove_var("ORCID_CLIENT_ID");
        std::env::remove_var("ORCID_CLIENT_SECRET");
    }
}
