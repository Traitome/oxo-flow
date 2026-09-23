//! Audit schema unification + complete run inserts (B3, B4).
//!
//! Production boots `db::init_db` (creates the audit_logs table first) and
//! then `infra::db::sqlite::init_pool`. The two schemas used to disagree on
//! columns, so the sqlite backend's `log_action` INSERT failed at runtime.

use oxo_flow_web::db;
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;

mod common;

// The audit tests initialize each DB layer explicitly (the very thing they
// pin), so the harness's combined `ensure_db` is unused in this binary.
#[allow(dead_code)]
fn ensure_db_placeholder() {
    let _ = common::ensure_db;
}

fn db_url() -> &'static str {
    common::db_url()
}

#[tokio::test]
async fn audit_log_action_survives_both_init_paths() {
    let url = db_url();
    // Production order: the legacy db::init_db creates audit_logs first.
    db::init_db(url).await.expect("db init");
    // Second init path must no-op cleanly and share the same schema.
    let backend = SqliteBackend::new(url).await.expect("backend connect");

    backend
        .log_action("default", "test.action", "test-target")
        .await
        .expect("log_action must succeed with the unified schema");

    // The row must carry both the result and metadata columns.
    let pool = db::pool();
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT action, result, metadata FROM audit_logs WHERE action = 'test.action'",
    )
    .fetch_one(pool)
    .await
    .expect("audit row readable through the db.rs pool");
    assert_eq!(row.0, "test.action");
    assert_eq!(row.1, "success");
    assert!(row.2.is_none());
}

#[tokio::test]
async fn runs_schema_accepts_backend_insert() {
    let url = db_url();
    db::init_db(url).await.expect("db init");

    // The production insert path is the sqlite backend's create_run; the
    // legacy insert_run helper was removed as dead code (its only remaining
    // purpose was this test).
    let backend = SqliteBackend::new(url).await.expect("backend connect");
    backend
        .create_run(&oxo_flow_web::infra::db::models::RunRow {
            id: "run-fill-1".into(),
            user_id: "default".into(),
            pipeline_id: None,
            pipeline_snapshot: String::new(),
            workflow_name: Some("fill-test".into()),
            status: "queued".into(),
            phase: "parsing".into(),
            pid: None,
            workdir: None,
            started_at: None,
            finished_at: None,
            created_at: String::new(),
        })
        .await
        .expect("create_run");

    let pool = db::pool();
    let row: (String, String, String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT pipeline_snapshot, workflow_name, phase, pid, workdir, created_at FROM runs WHERE id = 'run-fill-1'",
    )
    .fetch_one(pool)
    .await
    .expect("run row exists");
    assert_eq!(row.1, "fill-test");
    assert_eq!(row.2, "parsing");
    assert!(row.4.is_none(), "ad-hoc run has no workdir yet");
    assert!(!row.5.is_empty(), "created_at must be populated");
}
