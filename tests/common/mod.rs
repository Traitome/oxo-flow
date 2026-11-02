//! Shared integration-test helpers (issue #553): one canonical
//! `workspace_bin` (current_exe-derived — release/`CARGO_TARGET_DIR`
//! safe), `free_port`, and a web-server spawner whose bind is proven by
//! the child's own "Listening on" log line (a foreign process squatting
//! on the port cannot be mistaken for ours). The spawner retries on a
//! fresh port when the child dies before binding, and keeps waiting on a
//! still-alive child past the per-attempt deadline (#958 slow-start
//! under parallel load). Include with
//! `mod common;` (path-relative) or `#[path = "common/mod.rs"] mod common;`.

#![allow(dead_code)] // per-binary: unused helpers are expected

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn workspace_bin(name: &str) -> PathBuf {
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

/// An `oxo-flow` command builder (assert_cmd): the canonical way every
/// integration test spawns the CLI. Tests needing raw `std::process::Command`
/// (stdio piping, abort signalling) construct it from `workspace_bin`.
pub fn oxo_flow_cmd() -> assert_cmd::Command {
    assert_cmd::Command::new(workspace_bin("oxo-flow"))
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ─── Shared HTTP helpers for web-server tests ────────────────────────────

/// Read the response body as JSON, failing loudly when the server did not
/// answer with JSON (e.g. an HTML 500 error page or an empty body). The
/// per-file copies this replaces swallowed parse failures into
/// `Value::Null`, turning a non-JSON response into confusing downstream
/// assertion failures (or a silent 120 s poll timeout).
async fn read_json(
    status: reqwest::StatusCode,
    resp: reqwest::Response,
    context: &str,
) -> (reqwest::StatusCode, serde_json::Value) {
    let text = resp.text().await.unwrap_or_default();
    match serde_json::from_str(&text) {
        Ok(value) => (status, value),
        Err(e) => panic!("{context}: non-JSON response ({e}); status {status}; body: {text}"),
    }
}

/// One authenticated JSON API round-trip for any method — the single
/// definition the per-file `req`/`post`/`get`/`put` copies delegate to.
pub async fn api_req(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (reqwest::StatusCode, serde_json::Value) {
    let client = reqwest::Client::new();
    let mut r = match method {
        "POST" => client.post(format!("{base}{path}")),
        "PUT" => client.put(format!("{base}{path}")),
        "GET" => client.get(format!("{base}{path}")),
        "DELETE" => client.delete(format!("{base}{path}")),
        other => panic!("unsupported method: {other}"),
    };
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    let context = format!("{method} {base}{path}");
    let resp = r.send().await.unwrap_or_else(|e| panic!("{context}: {e}"));
    read_json(resp.status(), resp, &context).await
}

pub async fn api_get(
    base: &str,
    path: &str,
    token: Option<&str>,
) -> (reqwest::StatusCode, serde_json::Value) {
    api_req(base, "GET", path, token, None).await
}

pub async fn api_post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> (reqwest::StatusCode, serde_json::Value) {
    api_req(base, "POST", path, token, Some(body)).await
}

pub async fn api_put(
    base: &str,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (reqwest::StatusCode, serde_json::Value) {
    api_req(base, "PUT", path, Some(token), Some(body)).await
}

/// Poll `/api/health` until the server answers, then return. The panic
/// carries the log tail so a timeout is diagnosable (bind failure vs.
/// slow start vs. wrong port). Tests that need blocking semantics
/// (web_sse_shutdown) keep their own sync copy — this one is async.
pub async fn wait_ready(base: &str, log_path: &std::path::Path) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            Instant::now() < deadline,
            "server did not become ready at {base}\n{}",
            std::fs::read_to_string(log_path).unwrap_or_else(|_| "(no server log)".into())
        );
        if client.get(format!("{base}/api/health")).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub struct WebServer {
    pub child: Child,
    pub base: String,
    pub log_path: PathBuf,
}

impl Drop for WebServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_web_server_once(
    dir: &std::path::Path,
    port: u16,
    extra_envs: &[(&str, &str)],
    extra_args: &[&str],
) -> WebServer {
    let log_path = dir.join("web-server.log");
    let log_file = std::fs::File::create(&log_path).expect("create server log");
    let mut cmd = Command::new(workspace_bin("oxo-flow-web"));
    cmd.current_dir(dir)
        .env("OXO_FLOW_BIN", workspace_bin("oxo-flow"))
        .env("OXO_FLOW_HOST", "127.0.0.1")
        .env("OXO_FLOW_PORT", port.to_string())
        // The bind wait greps the log for the INFO-level "Listening on"
        // line — a dev shell exporting RUST_LOG=warn filters it out and
        // stalls the suite (same pin as oxo_flow_cmd_without_rust_log in
        // cli_integration.rs).
        .env_remove("RUST_LOG")
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
    WebServer {
        child,
        base: format!("http://127.0.0.1:{port}"),
        log_path,
    }
}

pub fn log_tail(s: &WebServer) -> String {
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

fn wait_for_bind(server: &mut WebServer, timeout: Duration) -> bool {
    // Match on the port only: the bind host string in the log line can
    // differ from the probe (`--host localhost` resolves to `[::1]`, not
    // `127.0.0.1`), and the port is what uniquely identifies this child —
    // other tests may be logging their own binds concurrently.
    let port = server
        .base
        .trim_start_matches("http://127.0.0.1:")
        .to_string();
    let needle = format!(":{port}");
    let deadline = Instant::now() + timeout;
    loop {
        if std::fs::read_to_string(&server.log_path)
            .is_ok_and(|log| log.contains("Listening on http://") && log.contains(&needle))
        {
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

/// Per-attempt bind deadline for the retry loop below. Generous enough for
/// slow-start under parallel test load (cold binary page-in, DB init, the
/// AI-provider auto-detection in `oxo-flow-web`'s startup path — all of
/// which happen BEFORE the "Listening on" line): #958 observed 15 s
/// abandoning alive-but-slow children, and each retry then started another
/// cold child with the same slow start, so all 5 attempts failed identically.
///
/// `pub` so the mirrored loop in `web_integration.rs` (`TestServer`) imports
/// it instead of retyping a constant — reuse-never-retype.
pub const BIND_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Spawn the web server, retrying on a fresh port when the child dies
/// before binding. `free_port` releases its probe listener before the
/// child binds, so a parallel test can steal the port (TOCTOU) and the
/// child then exits on the bind error.
///
/// A child that is still ALIVE at deadline expiry is a slow-start case,
/// not a failure: dropping it and starting a fresh cold child reproduces
/// the same slow start (issue #958), so the same child keeps waiting until
/// the overall deadline instead. Only an exited child triggers the
/// fresh-port retry (bind theft, config error).
pub fn spawn_web_server(
    dir: &std::path::Path,
    port: u16,
    extra_envs: &[(&str, &str)],
) -> WebServer {
    spawn_web_server_with_args(dir, port, extra_envs, &[])
}

/// Variant passing extra CLI arguments to `oxo-flow-web` (e.g. --base-path).
pub fn spawn_web_server_with_args(
    dir: &std::path::Path,
    port: u16,
    extra_envs: &[(&str, &str)],
    extra_args: &[&str],
) -> WebServer {
    // Total budget across all attempts; per-attempt waits stay under it.
    let overall_deadline = Instant::now() + 3 * BIND_WAIT_TIMEOUT;
    let mut last_log = String::new();
    for attempt in 1..=5 {
        let port = if attempt == 1 { port } else { free_port() };
        let mut server = spawn_web_server_once(dir, port, extra_envs, extra_args);
        loop {
            if wait_for_bind(&mut server, BIND_WAIT_TIMEOUT) {
                return server;
            }
            if matches!(server.child.try_wait(), Ok(Some(_))) {
                // Exited without binding: retry on a fresh port.
                break;
            }
            // Still alive past the per-attempt deadline — slow start under
            // load, not a bind failure. Keep waiting on THIS child; killing
            // it and respawning would just restart the cold-start cost.
            if Instant::now() > overall_deadline {
                panic!(
                    "web server did not bind within the overall deadline \
                     (alive but slow after {}s)\n{}",
                    3 * BIND_WAIT_TIMEOUT.as_secs(),
                    log_tail(&server)
                );
            }
        }
        last_log = log_tail(&server);
    }
    panic!("web server could not bind a free port after 5 attempts\n{last_log}");
}
