//! Shared test harness for web integration tests.
//!
//! The production router applies the audit middleware (issue #79 P1-05) to
//! every mutation. Any test that exercises a non-GET endpoint must
//! initialize the database first — call [`ensure_db`] at the top of the
//! test. `db::init_db` delegates to the canonical sqlite backend, so one
//! call covers schema, pool, and the global storage backend.
//!
//! File-backed (not `sqlite::memory:`): in-memory databases are
//! per-connection, so a pooled memory URL would silently fragment the
//! schema across connections.

/// A per-binary file-backed database URL in the temp dir. One database is
/// shared by all tests in a binary (the pool registries are OnceLocks).
pub fn db_url() -> &'static String {
    static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    URL.get_or_init(|| {
        // CARGO_TARGET_TMPDIR is set by cargo for integration-test binaries.
        let dir = std::env::var("CARGO_TARGET_TMPDIR")
            .or_else(|_| std::env::var("OXO_FLOW_TEST_TMPDIR"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let path = format!(
            "{}/oxo-flow-{}-test.db",
            dir.to_string_lossy(),
            std::process::id()
        );
        let _ = std::fs::remove_file(&path);
        // A stale -wal from a previous process gets replayed into a freshly
        // created .db and resurrects rows from a former run.
        let _ = std::fs::remove_file(format!("{path}-wal"));
        let _ = std::fs::remove_file(format!("{path}-shm"));
        format!("sqlite:{path}?mode=rwc")
    })
}

/// Initialize the database once. Concurrent calls are safe: the pool
/// registry is a OnceLock and repeat calls are no-ops.
pub async fn ensure_db() {
    oxo_flow_web::db::init_db(db_url()).await.ok();
}
