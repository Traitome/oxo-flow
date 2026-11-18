//! Run-tree immutability for saved-pipeline runs (issue #1130, audit RB-02).
//!
//! Runs created from a saved pipeline used to execute in the pipeline's
//! persistent `pipelines/<id>/` directory, so a later run overwrote the
//! earlier run's workflow, checkpoint, logs, and outputs. These tests drive
//! the production router and a stub CLI (`OXO_FLOW_BIN`) that writes
//! deterministic artifacts into its `--workdir`:
//!
//! - two sequential runs from one saved pipeline get distinct workdirs and
//!   leave the first run's artifacts byte-identical;
//! - a run executes the workflow snapshot it was created with — the request
//!   `toml_content` and the row's `pipeline_snapshot` — not the live pipeline
//!   directory or the saved pipeline row, even when both are mutated while
//!   the run is live.
//!
//! The stub is process-global (`OXO_FLOW_BIN`), so this binary's tests share
//! one workspace root and one stub; each test owns unique pipeline/run ids.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

mod common;

fn workflow(name: &str) -> String {
    format!(
        "[workflow]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n\
         [[rules]]\nname = \"hello\"\ninput = [\"in.txt\"]\noutput = [\"out.txt\"]\n\
         shell = \"cat {{input}} > {{output}}\"\n"
    )
}

/// Hermetic workspace + stub CLI, initialized exactly once per process.
/// SAFETY: every test calls this before its first workspace read; the
/// `OnceLock` makes the `set_var` calls happen on one thread before any
/// dependent request runs.
fn init_env() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::var("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let root = base.join(format!("run-snapshot-isolation-{}", std::process::id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace root");

        let stub = root.join("oxo-flow-stub");
        write_stub(&stub);
        unsafe {
            std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
            std::env::set_var("OXO_FLOW_BIN", &stub);
            std::env::set_var(
                "OXO_FLOW_AI_CONFIG",
                root.join("hermetic/.oxo-flow/ai_config.json"),
            );
        }
        oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
        workspace
    })
}

