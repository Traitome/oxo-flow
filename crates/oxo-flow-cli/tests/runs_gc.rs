//! Integration tests for `oxo-flow runs gc` (issue #1228): the safety-guard
//! matrix end to end against a real SQLite store and a real workspace tree.
//!
//! Seeding goes through the shared storage + workspace APIs (never retyped
//! SQL or a hand-built directory layout), and the sweep runs in a spawned
//! `oxo-flow` binary so it sees its own `DATABASE_URL` / `OXO_FLOW_WORKSPACE`
//! — the test process only sets its own env (integration test crates carry no
//! `forbid(unsafe_code)`; that attribute lives in `src/main.rs`), never the
//! child's.
//!
//! Guards covered here (the unit tests in `commands::runs_gc` pin the pure
//! decision logic):
//!
//! 1. a stale, terminal run's tree IS reclaimed;
//! 2. a live run's tree is NOT touched;
//! 3. a lineage-target tree (shared `workdir` with a later run) is NOT touched;
//! 4. `--dry-run` reports the same selection but deletes nothing;
//! 5. the `--older-than` age gate is honored at its boundary.

use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::models::RunRow;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use std::path::PathBuf;
use std::process::Command;

const SNAPSHOT: &str = r#"
[workflow]
name = "t"

[[rules]]
name = "r"
shell = "true"
output = ["o.txt"]
"#;

const OLD: &str = "2020-01-01T00:00:00Z";

fn run_row(
    id: &str,
    user_id: &str,
    status: &str,
    workdir: Option<String>,
    finished_at: Option<&str>,
    started_at: Option<&str>,
) -> RunRow {
    RunRow {
        id: id.to_string(),
        user_id: user_id.to_string(),
        pipeline_id: None,
        pipeline_snapshot: SNAPSHOT.to_string(),
        workflow_name: None,
        status: status.to_string(),
        phase: "reporting".to_string(),
        dry_run: false,
        pid: None,
        workdir,
        config_overrides: None,
        artifacts_incomplete: None,
        started_at: started_at.map(str::to_string),
        finished_at: finished_at.map(str::to_string),
        created_at: OLD.to_string(),
    }
}

/// Materialize a run tree and return its path as the `workdir` string the row
/// would carry (the canonical `runs/<run_id>/` directory).
fn materialize(user_id: &str, run_id: &str) -> PathBuf {
    let dir = oxo_flow_web::workspace::initialize_sandbox(user_id, run_id, SNAPSHOT)
        .expect("sandbox initializes");
    std::fs::write(dir.join("output.txt"), b"payload").expect("write output");
    dir
}

fn spawn_gc(db_url: &str, ws: &str, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    cmd.args(["runs", "gc"]).args(extra);
    cmd.env("DATABASE_URL", db_url);
    cmd.env("OXO_FLOW_WORKSPACE", ws);
    cmd.output().expect("oxo-flow runs gc spawns")
}

