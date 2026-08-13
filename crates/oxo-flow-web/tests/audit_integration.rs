//! Audit schema unification + complete run inserts (B3, B4).
//!
//! Production boots `db::init_db` (creates the audit_logs table first) and
//! then `infra::db::sqlite::init_pool`. The two schemas used to disagree on
//! columns, so the sqlite backend's `log_action` INSERT failed at runtime.

use oxo_flow_web::db;
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use std::sync::OnceLock;

static DB_URL: OnceLock<String> = OnceLock::new();

fn db_url() -> &'static str {
    DB_URL.get_or_init(|| {
        let dir = std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let path = format!("{dir}/audit-test.db");
        let _ = std::fs::remove_file(&path);
        format!("sqlite:{path}?mode=rwc")
    })
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
async fn insert_run_fills_all_columns() {
    let url = db_url();
    db::init_db(url).await.expect("db init");

    db::insert_run(&db::Run {
        id: "run-fill-1".into(),
        user_id: "default".into(),
        workflow_name: "fill-test".into(),
        status: "queued".into(),
        pid: None,
        started_at: None,
        finished_at: None,
    })
    .await
    .expect("insert_run");

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
