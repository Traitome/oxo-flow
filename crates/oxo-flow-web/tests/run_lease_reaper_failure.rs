//! The reaper's exit-record attribution is read-only and survives a failed
//! terminal write (issue #1402 delta review, MEDIUM).
//!
//! The first fix consumed `.exit-code` before writing the terminal row. If
//! that write then failed (a transient database outage is exactly the
//! campaign's scenario), the true outcome was gone and the next sweep would
//! finalize the run `failed`. The record is now read-only during
//! attribution and removed only after a terminal write succeeds (or the row
//! is already terminal).
//!
//! Scenario 1 proves the retry: with a forced terminal-write failure the
//! sweep leaves the row live and the record in place; the next sweep
//! attributes the preserved `0` as `completed`.
//!
//! Scenario 2 races two reapers on the same row: the terminal write is
//! first-wins, the final state is the record's real outcome, and the
//! record is removed.
//!
//! One test function on purpose: `OXO_FLOW_TEST_FINALIZE_TERMINAL_FAIL` is
//! process-global, so both scenarios run sequentially in one binary.

use chrono::{Duration, Utc};
use oxo_flow_web::infra::db::models::RunRow;

mod common;

fn run_row(id: &str, workdir: &std::path::Path) -> RunRow {
    RunRow {
        id: id.to_string(),
        user_id: "default".to_string(),
        pipeline_id: None,
        pipeline_snapshot: "[workflow]\nname = \"lease\"\n".to_string(),
        workflow_name: Some("lease".to_string()),
        status: "running".to_string(),
        phase: "executing".to_string(),
        dry_run: false,
        pid: None,
        workdir: Some(workdir.to_string_lossy().into_owned()),
        config_overrides: None,
        artifacts_incomplete: None,
        cluster_id: None,
        started_at: Some(Utc::now().to_rfc3339()),
        finished_at: None,
        created_at: Utc::now().to_rfc3339(),
    }
}

async fn status_of(id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
        .bind(id)
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("read run status")
}

#[tokio::test]
async fn a_failed_terminal_write_keeps_the_exit_record_for_the_next_sweep() {
    common::ensure_db().await;
    let backend = oxo_flow_web::infra::db::global_backend().expect("global backend");
    let root = tempfile::tempdir().expect("temp workdirs");
    let old = (Utc::now() - Duration::hours(1)).to_rfc3339();

    // Scenario 1 — the terminal write fails; the record must survive.
    let retry_dir = root.path().join("lease-write-failure");
    std::fs::create_dir_all(&retry_dir).expect("workdir");
    backend
        .create_run(&run_row("lease-write-failure", &retry_dir))
        .await
        .expect("create run");
    std::fs::write(retry_dir.join(".exit-code"), "0").expect("exit record");
    sqlx::query("UPDATE runs SET owner_instance = 'dead-instance', heartbeat_at = ? WHERE id = ?")
        .bind(&old)
        .bind("lease-write-failure")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("force stale lease");

    // SAFETY: this binary contains one test function, so no other thread
    // reads or writes the variable concurrently.
    unsafe { std::env::set_var("OXO_FLOW_TEST_FINALIZE_TERMINAL_FAIL", "1") };
    let reaped = oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref())
        .await
        .expect("reap with a failing terminal write");
    assert_eq!(reaped, 0, "a failed terminal write finalizes nothing");
    assert_eq!(
        status_of("lease-write-failure").await,
        "running",
        "the row stays live for the retry"
    );
    assert!(
        retry_dir.join(".exit-code").exists(),
        "the true outcome must survive a failed terminal write"
    );

    // Scenario 1b — with the write healthy the same record attributes the
    // real outcome, and only then is it removed.
    unsafe { std::env::remove_var("OXO_FLOW_TEST_FINALIZE_TERMINAL_FAIL") };
    let reaped = oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref())
        .await
        .expect("retry reap");
    assert_eq!(reaped, 1, "the retried sweep finalizes the run");
    assert_eq!(
        status_of("lease-write-failure").await,
        "completed",
        "the preserved exit record is attributed on the retry"
    );
    assert!(
        !retry_dir.join(".exit-code").exists(),
        "the record is removed after the terminal write succeeded"
    );

    // Scenario 2 — two reapers race the same row. Both read the same
    // read-only record; the terminal write is first-wins; the final state
    // is the record's real outcome and the record is removed.
    let race_dir = root.path().join("lease-race");
    std::fs::create_dir_all(&race_dir).expect("workdir");
    backend
        .create_run(&run_row("lease-race", &race_dir))
        .await
        .expect("create run");
    std::fs::write(race_dir.join(".exit-code"), "0").expect("exit record");
    sqlx::query("UPDATE runs SET owner_instance = 'dead-instance', heartbeat_at = ? WHERE id = ?")
        .bind(&old)
        .bind("lease-race")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("force stale lease");

    let (a, b) = tokio::join!(
        oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref()),
        oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref()),
    );
    let (a, b) = (a.expect("reaper a"), b.expect("reaper b"));
    assert_eq!(
        status_of("lease-race").await,
        "completed",
        "the record's real outcome wins the terminal write"
    );
    assert!(
        !race_dir.join(".exit-code").exists(),
        "the record is removed once the row is terminal"
    );
    // A raced row may be counted by both sweeps (each observed a terminal
    // row after its own finalization attempt); the count is a log aid, not
    // a partition of work.
    assert!(a <= 1 && b <= 1, "each sweep counts the row at most once");
    assert!(a + b >= 1, "at least one sweep finalized the row");
}
