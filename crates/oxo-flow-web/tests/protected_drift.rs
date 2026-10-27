//! Router-level tests for `GET /api/runs/{id}/protected-drift`
//! (issue #935 CLI→web parity slice 3; PR #941 review finding — the handler
//! itself was previously only exercised at the core-machinery level).
//!
//! All tests drive the production router (`build_router("personal")`)
//! against the shared file-backed DB harness (`common::ensure_db`) and a
//! real on-disk workdir, pinning: 404 no-leak, empty-trees early return,
//! baseline-diff output, and the corrupt-checkpoint warning (absent
//! checkpoint = clean new_trees; corrupt checkpoint = explicit warning).

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

/// A workflow snapshot declaring one protected tree.
fn drift_toml() -> String {
    "[workflow]\nname = \"drift-preview\"\n\n[[rules]]\nname = \"hello\"\ninput = [\"in.txt\"]\noutput = [\"out.txt\"]\nshell = \"cat {{input}} > {{output}}\"\n\n[engine]\nprotected_data_trees = [\"raw\"]\n"
        .to_string()
}

/// The same workflow WITHOUT an [engine] section.
fn plain_toml() -> String {
    "[workflow]\nname = \"plain\"\n\n[[rules]]\nname = \"hello\"\ninput = [\"in.txt\"]\noutput = [\"out.txt\"]\nshell = \"cat {{input}} > {{output}}\"\n"
        .to_string()
}

/// Seed a run whose workdir is a real temp directory containing `raw/`.
async fn seed_run_with_workdir(id: &str, snapshot: &str, workdir: &std::path::Path) {
    sqlx::query(
        "INSERT OR REPLACE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, \
         workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, ?, 'drift', 'succeeded', 'finished', NULL, ?, NULL, NULL, ?)",
    )
    .bind(id)
    .bind(snapshot)
    .bind(workdir.to_string_lossy().as_ref())
    .bind("2026-01-01T00:00:00Z")
    .execute(pool())
    .await
    .expect("seed run");
}

/// A missing run answers 404 and must not leak other users' runs.
#[tokio::test]
async fn drift_preview_404_for_an_unknown_run() {
    common::ensure_db().await;

    let (status, body) = get("/api/runs/no-such-run/protected-drift").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "NOT_FOUND", "{body}");
}

/// A snapshot without [engine] protected_data_trees short-circuits to an
/// empty, all-clear response regardless of what is on disk.
#[tokio::test]
async fn drift_preview_with_no_declared_trees_is_empty() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-no-trees");
    std::fs::create_dir_all(workdir.join("raw")).unwrap();
    std::fs::write(workdir.join("raw/S1.fq"), b"AAAA").unwrap();
    seed_run_with_workdir("run-drift-no-trees", &plain_toml(), &workdir).await;

    let (status, body) = get("/api/runs/run-drift-no-trees/protected-drift").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["drifted_files"].as_array().unwrap().len(), 0, "{body}");
    assert_eq!(body["new_trees"], 0, "{body}");
    assert_eq!(body["warnings"].as_array().unwrap().len(), 0, "{body}");
}

/// Declared tree + recorded baseline + truncated file → one drift line.
#[tokio::test]
async fn drift_preview_reports_baseline_drift() {
    common::ensure_db().await;
    use oxo_flow_core::executor::CheckpointState;

    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-drift");
    std::fs::create_dir_all(workdir.join("raw")).unwrap();
    std::fs::write(workdir.join("raw/S1.fq"), b"AAAA").unwrap();

    // Seed the baseline exactly like a prior run's gate would have.
    let ck_dir = workdir.join(".oxo-flow");
    std::fs::create_dir_all(&ck_dir).unwrap();
    let mut ck = CheckpointState::new();
    let manifest = oxo_flow_core::executor::checkpoint::snapshot_protected_tree(
        &workdir.join("raw"),
        &workdir,
    )
    .unwrap()
    .unwrap();
    ck.record_protected_tree_baseline("raw", manifest);
    ck.save_to_file(&CheckpointState::default_path(&workdir))
        .unwrap();

    // Drift: truncate the protected file AFTER the baseline.
    std::fs::write(workdir.join("raw/S1.fq"), b"").unwrap();

    seed_run_with_workdir("run-drift-hit", &drift_toml(), &workdir).await;

    let (status, body) = get("/api/runs/run-drift-hit/protected-drift").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let files = body["drifted_files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{body}");
    assert!(files[0].as_str().unwrap().contains("S1.fq"), "{body}");
    assert_eq!(body["new_trees"], 0, "{body}");
    assert_eq!(body["warnings"].as_array().unwrap().len(), 0, "{body}");

    // Read-only contract at the wire level: the checkpoint still holds
    // exactly the seeded baseline (the preview must not adopt anything).
    let ck_after = CheckpointState::load_from_file(&CheckpointState::default_path(&workdir))
        .expect("checkpoint still readable");
    assert_eq!(ck_after.protected_tree_baselines.len(), 1);
}

/// A declared tree with no checkpoint yet reports new_trees (adoption
/// preview), not drift.
#[tokio::test]
async fn drift_preview_without_checkpoint_counts_new_trees() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-fresh");
    std::fs::create_dir_all(workdir.join("raw")).unwrap();
    std::fs::write(workdir.join("raw/S1.fq"), b"AAAA").unwrap();
    seed_run_with_workdir("run-drift-fresh", &drift_toml(), &workdir).await;

    let (status, body) = get("/api/runs/run-drift-fresh/protected-drift").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["new_trees"], 1, "{body}");
    assert_eq!(body["drifted_files"].as_array().unwrap().len(), 0, "{body}");
    assert_eq!(body["warnings"].as_array().unwrap().len(), 0, "{body}");
}

/// A corrupt checkpoint must NOT look like a clean result (PR #941 review
/// finding): the response carries a warnings entry naming the checkpoint.
#[tokio::test]
async fn drift_preview_warns_on_a_corrupt_checkpoint() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-corrupt");
    std::fs::create_dir_all(workdir.join("raw")).unwrap();
    std::fs::write(workdir.join("raw/S1.fq"), b"AAAA").unwrap();

    let ck_dir = workdir.join(".oxo-flow");
    std::fs::create_dir_all(&ck_dir).unwrap();
    std::fs::write(workdir.join(".oxo-flow/checkpoint.json"), "{not json").unwrap();

    seed_run_with_workdir("run-drift-corrupt", &drift_toml(), &workdir).await;

    let (status, body) = get("/api/runs/run-drift-corrupt/protected-drift").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("checkpoint unreadable")),
        "corrupt checkpoint must be surfaced as a warning: {body}"
    );
    assert_eq!(body["drifted_files"].as_array().unwrap().len(), 0, "{body}");
}
