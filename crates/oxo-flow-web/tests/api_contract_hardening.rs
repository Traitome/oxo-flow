//! API contract hardening (audit findings F3, F4, F5a, F5b, P5-8, P5-10).
//!
//! Every case here pins a place where the API used to answer something other
//! than the truth: a broken workflow saved as success, pagination params
//! silently ignored, credentials reported usable after a master-key rotation,
//! host internals in run payloads, and bare-text rejections.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

static DB_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn db_url() -> &'static str {
    DB_URL.get_or_init(|| {
        let dir = std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let path = format!("{dir}/api-contract-hardening-test.db");
        let _ = std::fs::remove_file(&path);
        format!("sqlite:{path}?mode=rwc")
    })
}

async fn ensure_db() {
    let url = db_url();
    oxo_flow_web::db::init_db(url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(url).await;
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{method} {uri}: body is not JSON ({e}): {bytes:?}"));
    (status, value)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    request(app, "GET", uri, None).await
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(app, "POST", uri, Some(body)).await
}

async fn put(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(app, "PUT", uri, Some(body)).await
}

/// A definition the engine accepts, with exactly one rule.
const VALID_TOML: &str = r#"
[workflow]
name = "contract-test"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// Not TOML at all: a type the parser rejects instead of reading as a
/// zero-rule workflow.
const BROKEN_TOML: &str = "this is [ definitely not toml";

// ---------------------------------------------------------------------------
// F3 — a workflow the engine cannot parse must never be persisted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn save_pipeline_rejects_unparsable_toml() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": BROKEN_TOML, "name": "broken"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "broken TOML must not save: {body}"
    );
    assert_eq!(body["code"], "PARSE_ERROR", "{body}");
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{body}"
    );

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pipelines WHERE name = 'broken'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        stored, 0,
        "the unrunnable definition must not be in the database"
    );
}

#[tokio::test]
async fn save_pipeline_still_accepts_a_valid_definition() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": VALID_TOML, "name": "valid"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rules_count"], 1, "{body}");
}

#[tokio::test]
async fn update_pipeline_rejects_unparsable_toml_but_allows_a_rename() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": VALID_TOML, "name": "to-update"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().expect("pipeline id").to_string();

    // New content must parse — otherwise the stale rules_count would mask an
    // unrunnable definition.
    let (status, body) = put(
        &app,
        &format!("/api/pipelines/{id}"),
        json!({"toml_content": BROKEN_TOML}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "PARSE_ERROR", "{body}");

    // A name-only rename does not carry new content and keeps working.
    let (status, body) = put(
        &app,
        &format!("/api/pipelines/{id}"),
        json!({"name": "renamed"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rules_count"], 1, "{body}");
    assert_eq!(body["name"], "renamed");
}

// ---------------------------------------------------------------------------
// F4 — `page` must not be silently ignored on the cursor-paginated run list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_runs_rejects_page_offset_pagination() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = get(&app, "/api/runs?page=2").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "PAGE_NOT_SUPPORTED", "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("cursor"),
        "the error must point at the real pagination mechanism: {message}"
    );

    // The plain list is untouched.
    let (status, _) = get(&app, "/api/runs").await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// P5-8 — unusable query strings answer in the structured error envelope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_query_string_answers_in_the_error_envelope() {
    ensure_db().await;
    let app = server::build_router("personal");

    // `limit` is a usize: "abc" cannot deserialize into it.
    let (status, body) = get(&app, "/api/runs?limit=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_QUERY", "{body}");
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{body}"
    );
    assert!(
        body["detail"].is_string(),
        "the offending parameter must be named: {body}"
    );
    assert!(body["suggestion"].is_string(), "{body}");
}

