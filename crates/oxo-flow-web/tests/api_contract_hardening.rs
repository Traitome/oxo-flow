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

mod common;

async fn ensure_db() {
    common::ensure_db().await;
}

/// Reset the singleton webhook_config row to a known precondition.
///
/// All webhook tests in this binary share one file-backed DB and one row
/// (id = 1), and cargo runs them in parallel — so each test must seed the
/// state it asserts against instead of relying on another test's leftovers.
/// The row is a singleton (`CHECK (id = 1)`), so seeding alone still races:
/// every webhook test also takes this mutex to serialize its whole body.
static WEBHOOK_ROW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn seed_webhook_row(secret: Option<&str>) {
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("personal mode has SQLite");
    sqlx::query(
        "INSERT INTO webhook_config (id, url, secret, enabled, events, signature_scheme, updated_at) \
         VALUES (1, 'https://hooks.example/alive', ?, 0, '[]', 'sha256-keyed', '2026-10-25T00:00:00Z') \
         ON CONFLICT(id) DO UPDATE SET url = excluded.url, secret = excluded.secret, enabled = 0",
    )
    .bind(secret)
    .execute(pool)
    .await
    .unwrap();
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

#[tokio::test]
async fn create_run_accepts_include_files_and_config_overrides() {
    ensure_db().await;
    let app = server::build_router("personal");

    // The CLI→web milestone fields: [[include]] sources staged beside the
    // workflow, config overrides passed through as `--arg KEY=VALUE`. The
    // run itself is a dry-run, so nothing executes — the point is that the
    // typed contract accepts both fields end to end (staging happens before
    // the spawn; a 200 means the include files parsed, validated, and were
    // written).
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": VALID_TOML,
            "dry_run": true,
            "include_files": {"envs/pixi.toml": "[env]\n"},
            "config_overrides": ["min_quality=30"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["run_id"].as_str().is_some(), "{body}");
}

#[tokio::test]
async fn create_run_rejects_include_files_absolute_paths() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Absolute-path keys would escape the run sandbox — rejected with the
    // structured INVALID_INCLUDE_PATH code, never staged.
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": VALID_TOML,
            "dry_run": true,
            "include_files": {"/etc/passwd": "root:x:0:0"},
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_INCLUDE_PATH", "{body}");
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

// ---------------------------------------------------------------------------
// Audits #651/#665 — the cursor-pagination contract: `next_cursor` is null
// on the LAST page (not on every page), and `total` counts the whole filter
// population on every page (it must not shrink with the cursor).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_runs_cursor_and_total_contract() {
    ensure_db().await;

    // Seed three completed runs (plus the FK parents) directly: the
    // pagination contract is data-shape-driven, not spawn-driven. One
    // transaction = one connection, so the FK parents are visible to the
    // child inserts by definition.
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("personal mode has SQLite");
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, role, auth_type, os_user, password_hash, created_at) \
         VALUES ('default', 'default', 'user', 'env', 'default', '', '2026-09-28T00:00:00Z')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO pipelines (id, user_id, name, version, toml_content, rules_count, visibility, created_at, updated_at) \
         VALUES ('', 'default', 'cursor-contract', '0.1.0', '', 0, 'private', '2026-09-28T00:00:00Z', '2026-09-28T00:00:00Z')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    for i in 0..3 {
        sqlx::query(
            "INSERT OR IGNORE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, created_at) \
             VALUES (?, 'default', '', '{}', 'cursor-contract', 'completed', ?)",
        )
        .bind(format!("cursor-contract-{i}"))
        .bind(format!("2026-09-28T00:00:0{i}Z"))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let app = server::build_router("personal");

    // The `q` filter pins this test's dataset to its own three rows, so
    // runs other tests insert in parallel cannot skew `total`.
    // Page 1 (limit=2 of 3 rows): next_cursor present, total = the WHOLE
    // matching population.
    let (status, body) = get(&app, "/api/runs?limit=2&q=cursor-contract").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(2), "{body}");
    assert_eq!(body["total"].as_i64(), Some(3), "{body}");
    let cursor = body["next_cursor"]
        .as_str()
        .expect("page 1 of 2 must carry a next cursor")
        .to_string();

    // Page 2: the remaining row — next_cursor must be NULL (last page) and
    // total must STILL be 3, not the 1 row left from this cursor.
    let (status, body) = get(
        &app,
        &format!(
            "/api/runs?limit=2&q=cursor-contract&cursor={}",
            urlencoding::encode(&cursor)
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(1), "{body}");
    assert!(
        body["next_cursor"].is_null(),
        "the last page must not carry a next_cursor: {body}"
    );
    assert_eq!(body["total"].as_i64(), Some(3), "{body}");
}

