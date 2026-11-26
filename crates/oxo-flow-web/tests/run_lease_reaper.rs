//! Owner-lease heartbeat and stale-run reaping (issue #1402, regression test
//! for the measured kill -9 ghost row).
//!
//! The measured defect: a web process killed with `kill -9` left its
//! in-flight run `running` forever; only manual cancellation recovered it.
//! The fix gives every run an owner lease renewed by its process; another
//! instance finalizes a live run once the owner's heartbeat is older than
//! the lease TTL. These assertions pin the safety boundaries:
//!
//! - a stale row owned by a silent instance is finalized as `failed` and its
//!   durable quota reservation is released; a finished orphan whose exit
//!   record says 0 is attributed `completed` instead;
//! - a stale-looking row owned by the CURRENT process is never reaped (it is
//!   still monitored in-process);
//! - a row whose owner heartbeat is fresh is never reaped;
//! - terminal rows are never touched;
//! - reaping is idempotent.

use chrono::{Duration, Utc};
use oxo_flow_web::infra::db::models::RunRow;
use oxo_flow_web::infra::quota::QuotaConfig;

mod common;

fn run_row(id: &str, status: &str, workdir: &std::path::Path) -> RunRow {
    RunRow {
        id: id.to_string(),
        user_id: "default".to_string(),
        pipeline_id: None,
        pipeline_snapshot: "[workflow]\nname = \"lease\"\n".to_string(),
        workflow_name: Some("lease".to_string()),
        status: status.to_string(),
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

async fn force_lease(id: &str, owner: &str, heartbeat_at: &str) {
    sqlx::query("UPDATE runs SET owner_instance = ?, heartbeat_at = ? WHERE id = ?")
        .bind(owner)
        .bind(heartbeat_at)
        .bind(id)
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("force lease columns");
}

async fn status_and_heartbeat(id: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, heartbeat_at FROM runs WHERE id = ?")
        .bind(id)
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("read run lease columns")
}

#[tokio::test]
async fn stale_owner_lease_is_reaped_and_fresh_or_self_leases_are_not() {
    common::ensure_db().await;
    let backend = oxo_flow_web::infra::db::global_backend().expect("global backend");
    let root = tempfile::tempdir().expect("temp workdirs");
    let old = (Utc::now() - Duration::hours(1)).to_rfc3339();

    // Arrange — five live rows covering every reaper boundary, plus one
    // completed row that must never be selected.
    for (id, status) in [
        ("lease-target", "running"),
        ("lease-finished", "running"),
        ("lease-self", "running"),
        ("lease-fresh", "running"),
        ("lease-terminal", "completed"),
    ] {
        let workdir = root.path().join(id);
        std::fs::create_dir_all(&workdir).expect("workdir");
        backend
            .create_run(&run_row(id, status, &workdir))
            .await
            .expect("create run");
    }
    // The orphaned CLI wrote its exit record before the reaper looks: its
    // true outcome is `completed`, not `failed`.
    std::fs::write(root.path().join("lease-finished/.exit-code"), "0").expect("exit record");
    std::fs::write(root.path().join("lease-finished/execution.log"), "done\n").expect("log");
    // The target, the finished orphan and the completed row belong to a
    // dead instance and have an old heartbeat; the fresh row belongs to a
    // dead instance but beat now; the self row looks stale but is owned by
    // THIS process.
    force_lease("lease-target", "dead-instance", &old).await;
    force_lease("lease-finished", "dead-instance", &old).await;
    force_lease("lease-terminal", "dead-instance", &old).await;
    force_lease(
        "lease-self",
        oxo_flow_web::infra::run_lease::instance_id(),
        &old,
    )
    .await;
    force_lease("lease-fresh", "dead-instance", &Utc::now().to_rfc3339()).await;

    // The reaped run holds a durable quota reservation that must be released
    // by the terminal write (same path as a live completion).
    let limits = QuotaConfig::default();
    let now_ms = Utc::now().timestamp_millis();
    backend
        .quota_admit("lease-target", "default", 1, 64, &limits, now_ms)
        .await
        .expect("quota admit");
    assert_eq!(
        backend
            .quota_usage("default", now_ms)
            .await
            .expect("usage")
            .active_runs,
        1
    );

    // Act — a heartbeat renews only this process's own live rows.
    let touched = oxo_flow_web::infra::run_lease::heartbeat(backend.as_ref())
        .await
        .expect("heartbeat");
    assert!(touched >= 1, "the self-owned live row is renewed");
    let (_, renewed) = status_and_heartbeat("lease-self").await;
    assert_ne!(renewed.as_deref(), Some(old.as_str()), "heartbeat advanced");

    // Act — one reaper sweep.
    let reaped = oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref())
        .await
        .expect("reap");
    assert_eq!(
        reaped, 2,
        "both dead-owner stale live rows leave the live set"
    );

    // Assert — the target is terminal, the others are untouched.
    assert_eq!(status_and_heartbeat("lease-target").await.0, "failed");
    assert_eq!(
        status_and_heartbeat("lease-finished").await.0,
        "completed",
        "a finished orphan is attributed from its exit record, not failed"
    );
    assert_eq!(
        status_and_heartbeat("lease-self").await.0,
        "running",
        "a process never reaps its own rows"
    );
    assert_eq!(
        status_and_heartbeat("lease-fresh").await.0,
        "running",
        "a fresh heartbeat is a live owner"
    );
    assert_eq!(
        status_and_heartbeat("lease-terminal").await.0,
        "completed",
        "terminal rows are never selected"
    );
    assert_eq!(
        backend
            .quota_usage("default", now_ms)
            .await
            .expect("usage")
            .active_runs,
        0,
        "the reaped run's quota reservation is released"
    );

    // Idempotent: a second sweep finds nothing.
    assert_eq!(
        oxo_flow_web::infra::run_lease::reap_stale(backend.as_ref())
            .await
            .expect("second reap"),
        0
    );
}
