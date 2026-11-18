//! Router-level tests for the per-node environment on
//! `GET /api/runs/{id}/dag-status` (issue #1149, audit UX-07).
//!
//! The monitor's DAG canvas used to hard-code `environment: 'system'` for
//! every node, so a conda/docker/singularity rule was rendered as a system
//! rule. The handler now reports each node's environment as the RUN resolves
//! it — the same `apply_defaults` fold the run/dry-run paths make, which
//! materializes the documented precedence `env_group` → the rule's inline
//! environment → `[defaults] environment` (issue #1210). A node that no
//! longer maps to a snapshot rule reports `null` — never a fabricated
//! `system`.
//!
//! All tests drive the production router (`build_router("personal")`) against
//! the shared file-backed DB harness (`common::ensure_db`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

fn app() -> axum::Router {
    server::build_router("personal")
}

async fn get(uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

/// Seed a run pointing at `workdir` with the given pipeline snapshot.
async fn seed_run(id: &str, snapshot: &str, workdir: &std::path::Path) {
    sqlx::query(
        "INSERT OR REPLACE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, \
         workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, ?, 'env-fixture', 'succeeded', 'finished', NULL, ?, NULL, NULL, ?)",
    )
    .bind(id)
    .bind(snapshot)
    .bind(workdir.to_string_lossy().as_ref())
    .bind("2026-01-01T00:00:00Z")
    .execute(pool())
    .await
    .expect("seed run");
}

/// The multi-environment fixture: four rules across the sources the run path
/// honors — `[defaults] venv` and inline `docker`/`singularity` specs, both
/// folded onto rules by `apply_defaults` (the same call `run`/`dry-run`
/// make) — plus a downstream rule inheriting the defaults. `env_group` is
/// covered by its own test below (folded since issue #1210).
const MULTI_ENV_TOML: &str = r#"
[workflow]
name = "multi-env"
version = "1.0"

[defaults.environment]
venv = "envs/default"

[[rules]]
name = "trim"
output = ["trimmed.fq"]
shell = "trimmer {output}"

[[rules]]
name = "align"
input = ["trimmed.fq"]
output = ["aligned.bam"]
environment = { docker = "biocontainers/bwa:0.7.17" }
shell = "bwa mem {input} > {output}"

[[rules]]
name = "report"
input = ["aligned.bam"]
output = ["report.html"]
environment = { singularity = "library://demo/report:1" }
shell = "report {input}"

[[rules]]
name = "publish"
input = ["report.html"]
output = ["site/index.html"]
shell = "publish {input} {output}"
"#;

/// The `environment` value a node carries: `Some(kind)` for a known rule,
/// `None` when the field is JSON `null`. A missing key and `null` are the
/// same thing to callers here (the field is always serialized).
fn node_env<'a>(body: &'a Value, rule: &str) -> Option<&'a str> {
    let nodes = body["nodes"].as_array().expect("nodes array");
    let node = nodes
        .iter()
        .find(|n| n["label"] == rule)
        .unwrap_or_else(|| panic!("node '{rule}' missing from {body}"));
    node["environment"].as_str()
}

/// The canvas must show each rule's REAL environment, resolved through the
/// run's own precedence — and must not label any of them `system`.
#[tokio::test]
async fn dag_status_reports_each_rules_real_environment() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();

    seed_run("run-env-multi", MULTI_ENV_TOML, tmp.path()).await;

    let (status, body) = get("/api/runs/run-env-multi/dag-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(node_env(&body, "trim"), Some("venv"), "{body}");
    assert_eq!(node_env(&body, "align"), Some("docker"), "{body}");
    assert_eq!(node_env(&body, "report"), Some("singularity"), "{body}");
    assert_eq!(node_env(&body, "publish"), Some("venv"), "{body}");

    // The audit finding: no node may collapse to the hard-coded `system`.
    for node in body["nodes"].as_array().unwrap() {
        assert_ne!(
            node["environment"], "system",
            "rule '{}' mislabelled as system: {body}",
            node["label"]
        );
    }
}

/// Issue #1210: `env_group` takes precedence over a conflicting inline
/// environment in the documented order, and the fold is the same
/// `apply_defaults` the run executes. The canvas must paint the group's
/// backend — the environment the rule is actually wrapped in — not the
/// inline loser that `dry-run` used to execute before the fix.
#[tokio::test]
async fn dag_status_reports_the_env_group_backend_the_run_executes() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();

    let snapshot = r#"
[workflow]
name = "env-group"
version = "1.0"

[env_groups.group_env]
docker = "biocontainers/bwa:0.7.17"

[[rules]]
name = "with_group"
env_group = "group_env"
environment = { venv = "envs/inline" }
output = ["a.txt"]
shell = "true > {output}"

[[rules]]
name = "inline_only"
environment = { venv = "envs/inline" }
output = ["b.txt"]
shell = "true > {output}"
"#;
    seed_run("run-env-group", snapshot, tmp.path()).await;

    let (status, body) = get("/api/runs/run-env-group/dag-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        node_env(&body, "with_group"),
        Some("docker"),
        "env_group must win over the conflicting inline venv: {body}"
    );
    assert_eq!(node_env(&body, "inline_only"), Some("venv"), "{body}");
}

/// The other direction: a rule with no environment anywhere really does run
/// in `system`, and must keep saying so (next to one that does not).
#[tokio::test]
async fn dag_status_reports_system_only_for_a_rule_that_runs_in_system() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();

    let snapshot = r#"
[workflow]
name = "mixed"
version = "1.0"

[[rules]]
name = "plain"
output = ["a.txt"]
shell = "true > {output}"

[[rules]]
name = "tooled"
input = ["a.txt"]
output = ["b.txt"]
environment = { conda = "envs/tool.yaml" }
shell = "tool {input} {output}"
"#;
    seed_run("run-env-system", snapshot, tmp.path()).await;

    let (status, body) = get("/api/runs/run-env-system/dag-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(node_env(&body, "plain"), Some("system"), "{body}");
    assert_eq!(node_env(&body, "tooled"), Some("conda"), "{body}");
}

/// An orphan instance (its base rule is gone from the snapshot because the
/// workflow was edited since the run) has NO knowable environment. The
/// endpoint must answer `null` instead of inventing `system`.
#[tokio::test]
async fn dag_status_leaves_the_environment_null_for_an_orphan_instance() {
    common::ensure_db().await;
    use oxo_flow_core::executor::CheckpointState;

    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().to_path_buf();

    // The checkpoint records an instance whose base rule no longer exists in
    // the snapshot below.
    let mut ck = CheckpointState::new();
    ck.completed_rules.insert("ghost_S1".to_string());
    ck.save_to_file(&CheckpointState::default_path(&workdir))
        .unwrap();

    let snapshot = r#"
[workflow]
name = "edited-since-run"
version = "1.0"

[[rules]]
name = "qc"
output = ["qc.txt"]
shell = "qc > {output}"
"#;
    seed_run("run-env-orphan", snapshot, &workdir).await;

    let (status, body) = get("/api/runs/run-env-orphan/dag-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(node_env(&body, "qc"), Some("system"), "{body}");
    let orphan = node_env(&body, "ghost_S1");
    assert!(
        orphan.is_none(),
        "orphan instance must report an unknown environment, got {orphan:?}: {body}"
    );
}
