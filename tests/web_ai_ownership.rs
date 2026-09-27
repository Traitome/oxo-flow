//! AI-endpoint ownership enforcement (issue #515).
//!
//! `POST /api/ai/explain`, `/interpret`, and `/optimize` look runs and
//! pipelines up by bare id. They must enforce the same ownership rules as
//! the execution handlers: a foreign run/pipeline id answers 404 (not 403,
//! to avoid id probing), never leaks the victim's execution log, workdir
//! listing, or private pipeline TOML.

mod common;
use common::free_port;
use common::spawn_web_server;

use reqwest::Client;
use serde_json::{Value, json};

async fn put(base: &str, path: &str, token: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let resp = client
        .put(format!("{base}{path}"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

async fn post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let mut req = client.post(format!("{base}{path}")).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

/// PUT advanced AI options then GET must round-trip them (#545): the
/// Settings form read fields the GET never returned, silently reverting
/// saved values and wiping them on the next save.
#[tokio::test]
async fn user_ai_config_advanced_fields_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_web_server(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    let base = server.base.clone();

    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    let admin = login["token"].as_str().unwrap().to_string();
    let (status, body) = post(
        &base,
        "/api/users",
        Some(&admin),
        json!({"username": "carol", "password": "carol-pw"}),
    )
    .await;
    assert_eq!(status, 200, "create carol: {body}");
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "carol", "password": "carol-pw"}),
    )
    .await;
    assert_eq!(status, 200);
    let carol = login["token"].as_str().unwrap().to_string();

    // PUT advanced fields (as the Settings page does).
    let (status, body) = put(
        &base,
        "/api/ai/config/user",
        &carol,
        json!({
            "provider": "openai",
            "api_url": "https://api.example.com/v1",
            "model": "gpt-test",
            "api_key": "sk-test",
            "auto_retry_enabled": true,
            "max_correction_rounds": 5,
            "search_enabled": false,
            "monitor_enabled": false
        }),
    )
    .await;
    assert_eq!(status, 200, "PUT user config: {body}");

    // GET must return exactly what was stored — never defaults.
    let client = Client::new();
    let resp = client
        .get(format!("{base}/api/ai/config/user"))
        .bearer_auth(&carol)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let got: Value = resp.json().await.unwrap();
    let u = &got["user_config"];
    assert_eq!(u["auto_retry_enabled"], true, "{got}");
    assert_eq!(u["max_correction_rounds"], 5, "{got}");
    assert_eq!(u["search_enabled"], false, "{got}");
    assert_eq!(u["monitor_enabled"], false, "{got}");
    assert_eq!(u["provider"], "openai", "{got}");
    assert_eq!(u["api_key_set"], true, "{got}");
    assert!(
        !got.to_string().contains("sk-test"),
        "the api key must never be returned: {got}"
    );
}

/// Cross-tenant access through the AI surface must 404; the owner and the
/// admin still reach their own resources (AI itself stays unconfigured, so
/// the owner sees the provider error, not a 404).
#[tokio::test]
async fn ai_endpoints_enforce_run_and_pipeline_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_web_server(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    let base = server.base.clone();

    // Admin logs in and provisions two regular users.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    let admin = login["token"].as_str().unwrap().to_string();
    for name in ["alice", "mallory"] {
        let (status, body) = post(
            &base,
            "/api/users",
            Some(&admin),
            json!({"username": name, "password": format!("{name}-secret")}),
        )
        .await;
        assert_eq!(status, 200, "create {name}: {body}");
    }
    let mut tokens = std::collections::HashMap::new();
    for name in ["alice", "mallory"] {
        let (status, login) = post(
            &base,
            "/api/auth/login",
            None,
            json!({"username": name, "password": format!("{name}-secret")}),
        )
        .await;
        assert_eq!(status, 200, "{name} login: {login}");
        tokens.insert(name, login["token"].as_str().unwrap().to_string());
    }
    let alice = tokens["alice"].clone();
    let mallory = tokens["mallory"].clone();

    // Alice creates a private pipeline and a run over it.
    let toml = "[workflow]\nname = \"secret\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo secret-data > {output}\"\n";
    let (status, pipeline) = post(
        &base,
        "/api/pipelines",
        Some(&alice),
        json!({"toml_content": toml}),
    )
    .await;
    assert_eq!(status, 200, "create pipeline: {pipeline}");
    let pipeline_id = pipeline["id"].as_str().unwrap().to_string();

    let (status, run) = post(
        &base,
        "/api/runs",
        Some(&alice),
        json!({"toml_content": toml, "dry_run": true}),
    )
    .await;
    assert_eq!(status, 200, "create run: {run}");
    let run_id = run["run_id"].as_str().unwrap().to_string();

    // Unauthenticated requests stay 401 (route sits behind require_auth).
    let (status, _) = post(&base, "/api/ai/explain", None, json!({"run_id": run_id})).await;
    assert_eq!(status, 401, "explain must require auth");

    // Mallory (foreign user) must get 404 — existence itself is private.
    let (status, body) = post(
        &base,
        "/api/ai/explain",
        Some(&mallory),
        json!({"run_id": run_id}),
    )
    .await;
    assert_eq!(status, 404, "mallory explain on alice's run: {body}");

    let (status, body) = post(
        &base,
        "/api/ai/interpret",
        Some(&mallory),
        json!({"run_id": run_id}),
    )
    .await;
    assert_eq!(status, 404, "mallory interpret on alice's run: {body}");

    let (status, body) = post(
        &base,
        "/api/ai/optimize",
        Some(&mallory),
        json!({"pipeline_id": pipeline_id, "goal": "speed"}),
    )
    .await;
    assert_eq!(status, 404, "mallory optimize on alice's pipeline: {body}");

    // Unknown ids answer the same 404 (no existence oracle).
    let (status, _) = post(
        &base,
        "/api/ai/explain",
        Some(&mallory),
        json!({"run_id": "no-such-run"}),
    )
    .await;
    assert_eq!(status, 404, "unknown run id must not be distinguishable");

    // The owner still reaches the endpoint: the AI provider is disabled, so
    // the failure surfaces as the provider error (400), never a 404.
    for (path, body) in [
        ("/api/ai/explain", json!({"run_id": run_id})),
        ("/api/ai/interpret", json!({"run_id": run_id})),
    ] {
        let (status, resp) = post(&base, path, Some(&alice), body).await;
        assert_ne!(status, 404, "owner must not see 404 on {path}: {resp}");
    }
    let (status, resp) = post(
        &base,
        "/api/ai/optimize",
        Some(&alice),
        json!({"pipeline_id": pipeline_id, "goal": "speed"}),
    )
    .await;
    assert_ne!(status, 404, "owner optimize must not 404: {resp}");

    // Admins keep their cross-user access (same policy as load_owned_run).
    let (status, _) = post(
        &base,
        "/api/ai/explain",
        Some(&admin),
        json!({"run_id": run_id}),
    )
    .await;
    assert_ne!(status, 404, "admin explain must pass ownership");
}