#[tokio::test]
async fn runs_gc_guard_matrix_end_to_end() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("ws");
    let db_path = tmp.path().join("runs.db");
    let db_url = format!("sqlite://{}", db_path.display());
    let ws_str = ws.to_string_lossy().into_owned();
    // The test process resolves workspace paths through OXO_FLOW_WORKSPACE so
    // `initialize_sandbox` writes under the temp dir, never the source tree.
    // SAFETY: single-threaded seeding section; no other thread reads this var.
    unsafe { std::env::set_var("OXO_FLOW_WORKSPACE", &ws_str) };

    let backend = SqliteBackend::new(&db_url).await.expect("backend");
    backend.init().await.expect("schema");
    let user = backend
        .create_user("alice", "user", None)
        .await
        .expect("user row");

    // (1) stale, terminal run — eligible for reclamation.
    let eligible_dir = materialize(&user.id, "run-eligible");
    // (2) live run — must never be swept even though its tree is old.
    let live_dir = materialize(&user.id, "run-live");
    // (3) lineage pair: predecessor + a later retry sharing the same workdir.
    let lineage_dir = materialize(&user.id, "run-pred");
    // (5) fresh terminal run — kept at 90d, eligible at `--older-than 0`.
    let fresh_dir = materialize(&user.id, "run-fresh");

    let now = chrono::Utc::now().to_rfc3339();
    let lineage_wd = lineage_dir.to_string_lossy().into_owned();
    for row in [
        run_row(
            "run-eligible",
            &user.id,
            "completed",
            Some(eligible_dir.to_string_lossy().into_owned()),
            Some(OLD),
            Some(OLD),
        ),
        run_row(
            "run-live",
            &user.id,
            "running",
            Some(live_dir.to_string_lossy().into_owned()),
            None,
            Some(&now),
        ),
        run_row(
            "run-pred",
            &user.id,
            "completed",
            Some(lineage_wd.clone()),
            Some(OLD),
            Some(OLD),
        ),
        run_row(
            "run-succ",
            &user.id,
            "completed",
            Some(lineage_wd.clone()),
            Some(&now),
            Some(&now),
        ),
        run_row(
            "run-fresh",
            &user.id,
            "completed",
            Some(fresh_dir.to_string_lossy().into_owned()),
            Some(&now),
            Some(&now),
        ),
    ] {
        backend.create_run(&row).await.expect("run row");
    }
    drop(backend); // release the pool before the child writes migrations

    // ---- dry run: selection reported, nothing deleted -------------------
    let out = spawn_gc(&db_url, &ws_str, &["--dry-run", "--json"]);
    assert!(out.status.success(), "dry-run exit: {:?}", out.status);
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("dry-run emits JSON");
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["reclaimed_count"], 1, "only run-eligible: {report}");
    assert!(eligible_dir.exists(), "dry-run must delete nothing");
    assert!(live_dir.exists());
    assert!(lineage_dir.exists());
    assert!(fresh_dir.exists());

    // ---- real sweep at the default 90d gate -----------------------------
    let out = spawn_gc(&db_url, &ws_str, &["--json"]);
    assert!(out.status.success(), "sweep exit: {:?}", out.status);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sweep JSON");
    assert_eq!(report["reclaimed_count"], 1, "only run-eligible: {report}");

    // (1) reclaimed.
    assert!(!eligible_dir.exists(), "stale terminal tree reclaimed");
    // (2) live run spared.
    assert!(live_dir.exists(), "live run's tree must survive");
    // (3) lineage target spared.
    assert!(
        lineage_dir.exists(),
        "lineage-target tree must survive a later run's retry"
    );
    // (5) fresh run spared at the 90d default.
    assert!(fresh_dir.exists(), "run newer than 90d must survive");

    // ---- age boundary: --older-than 0 reclaims the fresh run ------------
    let out = spawn_gc(&db_url, &ws_str, &["--older-than", "0", "--json"]);
    assert!(out.status.success(), "boundary exit: {:?}", out.status);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("boundary JSON");
    assert_eq!(
        report["reclaimed_count"], 1,
        "exactly the fresh run at a 0d gate: {report}"
    );
    assert!(!fresh_dir.exists(), "0d gate reclaims the fresh run");
    assert!(live_dir.exists(), "live run still spared at a 0d gate");

    // ---- --user restricts the sweep -------------------------------------
    let out = spawn_gc(
        &db_url,
        &ws_str,
        &["--user", "alice", "--dry-run", "--json"],
    );
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("user JSON");
    assert_eq!(report["user"], "alice");
    assert_eq!(report["scanned"], 5, "all five runs belong to alice");
}

#[test]
fn runs_gc_help_shows_default_age_gate() {
    let out = Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
        .args(["runs", "gc", "--help"])
        .output()
        .expect("help runs");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("[default: 90]"),
        "runs gc --older-than must document the 90-day default: {help}"
    );
    for flag in ["--dry-run", "--older-than", "--user"] {
        assert!(help.contains(flag), "help must mention {flag}: {help}");
    }
}
