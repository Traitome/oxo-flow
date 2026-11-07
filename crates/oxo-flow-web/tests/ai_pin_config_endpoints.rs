//! `OXO_FLOW_AI_PROVIDER=disabled` must outrank the AI configuration-update
//! and probe endpoints, not just startup and generation.
//!
//! #1037 pinned startup and every generation surface, but the settings
//! endpoints still called `AiRegistry::reconfigure` directly: `POST
//! /api/ai/config` (and the PUT server/user variants) resurrected a real
//! provider on a pinned process, the next `POST /api/ai/test` dialed out,
//! and `/api/health` reported `connected`.
//!
//! Contract under test: configuration updates are refused with
//! `409 AI_DISABLED_BY_ENV` before any persistence or runtime change, and
//! the test probe answers `success: false` / `provider: "disabled"` without
//! constructing an HTTP client. Unpinned behavior is unchanged.
//!
//! Env mutation is process-global, so every test in this binary serializes
//! on [`ENV_LOCK`] and installs/restores the environment it needs (the same
//! pattern as `serve_startup_parity.rs`).

use std::net::TcpListener;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Process environment scoped to one test: a throwaway `$HOME` (so the real
/// `~/.oxo-flow/ai_config.json` is neither read nor rewritten) and the AI
/// provider pin either installed or removed.
struct EnvScope {
    prior_provider: Option<std::ffi::OsString>,
    prior_home: Option<std::ffi::OsString>,
    home: std::path::PathBuf,
}

impl EnvScope {
    fn new(pinned: bool) -> Self {
        let home = std::env::temp_dir().join(format!(
            "oxo-flow-ai-pin-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&home).expect("create temporary HOME");
        let prior_provider = std::env::var_os("OXO_FLOW_AI_PROVIDER");
        let prior_home = std::env::var_os("HOME");
        // SAFETY: env mutation is process-wide; every test in this binary
        // holds ENV_LOCK for its whole body and Drop restores both values.
        unsafe {
            std::env::set_var("HOME", &home);
            if pinned {
                std::env::set_var("OXO_FLOW_AI_PROVIDER", "disabled");
            } else {
                std::env::remove_var("OXO_FLOW_AI_PROVIDER");
            }
        }
        Self {
            prior_provider,
            prior_home,
            home,
        }
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        // SAFETY: see EnvScope::new — the lock is still held here because
        // the scope is declared after the guard.
        unsafe {
            match &self.prior_provider {
                Some(value) => std::env::set_var("OXO_FLOW_AI_PROVIDER", value),
                None => std::env::remove_var("OXO_FLOW_AI_PROVIDER"),
            }
            match &self.prior_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn count_provider_rows() -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM ai_provider_config")
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("count ai_provider_config rows")
}

fn fake_provider_body(api_url: &str) -> Value {
    json!({
        "provider": "openai",
        "api_key": "sk-must-not-activate",
        "api_url": api_url,
        "model": "gpt-4o",
    })
}

#[tokio::test]
async fn pinned_config_updates_are_rejected_and_every_state_surface_stays_disabled() {
    let _guard = ENV_LOCK.lock().await;
    common::ensure_db().await;
    let _env = EnvScope::new(true);
    oxo_flow_web::init_ai_provider(None).await;
    let app = server::build_router("personal");

    // Baseline health matches the operative reality.
    let (status, health) = get_json(&app, "/api/health").await;
    assert_eq!(status, StatusCode::OK, "{health:?}");
    assert_eq!(health["components"]["ai_provider"]["status"], "disabled");

    // The exact bypass the reviewer found: every configuration-update
    // endpoint must refuse with the shared structured error.
    let rows_before = count_provider_rows().await;
    let body = fake_provider_body("http://127.0.0.1:9/v1");
    for (method, uri) in [
        ("POST", "/api/ai/config"),
        ("PUT", "/api/ai/config/server"),
        ("PUT", "/api/ai/config/user"),
    ] {
        let (status, error) = request_json(&app, method, uri, body.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}: {error:?}");
        assert_eq!(error["code"], "AI_DISABLED_BY_ENV", "{method} {uri}");
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|m| m.contains("OXO_FLOW_AI_PROVIDER=disabled")),
            "{method} {uri}: {error:?}"
        );
        assert!(error["detail"].is_string(), "{method} {uri}: {error:?}");
        assert!(error["suggestion"].is_string(), "{method} {uri}: {error:?}");
    }
    // Refusal precedes persistence: no row was written by any variant.
    assert_eq!(
        count_provider_rows().await,
        rows_before,
        "a rejected configuration update must not persist"
    );

    // Nothing resurrected the registry or its status surfaces.
    assert!(!oxo_flow_ai::AI.is_enabled());
    let (_, config) = get_json(&app, "/api/ai/config").await;
    assert_eq!(config["provider"], "disabled");
    assert_eq!(config["is_configured"], false);

    let (_, effective) = get_json(&app, "/api/ai/config/effective").await;
    assert_eq!(effective["effective"]["provider"], "disabled");
    assert_eq!(effective["effective"]["is_configured"], false);
    assert_eq!(effective["disabled_by_env"], true);

    let (_, user) = get_json(&app, "/api/ai/config/user").await;
    assert_eq!(user["configured"], false, "{user:?}");
    assert_eq!(user["disabled_by_env"], true);

    let (_, server_config) = get_json(&app, "/api/ai/config/server").await;
    assert_eq!(server_config["configured"], false, "{server_config:?}");
    assert_eq!(server_config["disabled_by_env"], true);

    let (_, health) = get_json(&app, "/api/health").await;
    assert_eq!(health["components"]["ai_provider"]["status"], "disabled");

    // Generation endpoints stay gated: no provider, no outbound attempt.
    let (status, error) = request_json(
        &app,
        "POST",
        "/api/ai/translate",
        json!({"intent": "RNA-seq"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{error:?}");
    assert_eq!(error["code"], "AI_NOT_CONFIGURED");
}

#[tokio::test]
async fn pinned_test_probe_reports_disabled_without_any_connection() {
    let _guard = ENV_LOCK.lock().await;
    common::ensure_db().await;
    let _env = EnvScope::new(true);
    oxo_flow_web::init_ai_provider(None).await;
    let app = server::build_router("personal");

    // An instrumented endpoint instead of an unreachable blackhole: any
    // completed TCP handshake lands in this listener's backlog, so "no
    // outbound connection" is observable rather than inferred from latency.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe listener");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let probe_url = format!("http://{}/v1", listener.local_addr().unwrap());

    // Replay the reported sequence: configure first (now refused), then test.
    let (status, error) = request_json(
        &app,
        "POST",
        "/api/ai/config",
        fake_provider_body(&probe_url),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error:?}");
    assert_eq!(error["code"], "AI_DISABLED_BY_ENV");
    assert!(
        !oxo_flow_ai::AI.is_enabled(),
        "config update resurrected AI"
    );

    // The probe must answer promptly instead of hanging on a dial.
    let (status, body) = tokio::time::timeout(
        Duration::from_secs(5),
        request_json(&app, "POST", "/api/ai/test", json!({})),
    )
    .await
    .expect("pinned /api/ai/test must return without dialing out");
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["success"], false, "{body:?}");
    assert_eq!(body["provider"], "disabled", "{body:?}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("no connection attempted")),
        "{body:?}"
    );

    // Give a (hypothetical) async dial a moment to reach the listener.
    tokio::time::sleep(Duration::from_millis(100)).await;
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("pinned /api/ai/test opened a connection: {other:?}"),
    }
}

#[tokio::test]
async fn unpinned_config_updates_still_apply() {
    let _guard = ENV_LOCK.lock().await;
    common::ensure_db().await;
    let _env = EnvScope::new(false);
    oxo_flow_web::init_ai_provider(None).await;
    let app = server::build_router("personal");

    // Unpinned POST reconfigures the shared runtime exactly as before.
    let (status, body) = request_json(
        &app,
        "POST",
        "/api/ai/config",
        fake_provider_body("http://127.0.0.1:9/v1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["provider"], "openai");
    assert_eq!(body["is_configured"], true);

    let (_, health) = get_json(&app, "/api/health").await;
    assert_eq!(health["components"]["ai_provider"]["status"], "connected");

    // Unpinned PUT persists and applies the server tier too.
    let (status, body) = request_json(
        &app,
        "PUT",
        "/api/ai/config/server",
        json!({
            "provider": "deepseek",
            "api_key": "sk-test",
            "api_url": "http://127.0.0.1:9/v1",
            "model": "deepseek-chat",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "updated");
    assert_eq!(
        oxo_flow_web::ai_provider::AiProviderRegistry::global()
            .get_config()
            .provider,
        "deepseek"
    );

    // Leave the process-global registry disabled for any subsequent test.
    oxo_flow_ai::AI.disable().expect("reset registry");
}
