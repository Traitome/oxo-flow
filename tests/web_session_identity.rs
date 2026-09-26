//! Session identity integrity (issue #516).
//!
//! The shared env passwords (OXO_FLOW_USER_PASSWORD / _VIEWER_PASSWORD)
//! must never mint or adopt a privileged identity, and role resolution must
//! key on the canonical users.id resolved at login — never on a
//! client-chosen username.

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
async fn env_passwords_cannot_mint_or_adopt_privileged_identities() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = spawn_server_retrying(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin-pw"),
            ("OXO_FLOW_USER_PASSWORD", "shared-user-pw"),
        ],
    );
    assert!(
        wait_for_bind(&mut server, Duration::from_secs(15)),
        "server bind\n{}",
        log_tail(&server)
    );
    let base = server.base.clone();

    // The shared USER password must not sign in as "admin" (Path A of #516).
    let (status, body) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(status, 401, "user password must not mint admin: {body}");

    // The admin password still signs in as admin, and the session's role
    // resolves to admin via its canonical row.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin-pw"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    assert_eq!(login["role"], "admin");
    let admin = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&admin)).await;
    assert_eq!(status, 200);
    assert_eq!(me["role"], "admin", "admin /me: {me}");
    assert_eq!(me["username"], "admin", "admin /me username: {me}");
    let (status, _) = get(&base, "/api/users", Some(&admin)).await;
    assert_eq!(
        status, 200,
        "admin surface reachable with new-style session"
    );

    // An env-password identity logs in under its own name with role user.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "worker1", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(status, 200, "env user login: {login}");
    assert_eq!(login["role"], "user");
    let worker = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&worker)).await;
    assert_eq!(status, 200);
    assert_eq!(me["username"], "worker1", "env identity /me: {me}");
    assert_eq!(me["role"], "user", "env identity /me role: {me}");
    let (status, _) = get(&base, "/api/users", Some(&worker)).await;
    assert_eq!(status, 403, "env identity must stay non-admin");

    // A managed (UUID-keyed) admin account must not be adoptable through
    // the shared user password either — the name is not enough.
    let (status, body) = post(
        &base,
        "/api/users",
        Some(&admin),
        json!({"username": "boss", "password": "boss-own-pw", "role": "admin"}),
    )
    .await;
    assert_eq!(status, 200, "create managed admin: {body}");
    let (status, body) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "boss", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(
        status, 401,
        "shared user password must not adopt managed account: {body}"
    );
    // The account's own password still works and yields the admin role.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "boss", "password": "boss-own-pw"}),
    )
    .await;
    assert_eq!(status, 200, "managed admin own login: {login}");
    assert_eq!(login["role"], "admin");
    let boss = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&boss)).await;
    assert_eq!(status, 200);
    assert_eq!(me["username"], "boss");
    assert_eq!(me["role"], "admin");
}
