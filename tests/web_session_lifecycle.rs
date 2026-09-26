//! Session lifecycle: logout revocation + user-deletion cascade (#520).
//!
//! Sessions used to be irrevocable until expiry, and deleting a user left
//! their sessions and API keys authenticating for up to 24 hours.

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
        "DELETE" => client.delete(format!("{base}{path}")),
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
async fn logout_revokes_and_user_deletion_cascades() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_server_retrying(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    let base = server.base.clone();

    // Login → session valid.
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
    let (status, _) = req(&base, "GET", "/api/auth/me", Some(&admin), None).await;
    assert_eq!(status, 200, "fresh session must authenticate");

    // Logout revokes the presented session server-side.
    let (status, body) = req(
        &base,
        "POST",
        "/api/auth/logout",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "logout: {body}");
    assert_eq!(body["logged_out"], true, "{body}");
    let (status, me) = req(&base, "GET", "/api/auth/me", Some(&admin), None).await;
    assert_eq!(
        status, 200,
        "/me answers 200 with authenticated=false: {me}"
    );
    assert_eq!(
        me["authenticated"], false,
        "revoked token must not authenticate: {me}"
    );

    // Re-login for the rest of the flow.
    let (_, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "admin", "password": "secret-admin"})),
    )
    .await;
    let admin = login["token"].as_str().unwrap().to_string();

    // User-deletion cascade: the fired user's session dies with the row.
    let (status, body) = req(
        &base,
        "POST",
        "/api/users",
        Some(&admin),
        Some(json!({"username": "shortlived", "password": "short-pw"})),
    )
    .await;
    assert_eq!(status, 200, "create user: {body}");
    let user_id = body["id"].as_str().unwrap().to_string();
    let (_, login) = req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "shortlived", "password": "short-pw"})),
    )
    .await;
    let fired = login["token"].as_str().unwrap().to_string();
    let (status, _) = req(&base, "GET", "/api/auth/me", Some(&fired), None).await;
    assert_eq!(status, 200);

    let (status, _) = req(
        &base,
        "DELETE",
        &format!("/api/users/{user_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete user");
    let (_, me) = req(&base, "GET", "/api/auth/me", Some(&fired), None).await;
    assert_eq!(
        me["authenticated"], false,
        "deleted user's session must die: {me}"
    );
}
