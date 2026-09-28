//! `oxo-flow serve` entrypoint coverage (issue #672): the desktop .deb/
//! .rpm/AppImage launch path (`Exec=oxo-flow serve --open`) previously had
//! zero tests. These drive the REAL main binary with `serve` — not the
//! standalone `oxo-flow-web` binary that `common::spawn_web_server` targets.
//!
//! The bind is proven by polling `GET /api/health`: the serve path emits
//! no post-bind log line, so the log-matching `wait_for_bind` helper
//! cannot work here (both serve log lines are pre-bind).

mod common;

use common::{free_port, workspace_bin};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ServeProcess {
    child: Child,
    base: String,
    log_path: PathBuf,
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve_log_tail(s: &ServeProcess) -> String {
    match std::fs::read(&s.log_path) {
        Ok(bytes) => {
            let start = bytes.len().saturating_sub(4096);
            String::from_utf8_lossy(&bytes[start..]).into_owned()
        }
        Err(_) => "(no serve log)".to_string(),
    }
}

fn spawn_serve_once(dir: &Path, port: u16, base_path: &str) -> ServeProcess {
    let log_path = dir.join("serve.log");
    let log_file = std::fs::File::create(&log_path).expect("create serve log");
    let child = Command::new(workspace_bin("oxo-flow"))
        .current_dir(dir)
        .env("OXO_FLOW_DISABLE_RATE_LIMIT", "1")
        // A developer shell may export OXO_FLOW_OPEN_BROWSER=1; without
        // this pin every test run would pop a browser window.
        .env("OXO_FLOW_OPEN_BROWSER", "false")
        .env(
            "OXO_FLOW_FRONTEND_DIR",
            dir.join("missing-frontend").to_str().unwrap(),
        )
        .args([
            "serve",
            "--mode",
            "personal",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--base-path",
            base_path,
        ])
        .stdout(Stdio::from(log_file.try_clone().unwrap()))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("serve must start");
    ServeProcess {
        child,
        base: format!("http://127.0.0.1:{port}"),
        log_path,
    }
}

/// Poll `GET {path}/api/health` until the listener answers. Returns false
/// if the child exited first (bad argument, port steal lost) or the
/// deadline hit.
fn wait_ready(server: &mut ServeProcess, base_path: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(resp) = reqwest::blocking::get(format!("{}{base_path}/api/health", server.base))
            && resp.status().is_success()
        {
            return true;
        }
        if matches!(server.child.try_wait(), Ok(Some(_))) {
            return false;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Spawn `serve` on a fresh port, retrying when the child dies before
/// binding (free_port releases its probe before the child binds, so a
/// parallel test can steal the port — same TOCTOU window as
/// `common::spawn_web_server`). `base_path` is the ALREADY normalized
/// prefix (`""` or `/name`) used in the readiness probe.
fn spawn_serve(dir: &Path, base_path: &str) -> ServeProcess {
    let normalized = if base_path == "/" {
        String::new()
    } else {
        format!("/{}", base_path.trim_matches('/'))
    };
    let mut last_log = String::new();
    for _ in 0..5 {
        let mut server = spawn_serve_once(dir, free_port(), base_path);
        if wait_ready(&mut server, &normalized, Duration::from_secs(15)) {
            return server;
        }
        last_log = serve_log_tail(&server);
    }
    panic!("`oxo-flow serve` could not bind a free port after 5 attempts\n{last_log}");
}

/// Personal-mode serve on the main binary: bind + `GET /api/health`.
#[test]
fn serve_binds_and_serves_health() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_serve(dir.path(), "/");
    let resp = reqwest::blocking::get(format!("{}/api/health", server.base)).unwrap();
    assert!(
        resp.status().is_success(),
        "health must be served by `oxo-flow serve`\n{}",
        serve_log_tail(&server)
    );
}

/// `--base-path oxo-flow` — raw, no leading slash, the exact input the
/// CLI must normalize before axum's nest() sees it. Endpoints live under
/// the prefix; the unprefixed path must 404.
#[test]
fn serve_base_path_is_normalized_and_prefixed() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_serve(dir.path(), "oxo-flow");
    let client = reqwest::blocking::Client::new();
    let status = client
        .get(format!("{}/oxo-flow/api/health", server.base))
        .send()
        .unwrap()
        .status();
    assert_eq!(
        status,
        200,
        "health must be served under the base path\n{}",
        serve_log_tail(&server)
    );
    let status = client
        .get(format!("{}/api/health", server.base))
        .send()
        .unwrap()
        .status();
    assert_eq!(
        status,
        404,
        "unprefixed path must not be served\n{}",
        serve_log_tail(&server)
    );
}