/// A POSIX `/bin/sh` stub that behaves like a tiny engine: it records what
/// the workflow file looked like at start and after a (probe-controlled)
/// delay, then emits the checkpoint/log/output artifacts a real run leaves
/// behind. Workflows containing `slow-snapshot-probe` sleep so the test can
/// mutate the live pipeline mid-run.
fn write_stub(stub: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = r#"#!/bin/sh
wd=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--workdir" ]; then wd="$a"; fi
  prev="$a"
done
[ -n "$wd" ] || exit 0
mkdir -p "$wd/.oxo-flow/logs"
if [ -f "$wd/workflow.oxoflow" ]; then
  cksum "$wd/workflow.oxoflow" > "$wd/.probe-first"
fi
if grep -q "slow-snapshot-probe" "$wd/workflow.oxoflow" 2>/dev/null; then
  sleep 5
fi
if [ -f "$wd/workflow.oxoflow" ]; then
  cksum "$wd/workflow.oxoflow" > "$wd/.probe-second"
  cp "$wd/workflow.oxoflow" "$wd/workflow.executed.oxoflow"
fi
printf 'stub run in %s\n' "$(basename "$wd")" >> "$wd/.oxo-flow/logs/oxo-flow.log"
printf '{"probe": true}\n' > "$wd/.oxo-flow/checkpoint.json"
printf 'results for %s\n' "$(basename "$wd")" > "$wd/results.txt"
exit 0
"#;
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create_pipeline(toml: &str, name: &str) -> String {
    let (status, body) = send(
        &app(),
        "POST",
        "/api/pipelines",
        Some(json!({"name": name, "toml_content": toml})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_str().expect("pipeline id").to_string()
}

async fn create_run(toml: &str, pipeline_id: &str) -> String {
    let (status, body) = send(
        &app(),
        "POST",
        "/api/runs",
        Some(json!({
            "toml_content": toml,
            "pipeline_id": pipeline_id,
            "max_jobs": 1,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "queued", "{body}");
    body["run_id"].as_str().expect("run id").to_string()
}

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

async fn run_row(run_id: &str) -> (String, Option<String>, String) {
    sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT status, workdir, pipeline_snapshot FROM runs WHERE id = ?",
    )
    .bind(run_id)
    .fetch_one(pool())
    .await
    .expect("run row")
}

async fn wait_terminal(run_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool())
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn wait_for_file(path: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    hex::encode(Sha256::digest(&bytes))
}

/// The artifacts whose immutability the issue names: workflow, checkpoint,
/// engine log, and a produced output.
fn artifact_hashes(run_dir: &Path) -> Vec<(&'static str, String)> {
    [
        ("workflow.oxoflow", "workflow.oxoflow"),
        ("checkpoint", ".oxo-flow/checkpoint.json"),
        ("engine log", ".oxo-flow/logs/oxo-flow.log"),
        ("execution log", "execution.log"),
        ("output", "results.txt"),
    ]
    .into_iter()
    .map(|(label, rel)| (label, sha256(&run_dir.join(rel))))
    .collect()
}

/// RB-02 exit evidence: two sequential runs of one saved pipeline get
/// distinct workdirs; the first run's workflow, checkpoint, logs, output,
/// and stored snapshot are byte-identical afterwards.
#[tokio::test]
async fn sequential_runs_from_one_saved_pipeline_are_isolated() {
    let workspace = init_env().to_path_buf();
    common::ensure_db().await;

    let toml_a = workflow("rb02-sequential-a");
    let toml_b = workflow("rb02-sequential-b");
    let pipeline = create_pipeline(&toml_a, "rb02-sequential").await;

    let run_a = create_run(&toml_a, &pipeline).await;
    wait_terminal(&run_a).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let (_, workdir_a, snapshot_a) = run_row(&run_a).await;
    let dir_a = PathBuf::from(workdir_a.expect("run A has a workdir"));
    assert_eq!(snapshot_a, toml_a, "run A must persist its own snapshot");
    assert!(
        dir_a.ends_with(format!("runs/{run_a}")),
        "every run owns runs/<run_id>, got {}",
        dir_a.display()
    );
    assert!(
        !dir_a.to_string_lossy().contains("/pipelines/"),
        "a run may never execute in the persistent pipeline directory: {}",
        dir_a.display()
    );
    assert_eq!(
        std::fs::read_to_string(dir_a.join("workflow.oxoflow")).unwrap(),
        toml_a
    );

    let before = artifact_hashes(&dir_a);

    let run_b = create_run(&toml_b, &pipeline).await;
    wait_terminal(&run_b).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let (_, workdir_b, snapshot_b) = run_row(&run_b).await;
    let dir_b = PathBuf::from(workdir_b.expect("run B has a workdir"));
    assert_ne!(dir_a, dir_b, "each run must get a unique workdir");
    assert_eq!(snapshot_b, toml_b);
    assert_eq!(
        std::fs::read_to_string(dir_b.join("workflow.oxoflow")).unwrap(),
        toml_b,
        "run B executes the workflow it was created with"
    );

    let after = artifact_hashes(&dir_a);
    assert_eq!(
        before, after,
        "run A's workflow/checkpoint/logs/output changed after a later run"
    );
    assert_eq!(
        run_row(&run_a).await.2,
        toml_a,
        "run A's persisted snapshot was mutated by a later run"
    );

    // This pipeline's persistent directory is not created at all anymore
    // (another test in this binary plants one deliberately, so assert on
    // this pipeline's path, not the shared parent).
    assert!(
        !workspace
            .join("users/default/pipelines")
            .join(&pipeline)
            .exists(),
        "saved-pipeline runs must not materialize a shared pipeline workdir"
    );
}

/// RB-02 snapshot consistency: the run executes the snapshot it was created
/// from, even when the saved pipeline row AND a (legacy) live pipeline
/// directory change while the run is in flight.
#[tokio::test]
async fn run_executes_the_snapshot_not_a_mutated_pipeline() {
    let workspace = init_env().to_path_buf();
    common::ensure_db().await;

    let toml_a = format!("# slow-snapshot-probe\n{}", workflow("rb02-snapshot"));
    let toml_b = workflow("rb02-snapshot-mutated");
    let pipeline = create_pipeline(&toml_a, "rb02-snapshot").await;

    let run = create_run(&toml_a, &pipeline).await;
    let (_, workdir, _) = run_row(&run).await;
    let run_dir = PathBuf::from(workdir.expect("run workdir"));
    wait_for_file(&run_dir.join(".probe-first")).await;

    // Mid-run: rewrite the saved pipeline row and plant the workflow in the
    // legacy persistent pipeline directory. Neither may reach this run.
    let (status, body) = send(
        &app(),
        "PUT",
        &format!("/api/pipelines/{pipeline}"),
        Some(json!({"toml_content": toml_b})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let legacy = workspace.join("users/default/pipelines").join(&pipeline);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("workflow.oxoflow"), &toml_b).unwrap();

    wait_terminal(&run).await;
    wait_for_file(&run_dir.join(".probe-second")).await;

    let probe_first = std::fs::read_to_string(run_dir.join(".probe-first")).unwrap();
    let probe_second = std::fs::read_to_string(run_dir.join(".probe-second")).unwrap();
    assert_eq!(
        probe_first, probe_second,
        "the workflow visible to the run changed mid-run"
    );
    let expected = sha256(&run_dir.join("workflow.oxoflow"));
    assert_eq!(
        expected,
        hex::encode(Sha256::digest(toml_a.as_bytes())),
        "the run must execute the snapshot from creation, not a later version"
    );
    assert_eq!(
        std::fs::read_to_string(run_dir.join("workflow.executed.oxoflow")).unwrap(),
        toml_a
    );

    // The live pipeline really did change — the isolation is not vacuous.
    let (_, _, snapshot) = run_row(&run).await;
    assert_eq!(snapshot, toml_a);
    let stored: String = sqlx::query_scalar("SELECT toml_content FROM pipelines WHERE id = ?")
        .bind(&pipeline)
        .fetch_one(pool())
        .await
        .expect("pipeline row");
    assert_eq!(
        stored, toml_b,
        "the live pipeline mutation must have landed"
    );
}
