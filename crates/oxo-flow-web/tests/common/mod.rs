//! Shared test harness for web integration tests.
//!
//! The production router applies the audit middleware (issue #79 P1-05) to
//! every mutation: it inserts an audit row through the legacy `db::pool()`,
//! which **panics** when uninitialized. Any test that exercises a non-GET
//! endpoint must therefore initialize both DB layers first — call
//! [`ensure_db`] at the top of the test.
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
        format!("sqlite:{path}?mode=rwc")
    })
}

/// Initialize both DB layers once. Concurrent calls are safe: the pool
/// registries are OnceLocks and both pools point at the same file.
pub async fn ensure_db() {
    let url = db_url();
    oxo_flow_web::db::init_db(url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(url).await;
}
