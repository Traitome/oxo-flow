//! Pipeline ownership attribution (B9) + real effective AI config (B10).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tower::ServiceExt;

static DB_URL: OnceLock<String> = OnceLock::new();

fn db_url() -> &'static str {
    DB_URL.get_or_init(|| {
        let dir = std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let path = format!("{dir}/ownership-test.db");
        let _ = std::fs::remove_file(&path);
        format!("sqlite:{path}?mode=rwc")
    })
}

async fn ensure_db() {
    let url = db_url();
    oxo_flow_web::db::init_db(url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(url).await;
}

async fn post_json(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
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
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

const MINIMAL_TOML: &str = r#"
[workflow]
name = "ownership-test"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

#[tokio::test]
async fn save_pipeline_uses_default_owner_in_personal_mode() {
    ensure_db().await;
    let app = server::build_router("personal");
    let (status, body) = post_json(
        &app,
        "/api/pipelines",
        json!({"toml_content": MINIMAL_TOML, "name": "owned-by-default"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let id = body["id"].as_str().expect("pipeline id").to_string();

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let owner: String = sqlx::query_scalar("SELECT user_id FROM pipelines WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await
        .expect("pipeline row");
    // The bug: owner was hardcoded to the first admin row's UUID.
    assert_eq!(owner, "default");
}

#[tokio::test]
async fn effective_ai_config_reports_user_tier() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Save a user-level AI config (the same shape the settings UI uses).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/ai/config/user")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"provider": "deepseek", "model": "deepseek-v4-pro", "api_key": "sk-test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "PUT config/user must save");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/ai/config/effective")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    // The bug: tiers.user_provider was hardcoded to null.
    assert_eq!(body["tiers"]["user_provider"], "deepseek", "{body}");
}
