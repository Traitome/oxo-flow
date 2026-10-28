//! Mount-root security-header parity (issue #957).
//!
//! Under `--base-path /prefix` the trailing-slash mount root
//! (`GET /prefix/`) used to be registered on the OUTER router, outside
//! `build_router`'s middleware chain — the one page a sub-path
//! deployment's canonical URL points at was served with zero security
//! headers while everything nested under the mount carried them. The
//! shared `mount_with_base_path` helper now routes the mount root through
//! the same `security_headers` middleware; this file pins the parity.
//!
//! This lives in its own test binary because `set_base_path` is a
//! process-global first-call-wins static and the csp_gate binary pins the
//! root-mount contract (`""`) — a sub-path value must never be set in the
//! same process as those tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use tower::ServiceExt;

/// Mirrors the pin in `csp_gate.rs` — the policy is defined once in
/// `security_headers` and drift-checked there; here it only has to match
/// between the mount root and the nested app.
fn csp_of(resp: &axum::http::Response<Body>) -> String {
    resp.headers()
        .get("content-security-policy")
        .expect("every served response must carry a CSP header")
        .to_str()
        .unwrap()
        .to_string()
}

async fn get(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn mount_root_serves_security_headers_like_nested_routes() {
    server::set_running_mode("personal");
    server::set_base_path("/oxoflow");
    let app = server::mount_with_base_path(server::build_router("personal"), "/oxoflow");

    // The mount root — the route that used to bypass the middleware chain.
    let root = get(app.clone(), "/oxoflow/").await;
    assert_eq!(root.status(), StatusCode::OK);
    let root_csp = csp_of(&root);
    assert!(
        !root_csp.is_empty(),
        "GET /oxoflow/ must carry a CSP header (issue #957)"
    );

    // The reference point inside the nest: same policy string.
    let health = get(app.clone(), "/oxoflow/api/health").await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(
        root_csp,
        csp_of(&health),
        "mount root CSP must equal the nested app's CSP (issue #957)"
    );

    // The rest of the baseline travels with it (permissions-policy kept as a
    // one-off value check — the CSP equality above already pins the strongest).
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        assert_eq!(
            root.headers().get(name).expect(name).to_str().unwrap(),
            value,
            "mount root must carry {name} like every nested response"
        );
    }

    // The no-trailing-slash form (served by nest itself) keeps its headers
    // too — guard against the fix accidentally routing only one of the two.
    let no_slash = get(app, "/oxoflow").await;
    assert_eq!(no_slash.status(), StatusCode::OK);
    assert_eq!(csp_of(&no_slash), root_csp);
}

#[tokio::test]
async fn mount_root_serves_spa_index_with_injected_base_tags() {
    server::set_running_mode("personal");
    server::set_base_path("/oxoflow");
    let app = server::mount_with_base_path(server::build_router("personal"), "/oxoflow");

    let resp = get(app, "/oxoflow/").await;
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("read mount-root body");
    let html = String::from_utf8_lossy(&body);
    // The served page may be the "SPA not built" fallback (CI never builds
    // the frontend), but both variants carry the injected tags.
    assert!(
        html.contains("<base href=\"/oxoflow/\">"),
        "mount-root SPA must inject the sub-path <base> tag"
    );
    assert!(
        html.contains("<meta name=\"oxo-base\" content=\"/oxoflow\">"),
        "mount-root SPA must inject the oxo-base meta tag"
    );
}

#[test]
fn mount_with_base_path_root_mount_is_identity() {
    // A root mount must not wrap the router — nesting under "/" would
    // double-apply the SPA index route for nothing.
    let app = server::build_router("personal");
    let mounted = server::mount_with_base_path(app, "");
    // Root-mount contract: "/" serves the SPA index (pinned in csp_gate);
    // here the identity claim only needs one nested route answering.
    let resp = tokio::runtime::Runtime::new().unwrap().block_on(async {
        get(
            mounted,
            "/api/health", // any nested route still answers "/"
        )
        .await
    });
    assert_eq!(resp.status(), StatusCode::OK);
}
