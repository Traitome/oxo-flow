//! Production-scale SSE stress profile (issue #363).
//!
//! Goes beyond `web_sse_load.rs` (8 sequential runs, one reader): this
//! profile drives 64 concurrent runs in two waves against the REAL
//! `oxo-flow-web` binary while 8 long-lived `/api/events` subscribers and
//! one deliberately non-reading (slow) consumer hold their streams, then
//! asserts:
//!  - every subscriber receives every terminal event exactly once, with
//!    the correct type per outcome;
//!  - the slow consumer does not stall or starve delivery to the
//!    reading subscribers;
//!  - a latency profile (POST completion → terminal event on the
//!    measuring reader) is printed with `--nocapture`.

use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
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
    let log_path = dir.join(format!("server-{port}.log"));
    let mut cmd = Command::new(workspace_bin("oxo-flow-web"));
    cmd.env("OXO_FLOW_PORT", port.to_string())
        .stdout(Stdio::from(std::fs::File::create(&log_path).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&log_path).unwrap()));
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("failed to spawn web server");
    Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
        log_path,
    }
}

fn log_tail(server: &Server) -> String {
    std::fs::read_to_string(&server.log_path).unwrap_or_default()
}

fn parse_sse_frame(frame: &str) -> Option<(String, String)> {
    let line = frame.lines().find(|l| l.starts_with("data:"))?;
    let json: Value = serde_json::from_str(line.trim_start_matches("data:").trim()).ok()?;
    let event_type = json.get("type")?.as_str()?.to_string();
    let run_id = json.get("data")?.get("run_id")?.as_str()?.to_string();
    Some((event_type, run_id))
}

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
                    return;
                }
            }
        }
    })
}

/// A subscriber that connects and then never reads — holds the stream to
/// exercise server-side backpressure while other subscribers receive.
fn spawn_slow_consumer(base: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = Client::builder().build().unwrap();
        if let Ok(resp) = client.get(format!("{base}/api/events")).send().await {
            // Hold the response without consuming the body for the whole test.
            tokio::time::sleep(Duration::from_secs(240)).await;
            drop(resp);
        }
    })
}

