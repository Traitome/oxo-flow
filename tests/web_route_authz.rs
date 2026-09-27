//! Route-level authorization and spawn-budget gates (issue #519).
//!
//! Six homogeneous gaps, covered here end-to-end against the real server:
//! viewer cannot mutate runs; retry cannot loop past the #213 limiter;
//! create_run cannot execute a foreign private pipeline; save_template
//! cannot rewrite another user's template; license upload is admin-only
//! and bounded; webhook config GET is admin-only.

mod common;
use common::free_port;
use common::spawn_web_server;

use reqwest::Client;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

async fn req(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let mut r = match method {
        "POST" => client.post(format!("{base}{path}")),
        "PUT" => client.put(format!("{base}{path}")),
        "GET" => client.get(format!("{base}{path}")),
        _ => panic!("unsupported method"),
    };
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn route_authz_and_spawn_budget_gates() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_web_server(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    assert!(
        std::fs::read_to_string(&server.log_path)
            .is_ok_and(|log| log.contains("Listening on http://")),
        "server bound"
    );
    let base = server.base.clone();

    // Provision admin + a user + a viewer.
    let (status, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "admin", "password": "secret-admin"})),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    let admin = login["token"].as_str().unwrap().to_string();
    for (name, role) in [("alice", "user"), ("view", "viewer"), ("mallory", "user")] {
        let (status, body) = req(
            &base,
            "POST",
            "/api/users",
            Some(&admin),
            Some(json!({"username": name, "password": format!("{name}-pw"), "role": role})),
        )
        .await;
        assert_eq!(status, 200, "create {name}: {body}");
        let (status, login) = req(
            &base,
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": name, "password": format!("{name}-pw")})),
        )
        .await;
        assert_eq!(status, 200, "{name} login: {login}");
    }
    let (_, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "alice", "password": "alice-pw"})),
    )
    .await;
    let alice = login["token"].as_str().unwrap().to_string();
    let (_, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "view", "password": "view-pw"})),
    )
    .await;
    let viewer = login["token"].as_str().unwrap().to_string();
    let (_, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "mallory", "password": "mallory-pw"})),
    )
    .await;
    let mallory = login["token"].as_str().unwrap().to_string();

    // ── 1. Viewer cannot create runs (documented read-only role) ──
    let toml = "[workflow]\nname = \"authz\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n";
    let (status, body) = req(
        &base,
        "POST",
        "/api/runs",
        Some(&viewer),
        Some(json!({"toml_content": toml, "dry_run": true})),
    )
    .await;
    assert_eq!(status, 403, "viewer create_run must 403: {body}");

    // ── 3. create_run cannot execute a foreign PRIVATE pipeline ──
    let (status, pipeline) = req(
        &base,
        "POST",
        "/api/pipelines",
        Some(&alice),
        Some(json!({"toml_content": toml, "name": "private-p", "visibility": "private"})),
    )
    .await;
    assert_eq!(status, 200, "alice pipeline: {pipeline}");
    let pipeline_id = pipeline["id"].as_str().unwrap().to_string();
    let (status, body) = req(
        &base,
        "POST",
        "/api/runs",
        Some(&mallory),
        Some(json!({"toml_content": toml, "pipeline_id": pipeline_id})),
    )
    .await;
    assert_eq!(
        status, 404,
        "foreign private pipeline must read as not found: {body}"
    );
    assert_eq!(body["code"], "PIPELINE_NOT_FOUND", "{body}");

    // ── 4. save_template cannot rewrite another user's template ──
    let (status, tpl) = req(
        &base,
        "POST",
        "/api/templates",
        Some(&alice),
        Some(json!({
            "name": "alice-tpl",
            "toml_content": toml,
            "category": "general"
        })),
    )
    .await;
    assert_eq!(status, 200, "alice template create: {tpl}");
    let template_id = tpl["id"].as_str().unwrap().to_string();
    let (status, body) = req(
        &base,
        "POST",
        "/api/templates",
        Some(&mallory),
        Some(json!({
            "id": template_id,
            "name": "hijacked",
            "toml_content": toml
        })),
    )
    .await;
    assert_eq!(status, 403, "foreign template upsert must 403: {body}");

    // ── 5. license upload is admin-only and size-bounded ──
    let (status, _) = req(
        &base,
        "POST",
        "/api/license/upload",
        Some(&alice),
        Some(json!({"license_data": "whatever"})),
    )
    .await;
    assert_eq!(status, 403, "non-admin license upload must 403");
    let big = "x".repeat(65 * 1024);
    let (status, body) = req(
        &base,
        "POST",
        "/api/license/upload",
        Some(&admin),
        Some(json!({"license_data": big})),
    )
    .await;
    assert_eq!(status, 400, "oversized license payload must 400: {body}");

    // ── 6. webhook config GET is admin-only ──
    let (status, _) = req(&base, "GET", "/api/webhook", Some(&alice), None).await;
    assert_eq!(status, 403, "user webhook GET must 403");
    let (status, _) = req(&base, "GET", "/api/webhook", Some(&admin), None).await;
    assert_eq!(status, 200, "admin webhook GET must 200");

    // ── 2. retry pays the #213 limiter: rapid loop hits 429 ──
    // (default allowance: 5 runs/min; create below spends one slot)
    let failing = "[workflow]\nname = \"flaky\"\n\n[[rules]]\nname = \"boom\"\noutput = [\"o.txt\"]\nshell = \"false\"\n";
    let (status, run) = req(
        &base,
        "POST",
        "/api/runs",
        Some(&mallory),
        Some(json!({"toml_content": failing})),
    )
    .await;
    assert_eq!(status, 200, "failing run create: {run}");
    let run_id = run["run_id"].as_str().unwrap().to_string();
    // Wait for the run to reach a terminal state so retries are accepted.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (_, r) = req(
            &base,
            "GET",
            &format!("/api/runs/{run_id}"),
            Some(&mallory),
            None,
        )
        .await;
        let status_str = r["status"].as_str().unwrap_or("").to_string();
        if matches!(status_str.as_str(), "failed" | "completed" | "cancelled") {
            break;
        }
        assert!(Instant::now() < deadline, "run never reached terminal: {r}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // Rapid-fire retries: after the budget (create + 4 retries) is spent,
    // the next retry must answer 429 RUN_RATE_LIMITED.
    let mut saw_limited = false;
    for attempt in 0..8 {
        let (status, body) = req(
            &base,
            "POST",
            &format!("/api/runs/{run_id}/retry"),
            Some(&mallory),
            Some(json!({})),
        )
        .await;
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(body["code"], "RUN_RATE_LIMITED", "{body}");
            saw_limited = true;
            break;
        }
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "retry {attempt} unexpected status: {body}"
        );
    }
    assert!(saw_limited, "retry loop never hit the spawn-budget limiter");
}
