//! Content-Security-Policy gate (issue #948 P3-2).
//!
//! The SPA used to be served with `script-src 'self' 'unsafe-inline'`. The
//! session token lives in `localStorage['oxo_token']`, so any future XSS
//! could execute an inline payload AND exfiltrate the token in one shot.
//! Scripts now load only from bundled files (`script-src 'self'`); the built
//! index.html carries no `<script>`/`<style>` bodies (verified by the drift
//! test below), so nothing legitimate needs inline execution.
//!
//! Inline *style* remains allowed: React `style={{…}}` props are attribute
//! styles (governed by `style-src-attr`, not `style-src`), and CodeMirror
//! injects its editor theme at runtime. This is the documented trade-off —
//! see the comment on `security_headers` in `src/server.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use tower::ServiceExt;

/// The exact policy served on every response. If you change the CSP in
/// `security_headers`, update this constant in the same commit and justify
/// the relaxation here — the test exists so a weakening never lands silently.
const EXPECTED_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'";

#[tokio::test]
async fn csp_header_has_no_inline_script_source() {
    let app = server::build_router("personal");
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csp = resp
        .headers()
        .get("content-security-policy")
        .expect("every response carries a CSP header")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        csp, EXPECTED_CSP,
        "CSP changed — review the weakening before updating this pin"
    );
    assert!(
        !csp.contains("script-src 'self' 'unsafe-inline'")
            && !csp.contains("script-src 'unsafe-inline'"),
        "script-src must not allow inline script execution (issue #948 P3-2)"
    );
}

/// Drift gate: the shipped `static/index.html` must not rely on the
/// `unsafe-inline` we just removed. An inline `<script>` body (or an
/// `on*=`/`javascript:` handler smuggled into the committed HTML) would be
/// blocked by the new policy and render a blank page — catch it at test time.
#[test]
fn spa_index_carries_no_inline_script() {
    let index_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/index.html");
    let index_html =
        std::fs::read_to_string(&index_path).expect("static/index.html is tracked in git");
    for (tag, needle) in [
        ("inline <script> body", "<script>"),
        ("event-handler attribute", " on"),
        ("javascript: URL", "javascript:"),
    ] {
        let hit = if needle == " on" {
            // ` on` inside an attribute position: a cheap heuristic for
            // `onclick=`-style handlers; whitespace-normalized HTML makes
            // this precise enough to flag anything new for review.
            index_html.contains(" onclick")
                || index_html.contains(" onload")
                || index_html.contains(" onerror")
        } else {
            index_html.contains(needle)
        };
        assert!(
            !hit,
            "static/index.html gained {tag} — blocked by script-src 'self'"
        );
    }
}