async fn wait_ready(base: &str) {
    let client = Client::new();
    for _ in 0..60 {
        if client
            .get(format!("{base}/api/health"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("server never became ready");
}

async fn post_run(client: &Client, base: &str, toml: &str) -> (String, Instant) {
    let resp = client
        .post(format!("{base}/api/runs"))
        .json(&json!({"toml_content": toml}))
        .send()
        .await
        .expect("run create request failed");
    assert_eq!(resp.status(), 200, "run create failed");
    let run_id = resp.json::<Value>().await.unwrap()["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    (run_id, Instant::now())
}

const OK_RUN: &str = "[workflow]\nname = \"ok\"\n\n[[rules]]\nname = \"do\"\noutput = [\"out.txt\"]\nshell = \"sleep 2; echo done > {output}\"\n";
const BAD_RUN: &str = "[workflow]\nname = \"bad\"\n\n[[rules]]\nname = \"fail\"\noutput = [\"out2.txt\"]\nshell = \"exit 1\"\n";

const N_RUNS: usize = 64;
const N_READERS: usize = 8;

/// 64 concurrent runs in two waves, 8 subscribers + 1 slow consumer:
/// exactly-once delivery per subscriber, correct types, latency profile.
#[tokio::test]
async fn sse_production_scale_stress_profile() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = spawn_server(
        dir.path(),
        port,
        &[
            ("OXO_FLOW_MODE", "personal"),
            ("OXO_FLOW_RUNS_RATE_LIMIT", "0"),
            // 32 concurrent POSTs trip the general API rate limiter too.
            ("OXO_FLOW_DISABLE_RATE_LIMIT", "1"),
        ],
    );
    wait_ready(&server.base).await;

    // Raise the concurrent-run quota (personal mode allows admin ops
    // without auth): the default tier caps at 10 concurrent runs, which
    // would reject a 64-run burst with QUOTA_EXCEEDED before the SSE
    // fan-out is even exercised. Going through the admin API (rather
    // than an env var) also covers the quota control plane.
    let admin = Client::new();
    let resp = admin
        .put(format!("{}/api/quota", server.base))
        .json(&json!({
            "max_concurrent_runs": 128,
            "max_total_threads": 512,
            "max_total_memory_mb": 262144,
            "max_runs_per_day": 1000
        }))
        .send()
        .await
        .expect("quota update request failed");
    assert_eq!(resp.status(), 200, "quota update failed");

    // 8 reading subscribers + 1 slow consumer, all connected up front.
    let mut readers = Vec::new();
    for _ in 0..N_READERS {
        let (tx, rx) = mpsc::channel::<(String, String)>(256);
        readers.push((spawn_sse_reader(server.base.clone(), tx), rx));
    }
    let slow = spawn_slow_consumer(server.base.clone());
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Two waves of 32 runs posted concurrently; wave 2 fires while wave 1
    // is still executing (each OK run sleeps 2s). Every 4th run fails.
    let client = Client::new();
    let mut expected: HashMap<String, &'static str> = HashMap::new();
    let mut posted_at: HashMap<String, Instant> = HashMap::new();
    for wave in 0..2 {
        let mut tasks = Vec::new();
        for i in 0..32 {
            let idx = wave * 32 + i;
            let is_fail = idx % 4 == 3;
            let toml = if is_fail { BAD_RUN } else { OK_RUN };
            tasks.push(tokio::spawn({
                let client = client.clone();
                let base = server.base.clone();
                let toml = toml.to_string();
                async move { post_run(&client, &base, &toml).await }
            }));
        }
        for t in tasks {
            let (run_id, at) = t.await.unwrap();
            let idx = expected.len();
            let want = if idx % 4 == 3 {
                "run_failed"
            } else {
                "run_completed"
            };
            expected.insert(run_id.clone(), want);
            posted_at.insert(run_id, at);
        }
        // Give wave 1 a head start so the two waves genuinely overlap.
        if wave == 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    assert_eq!(expected.len(), N_RUNS);

    // Every reader drains until it has seen all 64 terminal events.
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut per_reader: Vec<HashMap<String, String>> = vec![HashMap::new(); N_READERS];
    let mut latencies: Vec<Duration> = Vec::new();
    let mut events_left = N_READERS * N_RUNS;
    while events_left > 0 {
        assert!(
            Instant::now() < deadline,
            "short by {events_left} of {} total terminal events\n{}",
            N_READERS * N_RUNS,
            log_tail(&server)
        );
        // Round-robin over readers with a short timeout each so one silent
        // stream cannot mask another's progress.
        for (ri, (_, rx)) in readers.iter_mut().enumerate() {
            if per_reader[ri].len() >= N_RUNS {
                continue;
            }
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some((event_type, run_id))) => {
                    let Some(want) = expected.get(&run_id) else {
                        continue; // foreign event (startup scans etc.)
                    };
                    assert_eq!(&event_type, want, "run {run_id}: {event_type} != {want}");
                    assert!(
                        per_reader[ri].insert(run_id.clone(), event_type).is_none(),
                        "reader {ri}: run {run_id} broadcast twice"
                    );
                    if ri == 0
                        && let Some(t0) = posted_at.get(&run_id)
                    {
                        latencies.push(t0.elapsed());
                    }
                    events_left -= 1;
                }
                Ok(None) => panic!("reader {ri}: stream ended prematurely"),
                Err(_) => {} // timeout — move to the next reader
            }
        }
    }

    for (ri, seen) in per_reader.iter().enumerate() {
        assert_eq!(
            seen.len(),
            N_RUNS,
            "reader {ri} received {}/{} terminal events\n{}",
            seen.len(),
            N_RUNS,
            log_tail(&server)
        );
    }

    // Latency profile (measuring reader #0): POST-completion → terminal event.
    latencies.sort();
    let pick = |q: f64| latencies[((latencies.len() - 1) as f64 * q) as usize];
    println!(
        "SSE stress profile: {} runs x {} readers, exactly-once OK; latency POST→event (reader#0): p50 {:?} / p95 {:?} / max {:?} (n={})",
        N_RUNS,
        N_READERS,
        pick(0.50),
        pick(0.95),
        latencies[latencies.len() - 1],
        latencies.len()
    );

    slow.abort();
    for (reader, _) in readers {
        reader.abort();
    }
    server.child.kill().unwrap();
    let _ = server.child.wait();
}
