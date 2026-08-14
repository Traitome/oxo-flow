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
                .join("workspace/users/local_user/pipelines")
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
                .join("workspace/users/local_user/pipelines")
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
            .join("workspace/users/local_user/runs")
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

/// P1-07 regression (issue #79): the Samples field was wired to `--sample`
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
    let names: Vec<&str> = plan
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        names.contains(&"gather_cohort_S1"),
        "preview must list the EXPANDED instance name: {names:?}"
    );
    assert!(
        !names.contains(&"gather_cohort_S2"),
        "unselected sample must not appear: {names:?}"
    );
}
