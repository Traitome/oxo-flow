//! Router-level tests for `GET /api/runs/{id}/provenance-verify`
//! (issue #276 web surface, CLI→web parity slice 4).
//!
//! All tests drive the production router (`build_router("personal")`)
//! against the shared file-backed DB harness (`common::ensure_db`). The
//! no-spawn branches pin 404 no-leak, 404 NO_WORKDIR, and 404
//! NO_CHECKPOINT. The spawn branches seed a real checkpoint exactly like a
//! `--provenance` run would leave behind and let the handler invoke the
//! actual CLI binary (`find_oxo_flow_binary`), pinning the exit-policy
//! mapping — drift is a 200 finding (`ok: false`), all-matched is `ok:
//! true`, and the no-checksums fallback forwards the CLI's `note` — plus
//! the read-only contract on the checkpoint file.

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

/// Seed a run row pointing at `workdir` (NULL when none is wanted).
async fn seed_run(id: &str, workdir: Option<&std::path::Path>) {
    sqlx::query(
        "INSERT OR REPLACE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, \
         workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, '[workflow]\nname = \"provenance\"\n', \
         'provenance', 'succeeded', 'finished', NULL, ?, NULL, NULL, ?)",
    )
    .bind(id)
    .bind(workdir.map(|p| p.to_string_lossy().into_owned()))
    .bind("2026-01-01T00:00:00Z")
    .execute(pool())
    .await
    .expect("seed run");
}

/// Seed a checkpoint exactly like a `--provenance` run would record: the
/// run's workdir plus one content-addressed output checksum. With `drift`,
/// a second recorded output is left vanished on disk.
fn seed_checkpoint(workdir: &std::path::Path, drift: bool) {
    use oxo_flow_core::executor::CheckpointState;

    let ck_dir = workdir.join(".oxo-flow");
    std::fs::create_dir_all(&ck_dir).unwrap();
    std::fs::write(workdir.join("out.txt"), b"AAAA").unwrap();

    let mut ck = CheckpointState::new();
    ck.set_workdir(workdir);
    let sha = oxo_flow_core::executor::checkpoint::compute_file_checksum(&workdir.join("out.txt"))
        .unwrap();
    ck.record_checksum("out.txt", sha);
    if drift {
        // A vanished output — exactly what integrity verification must catch.
        ck.record_checksum("gone.txt", "sha256:dead".into());
    }
    ck.save_to_file(&CheckpointState::default_path(workdir))
        .unwrap();
}

/// A checkpoint that recorded completed rules but no checksums (a run
/// without `--provenance`): the CLI's note fallback, exit 0.
fn seed_checkpoint_without_checksums(workdir: &std::path::Path) {
    use oxo_flow_core::executor::CheckpointState;

    let ck_dir = workdir.join(".oxo-flow");
    std::fs::create_dir_all(&ck_dir).unwrap();
    let mut ck = CheckpointState::new();
    ck.set_workdir(workdir);
    ck.completed_rules.insert("hello".to_string());
    ck.save_to_file(&CheckpointState::default_path(workdir))
        .unwrap();
}

/// A missing run answers 404 and must not leak other users' runs.
#[tokio::test]
async fn provenance_verify_404_for_an_unknown_run() {
    common::ensure_db().await;

    let (status, body) = get("/api/runs/no-such-run/provenance-verify").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "NOT_FOUND", "{body}");
}

/// A run with no recorded workdir cannot name a checkpoint to verify.
#[tokio::test]
async fn provenance_verify_404_when_the_run_has_no_workdir() {
    common::ensure_db().await;
    seed_run("run-prov-no-wd", None).await;

    let (status, body) = get("/api/runs/run-prov-no-wd/provenance-verify").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "NO_WORKDIR", "{body}");
}

/// No `.oxo-flow/checkpoint.json` in the workdir → 404 NO_CHECKPOINT; the
/// endpoint must not fabricate an empty audit result.
#[tokio::test]
async fn provenance_verify_404_when_the_checkpoint_is_absent() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-no-ckpt");
    std::fs::create_dir_all(&workdir).unwrap();
    seed_run("run-prov-no-ckpt", Some(&workdir)).await;

    let (status, body) = get("/api/runs/run-prov-no-ckpt/provenance-verify").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "NO_CHECKPOINT", "{body}");
}

/// All recorded checksums still match → 200, ok=true (CLI exit 0).
#[tokio::test]
async fn provenance_verify_all_matched_is_ok() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-ok");
    std::fs::create_dir_all(&workdir).unwrap();
    seed_checkpoint(&workdir, false);
    seed_run("run-prov-ok", Some(&workdir)).await;

    let (status, body) = get("/api/runs/run-prov-ok/provenance-verify").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["matched"], 1, "{body}");
    assert_eq!(body["mismatched"], 0, "{body}");
    assert_eq!(body["missing"], 0, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");
}

/// Drift (one matched, one vanished output) → HTTP 200 carrying the
/// finding (`ok: false`), mirroring the CLI's exit 1 = finding, not error.
#[tokio::test]
async fn provenance_verify_reports_drift_as_a_finding() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-drift");
    std::fs::create_dir_all(&workdir).unwrap();
    seed_checkpoint(&workdir, true);
    seed_run("run-prov-drift", Some(&workdir)).await;

    let (status, body) = get("/api/runs/run-prov-drift/provenance-verify").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["matched"], 1, "{body}");
    assert_eq!(body["missing"], 1, "{body}");
    assert_eq!(body["ok"], false, "{body}");
    // Entries are flat in the response (the CLI nests them under "verify").
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{body}");
    assert_eq!(entries[0]["file"], "gone.txt", "{body}");
    assert_eq!(entries[0]["status"], "missing", "{body}");
    assert_eq!(entries[1]["file"], "out.txt", "{body}");
    assert_eq!(entries[1]["status"], "matched", "{body}");
    assert!(body.get("verify").is_none(), "response must be flattened");

    // Read-only contract at the wire level: the checkpoint still holds
    // exactly the seeded checksums (verification must not mutate the
    // audit trail).
    let ck_after = oxo_flow_core::executor::CheckpointState::load_from_file(
        &oxo_flow_core::executor::CheckpointState::default_path(&workdir),
    )
    .expect("checkpoint still readable");
    assert_eq!(ck_after.checksums.len(), 2, "checkpoint unchanged");
}

/// A run spawned without `provenance: true` has no stored checksums — the
/// CLI's note fallback (exit 0, empty entries) forwards as 200 + note.
#[tokio::test]
async fn provenance_verify_without_checksums_reports_the_cli_note() {
    common::ensure_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("wd-note");
    std::fs::create_dir_all(&workdir).unwrap();
    seed_checkpoint_without_checksums(&workdir);
    seed_run("run-prov-note", Some(&workdir)).await;

    let (status, body) = get("/api/runs/run-prov-note/provenance-verify").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["matched"], 0, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    let note = body["note"].as_str().expect("note must be set");
    assert!(note.contains("--provenance"), "note: {note}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 0, "{body}");
}
