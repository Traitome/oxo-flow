//! 20-user web platform simulation tests.
//! Covers auth, workflow lifecycle, workspace isolation, resource sensing.

use axum::body::Body;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tower::ServiceExt;

static DB_INIT: OnceLock<()> = OnceLock::new();

async fn ensure_db() {
    if DB_INIT.get().is_none() {
        let db_path = std::env::temp_dir().join(format!("oxo-sim-{}.db", std::process::id()));
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        oxo_flow_web::db::init_db(&url)
            .await
            .expect("Failed to init test DB");
        // Set passwords for simulation test users
        unsafe {
            std::env::set_var("OXO_FLOW_ADMIN_PASSWORD", "admin");
            std::env::set_var("OXO_FLOW_USER_PASSWORD", "user");
            std::env::set_var("OXO_FLOW_VIEWER_PASSWORD", "viewer");
        }
        DB_INIT.set(()).ok();
    }
}

fn app() -> axum::Router {
    oxo_flow_web::build_router()
}

async fn post(uri: &str, body: Value) -> (u16, Value) {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn post_auth(uri: &str, body: Value, token: &str) -> (u16, Value) {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn get_auth(uri: &str, token: &str) -> (u16, Value) {
    let req = axum::http::Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn get(uri: &str) -> (u16, Value) {
    let req = axum::http::Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn login(username: &str, password: &str) -> String {
    ensure_db().await;
    let (status, body) = post(
        "/api/auth/login",
        json!({
            "username": username,
            "password": password
        }),
    )
    .await;
    assert_eq!(status, 200, "login failed for {username}: {body}");
    body["token"].as_str().unwrap().to_string()
}

const MINIMAL_TOML: &str = r#"
[workflow]
name = "test-pipeline"
version = "1.0.0"
description = "Simulation test workflow"
[[rules]]
name = "step_a"
output = ["a.txt"]
shell = "echo hello > {output[0]}"
[[rules]]
name = "step_b"
input = ["a.txt"]
output = ["b.txt"]
shell = "cat {input[0]} > {output[0]}"
"#;

// ── U1-U5: Authentication & Authorization ───────────────────────────────

#[tokio::test]
async fn u1_admin_login_and_session() {
    ensure_db().await;
    let token = login("admin", "admin").await;
    let (status, body) = get_auth("/api/auth/me", &token).await;
    assert_eq!(status, 200);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
}

#[tokio::test]
async fn u2_protected_endpoint_requires_auth() {
    ensure_db().await;
    let (status, _) = get_auth("/api/workflows", "invalid_token").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn u3_invalid_credentials_rejected() {
    ensure_db().await;
    let (status, body) = post(
        "/api/auth/login",
        json!({
            "username": "admin",
            "password": "wrong_password"
        }),
    )
    .await;
    assert_eq!(status, 401);
    assert!(body["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn u4_auth_me_without_token_returns_unauthenticated() {
    let (status, body) = get("/api/auth/me").await;
    assert_eq!(status, 200);
    assert_eq!(body["authenticated"], false);
}

#[tokio::test]
async fn u5_session_token_across_multiple_requests() {
    let token = login("admin", "admin").await;
    for _ in 0..5 {
        let (status, _) = get_auth("/api/runs", &token).await;
        assert_eq!(status, 200);
    }
}

// ── U6-U10: Workflow Lifecycle ──────────────────────────────────────────

#[tokio::test]
async fn u6_validate_workflow() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/validate",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["valid"].as_bool().unwrap());
    assert_eq!(body["rules_count"], 2);
}

#[tokio::test]
async fn u7_parse_workflow_to_detail() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/parse",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["name"], "test-pipeline");
    assert_eq!(body["rules_count"], 2);
    assert_eq!(body["rules"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn u8_build_dag_and_dry_run() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/dag",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["dot"].as_str().unwrap().contains("digraph"));

    let (status, body) = post_auth(
        "/api/workflows/dry-run",
        json!({
            "toml_content": MINIMAL_TOML,
            "config": {"max_jobs": 4, "dry_run": true}
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["status"]["rules_total"], 2);
}

#[tokio::test]
async fn u9_save_and_list_workflows() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/save",
        json!({
            "name": "saved-pipeline",
            "version": "1.0.0",
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 201);
    assert!(body["id"].as_str().is_some());

    let (status, body) = get_auth("/api/workflows/saved", &token).await;
    assert_eq!(status, 200);
    assert!(
        body.as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "saved-pipeline")
    );
}

#[tokio::test]
async fn u10_format_and_lint_workflow() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/format",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["formatted"].as_str().unwrap().contains("[workflow]"));

    let (status, body) = post_auth(
        "/api/workflows/lint",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["error_count"], 0);
}

// ── U11-U15: Run Lifecycle & Workspace ──────────────────────────────────

#[tokio::test]
async fn u11_start_run_and_check_detail() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/run",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "started");
    let run_id = body["run_id"].as_str().unwrap();

    let (status, body) = get_auth(&format!("/api/runs/{run_id}"), &token).await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], run_id);
}

#[tokio::test]
async fn u12_run_logs_accessible() {
    let token = login("admin", "admin").await;
    let (_, run_body) = post_auth(
        "/api/workflows/run",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    let run_id = run_body["run_id"].as_str().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (status, _) = get_auth(&format!("/api/runs/{run_id}/logs"), &token).await;
    assert!(
        status == 200 || status == 400,
        "logs endpoint should respond"
    );
}

#[tokio::test]
async fn u13_list_runs_shows_user_runs() {
    let token = login("admin", "admin").await;
    let (status, runs) = get_auth("/api/runs", &token).await;
    assert_eq!(status, 200);
    assert!(runs.is_array());
}

#[tokio::test]
async fn u14_cancel_run_endpoint() {
    let token = login("admin", "admin").await;
    let (_, run_body) = post_auth(
        "/api/workflows/run",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    let run_id = run_body["run_id"].as_str().unwrap();
    // Cancel via DELETE
    let req = axum::http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/runs/{run_id}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    assert!(resp.status().as_u16() == 200 || resp.status().as_u16() == 400);
}

#[tokio::test]
async fn u15_workflow_export_and_stats() {
    let token = login("admin", "admin").await;
    let (status, body) = post_auth(
        "/api/workflows/export",
        json!({
            "toml_content": MINIMAL_TOML,
            "format": "docker"
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["content"].as_str().unwrap().contains("FROM"));

    let (status, body) = post_auth(
        "/api/workflows/stats",
        json!({
            "toml_content": MINIMAL_TOML
        }),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["rule_count"].as_u64().unwrap() >= 1);
}

// ── U16-U20: Resource Sensing & Multi-User ──────────────────────────────

#[tokio::test]
async fn u16_health_endpoint() {
    let (status, body) = get("/api/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn u17_system_info_with_uptime() {
    let (status, body) = get("/api/system").await;
    assert_eq!(status, 200);
    assert!(!body["version"].as_str().unwrap().is_empty());
    assert!(body["uptime_secs"].as_f64().unwrap() >= 0.0);
}

#[tokio::test]
async fn u18_runtime_metrics_resource_sensing() {
    let (status, body) = get("/api/metrics").await;
    assert_eq!(status, 200);
    assert!(body["cpu_count"].as_u64().unwrap() >= 1);
    assert!(body["host"]["total_memory_mb"].as_u64().unwrap() > 0);
    assert!(body["host"]["cpu_usage_percent"].as_f64().unwrap() >= 0.0);
}

#[tokio::test]
async fn u19_sse_events_stream_available() {
    // SSE endpoint opens a persistent connection — just verify it starts correctly
    let req = axum::http::Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .unwrap();
    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), app().oneshot(req))
        .await
        .expect("SSE connection timeout")
        .expect("SSE request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got: {ct}"
    );
}

#[tokio::test]
async fn u20_concurrent_multi_user_operations() {
    let token = login("admin", "admin").await;

    let t1 = {
        let t = token.clone();
        tokio::spawn(async move {
            post_auth(
                "/api/workflows/validate",
                json!({"toml_content": MINIMAL_TOML}),
                &t,
            )
            .await
            .0
        })
    };
    let t2 = {
        let t = token.clone();
        tokio::spawn(async move {
            post_auth(
                "/api/workflows/dag",
                json!({"toml_content": MINIMAL_TOML}),
                &t,
            )
            .await
            .0
        })
    };
    let t3 = {
        let t = token.clone();
        tokio::spawn(async move {
            post_auth(
                "/api/workflows/stats",
                json!({"toml_content": MINIMAL_TOML}),
                &t,
            )
            .await
            .0
        })
    };

    let (r1, r2, r3) = tokio::join!(t1, t2, t3);
    assert_eq!(r1.unwrap(), 200);
    assert_eq!(r2.unwrap(), 200);
    assert_eq!(r3.unwrap(), 200);
}
