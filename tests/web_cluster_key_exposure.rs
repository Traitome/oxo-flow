//! Cluster SSH-key exposure (issue #517).
//!
//! `GET /api/clusters` must never return the stored `ssh_key` — any
//! authenticated user could previously read the server's key paths (or any
//! pasted key material). Responses carry `ssh_key_set` instead, and the
//! probe endpoint (which actively uses the credential) is admin-only
//! outside personal mode.

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

fn spawn_server(dir: &std::path::Path, port: u16, extra_envs: &[(&str, &str)]) -> Server {
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

fn wait_for_bind(server: &mut Server, timeout: Duration) -> bool {
    let needle = format!(
        "Listening on http://{}",
        server.base.trim_start_matches("http://")
    );
    let deadline = Instant::now() + timeout;
    loop {
        if std::fs::read_to_string(&server.log_path).is_ok_and(|log| log.contains(&needle)) {
            return true;
        }
        if matches!(server.child.try_wait(), Ok(Some(_))) {
            return false;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_server_retrying(dir: &std::path::Path, port: u16, extra_envs: &[(&str, &str)]) -> Server {
    let mut last_log = String::new();
    for attempt in 1..=5 {
        let port = if attempt == 1 { port } else { free_port() };
        let mut server = spawn_server(dir, port, extra_envs);
        if wait_for_bind(&mut server, Duration::from_secs(15)) {
            return server;
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

async fn get(base: &str, path: &str, token: Option<&str>) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let mut req = client.get(format!("{base}{path}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn cluster_responses_never_carry_ssh_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = spawn_server_retrying(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    assert!(
        wait_for_bind(&mut server, Duration::from_secs(15)),
        "server bind\n{}",
        log_tail(&server)
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
        json!({"username": "view", "password": "view-pw", "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 200, "create viewer: {body}");
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "view", "password": "view-pw"}),
    )
    .await;
    assert_eq!(status, 200, "viewer login: {login}");
    let viewer = login["token"].as_str().unwrap().to_string();

    // Admin stores a cluster with sensitive key material.
    let (status, upserted) = post(
        &base,
        "/api/clusters",
        Some(&admin),
        json!({
            "id": "lab",
            "name": "Lab",
            "ssh_host": "login.lab.example.edu",
            "ssh_user": "bioinf",
            "ssh_key": "SUPER-SECRET-KEY-MATERIAL",
            "scheduler": "slurm",
            "enabled": true
        }),
    )
    .await;
    assert_eq!(status, 200, "upsert: {upserted}");

    // The upsert response must not echo the key either.
    assert!(
        !upserted.to_string().contains("SUPER-SECRET-KEY-MATERIAL"),
        "upsert response leaks key: {upserted}"
    );

    // The list must not expose the key to anyone — admin or viewer.
    for token in [&admin, &viewer] {
        let (status, list) = get(&base, "/api/clusters", Some(token)).await;
        assert_eq!(status, 200, "list as {token}: {list}");
        let text = list.to_string();
        assert!(
            !text.contains("SUPER-SECRET-KEY-MATERIAL"),
            "list leaks key: {text}"
        );
        assert!(
            list[0]["ssh_key"].is_null(),
            "ssh_key field must be absent: {list}"
        );
        assert_eq!(list[0]["ssh_key_set"], true, "ssh_key_set flag: {list}");
    }

    // The probe endpoint actively uses the credential — admin-only.
    let (status, _) = post(&base, "/api/clusters/lab/probe", Some(&viewer), json!({})).await;
    assert_eq!(status, 403, "viewer must not probe: cluster credential use");
}
