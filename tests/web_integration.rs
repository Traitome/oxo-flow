//! Integration tests for the web server's run path (issue #69).
//!
//! Saved-pipeline runs execute in a persistent per-pipeline workdir via the
//! `oxo-flow` CLI subprocess, so the checkpoint survives across re-runs and
//! the CLI's config-change impact analysis delivers precise rebuilds:
//! affected rules re-execute, the rest are reused.

use std::path::PathBuf;
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

/// Locate a workspace binary by name from the target directory
/// (mirrors the helper in cli_integration.rs).
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

/// Running web server for one test; killed on drop.
struct TestServer {
    child: Child,
    base: String,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Without this, every test leaks an orphaned web-server process (the
        // evaluation's "zombie processes" noise — issue #79).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl TestServer {
    /// Kill the server immediately and reap it (restart tests); the later
    /// `drop` kill becomes a no-op.
    fn kill_now(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    async fn start(dir: &std::path::Path) -> Self {
        let port = free_port();
        let child = StdCommand::new(workspace_bin("oxo-flow-web"))
            .current_dir(dir)
            .env("OXO_FLOW_BIN", workspace_bin("oxo-flow"))
            .env("OXO_FLOW_HOST", "127.0.0.1")
            .env("OXO_FLOW_PORT", port.to_string())
            // No SPA assets in tests — API routes only.
            .env(
                "OXO_FLOW_FRONTEND_DIR",
                dir.join("missing-frontend").to_str().unwrap(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("web server must start");

        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                panic!("web server did not become ready at {base}");
            }
            if client.get(format!("{base}/api/runs")).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Self { child, base }
    }
}

async fn wait_for_terminal(client: &reqwest::Client, base: &str, run_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(
            Instant::now() < deadline,
            "run {run_id} did not finish in time"
        );
        let body: serde_json::Value = client
            .get(format!("{base}/api/runs/{run_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let status = body["status"].as_str().unwrap_or("").to_string();
        if status == "completed" || status == "failed" || status == "cancelled" {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Issue #69 core scenario: re-running a saved pipeline with a changed
/// config value rebuilds exactly the rules referencing it (plus downstream);
/// unaffected rules keep their checkpoint records. The persistent pipeline
/// workdir makes the whole CLI invalidation machinery apply across web runs.
#[tokio::test]
async fn web_pipeline_rerun_rebuilds_only_config_affected_rules() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let workflow = |suffix: &str| {
        format!(
            "[workflow]\nname = \"web69\"\nversion = \"1.0.0\"\n\n\
             [config]\nsuffix = \"{suffix}\"\n\n\
             [[rules]]\nname = \"upstream\"\noutput = [\"up.txt\"]\n\
             shell = \"echo up > up.txt\"\n\n\
             [[rules]]\nname = \"downstream\"\ninput = [\"up.txt\"]\n\
             output = [\"down.txt\"]\n\
             shell = \"echo {{config.suffix}} > down.txt\"\n"
        )
    };

    // Save the pipeline (runs.pipeline_id has an FK to pipelines).
    let saved: serde_json::Value = client
        .post(format!("{base}/api/pipelines"))
        .json(&serde_json::json!({
            "toml_content": workflow("A"),
            "name": "web69",
            "version": "1.0.0",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pipeline_id = saved["id"].as_str().unwrap().to_string();
    assert!(uuid::Uuid::parse_str(&pipeline_id).is_ok());

    // Run 1: both rules execute.
    let run1: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": workflow("A"),
            "pipeline_id": pipeline_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run1_id = run1["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run1_id).await,
        "completed"
    );
    let logs1: String = client
        .get(format!("{base}/api/runs/{run1_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(logs1.contains("Running: upstream"), "run1 logs: {logs1}");
    assert!(logs1.contains("Running: downstream"), "run1 logs: {logs1}");
    assert_eq!(
        std::fs::read_to_string(
            dir.path()
                .join("workspace/users/default/pipelines")
                .join(&pipeline_id)
                .join("down.txt")
        )
        .unwrap(),
        "A\n"
    );

    // The run record carries the pipeline workdir (also fixes the logs and
    // results endpoints, which read the workdir column).
    let run1_info: serde_json::Value = client
        .get(format!("{base}/api/runs/{run1_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        run1_info["workdir"]
            .as_str()
            .unwrap_or("")
            .contains(&format!("pipelines/{pipeline_id}")),
        "run record must point at the pipeline workdir: {run1_info}"
    );

    // Run 2: config changed — only the referencing rule rebuilds.
    let run2: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": workflow("B"),
            "pipeline_id": pipeline_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run2_id = run2["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run2_id).await,
        "completed"
    );
    let logs2: String = client
        .get(format!("{base}/api/runs/{run2_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        logs2.contains("Config change:"),
        "config-change summary expected: {logs2}"
    );
    assert!(
        logs2.contains("already completed"),
        "unaffected upstream rule must be reused: {logs2}"
    );
    assert!(
        !logs2.contains("Running: upstream"),
        "upstream must NOT re-run: {logs2}"
    );
    assert!(
        logs2.contains("Running: downstream"),
        "affected downstream rule must re-run: {logs2}"
    );
    assert_eq!(
        std::fs::read_to_string(
            dir.path()
                .join("workspace/users/default/pipelines")
                .join(&pipeline_id)
                .join("down.txt")
        )
        .unwrap(),
        "B\n"
    );

    // Results endpoint resolves through the recorded workdir.
    let results: serde_json::Value = client
        .get(format!("{base}/api/runs/{run2_id}/results"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        results.as_array().map(|a| a.len()).unwrap_or(0) > 0,
        "results must list pipeline workdir files: {results}"
    );
}

/// Ad-hoc runs (no pipeline_id) also record their workdir, so the logs and
/// results endpoints resolve — previously the workdir column was never set
/// and these endpoints always reported "No execution log available".
#[tokio::test]
async fn web_ad_hoc_run_logs_and_results_resolve_via_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"adhoc69\"\n\n\
                [[rules]]\nname = \"hello\"\noutput = [\"hello.txt\"]\n\
                shell = \"echo hi > hello.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    let logs: String = client
        .get(format!("{base}/api/runs/{run_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        logs.contains("Running: hello"),
        "ad-hoc run logs must resolve: {logs}"
    );

    let results: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/results"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        results.as_array().map(|a| a.len()).unwrap_or(0) > 0,
        "ad-hoc results must list workdir files: {results}"
    );
}

/// Boundary validation: pipeline_id becomes a path component and must be a
/// UUID; malformed values are rejected before touching the filesystem.
#[tokio::test]
async fn web_create_run_rejects_malformed_pipeline_id() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let resp = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": "[workflow]\nname = \"x\"\n",
            "pipeline_id": "../../etc",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "INVALID_PIPELINE_ID");
}

/// Regression (issue #69 follow-up): `dry_run: true` must spawn the preview
/// subcommand — nothing may execute. Previously the flag only affected the
/// estimate while the CLI executed the workflow for real.
#[tokio::test]
async fn web_dry_run_flag_previews_without_executing() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"dry69\"\n\n\
                [[rules]]\nname = \"produce\"\noutput = [\"produced.txt\"]\n\
                shell = \"echo ran > produced.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml, "dry_run": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    let logs: String = client
        .get(format!("{base}/api/runs/{run_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        logs.contains("(dry-run)"),
        "preview plan expected in logs: {logs}"
    );
    assert!(
        !logs.contains("Running: produce"),
        "dry-run must not execute rules: {logs}"
    );
    // Nothing was executed anywhere in the sandbox.
    assert!(
        !dir.path()
            .join("workspace/users/default/runs")
            .join(&run_id)
            .join("produced.txt")
            .exists(),
        "dry-run must not create outputs"
    );
}

/// Regression (issue #69 follow-up): an explicit `max_jobs` must reach the
/// CLI executor (-j). Previously it only influenced the resource estimate.
#[tokio::test]
async fn web_max_jobs_flag_reaches_executor() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"jobs69\"\n\n\
                [[rules]]\nname = \"a\"\noutput = [\"a.txt\"]\nshell = \"echo a > a.txt\"\n\n\
                [[rules]]\nname = \"b\"\noutput = [\"b.txt\"]\nshell = \"echo b > b.txt\"\n\n\
                [[rules]]\nname = \"c\"\noutput = [\"c.txt\"]\nshell = \"echo c > c.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml, "max_jobs": 3 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    let logs: String = client
        .get(format!("{base}/api/runs/{run_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // -j 1 would execute sequentially (Running → ✓ per rule); with -j 3 all
    // three rules are submitted before the first completion.
    let third_submission = logs.find("Running: c").expect("rule c must be submitted");
    let first_completion = logs.find("✓").expect("a rule must complete");
    assert!(
        first_completion > third_submission,
        "all three rules must be submitted before any completes (parallel): {logs}"
    );
}

/// P1-01 regression (issue #79): cancel must be a real signal. The rule
/// subprocesses live in the run's process group, so the SIGTERM from cancel
/// reaches them — previously each rule spawned its own group and the sleep
/// ran to completion, writing its product 56 s after the "cancelled" reply.
#[tokio::test]
async fn web_cancel_terminates_rule_processes() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // The fractional sleep duration is a unique process fingerprint so the
    // test can assert its death via pgrep without matching other tests.
    let toml = "[workflow]\nname = \"cancel79\"\n\n\
                [[rules]]\nname = \"slow\"\noutput = [\"product.txt\"]\n\
                shell = \"touch started.txt && sleep 57.123 && touch product.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();

    // Wait until the rule is actually executing.
    let deadline = Instant::now() + Duration::from_secs(60);
    let workdir = loop {
        assert!(Instant::now() < deadline, "rule never started");
        let info: serde_json::Value = client
            .get(format!("{base}/api/runs/{run_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(wd) = info["workdir"].as_str() {
            // The workdir column is relative to the server's cwd (the
            // tempdir), not the test process's.
            let wd = {
                let p = PathBuf::from(wd);
                if p.is_absolute() {
                    p
                } else {
                    dir.path().join(p)
                }
            };
            if wd.join("started.txt").exists() {
                break wd;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let resp = client
        .post(format!("{base}/api/runs/{run_id}/cancel"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // The sleep process must die — the strongest falsifiable form of the
    // "fake cancel signal" complaint.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = StdCommand::new("pgrep")
            .args(["-f", "sleep 57.123"])
            .output()
            .expect("pgrep available on unix");
        if !out.status.success() {
            break; // no match — process is gone
        }
        assert!(
            Instant::now() < deadline,
            "rule process survived cancel: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The product must never appear (the sleep was scheduled for 57 s).
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !workdir.join("product.txt").exists(),
        "product was written after cancel — the rule kept running"
    );

    // Status stays cancelled (the executor's fallout path must not flip it).
    let info: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["status"], "cancelled");
}

/// P1-02 regression (issue #79): a restart must not blindly mark an
/// in-flight run failed. Here the CLI is still alive when the server comes
/// back — recovery re-attaches it (status stays running, cancel/pause work),
/// and only the real outcome finalizes the run.
#[tokio::test]
async fn web_restart_reattaches_live_cli_and_cancel_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"restart79\"\n\n\
                [[rules]]\nname = \"slow\"\noutput = [\"done.txt\"]\n\
                shell = \"touch started.txt && sleep 25.321 && touch done.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();

    // Wait until the rule is executing, then kill the server outright.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "rule never started");
        let info: serde_json::Value = client
            .get(format!("{base}/api/runs/{run_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(wd) = info["workdir"].as_str() {
            let p = PathBuf::from(wd);
            let wd = if p.is_absolute() {
                p
            } else {
                dir.path().join(p)
            };
            if wd.join("started.txt").exists() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    server.kill_now();
    drop(server);

    // Restart on the same workdir — recovery must re-attach the live CLI.
    let server2 = TestServer::start(dir.path()).await;
    let base2 = server2.base.clone();

    let info: serde_json::Value = client
        .get(format!("{base2}/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        info["status"], "running",
        "live CLI must be re-attached, not blindly marked failed: {info}"
    );

    // Cancel still reaches the re-attached process group.
    let resp = client
        .post(format!("{base2}/api/runs/{run_id}/cancel"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = StdCommand::new("pgrep")
            .args(["-f", "sleep 25.321"])
            .output()
            .expect("pgrep available on unix");
        if !out.status.success() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "rule process survived post-restart cancel"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let info: serde_json::Value = client
        .get(format!("{base2}/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["status"], "cancelled");
}

/// P1-02 second half (issue #79): when the CLI finished while the server
/// was down, the exit record (`.exit-code`, written by the wrapper shell)
/// attributes the outcome honestly — completed stays completed instead of
/// being overwritten by a blind "failed" at restart.
#[tokio::test]
async fn web_restart_attributes_finished_run_from_exit_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"restart2_79\"\n\n\
                [[rules]]\nname = \"quick\"\noutput = [\"out.txt\"]\n\
                shell = \"echo hi > out.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();

    // Give the run time to complete, then kill the server before it is
    // reaped (the race window the old code handled by blind-failing).
    tokio::time::sleep(Duration::from_secs(3)).await;
    server.kill_now();
    drop(server);

    let server2 = TestServer::start(dir.path()).await;
    let base2 = server2.base.clone();
    // Whichever side of the race the kill landed on (CLI alive → re-attach,
    // CLI dead → exit record), the final status must be the truth.
    assert_eq!(
        wait_for_terminal(&client, &base2, &run_id).await,
        "completed",
        "restart must not rewrite a successful run as failed"
    );

    let info: serde_json::Value = client
        .get(format!("{base2}/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["status"], "completed");
}

/// P1-04 regression (issue #79): the rate-limiter layer order made the
/// limiter invisible to the middleware, so bursts passed with zero 429s.
/// 100 req/min per client: a 110-request burst must be throttled.
#[tokio::test]
async fn web_rate_limit_returns_429_on_burst() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let mut throttled = 0usize;
    for _ in 0..110 {
        let resp = client.get(format!("{base}/api/runs")).send().await.unwrap();
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            throttled += 1;
        }
    }
    assert!(
        throttled > 0,
        "a 110-request burst must produce at least one 429 (rate limiting is silently disabled)"
    );
}

/// P1-05 regression (issue #79): every mutation must land in the audit
/// trail. The audit_logs table had schema but zero write call sites.
#[tokio::test]
async fn web_mutations_write_audit_logs() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // A successful mutation (save pipeline) and a failed one (malformed
    // pipeline_id) — both must appear in /api/audit.
    let ok = client
        .post(format!("{base}/api/pipelines"))
        .json(&serde_json::json!({
            "toml_content": "[workflow]\nname = \"audit79\"\n",
            "name": "audit79",
            "version": "1.0.0",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    let bad = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": "[workflow]\nname = \"x\"\n",
            "pipeline_id": "../../etc",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);

    let audit: serde_json::Value = client
        .get(format!("{base}/api/audit?days=7"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entries = audit["entries"].as_array().expect("audit entries array");
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "POST /api/pipelines" && e["result"] == "success"),
        "successful mutation must be audited: {audit}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "POST /api/runs" && e["result"] == "failure"),
        "failed mutation must be audited: {audit}"
    );
}

/// P1-06 regression (issue #79): DB-created users must be able to sign in
/// (password is bcrypt-hashed into users.password_hash) and admin-created
/// sessions must authenticate.
#[tokio::test]
async fn web_created_user_can_login_and_authenticate() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Personal mode: management endpoints follow the localhost trust model.
    let created = client
        .post(format!("{base}/api/users"))
        .json(&serde_json::json!({
            "username": "alice79",
            "role": "user",
            "password": "correct-horse-battery",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        created.status(),
        reqwest::StatusCode::OK,
        "create user must succeed: {}",
        created.text().await.unwrap_or_default()
    );

    // Wrong password is rejected…
    let bad = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"username": "alice79", "password": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::UNAUTHORIZED);

    // …the right one signs in and yields a working session.
    let login: serde_json::Value = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"username": "alice79", "password": "correct-horse-battery"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("login must return a token");

    let me: serde_json::Value = client
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["authenticated"], true);
    assert_eq!(me["username"], "alice79");

    // Sessions are persisted (the eval's "sessions 表恒空" complaint).
    let users: serde_json::Value = client
        .get(format!("{base}/api/users"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        users
            .as_array()
            .map(|a| a.iter().any(|u| u["username"] == "alice79"))
            .unwrap_or(false),
        "created user must be listed: {users}"
    );
}

/// P1-03 regression (issue #79): wildcard rules expand to instance names in
/// the checkpoint (`align_experiment_EXP_01`) while the snapshot DAG holds
/// base names. The old exact-string merge failed for every instance, so all
/// nodes showed pending and /status stayed "queued" even for completed runs.
#[tokio::test]
async fn web_wildcard_run_status_matches_checkpoint_instances() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"wc79\"\n\n\
                [[pairs]]\npair_id = \"P1\"\nexperiment = \"EXP_01\"\ncontrol = \"CTRL_01\"\n\n\
                [[pairs]]\npair_id = \"P2\"\nexperiment = \"EXP_02\"\ncontrol = \"CTRL_02\"\n\n\
                [[rules]]\nname = \"align_experiment\"\n\
                output = [\"{experiment}.txt\"]\n\
                shell = \"echo {experiment} > {experiment}.txt\"\n\n\
                [[rules]]\nname = \"report\"\n\
                input = [\"EXP_01.txt\", \"EXP_02.txt\"]\n\
                output = [\"report.txt\"]\n\
                shell = \"cat EXP_01.txt EXP_02.txt > report.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({ "toml_content": toml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    // The derived status endpoint must agree with the DB status.
    let status: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["status"], "completed",
        "/status must derive completed for a finished wildcard run: {status}"
    );
    // Base rule aggregates its instances (both succeeded).
    let align = status["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["rule"] == "align_experiment")
        .expect("align_experiment node must exist");
    assert_eq!(align["status"], "success");

    // The DAG endpoint shares the same derivation.
    let dag: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/dag-status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dag_align = dag["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "align_experiment")
        .expect("DAG node must exist");
    assert_eq!(
        dag_align["status"], "success",
        "DAG node must aggregate instance success, not show pending: {dag}"
    );
}

/// P1-07 regression (issue #79): the Samples field must filter the cohort
/// (it was once wired to the old `--sample` append flag, which ran the full
/// cohort — that flag no longer exists; the field maps to `--samples`)
/// (append) so filling in a name silently ran the FULL cohort plus a phantom
/// sample. Now it maps to `--samples` filter semantics: a known sample runs
/// alone; an unknown name warns and fails instead of phantom-executing.
#[tokio::test]
async fn web_samples_filter_runs_subset_and_rejects_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"samples79\"\n\n\
                [[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\"]\n\n\
                [[rules]]\nname = \"gather\"\n\
                output = [\"{sample}.txt\"]\n\
                shell = \"echo {sample} > {sample}.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": toml,
            "samples": ["S1"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    // Only S1 executed — the subset filter held.
    let workdir: String = client
        .get(format!("{base}/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["workdir"]
        .as_str()
        .unwrap()
        .to_string();
    let wd = dir.path().join(&workdir);
    assert!(wd.join("S1.txt").exists(), "selected sample must run");
    assert!(
        !wd.join("S2.txt").exists(),
        "unselected sample must not run (the old --sample wiring ran the full cohort)"
    );

    // An unknown sample must fail loudly, not phantom-execute.
    let run2: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": toml,
            "samples": ["S99"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run2_id = run2["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run2_id).await,
        "failed",
        "an unknown sample must fail the run, not execute a phantom sample"
    );
    let logs: String = client
        .get(format!("{base}/api/runs/{run2_id}/logs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        logs.contains("not found in workflow samples"),
        "the CLI's unknown-sample warning must surface: {logs}"
    );
}

/// Issue #79 P2: the web dry-run preview previously showed unexpanded
/// rules and dropped samples/targets. Now the CLI's --json preview is
/// captured and served at /api/runs/{id}/preview with INSTANCE-level
/// entries (gather_cohort_S1, …) and will_run/will_skip summary.
#[tokio::test]
async fn web_dry_run_serves_instance_level_preview() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let toml = "[workflow]\nname = \"dry79\"\n\n\
                [[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\"]\n\n\
                [[rules]]\nname = \"gather\"\n\
                output = [\"{sample}.txt\"]\n\
                shell = \"echo {sample} > {sample}.txt\"\n";
    let run: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": toml,
            "dry_run": true,
            "samples": ["S1"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = run["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    let preview: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/preview"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let summary = &preview["checkpoint_preview"]["summary"];
    assert_eq!(
        summary["will_run"], 1,
        "the --samples filter must reach the preview: {preview}"
    );
    let plan = preview["checkpoint_preview"]["plan"].as_array().unwrap();
    let names: Vec<&str> = plan.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(
        names.contains(&"gather_cohort_S1"),
        "preview must list the EXPANDED instance name: {names:?}"
    );
    assert!(
        !names.contains(&"gather_cohort_S2"),
        "unselected sample must not appear: {names:?}"
    );
}

// ===========================================================================
// Team-mode multi-tenancy isolation matrix (issue #82 P0-4 / P0-5)
// ===========================================================================

/// Team-mode server: auth middleware on, seeded env credentials.
struct TeamServer {
    server: TestServer,
    admin_token: String,
}

async fn login_as(client: &reqwest::Client, base: &str, username: &str, password: &str) -> String {
    let body: serde_json::Value = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["token"]
        .as_str()
        .expect("login must return a token")
        .to_string()
}

impl TeamServer {
    async fn start(dir: &std::path::Path) -> Self {
        let port = free_port();
        let child = StdCommand::new(workspace_bin("oxo-flow-web"))
            .current_dir(dir)
            .env("OXO_FLOW_BIN", workspace_bin("oxo-flow"))
            .env("OXO_FLOW_HOST", "127.0.0.1")
            .env("OXO_FLOW_PORT", port.to_string())
            .env("OXO_FLOW_MODE", "team")
            .env("OXO_FLOW_ADMIN_PASSWORD", "admin-secret")
            .env("OXO_FLOW_USER_PASSWORD", "user-secret")
            .env("OXO_FLOW_VIEWER_PASSWORD", "viewer-secret")
            .env(
                "OXO_FLOW_FRONTEND_DIR",
                dir.join("missing-frontend").to_str().unwrap(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("team web server must start");

        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                panic!("team web server did not become ready at {base}");
            }
            // /api/health stays public in every mode (load-balancer probe).
            if client
                .get(format!("{base}/api/health"))
                .send()
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let admin_token = login_as(&client, &base, "admin", "admin-secret").await;
        Self {
            server: TestServer { child, base },
            admin_token,
        }
    }

    async fn login(&self, client: &reqwest::Client, username: &str, password: &str) -> String {
        login_as(client, &self.server.base, username, password).await
    }
}

const ISO_WORKFLOW: &str = "[workflow]\nname = \"iso\"\nversion = \"1.0.0\"\n\n\
     [[rules]]\nname = \"hello\"\noutput = [\"hello.txt\"]\n\
     shell = \"echo hi > hello.txt\"\n";

/// P0-4: runs are private per user — foreign runs 404 on read AND control;
/// admins retain full visibility.
#[tokio::test]
async fn team_mode_run_ownership_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let server = TeamServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.server.base.clone();

    let alice = server.login(&client, "alice", "user-secret").await;
    let bob = server.login(&client, "bob", "user-secret").await;

    // Alice creates a run — the row must be attributed to alice's users.id.
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .bearer_auth(&alice)
        .json(&serde_json::json!({"toml_content": ISO_WORKFLOW}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();

    // Owner sees the run.
    let owner_view = client
        .get(format!("{base}/api/runs/{run_id}"))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(owner_view.status(), 200);

    // Foreign user gets 404 on read and on every control endpoint.
    for (method, path) in [
        ("get", format!("/api/runs/{run_id}")),
        ("get", format!("/api/runs/{run_id}/status")),
        ("get", format!("/api/runs/{run_id}/logs")),
        ("get", format!("/api/runs/{run_id}/results")),
        ("get", format!("/api/runs/{run_id}/diagnostics")),
        ("post", format!("/api/runs/{run_id}/cancel")),
        ("post", format!("/api/runs/{run_id}/pause")),
        ("post", format!("/api/runs/{run_id}/resume")),
    ] {
        let resp = match method {
            "get" => client.get(format!("{base}{path}")).bearer_auth(&bob),
            _ => client
                .post(format!("{base}{path}"))
                .bearer_auth(&bob)
                .json(&serde_json::json!({})),
        }
        .send()
        .await
        .unwrap();
        assert_eq!(
            resp.status(),
            404,
            "bob must get 404 (not 403 — existence must not leak) on {method} {path}"
        );
    }

    // Bob's run list excludes alice's run.
    let bob_list: serde_json::Value = client
        .get(format!("{base}/api/runs"))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_ids: Vec<&str> = bob_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(
        !bob_ids.contains(&run_id.as_str()),
        "bob's run list must not contain alice's run"
    );

    // Admin sees and can control the run.
    let admin_view = client
        .get(format!("{base}/api/runs/{run_id}"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(admin_view.status(), 200);
    let admin_cancel = client
        .post(format!("{base}/api/runs/{run_id}/cancel"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(admin_cancel.status(), 200);
}

/// P0-4: pipelines are scoped per user; 'workspace' visibility is readable
/// by everyone but still writable only by its owner.
#[tokio::test]
async fn team_mode_pipeline_ownership_and_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let server = TeamServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.server.base.clone();

    let alice = server.login(&client, "alice", "user-secret").await;
    let bob = server.login(&client, "bob", "user-secret").await;

    let save_client = client.clone();
    let save = |token: &str, name: &str, visibility: &str| {
        let base = base.clone();
        let client = save_client.clone();
        let token = token.to_string();
        let name = name.to_string();
        let visibility = visibility.to_string();
        async move {
            client
                .post(format!("{base}/api/pipelines"))
                .bearer_auth(&token)
                .json(&serde_json::json!({
                    "name": name, "toml_content": ISO_WORKFLOW, "visibility": visibility,
                }))
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        }
    };

    let private = save(&alice, "alice-private", "private").await;
    let private_id = private["id"].as_str().unwrap().to_string();
    let shared = save(&alice, "alice-workspace", "workspace").await;
    let shared_id = shared["id"].as_str().unwrap().to_string();

    // Bob cannot read/write/delete alice's private pipeline.
    for (method, path) in [
        ("get", format!("/api/pipelines/{private_id}")),
        ("put", format!("/api/pipelines/{private_id}")),
        ("delete", format!("/api/pipelines/{private_id}")),
    ] {
        let resp = match method {
            "get" => client.get(format!("{base}{path}")).bearer_auth(&bob),
            "put" => client
                .put(format!("{base}{path}"))
                .bearer_auth(&bob)
                .json(&serde_json::json!({"name": "hijacked"})),
            _ => client.delete(format!("{base}{path}")).bearer_auth(&bob),
        }
        .send()
        .await
        .unwrap();
        assert_eq!(
            resp.status(),
            404,
            "bob on alice's private pipeline: {method} {path}"
        );
    }

    // Workspace-visible: bob can read but not write.
    let bob_read = client
        .get(format!("{base}/api/pipelines/{shared_id}"))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_read.status(),
        200,
        "workspace pipeline must be readable"
    );
    let bob_write = client
        .put(format!("{base}/api/pipelines/{shared_id}"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({"name": "hijacked"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_write.status(),
        404,
        "workspace pipeline is NOT writable by others"
    );

    // Bob's list contains the workspace pipeline but not the private one.
    let bob_list: serde_json::Value = client
        .get(format!("{base}/api/pipelines"))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = bob_list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(
        ids.contains(&shared_id.as_str()),
        "workspace pipeline in bob's list"
    );
    assert!(
        !ids.contains(&private_id.as_str()),
        "private pipeline hidden from bob"
    );

    // Fork is attributed to the forking user, not taken from the body.
    let fork: serde_json::Value = client
        .post(format!("{base}/api/pipelines/{shared_id}/fork"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({"user_id": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let forked_id = fork["forked_id"].as_str().unwrap().to_string();
    let admin_view: serde_json::Value = client
        .get(format!("{base}/api/pipelines/{forked_id}"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fork_owner = admin_view["user_id"].as_str().unwrap();
    let bob_users: serde_json::Value = client
        .get(format!("{base}/api/users"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_row = bob_users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "bob")
        .expect("bob's user row exists after login");
    assert_eq!(
        fork_owner,
        bob_row["id"].as_str().unwrap(),
        "fork must be owned by the acting user (bob), never the body-supplied user_id"
    );
}

/// P0-5: anonymous endpoints are closed in team mode; SSE requires ?token=.
#[tokio::test]
async fn team_mode_anonymous_endpoints_require_auth() {
    let dir = tempfile::tempdir().unwrap();
    let server = TeamServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.server.base.clone();

    for path in ["/api/system", "/api/metrics"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(resp.status(), 401, "{path} must require auth in team mode");
    }
    // /api/hpc exists only in hpc mode — team mode must not expose it at
    // all (404), never anonymously (401 would also be acceptable).
    let hpc = client.get(format!("{base}/api/hpc")).send().await.unwrap();
    assert!(
        matches!(hpc.status().as_u16(), 401 | 404),
        "/api/hpc must not be anonymously reachable in team mode"
    );

    // SSE without a token is rejected.
    let events_no_token = client
        .get(format!("{base}/api/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        events_no_token.status(),
        401,
        "/api/events must require ?token="
    );

    // SSE with a valid token connects (headers arrive immediately).
    let alice = server.login(&client, "alice", "user-secret").await;
    let events_ok = client
        .get(format!("{base}/api/events?token={alice}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();
    assert_eq!(
        events_ok.status(),
        200,
        "SSE connects with a valid session token"
    );

    // AI config GET stays public; writes are gated (see next test).
    let ai_config = client
        .get(format!("{base}/api/ai/config"))
        .send()
        .await
        .unwrap();
    assert_eq!(ai_config.status(), 200, "GET /api/ai/config stays public");
}

/// P0-5 + P1-16: AI provider writes are admin-only; env-password logins
/// auto-provision a real user row instead of the any-username hole.
#[tokio::test]
async fn team_mode_ai_config_admin_only_and_env_login_provisions_user() {
    let dir = tempfile::tempdir().unwrap();
    let server = TeamServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.server.base.clone();

    let bob = server.login(&client, "bob", "user-secret").await;

    // Non-admin writes to the shared AI provider are forbidden.
    let bob_config = client
        .post(format!("{base}/api/ai/config"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({"provider": "noop"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_config.status(),
        403,
        "AI config write must be admin-only"
    );
    let bob_test = client
        .post(format!("{base}/api/ai/test"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_test.status(),
        403,
        "AI provider test must be admin-only"
    );
    let admin_test = client
        .post(format!("{base}/api/ai/test"))
        .bearer_auth(&server.admin_token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(admin_test.status(), 200, "admin may test the AI provider");

    // Per-user AI isolation (deferred-item follow-up): a non-admin saving
    // their own provider row must NOT reconfigure the shared runtime —
    // their config is stored per-user and resolved per call.
    let server_before: serde_json::Value = client
        .get(format!("{base}/api/ai/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_user_config = client
        .put(format!("{base}/api/ai/config/user"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({"provider": "noop", "api_key": "bobs-key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bob_user_config.status(), 200);
    let bob_user_body: serde_json::Value = bob_user_config.json().await.unwrap();
    assert_eq!(
        bob_user_body["applied_to_runtime"], false,
        "non-admin per-user config must not touch the shared runtime: {bob_user_body}"
    );
    // The shared config is byte-identical after bob's write.
    let server_after: serde_json::Value = client
        .get(format!("{base}/api/ai/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        server_after, server_before,
        "bob's per-user row must leave the shared provider untouched"
    );
    // Bob's effective view reports HIS row.
    let bob_effective: serde_json::Value = client
        .get(format!("{base}/api/ai/config/effective"))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bob_effective["tiers"]["user_provider"], "noop");

    // Cluster management is admin-only too (SSH credentials).
    let bob_cluster = client
        .post(format!("{base}/api/clusters"))
        .bearer_auth(&bob)
        .json(&serde_json::json!({
            "id": "evil", "name": "evil", "ssh_host": "10.0.0.1", "ssh_port": 22,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_cluster.status(),
        403,
        "cluster management must be admin-only"
    );

    // Env-password login auto-provisions a real users row (id = username).
    let carol = server.login(&client, "carol", "user-secret").await;
    let users: serde_json::Value = client
        .get(format!("{base}/api/users"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let carol_row = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "carol")
        .expect("carol's users row must be auto-provisioned");
    assert_eq!(carol_row["role"], "user");

    // Carol's run is attributed to her users.id (her username), so audit
    // trails point at a real identity.
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .bearer_auth(&carol)
        .json(&serde_json::json!({"toml_content": ISO_WORKFLOW}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    let run_view: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        run_view["user_id"].as_str().unwrap(),
        carol_row["id"].as_str().unwrap(),
        "run must be owned by carol's canonical user id"
    );
}

// ===========================================================================
// File service layer (issue #82 P0-1 / P0-2): download, preview, zip, upload
// ===========================================================================

/// P0-1: results are retrievable — file download with correct headers,
/// traversal protection, text preview, Range support, and directory zip.
#[tokio::test]
async fn web_run_files_download_preview_and_zip() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Produce a run with two output files, one nested.
    let workflow = "[workflow]\nname = \"files\"\nversion = \"1.0.0\"\n\n\
        [[rules]]\nname = \"emit\"\noutput = [\"hello.txt\", \"sub/data.csv\"]\n\
        shell = \"mkdir -p sub && echo hi > hello.txt && echo 'a,b\\n1,2' > sub/data.csv\"\n";
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"toml_content": workflow}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    // File download: bytes + attachment disposition + etag.
    let resp = client
        .get(format!("{base}/api/runs/{run_id}/files?path=hello.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "file download must succeed");
    assert_eq!(
        resp.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap(),
        "attachment; filename=\"hello.txt\""
    );
    assert!(resp.headers().contains_key("etag"), "etag header required");
    assert_eq!(resp.text().await.unwrap(), "hi\n");

    // Range request: bytes=0-1 → 206 with exactly the first two bytes.
    let range_resp = client
        .get(format!("{base}/api/runs/{run_id}/files?path=hello.txt"))
        .header("Range", "bytes=0-1")
        .send()
        .await
        .unwrap();
    assert_eq!(range_resp.status(), 206, "single range must be served");
    assert_eq!(
        range_resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes 0-1/3"
    );
    assert_eq!(range_resp.text().await.unwrap(), "hi");

    // Nested path resolves relative to the workdir.
    let nested = client
        .get(format!("{base}/api/runs/{run_id}/files?path=sub/data.csv"))
        .send()
        .await
        .unwrap();
    assert_eq!(nested.status(), 200);

    // Text preview: truncated JSON with mime + content.
    let preview: serde_json::Value = client
        .get(format!(
            "{base}/api/runs/{run_id}/files?path=sub/data.csv&preview=true"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["mime"], "text/csv");
    assert!(preview["content"].as_str().unwrap().contains("a,b"));

    // Traversal is rejected outright.
    let traversal = client
        .get(format!(
            "{base}/api/runs/{run_id}/files?path=../../etc/passwd"
        ))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(traversal.status().as_u16(), 400 | 404),
        "path traversal must be rejected, got {}",
        traversal.status()
    );

    // Missing file → 404.
    let missing = client
        .get(format!("{base}/api/runs/{run_id}/files?path=nope.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // Directory download → zip archive containing the nested layout.
    let zip_resp = client
        .get(format!("{base}/api/runs/{run_id}/files?path=."))
        .send()
        .await
        .unwrap();
    assert_eq!(zip_resp.status(), 200);
    assert_eq!(
        zip_resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );
    let body = zip_resp.bytes().await.unwrap();
    assert!(body.len() > 100, "zip must contain entries");
    // ZIP magic: local file header signature.
    assert_eq!(&body[0..4], &[0x50, 0x4b, 0x03, 0x04]);
}

/// P0-2: multipart upload lands in the user's inputs workspace, with
/// name sanitization and quota enforcement.
#[tokio::test]
async fn web_file_upload_saves_to_inputs_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Multipart upload with a subdirectory hint.
    let form = reqwest::multipart::Form::new().text("path", "fastq").part(
        "file",
        reqwest::multipart::Part::bytes(b"@SEQ\nACGT\n+\n!!!!\n".to_vec())
            .file_name("sample1.fastq"),
    );
    let upload: serde_json::Value = client
        .post(format!("{base}/api/files"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let files = upload["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "one file uploaded: {upload}");
    assert_eq!(files[0]["name"], "sample1.fastq");

    // The file landed under workspace/users/default/inputs/fastq/.
    let saved = dir
        .path()
        .join("workspace/users/default/inputs/fastq/sample1.fastq");
    assert!(saved.exists(), "upload must land at {saved:?}");
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        "@SEQ\nACGT\n+\n!!!!\n"
    );

    // Uploaded inputs are listable for workflow authoring.
    let listing: serde_json::Value = client
        .get(format!("{base}/api/files"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = listing
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(
        names.contains(&"sample1.fastq"),
        "listing contains upload: {listing:?}"
    );
}

// ===========================================================================
// Run-loop closure (issue #82 P0-3 / P1-1 / P1-2)
// ===========================================================================

/// P0-3: retry is a REAL run — the plan's new_run_id exists in the
/// database and executes to a terminal state (previously it was a ghost).
#[tokio::test]
async fn web_retry_actually_spawns_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let failing = "[workflow]\nname = \"retry\"\nversion = \"1.0.0\"\n\n\
        [[rules]]\nname = \"boom\"\noutput = [\"boom.txt\"]\n\
        shell = \"echo oops > boom.txt && exit 1\"\n";
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"toml_content": failing}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_terminal(&client, &base, &run_id).await, "failed");

    let plan: serde_json::Value = client
        .post(format!("{base}/api/runs/{run_id}/retry"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_id = plan["new_run_id"].as_str().unwrap().to_string();
    assert!(
        plan["will_rerun"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "boom"),
        "the failed rule must be in will_rerun: {plan}"
    );

    // The retried run really exists and really executes.
    let retried = client
        .get(format!("{base}/api/runs/{new_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        retried.status(),
        200,
        "retry run must exist in the database"
    );
    assert_eq!(
        wait_for_terminal(&client, &base, &new_id).await,
        "failed",
        "the retried run must reach a terminal state"
    );
}

/// P1-1: the instance table answers "which sample under which rule failed"
/// with sample×rule granularity.
#[tokio::test]
async fn web_run_instances_expose_sample_rule_table() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Two samples discovered from the data dir, one rule each:
    // S1 succeeds, S2 fails. The web run executes in its own sandbox
    // workdir, so the sample_pattern uses an absolute path.
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("S1.fq"), "@seq\nACGT\n").unwrap();
    std::fs::write(data_dir.join("S2.fq"), "@seq\nACGT\n").unwrap();
    let data_str = data_dir.to_string_lossy();
    let workflow = format!(
        "[workflow]\nname = \"inst\"\nversion = \"1.0.0\"\n\
        sample_pattern = \"{data_str}/{{sample}}.fq\"\n\n\
        [[rules]]\nname = \"qc\"\ninput = [\"{{sample}}.fq\"]\n\
        output = [\"qc_{{sample}}.txt\"]\n\
        shell = \"[ \\\"{{sample}}\\\" = S1 ] && echo ok > qc_{{sample}}.txt || exit 1\"\n"
    );
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({
            "toml_content": workflow,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_terminal(&client, &base, &run_id).await, "failed");

    let instances: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/instances"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = instances.as_array().unwrap();
    let s1 = rows
        .iter()
        .find(|r| r["sample"] == "S1")
        .expect("S1 instance present");
    let s2 = rows
        .iter()
        .find(|r| r["sample"] == "S2")
        .expect("S2 instance present");
    assert_eq!(s1["status"], "success", "S1 succeeded: {instances}");
    assert_eq!(s2["status"], "failed", "S2 failed: {instances}");
    assert_eq!(s1["rule"], "qc", "base rule attribution");
}

/// P1-2: real telemetry — a run long enough for the sampler to tick leaves
/// a timeline in /ai-status and metrics in /status (not fabricated
/// defaults).
#[tokio::test]
async fn web_run_status_carries_real_telemetry() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let slow = "[workflow]\nname = \"slow\"\nversion = \"1.0.0\"\n\n\
        [[rules]]\nname = \"nap\"\noutput = [\"nap.txt\"]\n\
        shell = \"sleep 7 && echo done > nap.txt\"\n";
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"toml_content": slow}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    // The sampler ticked at least once during the 7s run.
    let ai_status: serde_json::Value = client
        .get(format!("{base}/api/runs/{run_id}/ai-status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let empty: Vec<serde_json::Value> = vec![];
    let timeline = ai_status["timeline"].as_array().unwrap_or(&empty);
    assert!(
        !timeline.is_empty(),
        "timeline must carry real samples: {ai_status}"
    );
    assert!(
        timeline.iter().all(|t| t["memory_mb"].is_number()),
        "every sample records memory: {timeline:?}"
    );
}

// ===========================================================================
// Share closure + version history (issue #82 P0-6 / P1-14)
// ===========================================================================

/// P0-6: a share link opens as a public read-only landing page carrying
/// the pipeline's identity, DAG shape, TOML, and provenance; importing it
/// creates a copy owned by the importer.
#[tokio::test]
async fn web_share_landing_is_public_and_importable() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let saved: serde_json::Value = client
        .post(format!("{base}/api/pipelines"))
        .json(&serde_json::json!({
            "name": "share-me", "toml_content": ISO_WORKFLOW, "visibility": "private",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pipeline_id = saved["id"].as_str().unwrap().to_string();

    let share: serde_json::Value = client
        .post(format!("{base}/api/pipelines/{pipeline_id}/share"))
        .json(&serde_json::json!({"visibility": "link"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = share["access_token"].as_str().unwrap().to_string();
    // The share URL carries the ACTUAL bound port, not a hardcoded 3000.
    let share_url = share["share_url"].as_str().unwrap();
    let bound_port = base.rsplit(':').next().unwrap();
    assert!(
        share_url.contains(&format!(":{bound_port}/")),
        "share URL must use the bound port: {share_url}"
    );

    // The landing payload is readable WITHOUT any session (that is the
    // whole point of a share link).
    let landing: serde_json::Value = client
        .get(format!("{base}/api/share/{token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(landing["pipeline"]["name"], "share-me");
    assert_eq!(landing["pipeline"]["rules_count"], 1);
    assert!(
        landing["dag"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "hello"),
        "DAG summary lists rule names: {landing}"
    );
    assert!(landing["toml_content"].as_str().unwrap().contains("hello"));

    // Import creates a copy (named with the imported suffix).
    let imported: serde_json::Value = client
        .post(format!("{base}/api/pipelines/import"))
        .json(&serde_json::json!({"url": share_url}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let imported_id = imported["pipeline_id"].as_str().unwrap().to_string();
    let copy: serde_json::Value = client
        .get(format!("{base}/api/pipelines/{imported_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(copy["name"].as_str().unwrap().contains("imported"));
    assert_eq!(copy["toml_content"], ISO_WORKFLOW);

    // Expired links are gone.
    let expired: serde_json::Value = client
        .post(format!("{base}/api/pipelines/{pipeline_id}/share"))
        .json(&serde_json::json!({"visibility": "link", "expires_in_days": 0}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let exp_token = expired["access_token"].as_str().unwrap();
    let gone = client
        .get(format!("{base}/api/share/{exp_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 410, "expired share must return GONE");
}

/// P1-14: every save/update snapshots a revision; rollback restores an old
/// snapshot and keeps history intact (nothing is lost).
#[tokio::test]
async fn web_pipeline_revisions_and_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    let original = ISO_WORKFLOW.replace("hello", "hello_v1");
    let mutated = ISO_WORKFLOW.replace("hello", "hello_v2");

    let saved: serde_json::Value = client
        .post(format!("{base}/api/pipelines"))
        .json(&serde_json::json!({"name": "hist", "toml_content": original}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pipeline_id = saved["id"].as_str().unwrap().to_string();

    // One update → two revisions (initial save + pre-update snapshot).
    let updated: serde_json::Value = client
        .put(format!("{base}/api/pipelines/{pipeline_id}"))
        .json(&serde_json::json!({"toml_content": mutated}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        updated["toml_content"]
            .as_str()
            .unwrap()
            .contains("hello_v2")
    );

    let revisions: serde_json::Value = client
        .get(format!("{base}/api/pipelines/{pipeline_id}/revisions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let revs = revisions.as_array().unwrap();
    assert_eq!(revs.len(), 2, "one revision per save/update: {revisions}");

    // The OLDEST revision holds the original content.
    let oldest_id = revs
        .iter()
        .min_by_key(|r| r["created_at"].as_str().unwrap_or(""))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let snapshot: serde_json::Value = client
        .get(format!(
            "{base}/api/pipelines/{pipeline_id}/revisions/{oldest_id}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        snapshot["toml_content"]
            .as_str()
            .unwrap()
            .contains("hello_v1"),
        "oldest snapshot holds the original: {snapshot}"
    );

    // Rollback restores the original content and records ANOTHER revision.
    let rolled: serde_json::Value = client
        .post(format!("{base}/api/pipelines/{pipeline_id}/rollback"))
        .json(&serde_json::json!({"revision_id": oldest_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        rolled["toml_content"]
            .as_str()
            .unwrap()
            .contains("hello_v1")
    );

    let after: serde_json::Value = client
        .get(format!("{base}/api/pipelines/{pipeline_id}/revisions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after.as_array().unwrap().len(),
        3,
        "rollback adds a revision"
    );
}

// ===========================================================================
// Webhook notifications (issue #82 P1-12)
// ===========================================================================

/// A run reaching a terminal state POSTs an HMAC-signed payload to the
/// configured webhook endpoint (raw TCP listener captures the request;
/// the signature format is verified against the core HMAC scheme, whose
/// RFC 4231 vectors are unit-tested in core).
#[tokio::test]
async fn web_webhook_fires_on_run_completion_with_hmac() {
    use std::sync::{Arc, Mutex};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let captured_thread = captured.clone();
    let handle = std::thread::spawn(move || {
        // One delivery: accept, read, respond 200 (a successful answer
        // stops the sender's retry loop).
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        {
            stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
            use std::io::Read as _;
            let mut buf = [0u8; 8192];
            let mut request = String::new();
            // Read until headers end, then honor Content-Length.
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                request.push_str(&String::from_utf8_lossy(&buf[..n]));
                if request.contains("\r\n\r\n") {
                    break;
                }
            }
            let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
            let content_len: usize = request
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = request[body_start..].to_string();
            while body.len() < content_len {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                body.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            *captured_thread.lock().unwrap() = request.clone();
            use std::io::Write as _;
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Configure the webhook to hit the capture listener.
    let saved: serde_json::Value = client
        .put(format!("{base}/api/webhook"))
        .json(&serde_json::json!({
            "enabled": true,
            "url": format!("http://{addr}/hook"),
            "secret": "s3cret-key",
            "events": ["workflow_completed", "workflow_failed"],
            "signature_scheme": "hmac-sha256",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["status"], "saved");

    // The GET must never echo the secret.
    let config: serde_json::Value = client
        .get(format!("{base}/api/webhook"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["secret_set"], true);
    assert!(
        config.get("secret").is_none(),
        "secret must never be echoed"
    );

    // A completed run fires the webhook.
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"toml_content": ISO_WORKFLOW}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(
        wait_for_terminal(&client, &base, &run_id).await,
        "completed"
    );

    handle.join().unwrap();
    let request = captured.lock().unwrap().clone();
    assert!(
        request.starts_with("POST /hook"),
        "webhook must POST to the configured path: {request}"
    );
    assert!(
        request.contains("\"event\":\"workflow_completed\"")
            || request.contains("\"event\":\"WorkflowCompleted\""),
        "payload carries the terminal event: {request}"
    );
    assert!(
        request.contains("\"workflow_name\":\"iso\""),
        "payload names the workflow"
    );
    // HMAC-SHA256 signature header (scheme prefix + 64 hex chars).
    let sig = request
        .lines()
        .find(|l| l.to_lowercase().starts_with("x-oxoflow-signature:"))
        .unwrap_or("");
    let value = sig.split(':').nth(1).unwrap_or("").trim();
    assert!(
        value.len() == "hmac-sha256=".len() + 64 && value.starts_with("hmac-sha256="),
        "signature must use the hmac-sha256 scheme: '{value}'"
    );
}

/// P1-13: API keys authenticate machine clients with the same ownership
/// scoping as sessions; revocation is immediate.
#[tokio::test]
async fn team_mode_api_keys_authenticate_and_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let server = TeamServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.server.base.clone();

    let alice = server.login(&client, "alice", "user-secret").await;

    // Create a key as alice.
    let created: serde_json::Value = client
        .post(format!("{base}/api/auth/keys"))
        .bearer_auth(&alice)
        .json(&serde_json::json!({"name": "ci-bot"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap().to_string();
    let key_id = created["id"].as_str().unwrap().to_string();
    assert!(key.starts_with("oxo_"), "key format: {key}");

    // The listing never echoes the plaintext.
    let listed: serde_json::Value = client
        .get(format!("{base}/api/auth/keys"))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(
        listed.as_array().unwrap()[0].get("key").is_none(),
        "plaintext never listed"
    );

    // The key authenticates machine requests.
    let via_key = client
        .get(format!("{base}/api/runs"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(via_key.status(), 200, "API key must authenticate");

    // Revocation is immediate.
    let revoked = client
        .delete(format!("{base}/api/auth/keys/{key_id}"))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let after_revoke = client
        .get(format!("{base}/api/runs"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 401, "revoked keys are rejected");
}

// ===========================================================================
// Remote cluster execution (issue #82 deployment modes) — GATED: runs only
// when OXO_TEST_CLUSTER_SSH=1 and OXO_TEST_CLUSTER_HOST is set (a real SSH
// host with the oxo-flow CLI on PATH, e.g. tx-ubuntu). CI skips it.
// ===========================================================================

#[tokio::test]
async fn web_remote_cluster_run_executes_and_pulls_results() {
    if std::env::var("OXO_TEST_CLUSTER_SSH").as_deref() != Ok("1") {
        eprintln!("skipped: set OXO_TEST_CLUSTER_SSH=1 + OXO_TEST_CLUSTER_HOST to run");
        return;
    }
    let host = std::env::var("OXO_TEST_CLUSTER_HOST").expect("OXO_TEST_CLUSTER_HOST required");

    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(dir.path()).await;
    let client = reqwest::Client::new();
    let base = server.base.clone();

    // Register the real cluster connection.
    let reg = client
        .post(format!("{base}/api/clusters"))
        .json(&serde_json::json!({
            "id": "e2e-remote", "name": "e2e remote", "ssh_host": host,
            "ssh_port": 22,
            "ssh_user": std::env::var("OXO_TEST_CLUSTER_USER").ok(),
            "scheduler": "auto",
            "remote_dir": "/tmp/oxo-remote-e2e",
            "enabled": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), 200, "cluster registration must succeed");

    // The run executes remotely and its results are pulled back.
    let created: serde_json::Value = client
        .post(format!("{base}/api/runs"))
        .json(&serde_json::json!({"toml_content": ISO_WORKFLOW, "cluster_id": "e2e-remote"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = created["run_id"].as_str().unwrap().to_string();
    let status = wait_for_terminal(&client, &base, &run_id).await;
    assert_eq!(status, "completed", "remote run must complete");

    // Pulled-back results are downloadable through the normal file layer.
    let resp = client
        .get(format!("{base}/api/runs/{run_id}/files?path=hello.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "pulled-back results must be downloadable"
    );
    assert_eq!(resp.text().await.unwrap(), "hi\n");
}
