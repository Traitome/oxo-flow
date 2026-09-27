//! Serve/desktop startup parity (issue #569).
//!
//! `oxo-flow serve` and the desktop shell previously skipped three startup
//! steps that only the standalone `oxo-flow-web` binary performed:
//! `set_bound_port` after bind (share URLs pointed at :3000),
//! `restore_ai_config_from_db` (a saved provider key was lost on restart),
//! and `DATABASE_URL` handling (hardcoded `sqlite://oxo-flow.db`).
//!
//! The hoisted helpers [`oxo_flow_web::init_database`] and
//! [`oxo_flow_web::init_ai_provider`] are what every entry point now calls;
//! these tests drive them directly, plus the share-URL port the fix feeds.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
    let app = server::build_router("personal");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
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

/// A SQLite `DATABASE_URL` routes through the SQLite path: `init_database`
/// initializes both DB layers, and a pipeline insert lands in the table.
#[tokio::test]
async fn init_database_sqlite_initializes_both_layers() {
    // The harness owns the env in this binary (see `ENV_LOCK` note below);
    // reuse its file-backed URL so other tests in this binary share the pool.
    let _guard = ENV_LOCK.lock().await;
    let url = common::db_url().clone();
    unsafe { std::env::set_var("DATABASE_URL", url) };
    oxo_flow_web::init_database()
        .await
        .expect("sqlite URL must initialize cleanly");

    // Both layers usable: the v0.8 pool answers writes…
    sqlx::query("CREATE TABLE IF NOT EXISTS init_database_probe (v TEXT)")
        .execute(pool())
        .await
        .expect("v0.8 pool initialized");
    // …and the legacy pool answers reads.
    let legacy: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'")
        .fetch_one(oxo_flow_web::db::pool())
        .await
        .expect("legacy pool initialized");
    let _ = legacy;
}

/// The registry must surface the DB-tier provider after `init_ai_provider`,
/// proving the restore step that `oxo-flow serve` previously skipped now
/// runs through the shared helper.
#[tokio::test]
async fn init_ai_provider_restores_db_tier() {
    common::ensure_db().await;
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, created_at, updated_at)
         VALUES (?, 'default', 'claude', 'sk-parity-test', '', 'claude-sonnet-4-20250514', 0, 0, 0, 3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')
         ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider, api_key=excluded.api_key, updated_at=excluded.updated_at",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(pool())
    .await
    .unwrap();

    // This binary runs alongside the developer's own `~/.oxo-flow/ai_config.json`
    // and AI provider env vars: without removing them, the env tier wins and the
    // DB restore (which only fires when env left the provider unset) is skipped.
    // `ENV_LOCK` serializes this against the other env-touching tests.
    let _guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::remove_var("OXO_FLOW_AI_PROVIDER");
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OXO_FLOW_AI_API_KEY");
    }

    oxo_flow_web::init_ai_provider(None).await;

    let config = oxo_flow_web::ai_provider::AiProviderRegistry::global().get_config();
    assert_eq!(config.provider, "claude", "{config:?}");
}

/// The regression #569 fixes: with `set_bound_port` fed by the real bind
/// (as `start_server_with_mode` now does), a share URL carries the bound
/// port instead of the :3000 fallback.
#[tokio::test]
async fn share_url_reports_the_bound_port() {
    common::ensure_db().await;

    sqlx::query(
        "INSERT OR REPLACE INTO pipelines (id, user_id, name, version, toml_content, \
         rules_count, visibility, created_at, updated_at) \
         VALUES ('pl-569', 'default', 'p', '1.0.0', '[workflow]\nname = \"p\"', 0, \
         'private', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .execute(pool())
    .await
    .expect("seed pipeline");

    // Set the port exactly as the hoisted startup step does after bind.
    let bound = 45891_u16;
    oxo_flow_web::server::set_bound_port(bound);

    let (status, body) = post_json(
        "/api/pipelines/pl-569/share",
        json!({"visibility": "link", "expires_in_days": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["share_url"].as_str().expect("share_url").to_string();
    assert!(
        url.contains(&format!(":{bound}/share/")),
        "share URL must carry the bound port {bound}, got: {url}"
    );
}

/// One test binary mutates process env; tokio tests share the process, so
/// env-touching tests serialize on this async-aware mutex (a sync guard
/// held across `.await` trips `clippy::await_holding_lock`).
pub static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
