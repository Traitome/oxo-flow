//! Web deployment + 3-role simulation matrix (24h campaign, web lane).
//!
//! Drives the REAL `oxo-flow-web` server binary through the full surface:
//! unauthenticated / user / admin role walkthrough, pipeline + run flows
//! (dry-run preview, execution, cancel), share-token access, SQLite
//! restart persistence, and --base-path deployment. Single sequential
//! test to avoid parallel-server flakiness.

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

fn spawn_server(
    dir: &std::path::Path,
    port: u16,
    extra_envs: &[(&str, &str)],
    extra_args: &[&str],
) -> Server {
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
    cmd.args(extra_args);
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

async fn wait_ready(base: &str) {
    let client = Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            Instant::now() < deadline,
            "server did not become ready at {base}"
        );
        if client
            .get(format!("{base}/api/health"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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

/// The full matrix, one sequential flow.
#[tokio::test]
async fn web_role_matrix_full_flow() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = spawn_server(
        dir.path(),
        port,
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
        &[],
    );
    wait_ready(&server.base).await;
    let base = server.base.clone();

    // ── 1. Unauthenticated walkthrough ──────────────────────────────────
    for path in ["/api/system", "/api/runs", "/api/users", "/api/audit"] {
        let (status, body) = get(&base, path, None).await;
        assert_eq!(status, 401, "{path} must require auth: {body}");
        assert_eq!(
            body["code"],
            "AUTH_REQUIRED",
            "{path} envelope mismatch: {body}\n{}",
            log_tail(&server)
        );
    }
    let (status, _) = get(&base, "/api/health", None).await;
    assert_eq!(status, 200, "health must be public");
    let (status, spec) = get(&base, "/api/openapi.json", None).await;
    assert_eq!(status, 200, "OpenAPI spec must be public");
    assert!(
        spec["paths"].as_object().unwrap().len() > 50,
        "spec path count"
    );

    // ── 2. Admin login + admin surface ──────────────────────────────────
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    assert_eq!(login["role"], "admin", "login role must be admin: {login}");
    let admin_token = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&admin_token)).await;
    assert_eq!(status, 200);
    assert_eq!(me["role"], "admin", "/me must agree with login role");

    // Create a regular user via the admin API.
    let (status, user) = post(
        &base,
        "/api/users",
        Some(&admin_token),
        json!({"username": "alice", "password": "alice-secret"}),
    )
    .await;
    assert_eq!(status, 200, "create user: {user}");
    let (status, users) = get(&base, "/api/users", Some(&admin_token)).await;
    assert_eq!(status, 200);
    let names: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["username"].as_str())
        .collect();
    assert!(names.contains(&"alice"), "user list must contain alice");
    let (status, _) = get(&base, "/api/audit", Some(&admin_token)).await;
    assert_eq!(status, 200, "admin must read the audit trail");

    // ── 3. Regular-user login + RBAC ────────────────────────────────────
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "alice", "password": "alice-secret"}),
    )
    .await;
    assert_eq!(status, 200, "alice login: {login}");
    assert_eq!(login["role"], "user");
    let alice = login["token"].as_str().unwrap().to_string();
    let (status, _) = get(&base, "/api/users", Some(&alice)).await;
    assert_eq!(status, 403, "user must not list users");
    let (status, _) = get(&base, "/api/audit", Some(&alice)).await;
    assert_eq!(status, 403, "user must not read the audit trail");
    let (status, _) = get(&base, "/api/system", Some(&alice)).await;
    assert_eq!(status, 200, "system info is auth-only per docs");

    // ── 4. Pipeline + dry-run + execution flows (as alice) ──────────────
    let toml = "[workflow]\nname = \"matrix\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n";
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
    assert_eq!(status, 200, "dry-run: {run}");
    let run_id = run["run_id"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut preview_ok = false;
    while Instant::now() < deadline {
        let (_, preview) = get(&base, &format!("/api/runs/{run_id}/preview"), Some(&alice)).await;
        if preview.get("checkpoint_preview").is_some() {
            preview_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        preview_ok,
        "dry-run preview must be served (instance-level)"
    );

    let (status, run) = post(
        &base,
        "/api/runs",
        Some(&alice),
        json!({"toml_content": toml}),
    )
    .await;
    assert_eq!(status, 200, "run: {run}");
    let run_id = run["run_id"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut terminal = String::new();
    while Instant::now() < deadline {
        let (_, body) = get(&base, &format!("/api/runs/{run_id}"), Some(&alice)).await;
        terminal = body["status"].as_str().unwrap_or("").to_string();
        if matches!(terminal.as_str(), "completed" | "failed" | "cancelled") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(
        terminal,
        "completed",
        "run must complete: {}",
        log_tail(&server)
    );

    // Ownership isolation: alice must NOT see an admin run (404, no leak).
    let (status, admin_run) = post(
        &base,
        "/api/runs",
        Some(&admin_token),
        json!({"toml_content": toml}),
    )
    .await;
    assert_eq!(status, 200, "admin run: {admin_run}");
    let admin_run_id = admin_run["run_id"].as_str().unwrap().to_string();
    let (status, _) = get(&base, &format!("/api/runs/{admin_run_id}"), Some(&alice)).await;
    assert_eq!(status, 404, "alice must get 404 on admin's run (no leak)");

    // ── 5. Cancel flow (SIGTERM-immune rule exercises the escalation) ───
    let toml_slow = "[workflow]\nname = \"slow\"\n\n[[rules]]\nname = \"stubborn\"\noutput = [\"later.txt\"]\nshell = \"trap '' TERM; sleep 60; echo done > {output}\"\n";
    let (status, pipeline_slow) = post(
        &base,
        "/api/pipelines",
        Some(&alice),
        json!({"toml_content": toml_slow}),
    )
    .await;
    assert_eq!(status, 200, "slow pipeline: {pipeline_slow}");
    let (status, run_slow) = post(
        &base,
        "/api/runs",
        Some(&alice),
        json!({"toml_content": toml_slow}),
    )
    .await;
    assert_eq!(status, 200);
    let slow_run = run_slow["run_id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_secs(3)).await; // let it start
    let (code, _) = post(
        &base,
        &format!("/api/runs/{slow_run}/cancel"),
        Some(&alice),
        Value::Null,
    )
    .await;
    assert_eq!(code, 200, "cancel must be accepted: {code}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut terminal = String::new();
    while Instant::now() < deadline {
        let (_, body) = get(&base, &format!("/api/runs/{slow_run}"), Some(&alice)).await;
        terminal = body["status"].as_str().unwrap_or("").to_string();
        if terminal == "cancelled" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(
        terminal,
        "cancelled",
        "SIGTERM-immune rule must still be cancelled: {}",
        log_tail(&server)
    );

    // ── 6. Share flow: anonymous read, then ownership persists ─────────
    let (status, share) = post(
        &base,
        &format!("/api/pipelines/{pipeline_id}/share"),
        Some(&alice),
        json!({"visibility": "public", "expires_in_days": 30}),
    )
    .await;
    assert_eq!(status, 200, "share: {share}");
    let token = share["access_token"].as_str().unwrap().to_string();
    let (status, shared) = get(&base, &format!("/api/share/{token}"), None).await;
    assert_eq!(
        status, 200,
        "shared pipeline must be anonymously readable: {shared}"
    );

    // ── 7. SQLite restart persistence ───────────────────────────────────
    server.child.kill().unwrap();
    let _ = server.child.wait();
    let port2 = free_port();
    let mut server2 = spawn_server(
        dir.path(),
        port2,
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
        &[],
    );
    wait_ready(&server2.base).await;
    let base2 = server2.base.clone();
    // Admin still logs in (bcrypt users persisted).
    let (status, login) = post(
        &base2,
        "/api/auth/login",
        None,
        json!({"username": "alice", "password": "alice-secret"}),
    )
    .await;
    assert_eq!(status, 200, "alice must survive restart: {login}");
    let alice2 = login["token"].as_str().unwrap().to_string();
    // Pipeline + runs persisted.
    let (status, body) = get(&base2, "/api/pipelines", Some(&alice2)).await;
    assert_eq!(status, 200, "pipelines after restart: {body}");
    assert!(
        body.as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == pipeline_id),
        "pipeline must survive restart"
    );
    let (status, _) = get(&base2, &format!("/api/runs/{run_id}"), Some(&alice2)).await;
    assert_eq!(status, 200, "run must survive restart");
    server2.child.kill().unwrap();
    let _ = server2.child.wait();
}

/// --base-path deployment: endpoints under the prefix, unprefixed 404.
#[tokio::test]
async fn web_base_path_prefixes_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = spawn_server(
        dir.path(),
        port,
        &[("OXO_FLOW_MODE", "personal")],
        &["--base-path", "/oxo-flow"],
    );
    let base = format!("http://127.0.0.1:{port}");
    wait_ready(&base).await;
    let client = Client::new();
    let (status, _) = client
        .get(format!("{base}/oxo-flow/api/health"))
        .send()
        .await
        .map(|r| (r.status(), r.json::<Value>()))
        .unwrap();
    assert_eq!(status, 200, "health must be under the base path");
    let status = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404, "unprefixed path must not be served");
    server.child.kill().unwrap();
    let _ = server.child.wait();
}
