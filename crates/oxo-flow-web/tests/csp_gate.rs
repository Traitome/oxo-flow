//! Content-Security-Policy gate (issue #948 P3-2).
//!
//! The SPA used to be served with `script-src 'self' 'unsafe-inline'`. The
//! session token lives in `localStorage['oxo_token']`, so any future XSS
//! could execute an inline payload AND exfiltrate the token in one shot.
//! Scripts now load only from bundled files (`script-src 'self'`); the built
//! index.html carries no `<script>`/`<style>` bodies (verified by the drift
//! test below), and the server injects no inline script either — the
//! sub-path mount prefix is delivered as a `<meta name="oxo-base">` tag
//! that the bundled JS reads from the DOM.
//!
//! Inline *style* remains allowed: `style-src-attr` is not set, so style
//! attributes (React `style={{…}}` props, CodeMirror's runtime-injected
//! editor theme) fall back to `style-src`, whose `'unsafe-inline'` covers
//! them. This is the documented trade-off — see the comment on
//! `security_headers` in `src/server.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use tower::ServiceExt;

/// The exact policy served on every response. If you change the CSP in
/// `security_headers`, update this constant in the same commit and justify
/// the relaxation here — the test exists so a weakening never lands silently.
const EXPECTED_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'";

/// Event-handler attributes (`onclick=`-style) that the policy would not
/// block (they are not scripts per se) but that the drift gate treats as a
/// regression signal: the built index.html has never carried any, so one
/// appearing means someone hand-edited the shell and it needs review.
static EVENT_HANDLER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

fn event_handler_re() -> &'static regex::Regex {
    EVENT_HANDLER_RE.get_or_init(|| {
        // Whitespace-insensitive attribute match: `onclick =` variants and
        // single-quoted values included. Matches only in attribute position
        // (preceded by whitespace), so text content never trips it.
        regex::Regex::new(r#"(?i)\son[a-z]+\s*=\s*["']"#).expect("valid regex")
    })
}

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
        ("javascript: URL", "javascript:"),
    ] {
        assert!(
            !index_html.contains(needle),
            "static/index.html gained {tag} — blocked by script-src 'self'"
        );
    }
    assert!(
        !event_handler_re().is_match(&index_html),
        "static/index.html gained an on*= event handler — review before serving"
    );
}

/// The SERVED index (not just the committed one) is what the browser
/// executes: `load_spa_index_html` injects a `<base>` tag and the
/// `oxo-base` meta tag right after `<head>`. This pins the injection
/// contract end-to-end — the injected markup must stay CSP-clean (no
/// inline script sneaking back in) and must carry the mount prefix the
/// frontend reads via `oxoBase()`. The served page may legitimately be
/// the "SPA not built" fallback (CI never builds the frontend), so the
/// fallback itself must satisfy the same injection contract — it carries
/// a `<head>` for exactly that reason.
///
/// SAFETY: `set_base_path`/`set_running_mode` are process-global
/// first-call-wins statics; the other tests in this binary never set them
/// (and `build_router` itself doesn't either), so setting the root-mount
/// contract ("") here is deterministic.
#[tokio::test]
async fn served_spa_index_is_csp_clean_and_carries_base_meta() {
    server::set_running_mode("personal");
    server::set_base_path(""); // root mount: base_path() contract is ""
    let app = server::build_router("personal");
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("SPA index carries content-type")
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.starts_with("text/html"));
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("read SPA index body");
    let html = String::from_utf8_lossy(&body);

    // CSP contract: no inline script bodies, no event handlers — the same
    // drift rules as the committed-file test above, but against the served
    // bytes (which include the server's injection).
    assert!(
        !html.contains("<script>"),
        "served index gained an inline <script> body — blocked by script-src 'self'"
    );
    assert!(
        !event_handler_re().is_match(&html),
        "served index gained an on*= event handler — blocked/review before serving"
    );
    // Sub-path support (issue #79): the injected tags must be present.
    assert!(
        html.contains("<base href=\"/\">"),
        "served index must inject <base href=\"/\"> at root mount"
    );
    assert!(
        html.contains("<meta name=\"oxo-base\" content=\"\">"),
        "served index must inject the oxo-base meta tag the frontend reads"
    );
}
