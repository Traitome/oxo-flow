//! SSE terminal-broadcast verification under load + crash recovery
//! (issue #67 §4).
//!
//! Drives the REAL `oxo-flow-web` binary:
//!  - load: 8 concurrent runs (success + failure mix) must each deliver
//!    their terminal SSE event exactly once to a connected /api/events
//!    stream, with the correct type per outcome;
//!  - crash: a server killed mid-run must not lose the terminal
//!    broadcast — after restart, the startup re-attach path
//!    (resume_monitoring) finalizes the orphaned run and broadcasts.
//!
//! Single sequential test to avoid parallel-server flakiness.

use reqwest::Client;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn workspace_bin(name: &str) -> PathBuf {
    let target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    for candidate in [
        target_dir.join(name),
        target_dir.join("deps").join(name),
        target_dir.join(format!("{name}.exe")),
    ] {
        if candidate.exists() {
            return candidate;
        }
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

async fn post_run(base: &str, toml: &str) -> String {
    let client = Client::new();
    let resp = client
        .post(format!("{base}/api/runs"))
        .json(&json!({"toml_content": toml}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "run create failed");
    resp.json::<Value>().await.unwrap()["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Parse SSE frames ("data: {json}\n\n") into (type, run_id) pairs.
fn parse_sse_frame(frame: &str) -> Option<(String, String)> {
    let line = frame.lines().find(|l| l.starts_with("data:"))?;
    let json: Value = serde_json::from_str(line.trim_start_matches("data:").trim()).ok()?;
    let event_type = json.get("type")?.as_str()?.to_string();
    let run_id = json
        .get("data")?
        .get("run_id")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    Some((event_type, run_id))
}

/// Open the SSE stream and pipe terminal events into the channel.
fn spawn_sse_reader(
    base: String,
    tx: mpsc::Sender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = Client::new();
        let resp = client
            .get(format!("{base}/api/events"))
            .send()
            .await
            .expect("SSE stream must connect");
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        use tokio_stream::StreamExt;
        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else { break };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(pos) = buf.find("\n\n") {
                let frame: String = buf.drain(..pos + 2).collect();
                if let Some((event_type, run_id)) = parse_sse_frame(&frame)
                    && (event_type == "run_completed" || event_type == "run_failed")
                    && tx.send((event_type, run_id)).await.is_err()
                {
                    return; // test side dropped the receiver
                }
            }
        }
    })
}

const OK_RUN: &str = "[workflow]\nname = \"ok\"\n\n[[rules]]\nname = \"do\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output}\"\n";
const BAD_RUN: &str = "[workflow]\nname = \"bad\"\n\n[[rules]]\nname = \"fail\"\noutput = [\"out2.txt\"]\nshell = \"exit 1\"\n";

/// 8 concurrent runs (6 success + 2 failure): every run's terminal event
/// must arrive exactly once with the correct type.
#[tokio::test]
async fn sse_terminal_events_under_load() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = spawn_server(
        dir.path(),
        port,
        &[
            ("OXO_FLOW_MODE", "personal"),
            // This test posts 8 runs back-to-back to exercise SSE fan-out;
            // exempt it from the dedicated run-creation rate limit (#213).
            ("OXO_FLOW_RUNS_RATE_LIMIT", "0"),
        ],
    );
    wait_ready(&server.base).await;

    let (tx, mut rx) = mpsc::channel::<(String, String)>(64);
    let reader = spawn_sse_reader(server.base.clone(), tx);

    // Let the stream connect before the runs fire.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut expected = std::collections::HashMap::new();
    for i in 0..8 {
        let toml = if i < 6 { OK_RUN } else { BAD_RUN };
        let run_id = post_run(&server.base, toml).await;
        expected.insert(run_id, if i < 6 { "run_completed" } else { "run_failed" });
    }

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = std::collections::HashMap::new();
    while seen.len() < expected.len() {
        assert!(
            Instant::now() < deadline,
            "only {}/{} terminal events arrived: {seen:?}\n{}",
            seen.len(),
            expected.len(),
            log_tail(&server)
        );
        let Ok(Some((event_type, run_id))) =
            tokio::time::timeout(Duration::from_secs(30), rx.recv()).await
        else {
            continue;
        };
        let Some(want) = expected.get(&run_id) else {
            continue; // foreign event (startup scans etc.) — ignore
        };
        assert_eq!(
            &event_type, want,
            "run {run_id} broadcast {event_type}, expected {want}"
        );
        // Exactly-once: duplicates would panic on re-insert.
        assert!(
            seen.insert(run_id.clone(), event_type).is_none(),
            "run {run_id} broadcast twice"
        );
    }
    reader.abort();
    server.child.kill().unwrap();
    let _ = server.child.wait();
}

/// A server killed mid-run must not lose the terminal broadcast: after
/// restart the orphaned run is re-attached and finalizes with a broadcast.
#[tokio::test]
async fn sse_terminal_event_survives_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = spawn_server(dir.path(), port, &[("OXO_FLOW_MODE", "personal")]);
    wait_ready(&server.base).await;

    let slow_toml = "[workflow]\nname = \"slow\"\n\n[[rules]]\nname = \"wait\"\noutput = [\"later.txt\"]\nshell = \"sleep 25; echo ok > {output}\"\n";
    let run_id = post_run(&server.base, slow_toml).await;
    tokio::time::sleep(Duration::from_secs(3)).await; // let the CLI child start

    // Crash the server while the run executes (the CLI survives in its
    // own process group).
    server.child.kill().unwrap();
    let _ = server.child.wait();

    // Restart on the same working dir: startup re-attach (db.rs) must
    // resume monitoring the executing run.
    let port2 = free_port();
    let mut server2 = spawn_server(dir.path(), port2, &[("OXO_FLOW_MODE", "personal")]);
    wait_ready(&server2.base).await;

    let (tx, mut rx) = mpsc::channel::<(String, String)>(16);
    let reader = spawn_sse_reader(server2.base.clone(), tx);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        assert!(
            Instant::now() < deadline,
            "terminal event for {run_id} never arrived after restart\n{}",
            log_tail(&server2)
        );
        let Ok(Some((event_type, got_run))) =
            tokio::time::timeout(Duration::from_secs(30), rx.recv()).await
        else {
            continue;
        };
        if got_run != run_id {
            continue;
        }
        assert_eq!(event_type, "run_completed", "restarted run outcome");
        break;
    }
    reader.abort();
    server2.child.kill().unwrap();
    let _ = server2.child.wait();
}
