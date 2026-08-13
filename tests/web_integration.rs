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
    _child: Child,
    base: String,
}

impl TestServer {
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
        Self {
            _child: child,
            base,
        }
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
        if status == "success" || status == "failed" {
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
    assert_eq!(wait_for_terminal(&client, &base, &run1_id).await, "success");
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
    assert_eq!(wait_for_terminal(&client, &base, &run2_id).await, "success");
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
    assert_eq!(wait_for_terminal(&client, &base, &run_id).await, "success");

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
    assert_eq!(wait_for_terminal(&client, &base, &run_id).await, "success");

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
    assert_eq!(wait_for_terminal(&client, &base, &run_id).await, "success");

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
