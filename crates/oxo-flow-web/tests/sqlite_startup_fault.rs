//! Fault-injection coverage for SQLite boot failures.

#[tokio::test]
async fn sqlite_init_returns_error_for_an_unusable_database_url() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let blocker = tmp.path().join("not-a-directory");
    std::fs::write(&blocker, b"regular file blocks the database parent")
        .expect("create path blocker");
    let database_url = format!(
        "sqlite:{}?mode=rwc",
        blocker.join("database.sqlite").display()
    );

    let result = oxo_flow_web::infra::db::sqlite::init_pool(&database_url).await;
    assert!(
        result.is_err(),
        "unusable SQLite database URL must return Err, not panic: {result:?}"
    );
}

#[tokio::test]
async fn db_init_propagates_the_canonical_sqlite_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let blocker = tmp.path().join("not-a-directory");
    std::fs::write(&blocker, b"regular file blocks the database parent")
        .expect("create path blocker");
    let database_url = format!(
        "sqlite:{}?mode=rwc",
        blocker.join("database.sqlite").display()
    );

    let result = oxo_flow_web::db::init_db(&database_url).await;
    assert!(
        result.is_err(),
        "db::init_db must propagate SQLite initialization failures"
    );
}