// ---------------------------------------------------------------------------
// P5-10 — run payloads carry no host process id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_endpoints_do_not_expose_the_host_pid() {
    ensure_db().await;
    let app = server::build_router("personal");

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, pid, workdir, started_at, finished_at, created_at, workflow_name) \
         VALUES ('pid-contract-run', 'default', NULL, '[workflow]\nname = \"x\"', 'completed', 'done', 424242, NULL, NULL, NULL, '2026-01-01T00:00:00Z', 'x')",
    )
    .execute(pool)
    .await
    .unwrap();

    let (status, detail) = get(&app, "/api/runs/pid-contract-run").await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(
        detail.get("pid").is_none(),
        "GET /api/runs/{{id}} leaked the host pid: {detail}"
    );

    let (status, list) = get(&app, "/api/runs?q=pid-contract-run").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "the seeded run must be listed: {list}");
    for item in items {
        assert!(
            item.get("pid").is_none(),
            "GET /api/runs leaked a host pid: {item}"
        );
    }
}

// ---------------------------------------------------------------------------
// F5a — plaintext key storage is visible to API consumers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_reports_the_ai_key_storage_mode() {
    ensure_db().await;
    if std::env::var("OXO_FLOW_MASTER_KEY").is_ok_and(|v| !v.is_empty()) {
        println!("SKIP: OXO_FLOW_MASTER_KEY is set in this environment");
        return;
    }
    let app = server::build_router("personal");
    let (status, body) = get(&app, "/api/health").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["components"]["ai_key_storage"], "plaintext",
        "without a master key the health endpoint must say so: {body}"
    );
}

// ---------------------------------------------------------------------------
// F5b — an unreadable stored credential is not "configured"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_ai_config_reports_an_unreadable_credential_as_unconfigured() {
    ensure_db().await;
    let app = server::build_router("personal");

    // No master key in the test process, so `seal` stores plaintext and the
    // saved configuration is usable as-is.
    let (status, body) = put(
        &app,
        "/api/ai/config/server",
        json!({"provider": "deepseek", "model": "deepseek-v4-pro", "api_key": "sk-contract-test", "api_url": "https://api.example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = get(&app, "/api/ai/config/server").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["configured"], true, "{body}");
    assert_eq!(body["requires_reauth"], false, "{body}");
    assert_eq!(body["server_config"]["model"], "deepseek-v4-pro", "{body}");
    assert!(
        body["server_config"].get("api_key").is_none(),
        "the stored credential must never be echoed back: {body}"
    );

    // Simulate a master-key rotation: the row survives, its ciphertext does
    // not decrypt under the current key. The API must stop claiming AI is
    // configured and say what to do about it.
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query("UPDATE ai_provider_config SET api_key = 'v1:bm90LXRoZS1rZXk6bm90LXRoZS1rZXk' WHERE user_id IS NULL")
        .execute(pool)
        .await
        .unwrap();

    let (status, body) = get(&app, "/api/ai/config/server").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["configured"], false,
        "an unreadable credential is not a configured AI: {body}"
    );
    assert_eq!(body["requires_reauth"], true, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("OXO_FLOW_MASTER_KEY") && message.contains("re-enter"),
        "the message must say how to recover: {message}"
    );
}

// ---------------------------------------------------------------------------
// F-4 — POST /api/runs speaks ONE typed contract (issue #324)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_run_accepts_the_flat_wire_contract() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": VALID_TOML,
            "max_jobs": 2,
            "dry_run": true,
            "keep_going": false,
            "pipeline_id": null,
            "samples": [],
            "targets": [],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["run_id"].as_str().is_some_and(|s| !s.is_empty()),
        "a created run returns its id: {body}"
    );
}

#[tokio::test]
async fn create_run_keeps_the_stable_missing_toml_error() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(&app, "/api/runs", json!({ "max_jobs": 2, "dry_run": true })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "MISSING", "{body}");
    assert_eq!(body["message"], "toml_content required", "{body}");
}

// ---------------------------------------------------------------------------
// F-6① — empty-body POSTs must not demand a JSON content type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_and_resume_accept_empty_bodies() {
    ensure_db().await;
    let app = server::build_router("personal");

    // No Content-Type header, no body — the action verbs must still reach
    // the handler (a missing run then answers 404, not a 415 media-type
    // rejection aimed at a body the client never sent). Raw oneshot calls
    // because a 415's plain-text body is not JSON (the `request` helper
    // assumes JSON responses).
    for action in ["pause", "resume", "cancel"] {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/runs/does-not-exist/{action}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "empty-body {action} must not be rejected as 415"
        );
    }
}
