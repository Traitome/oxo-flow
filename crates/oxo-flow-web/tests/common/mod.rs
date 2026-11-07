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

/// Per-binary scratch directory for test artifacts.
fn scratch_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        // CARGO_TARGET_TMPDIR is set by cargo for integration-test binaries.
        std::env::var("CARGO_TARGET_TMPDIR")
            .or_else(|_| std::env::var("OXO_FLOW_TEST_TMPDIR"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
    })
}

/// A per-binary file-backed database URL in the temp dir. One database is
/// shared by all tests in a binary (the pool registries are OnceLocks).
pub fn db_url() -> &'static String {
    static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    URL.get_or_init(|| {
        let path = format!(
            "{}/oxo-flow-{}-test.db",
            scratch_dir().to_string_lossy(),
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

/// The AI config file this test process is allowed to write.
///
/// In-process tests drive `PUT/POST /api/ai/config*`, which reconfigure the
/// shared registry and persist through `ai_config_path()`. Without a
/// redirect that write lands in the operator's real
/// `$HOME/.oxo-flow/ai_config.json` — an integration test once overwrote it
/// with placeholder values. `ensure_db` points `OXO_FLOW_AI_CONFIG` here.
pub fn ai_config_path() -> &'static std::path::Path {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = scratch_dir().join(format!(
            "oxo-flow-{}-test-ai-config.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    })
}

/// Redirect the AI config writer to this process's scratch directory.
fn isolate_ai_config() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set exactly once during test setup, before any test's first
        // AI config write. Nothing in this test binary depends on a
        // pre-existing value for this variable.
        unsafe {
            std::env::set_var(
                oxo_flow_ai::provider::AI_CONFIG_OVERRIDE_ENV,
                ai_config_path(),
            )
        };
    });
}

/// Point the AI session archive at a per-process scratch directory.
///
/// Integration tests exercise real chat/generation paths; an aborted run
/// used to archive into the operator's `$HOME/.oxo-flow/ai_sessions`, where
/// test records could rotate out the very session under investigation
/// (issue #1040 B1/B3). One shared directory per test binary.
fn isolate_ai_sessions() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir =
            std::env::temp_dir().join(format!("oxo-flow-test-ai-sessions-{}", std::process::id()));
        // SAFETY: set exactly once during test setup. The AI session writer
        // is the only consumer, and no test depends on a pre-existing value
        // for this variable.
        unsafe { std::env::set_var("OXO_FLOW_AI_SESSION_DIR", &dir) };
    });
}

/// Initialize the database once. Concurrent calls are safe: the pool
/// registry is a OnceLock and repeat calls are no-ops. Also isolates the AI
/// session archive and the AI config file from the operator's real ones.
pub async fn ensure_db() {
    isolate_ai_sessions();
    isolate_ai_config();
    oxo_flow_web::db::init_db(db_url()).await.ok();
}
