//! Route-level authorization and spawn-budget gates (issue #519).
//!
//! Six homogeneous gaps, covered here end-to-end against the real server:
//! viewer cannot mutate runs; retry cannot loop past the #213 limiter;
//! create_run cannot execute a foreign private pipeline; save_template
//! cannot rewrite another user's template; license upload is admin-only
//! and bounded; webhook config GET is admin-only.

use reqwest::Client;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn workspace_bin(name: &str) -> PathBuf {
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Server {
    child: Child,
    base: String,
    log_path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn log_tail(s: &Server) -> String {
    match std::fs::read(&s.log_path) {
        Ok(bytes) => {
            let tail = if bytes.len() > 4096 {
                &bytes[bytes.len() - 4096..]
            } else {
                &bytes[..]
            };
            String::from_utf8_lossy(tail).into_owned()
        }
        Err(_) => "(no server log)".to_string(),
    }
}

fn spawn_server_retrying(dir: &std::path::Path, port: u16, extra_envs: &[(&str, &str)]) -> Server {
    let mut last_log = String::new();
    for attempt in 1..=5 {
        let port = if attempt == 1 { port } else { free_port() };
        let log_path = dir.join("web-server.log");
        let log_file = std::fs::File::create(&log_path).expect("create server log");
        let mut cmd = Command::new(workspace_bin("oxo-flow-web"));
        cmd.current_dir(dir)
            .env("OXO_FLOW_BIN", workspace_bin("oxo-flow"))
            .env("OXO_FLOW_HOST", "127.0.0.1")
            .env("OXO_FLOW_PORT", port.to_string())
            .env(
                "OXO_FLOW_FRONTEND_DIR",
                dir.join("missing-frontend").to_str().unwrap(),
            );
        for (k, v) in extra_envs {
            cmd.env(k, v);
        }
        let child = cmd
            .stdout(Stdio::from(log_file.try_clone().unwrap()))
            .stderr(Stdio::from(log_file))
            .spawn()
            .expect("web server must start");
        let mut server = Server {
            child,
            base: format!("http://127.0.0.1:{port}"),
            log_path,
        };
        let needle = format!(
            "Listening on http://{}",
            server.base.trim_start_matches("http://")
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if std::fs::read_to_string(&server.log_path).is_ok_and(|log| log.contains(&needle)) {
                return server;
            }
            if matches!(server.child.try_wait(), Ok(Some(_))) {
                break;
            }
            if Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        last_log = log_tail(&server);
    }
    panic!("web server could not bind a free port after 5 attempts\n{last_log}");
}

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
    let server = spawn_server_retrying(
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
