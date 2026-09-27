//! Shared integration-test helpers (issue #553): one canonical
//! `workspace_bin` (current_exe-derived — release/`CARGO_TARGET_DIR`
//! safe), `free_port`, and a web-server spawner whose bind is proven by
//! the child's own "Listening on" log line (a foreign process squatting
//! on the port cannot be mistaken for ours), with a retry on a fresh
//! port when the child dies before binding. Include with
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

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

/// Spawn the web server, retrying on a fresh port when the child dies
/// before binding. `free_port` releases its probe listener before the
/// child binds, so a parallel test can steal the port (TOCTOU) and the
/// child then exits on the bind error.
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
    let mut last_log = String::new();
    for attempt in 1..=5 {
        let port = if attempt == 1 { port } else { free_port() };
        let mut server = spawn_web_server_once(dir, port, extra_envs, extra_args);
        if wait_for_bind(&mut server, Duration::from_secs(15)) {
            return server;
        }
        last_log = log_tail(&server);
    }
    panic!("web server could not bind a free port after 5 attempts\n{last_log}");
}
