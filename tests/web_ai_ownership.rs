//! AI-endpoint ownership enforcement (issue #515).
//!
//! `POST /api/ai/explain`, `/interpret`, and `/optimize` look runs and
//! pipelines up by bare id. They must enforce the same ownership rules as
//! the execution handlers: a foreign run/pipeline id answers 404 (not 403,
//! to avoid id probing), never leaks the victim's execution log, workdir
//! listing, or private pipeline TOML.

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

fn spawn_server_once(dir: &std::path::Path, port: u16, extra_envs: &[(&str, &str)]) -> Server {
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
    Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
        log_path,
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

fn spawn_server(dir: &std::path::Path, port: u16, extra_envs: &[(&str, &str)]) -> Server {
    let mut last_log = String::new();
    for attempt in 1..=5 {
        let port = if attempt == 1 { port } else { free_port() };
        let mut server = spawn_server_once(dir, port, extra_envs);
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

/// Cross-tenant access through the AI surface must 404; the owner and the
/// admin still reach their own resources (AI itself stays unconfigured, so
/// the owner sees the provider error, not a 404).
#[tokio::test]
async fn ai_endpoints_enforce_run_and_pipeline_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_server(
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