// ---------------------------------------------------------------------------
// Issue #907 — webhook config validation (N5) and structured body rejections
// (P2-9, via the ApiJson extractor every POST/PUT handler now uses).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_rejects_a_url_without_a_parseable_host() {
    ensure_db().await;
    let app = server::build_router("personal");

    // The old prefix check ("starts_with https://") accepted these. Note
    // `https:///path` is deliberately absent: url's special-scheme rules
    // normalize it to host "path", so it is genuinely valid.
    for url in [
        "https://",
        "http:// ",
        "not-a-url",
        "ftp://x.example",
        "https://",
    ] {
        let (status, body) = put(
            &app,
            "/api/webhook",
            json!({"enabled": true, "url": url, "secret": "s"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "url {url:?}: {body}");
        assert_eq!(body["code"], "INVALID_URL", "url {url:?}: {body}");
    }
}

#[tokio::test]
async fn webhook_enable_without_any_secret_is_refused() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Fresh config, no secret stored, none supplied: enabling must fail with
    // a dedicated code (a webhook URL is a bearer credential; unsigned
    // delivery lets anyone who guesses the URL forge events). The row is
    // seeded here — cargo runs this binary's tests in parallel against one
    // shared DB row, so another test's leftover secret must not leak in.
    // seeded under the row lock: cargo runs this binary's tests in parallel
    // against one shared DB row, so the seed itself must not interleave
    // with another webhook test's request.
    let _guard = WEBHOOK_ROW.lock().await;
    seed_webhook_row(None).await;
    let (status, body) = put(
        &app,
        "/api/webhook",
        json!({"enabled": true, "url": "https://hooks.example/alive"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "WEBHOOK_SECRET_REQUIRED", "{body}");

    // Supplying a secret makes the same request succeed.
    let (status, body) = put(
        &app,
        "/api/webhook",
        json!({"enabled": true, "url": "https://hooks.example/alive", "secret": "k"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "saved", "{body}");
}

#[tokio::test]
async fn webhook_disable_allows_clearing_the_secret() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Seed our own stored secret instead of depending on a previous test's
    // shared-DB leftovers; disabling must never demand one (the gate only
    // guards going live).
    let _guard = WEBHOOK_ROW.lock().await;
    seed_webhook_row(Some("leftover-key")).await;
    let (status, body) = put(&app, "/api/webhook", json!({"enabled": false, "url": ""})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn malformed_json_body_answers_in_the_error_envelope() {
    ensure_db().await;
    let app = server::build_router("personal");

    // P2-9: a body that does not deserialize used to surface as axum's
    // plain-text 422; through ApiJson it must arrive in the house envelope.
    let req = Request::builder()
        .method("POST")
        .uri("/api/pipelines")
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("rejection must be JSON");
    assert_eq!(body["code"], "INVALID_BODY", "{body}");
    assert!(body["suggestion"].is_string(), "{body}");
}

#[tokio::test]
async fn login_bruteforce_lockout_answers_429() {
    ensure_db().await;
    let app = server::build_router("personal");

    // T5: 10 consecutive bad logins for one account must trip the
    // (account, ip) brute-force limiter; the 11th is refused with the
    // structured 429 before any credential work. In-process requests carry
    // no ConnectInfo, so every attempt lands in the shared "unknown"
    // bucket — exactly the path a test can exercise without socket setup.
    let mut last = (StatusCode::UNAUTHORIZED, json!({}));
    for i in 0..10 {
        last = post(
            &app,
            "/api/auth/login",
            json!({"username": "bruteforce_probe_user", "password": "wrong"}),
        )
        .await;
        assert_eq!(last.0, StatusCode::UNAUTHORIZED, "attempt {i}: {last:?}");
    }
    let (last_status, _) = last;
    assert_eq!(last_status, StatusCode::UNAUTHORIZED);

    let (status, body) = post(
        &app,
        "/api/auth/login",
        json!({"username": "bruteforce_probe_user", "password": "wrong"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "RATE_LIMITED", "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").contains("retry"),
        "{body}"
    );

    // A different account from the same bucket is unaffected (per-account
    // keying, so one sprayed name cannot lock out the rest).
    let (status, body) = post(
        &app,
        "/api/auth/login",
        json!({"username": "other_probe_user", "password": "wrong"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

// ---------------------------------------------------------------------------
// Issue #907 — P2-5 (stored version derives from the TOML, not the caller),
// #909 NIT-1 (an unopenable stored secret is not a usable secret), and
// P3-14 (dry runs are flagged in the run records, not posing as executions).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_version_tracks_the_toml_not_the_request_body() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Save with version = 1.2.3 INSIDE the TOML and a deliberately
    // conflicting `version` JSON field: the TOML is the source of truth.
    let toml_v1 = VALID_TOML.replace(
        "[workflow]\nname = \"contract-test\"",
        "[workflow]\nname = \"contract-test\"\nversion = \"1.2.3\"",
    );
    let (status, body) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": toml_v1, "name": "version-sync", "version": "9.9.9"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"]
        .as_str()
        .expect("pipeline id: {body}")
        .to_string();
    assert_eq!(
        body["version"], "1.2.3",
        "the TOML version wins over the request field: {body}"
    );

    // Update the TOML to 2.0.0 with NO version field at all — the stored
    // version must follow the content (the UPDATE previously omitted the
    // version column entirely).
    let toml_v2 = toml_v1.replace("version = \"1.2.3\"", "version = \"2.0.0\"");
    let (status, body) = put(
        &app,
        &format!("/api/pipelines/{id}"),
        json!({"toml_content": toml_v2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], "2.0.0", "{body}");

    // And GET agrees with the update response (no stale stored copy).
    let (status, body) = get(&app, &format!("/api/pipelines/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["version"], "2.0.0",
        "GET must return the synced version: {body}"
    );
}

#[tokio::test]
async fn webhook_enable_refuses_a_stored_secret_that_does_not_decrypt() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Seed the config row with a `v1:` ciphertext that no master key can
    // open (the test binary never sets OXO_FLOW_MASTER_KEY, so crypto::open
    // degrades it to "") — the state a rotated master key leaves behind.
    // The old `secret.is_some()` gate accepted this row and enabled
    // unsigned, forgeable deliveries. Seed under the row lock (parallel
    // tests share the row); the stored value is present but NOT usable.
    let _guard = WEBHOOK_ROW.lock().await;
    seed_webhook_row(Some("v1:bm90LWE6cmVhbGtleQ")).await;
    let (status, body) = put(
        &app,
        "/api/webhook",
        json!({"enabled": true, "url": "https://hooks.example/alive"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "WEBHOOK_SECRET_REQUIRED", "{body}");

    // Supplying a fresh secret heals the row and the same request succeeds.
    let (status, body) = put(
        &app,
        "/api/webhook",
        json!({"enabled": true, "url": "https://hooks.example/alive", "secret": "k"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Cleanup: clear the row entirely, so no usable secret leaks from the
    // shared-file DB into the other webhook tests running in parallel.
    // The seed itself is safe out-of-lock (every other webhook test seeds
    // its own state under the lock first), but keep the write inside it
    // anyway so no webhook-row write ever races.
    drop(_guard);
    let _guard = WEBHOOK_ROW.lock().await;
    seed_webhook_row(None).await;
}

#[tokio::test]
async fn dry_run_launches_are_flagged_in_the_run_record() {
    ensure_db().await;
    let app = server::build_router("personal");

    // A dry-run creation must persist dry_run = true (P3-14): the runs
    // table previously had no such column, so a plan-only run was
    // indistinguishable from a real execution in every run listing.
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({"toml_content": VALID_TOML, "dry_run": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run id: {body}").to_string();

    let (status, body) = get(&app, &format!("/api/runs/{run_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["dry_run"], true,
        "single-run GET must flag the dry run: {body}"
    );

    let (status, body) = get(&app, &format!("/api/runs/{run_id}/status")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = get(&app, "/api/runs?limit=100").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let flagged = body["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|r| r["id"] == run_id.as_str())
        .expect("the new run appears in the list");
    assert_eq!(
        flagged["dry_run"], true,
        "the run list must flag the dry run: {flagged}"
    );

    // A normal run stays dry_run = false (default, and the historical rows'
    // migration default).
    let (status, body) = post(&app, "/api/runs", json!({"toml_content": VALID_TOML})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let normal_id = body["run_id"].as_str().expect("run id: {body}").to_string();
    let (status, body) = get(&app, &format!("/api/runs/{normal_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], false, "{body}");
}
