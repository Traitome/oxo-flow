use async_trait::async_trait;

pub mod models;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod sqlite;

static GLOBAL_BACKEND: std::sync::OnceLock<std::sync::Arc<dyn StorageBackend>> =
    std::sync::OnceLock::new();

/// Register the active storage backend for the process.
pub fn set_global_backend(backend: std::sync::Arc<dyn StorageBackend>) {
    let _ = GLOBAL_BACKEND.set(backend);
}

/// Retrieve the active storage backend for the process.
pub fn global_backend() -> Option<std::sync::Arc<dyn StorageBackend>> {
    if let Some(b) = GLOBAL_BACKEND.get() {
        return Some(b.clone());
    }
    if let Ok(pool) = sqlite::try_pool() {
        return Some(std::sync::Arc::new(sqlite::SqliteBackend::from_pool(
            pool.clone(),
        )));
    }
    None
}

/// Columns written by every run insert (plain and guarded), in bind order.
///
/// Both the guarded insert and the plain `create_run` insert are built from
/// this one list, so a column added to one path cannot silently drift from
/// the other (audit RB-02).
pub(crate) const RUN_INSERT_COLUMNS: &str = "id, user_id, pipeline_id, pipeline_snapshot, \
     workflow_name, status, phase, dry_run, pid, workdir, config_overrides, \
     artifacts_incomplete, cluster_id, started_at, finished_at, created_at";

/// Run statuses that count as live for admission guards.
pub(crate) const RUN_LIVE_STATUSES_SQL: &str = "('queued', 'running', 'paused')";

/// Build the unguarded run insert used by `create_run`.
pub(crate) fn plain_run_insert_sql(placeholder: impl Fn(usize) -> String) -> String {
    let values: Vec<String> = (1..=16).map(&placeholder).collect();
    format!(
        "INSERT INTO runs ({RUN_INSERT_COLUMNS}) VALUES ({})",
        values.join(", ")
    )
}

/// Build the guarded run insert shared by the workdir and pipeline claims.
///
/// The insert names 16 row values followed by one guard value; the statement
/// only lands while no live run matches `guard_column`. `placeholder(i)`
/// yields the dialect's bind marker for the 1-based i-th value (`?` for
/// SQLite, `$n` for PostgreSQL).
///
/// The `NOT EXISTS` predicate handles the ordinary case; the `ON CONFLICT DO
/// NOTHING` clause is what makes the claim atomic under PostgreSQL READ
/// COMMITTED, where two concurrent transactions can both evaluate
/// `NOT EXISTS` before either commits. It relies on the partial unique
/// indexes below (created by both schema bootstraps):
///
/// - `idx_runs_live_pipeline` on `runs(pipeline_id)` where the run is live,
/// - `idx_runs_live_workdir` on `runs(workdir)` where the run is live.
///
/// A loser inserts zero rows either way and callers map that to the
/// documented `409 RUN_NOT_TERMINAL`.
pub(crate) fn guarded_run_insert_sql(
    guard_column: &str,
    placeholder: impl Fn(usize) -> String,
) -> String {
    let values: Vec<String> = (1..=16).map(&placeholder).collect();
    format!(
        "INSERT INTO runs ({RUN_INSERT_COLUMNS}) SELECT {} WHERE NOT EXISTS \
         (SELECT 1 FROM runs WHERE {guard_column} = {} AND status IN {RUN_LIVE_STATUSES_SQL}) \
         ON CONFLICT DO NOTHING",
        values.join(", "),
        placeholder(17)
    )
}

/// Test-only: an in-memory backend carrying the tables the OAuth unit tests
/// share through the process-global storage handle (which is set once per
/// test process, so every OAuth unit test must use this same schema).
#[cfg(test)]
pub async fn oauth_test_backend() -> std::sync::Arc<dyn StorageBackend> {
    // The global handle is set-once, so every caller must get the SAME
    // backend — otherwise a caller that loses the set race would write rows
    // into its private pool while verification reads the global one.
    static INIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static BACKEND: std::sync::OnceLock<std::sync::Arc<dyn StorageBackend>> =
        std::sync::OnceLock::new();
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let _guard = INIT.lock().await;
    if let Some(backend) = BACKEND.get() {
        return backend.clone();
    }
    if let Some(backend) = global_backend() {
        let _ = BACKEND.set(backend.clone());
        return backend;
    }
    // File-backed, like `tests/common`: each `#[tokio::test]` runs on its
    // own runtime, so connections to a `sqlite::memory:` pool die with the
    // first test's runtime and the next test would see an empty database.
    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("oauth test temp dir"));
    let path = dir.path().join("oauth-unit-test.db");
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("oauth test pool");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS oauth_states (state TEXT PRIMARY KEY, \
         provider TEXT NOT NULL DEFAULT '', redirect_uri TEXT NOT NULL DEFAULT '', \
         binding_hash TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("oauth_states schema");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, \
         role TEXT NOT NULL, auth_type TEXT NOT NULL, os_user TEXT, \
         password_hash TEXT, created_at TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("users schema");
    // The OAuth callback records a REQUIRED sign-in audit row (WEB-15), so
    // the shared test schema must carry the audit table the production
    // bootstrap always creates.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit_logs (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, \
         action TEXT NOT NULL, target TEXT NOT NULL, result TEXT NOT NULL DEFAULT 'success', \
         metadata TEXT, timestamp TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("audit_logs schema");
    // The audit chain (issue #1372) is written in the same transaction as
    // every audit row; the shared fixture must carry it or the OAuth callback's
    // REQUIRED sign-in row would fail closed.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit_chain (seq INTEGER PRIMARY KEY, audit_id TEXT NOT NULL, \
         phase TEXT NOT NULL, user_id TEXT NOT NULL, action TEXT NOT NULL, target TEXT NOT NULL, \
         result TEXT NOT NULL, metadata TEXT, timestamp TEXT NOT NULL, prev_hash TEXT NOT NULL, \
         hash TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("audit_chain schema");
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_chain_audit_phase \
         ON audit_chain(audit_id, phase)",
    )
    .execute(&pool)
    .await
    .expect("audit_chain unique index");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit_chain_head (id INTEGER PRIMARY KEY CHECK (id = 1), \
         seq INTEGER NOT NULL, hash TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("audit_chain_head schema");
    sqlx::query("INSERT OR IGNORE INTO audit_chain_head (id, seq, hash) VALUES (1, 0, ?)")
        .bind(oxo_flow_core::audit_chain::GENESIS_HASH)
        .execute(&pool)
        .await
        .expect("audit_chain_head genesis");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, \
         created_at TEXT NOT NULL, expires_at TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("sessions schema");
    let backend: std::sync::Arc<dyn StorageBackend> =
        std::sync::Arc::new(sqlite::SqliteBackend::from_pool(pool));
    let _ = BACKEND.set(backend.clone());
    set_global_backend(backend.clone());
    backend
}

#[derive(Debug, Clone)]
pub struct Pagination {
    pub page: usize,
    pub per_page: usize,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: 20,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Paginated<T: serde::Serialize> {
    pub items: Vec<T>,
    pub page: usize,
    pub per_page: usize,
    pub total_items: u64,
    pub total_pages: u64,
}

/// Outcome of [`StorageBackend::update_pipeline_cas`] (issue #1136).
///
/// The three states are mutually exclusive by construction — the storage
/// layer classifies a zero-row compare-and-swap inside the same call, so
/// callers never need a second read to tell a stale token from a deleted
/// pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineWriteOutcome {
    /// The guard matched; the row (and its revision snapshot, when one was
    /// requested) committed. Carries the new stored revision.
    Applied { revision: i64 },
    /// The stored revision did not equal `expected_revision`; nothing was
    /// written. Carries the revision now stored, for the client to re-read.
    Conflict { current_revision: i64 },
    /// No pipeline row with this id exists; nothing was written.
    NotFound,
}

/// The #207 migration surface: one storage abstraction the domains route
/// through, so a PostgreSQL backend becomes a matter of wiring, not rewrites.
///
/// Scope — users, auth sessions, API keys, OAuth CSRF state, pipelines and
/// revisions, templates, shares, run-record metadata, clusters, chat history,
/// audit logging, webhook configuration, and AI provider configuration all
/// consume this trait. The `/api/runs*` domain gates on an initialized
/// backend of either kind with `RUNS_REQUIRE_STORAGE_BACKEND` — the check
/// passes for SQLite or PostgreSQL and fires only when NO backend is
/// initialized; PostgreSQL deployments advertise the served/gated split
/// through `PG_CAPABILITY_MATRIX`. A new domain handler
/// must call the trait, not add another raw-SQL path.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn init(&self) -> Result<(), String>;
    async fn health(&self) -> Result<bool, String>;

    async fn create_user(
        &self,
        username: &str,
        role: &str,
        password_hash: Option<&str>,
    ) -> Result<models::UserRow, String>;
    async fn get_user_by_id(&self, id: &str) -> Result<Option<models::UserRow>, String>;
    async fn get_user_by_username(&self, username: &str)
    -> Result<Option<models::UserRow>, String>;
    async fn list_users(&self) -> Result<Vec<models::UserRow>, String>;
    async fn delete_user(&self, id: &str) -> Result<(), String>;

    async fn save_pipeline(&self, p: &models::PipelineRow) -> Result<models::PipelineRow, String>;
    /// Insert a NEW pipeline row and its initial revision snapshot in one
    /// transaction (issue #1136): either both commit or the call errors and
    /// neither is visible. Replaces the old `save_pipeline` + best-effort
    /// `record_pipeline_revision` sequence on the create path.
    ///
    /// `p.id` must be set by the caller (the create handler generates it).
    async fn create_pipeline_with_revision(&self, p: &models::PipelineRow) -> Result<(), String>;
    /// Atomic compare-and-swap of an existing pipeline row plus an optional
    /// pre-write revision snapshot, in ONE transaction (issue #1136).
    ///
    /// The write applies only when the stored `revision` equals
    /// `expected_revision`; otherwise nothing is written. `None` restores
    /// the legacy unconditional update (backward compatibility for clients
    /// that do not send a token) — the revision still advances by one.
    /// When `snapshot` is `Some`, its insert (and the 50-row retention prune)
    /// shares the transaction: if either fails, the pipeline update is rolled
    /// back and the call errors, so a request can never report success
    /// without its revision row.
    async fn update_pipeline_cas(
        &self,
        p: &models::PipelineRow,
        expected_revision: Option<i64>,
        snapshot: Option<&models::RevisionSnapshot<'_>>,
    ) -> Result<PipelineWriteOutcome, String>;
    async fn get_pipeline(&self, id: &str) -> Result<Option<models::PipelineRow>, String>;
    async fn list_pipelines(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::PipelineRow>, String>;
    async fn delete_pipeline(&self, id: &str) -> Result<(), String>;

    async fn create_run(&self, run: &models::RunRow) -> Result<models::RunRow, String>;
    async fn create_run_if_workdir_idle(&self, run: &models::RunRow) -> Result<bool, String>;
    /// Insert a queued run only while no live run exists for the same saved
    /// pipeline. Runs of one saved pipeline are admitted one at a time so a
    /// pipeline's run history stays a linear sequence; ad-hoc runs are
    /// isolated by their unique workdir and do not take this guard.
    async fn create_run_if_pipeline_idle(&self, run: &models::RunRow) -> Result<bool, String>;
    async fn update_run_status(&self, id: &str, status: &str, phase: &str) -> Result<(), String>;
    async fn get_run(&self, id: &str) -> Result<Option<models::RunRow>, String>;
    async fn list_runs(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::RunRow>, String>;
    async fn mark_run_running(&self, id: &str, started_at: &str) -> Result<bool, String>;
    async fn update_run_pid(&self, id: &str, pid: i64) -> Result<(), String>;
    async fn is_run_cancelled(&self, id: &str) -> Result<bool, String>;
    /// Record a terminal state. Terminal states are final: the first
    /// terminal write wins, so a late failure path can never overwrite a
    /// `cancelled` (or `completed`) row — the cancel-vs-failed race.
    /// Returns `true` when this call made the transition.
    async fn record_run_terminal(
        &self,
        id: &str,
        status: &str,
        phase: &str,
        finished_at: &str,
    ) -> Result<bool, String>;
    /// Compare-and-set run status transition (RB-07 / #1256). Moves a run to
    /// `to` only when its stored status is currently in `from`; the guarded
    /// `WHERE id = ? AND status IN (…)` predicate closes the read-then-write
    /// window that let a pause/resume clobber a concurrently finalized row.
    /// Honors the same timestamp semantics as [`Self::update_run_status`]:
    /// a terminal `to` stamps `finished_at`, a `to` of `"running"` does
    /// `COALESCE(started_at, ?)`. Returns `true` when this call moved the
    /// row, `false` when the row was absent or already left `from` (a lost
    /// race — the caller must treat it as a conflict, never a success).
    async fn transition_run_status(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
        phase: &str,
    ) -> Result<bool, String>;
    /// Record that a run's artifacts could not be fully materialized locally
    /// (`reason` is the result-pull error). Deliberately does NOT touch the
    /// run's status: the note is independent of the terminal state, so a run
    /// that was already `cancelled` (or that a later path finalizes) still
    /// carries the reason. Returns `true` when a row was updated (issue
    /// #1112 / audit RB-05).
    async fn record_artifacts_incomplete(&self, id: &str, reason: &str) -> Result<bool, String>;
    /// Delete a run row outright (RB-03 / #1131). A create-run whose
    /// pre-spawn staging fails after the row was inserted compensates by
    /// removing the row, so a run that never started cannot survive as a
    /// ghost `queued` row that the list/cancel endpoints see but nothing
    /// executes. Dependents with a FK (`run_nodes`) cascade; the durable
    /// quota reservation has no FK and is released separately by the caller.
    async fn delete_run(&self, id: &str) -> Result<(), String>;
    async fn get_run_pid(&self, id: &str) -> Result<Option<i64>, String>;
    async fn list_runs_cursor(
        &self,
        filters: &models::RunFilters,
    ) -> Result<models::CursorPage<models::RunRow>, String>;
    async fn upsert_run_node(&self, node: &models::RunNodeRow) -> Result<(), String>;
    async fn list_run_nodes(&self, run_id: &str) -> Result<Vec<models::RunNodeRow>, String>;
    async fn get_cluster(&self, id: &str) -> Result<Option<models::ClusterRow>, String>;

    async fn create_session(
        &self,
        user_id: &str,
        token: &str,
        expires_at: &str,
    ) -> Result<(), String>;
    async fn get_session(&self, token: &str) -> Result<Option<models::SessionRow>, String>;
    /// Revoke a session, returning the number of rows removed (#1135).
    ///
    /// A `0` means the row was already gone (benign — e.g. a double logout);
    /// any non-zero value is a real revoke. Callers MUST NOT treat an `Err`
    /// as a no-op: a failed delete leaves the token live.
    async fn delete_session(&self, token: &str) -> Result<u64, String>;
    async fn cleanup_expired_sessions(&self) -> Result<u64, String>;

    async fn list_templates(&self) -> Result<Vec<models::TemplateRow>, String>;
    async fn get_template(&self, id: &str) -> Result<Option<models::TemplateRow>, String>;
    async fn save_template(&self, t: &models::TemplateRow) -> Result<models::TemplateRow, String>;
    async fn delete_template(&self, id: &str) -> Result<(), String>;

    async fn create_share(&self, share: &models::ShareRow) -> Result<models::ShareRow, String>;
    async fn get_share_by_token(&self, token: &str) -> Result<Option<models::ShareRow>, String>;
    async fn list_shares(&self, pipeline_id: &str) -> Result<Vec<models::ShareRow>, String>;
    async fn revoke_share(&self, id: &str) -> Result<(), String>;

    async fn log_action(&self, user_id: &str, action: &str, target: &str) -> Result<(), String>;

    async fn get_ai_provider_config(
        &self,
        user_id: Option<&str>,
    ) -> Result<Option<models::AiProviderConfigRow>, String>;
    /// Persist an AI provider row and return the row AS STORED.
    ///
    /// When `preserve_api_key` is true, an existing row's `api_key` is kept
    /// and `config.api_key` is ignored. Both backends re-read the row after
    /// the write, so the return value reflects the durable key/provider
    /// values (callers rely on this for blank-key saves and runtime
    /// activation — a backend returning the input row would reintroduce the
    /// blank-key activation bug).
    async fn save_ai_provider_config(
        &self,
        config: &models::AiProviderConfigRow,
        preserve_api_key: bool,
    ) -> Result<models::AiProviderConfigRow, String>;

    async fn create_api_key(&self, key: &models::ApiKeyRow) -> Result<models::ApiKeyRow, String>;
    async fn list_api_keys(&self, user_id: &str) -> Result<Vec<models::ApiKeyRow>, String>;
    async fn revoke_api_key(&self, id: &str, user_id: &str) -> Result<bool, String>;
    async fn resolve_api_key(
        &self,
        key_hash: &str,
        touched_at: &str,
    ) -> Result<Option<(String, String)>, String>;
    async fn delete_api_keys_for_user(&self, user_id: &str) -> Result<u64, String>;

    /// Persist a pending OAuth CSRF state bound to the `provider` and
    /// `redirect_uri` it was issued for (issue #1141) and to the initiating
    /// browser via `binding_hash` (issue #1197). Only the digest of the
    /// browser binding value is stored.
    async fn store_oauth_state(
        &self,
        state: &str,
        provider: &str,
        redirect_uri: &str,
        binding_hash: &str,
        created_at: &str,
    ) -> Result<(), String>;
    /// Atomically consume (delete + return) a pending state. `None` means the
    /// state was never issued or was already consumed (replay).
    async fn consume_oauth_state(
        &self,
        state: &str,
    ) -> Result<Option<models::OAuthStateRow>, String>;
    async fn cleanup_stale_oauth_states(&self, older_than: &str) -> Result<u64, String>;

    async fn ensure_env_user(
        &self,
        username: &str,
        role: &str,
    ) -> Result<Option<(String, String)>, String>;
    async fn ensure_oauth_user(
        &self,
        oauth_user_id: &str,
        oauth_username: &str,
        fallback_username: &str,
    ) -> Result<String, String>;
    /// Convenience: record a successful audit action without metadata.
    ///
    /// Request-handler call sites go through
    /// `crate::audit::{audit_required, audit_best_effort}` so the audit's
    /// durability class is explicit (WEB-15 / issue #1140); since #1199 no
    /// handler calls this method. It remains for the storage contract and
    /// storage-level tests.
    async fn log_action_with_metadata(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
        metadata: Option<&str>,
    ) -> Result<(), String>;

    /// Low-level audit row writer — the ONE insert into `audit_logs`.
    ///
    /// Do not call this directly from request handlers: use
    /// `crate::audit::{audit_required, audit_best_effort}` so a failed write
    /// fails the request closed (required) or warns (best-effort) per the
    /// classification in `crate::audit::audit_durability_for`. Since #1199
    /// no handler calls this method — `crate::audit::write_audit` is the only
    /// production caller, and this stays public for the storage contract and
    /// storage-level tests.
    async fn record_audit_log(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
        result: &str,
        metadata: Option<&str>,
    ) -> Result<(), String>;
    /// Write-ahead audit intent (issue #1311 / WEB-15 D1): durably insert a
    /// `pending` audit row (id + actor + action + target) BEFORE the handler
    /// mutates state, in one transaction with the synthetic-user seed.
    async fn begin_audit_intent(
        &self,
        id: &str,
        user_id: &str,
        action: &str,
        target: &str,
        metadata: Option<&str>,
    ) -> Result<(), String>;
    /// Finalize an intent written by [`Self::begin_audit_intent`]: replace the
    /// `pending` marker with the observed outcome (`success` / `failure`).
    /// Errors when the row is gone.
    async fn finalize_audit_intent(
        &self,
        id: &str,
        result: &str,
        metadata: Option<&str>,
    ) -> Result<(), String>;
    async fn list_audit_logs(
        &self,
        cutoff: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<models::AuditLogRow>, i64), String>;
    async fn list_recent_audit_logs(&self, limit: u32) -> Result<Vec<models::AuditLogRow>, String>;
    /// Consistent snapshot of the append-only audit chain plus the full
    /// `audit_logs` table (issue #1372).
    ///
    /// Read in ONE transaction so the head, the events and the current log
    /// rows describe one moment with respect to chain appends — an export
    /// must never splice a head from one instant with events from another.
    /// SQLite's read transaction gives a stable WAL snapshot without blocking
    /// writers; PostgreSQL takes `FOR SHARE` on the head row, which makes
    /// appenders (they take `FOR UPDATE`) wait for the snapshot to finish.
    ///
    /// PostgreSQL READ COMMITTED still snapshots each plain `SELECT`
    /// separately, so a concurrent `users` → `audit_logs` cascade delete can
    /// commit between the event and log reads; that window can surface a
    /// transient `MISSING_LOG_ROW` for an untampered chain. The snapshot is
    /// therefore consistent for chain appends, not fully serialized against
    /// log-row cascade deletes. A missing/deleted head row is an error (the
    /// handlers map it to `500 DB_ERROR`): a structurally broken chain is
    /// fail-closed, not a 200 finding.
    async fn audit_chain_snapshot(&self) -> Result<models::AuditChainSnapshot, String>;
    async fn get_run_metrics(&self) -> Result<(i64, u64), String>;

    async fn get_webhook_config(&self) -> Result<Option<models::WebhookConfigRow>, String>;
    async fn save_webhook_config(
        &self,
        url: &str,
        secret: Option<&str>,
        enabled: bool,
        events: &str,
        signature_scheme: &str,
    ) -> Result<(), String>;

    async fn is_chat_session_writable(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<bool, String>;
    async fn save_chat_user_message(
        &self,
        session_id: &str,
        user_id: &str,
        title: &str,
        message: &str,
    ) -> Result<bool, String>;
    async fn save_chat_assistant_message(
        &self,
        session_id: &str,
        user_id: &str,
        content: &str,
    ) -> Result<bool, String>;
    async fn list_chat_sessions(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Vec<models::ChatSessionRow>, String>;

    async fn list_clusters(&self) -> Result<Vec<models::ClusterRow>, String>;
    async fn save_cluster(&self, cluster: &models::ClusterRow) -> Result<(), String>;
    async fn delete_cluster(&self, id: &str) -> Result<bool, String>;

    async fn get_latest_terminal_run_for_pipeline(
        &self,
        pipeline_id: &str,
    ) -> Result<Option<(String, Option<String>)>, String>;
    async fn record_pipeline_revision(
        &self,
        pipeline_id: &str,
        user_id: &str,
        version: &str,
        toml_content: &str,
    ) -> Result<(), String>;
    async fn list_pipeline_revisions(
        &self,
        pipeline_id: &str,
        limit: u32,
    ) -> Result<Vec<models::PipelineRevisionRow>, String>;
    async fn get_pipeline_revision(
        &self,
        pipeline_id: &str,
        revision_id: &str,
    ) -> Result<Option<models::PipelineRevisionRow>, String>;
    /// Candidate window for `POST /api/pipelines/search` (WEB-13 / issue
    /// #1138): the SQL scopes by owner/shared visibility BEFORE `LIMIT` so
    /// foreign private rows cannot crowd the caller's own results out of the
    /// page (audit #664). It is an optimization, not the policy — the handler
    /// MUST re-filter every returned row with
    /// `workflow::visibility::can_read_pipeline`, which is the single source
    /// of truth. Any change to the predicate's policy must keep this query a
    /// superset of it.
    async fn search_pipelines(
        &self,
        query: &str,
        user_id: Option<&str>,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String>;
    /// Candidate window for `GET /api/pipelines` — see
    /// [`Self::search_pipelines`] for the contract: the handler re-filters
    /// with `workflow::visibility::can_read_pipeline`.
    async fn list_pipelines_visible(
        &self,
        user_id: &str,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String>;

    // ── Durable quota state (RB-09 / issue #1133) ───────────────────────
    //
    // Quota counters used to live in a process-local `Mutex<HashMap>`:
    // admission was a separate check-then-record pair and every restart (or
    // second instance) started from zero. These methods route the state
    // through the same storage abstraction as runs/audit/users.

    /// Atomically admit a run: check the per-user limits and record the
    /// run's resource reservation in ONE transaction.
    ///
    /// Returns `(allowed, usage)` — usage is the post-admission snapshot
    /// when allowed, the current (unchanged) snapshot when rejected. The
    /// check and the reservation are a single compare-and-swap, so two
    /// concurrent admissions cannot both pass under a limit of one. The
    /// reservation insert doubles as the idempotency gate: re-admitting an
    /// already-reserved `run_id` returns `(true, current usage)` without
    /// double-counting.
    ///
    /// `now_ms` is Unix epoch milliseconds; the daily window starts fresh
    /// when the stored `window_started_at` is older than 24 h.
    async fn quota_admit(
        &self,
        run_id: &str,
        user_id: &str,
        threads: u32,
        memory_mb: u64,
        limits: &crate::infra::quota::QuotaConfig,
        now_ms: i64,
    ) -> Result<(bool, crate::infra::quota::QuotaUsage), String>;

    /// Release a run's reservation and decrement its user's counters.
    ///
    /// Idempotent: `Ok(false)` means no reservation existed (already
    /// released, never admitted, or reclaimed by reconciliation) and
    /// nothing was decremented. `DELETE ... RETURNING` (the reservation
    /// lookup and the release gate in one statement) plus the counter
    /// decrement run in one transaction, so concurrent releases cannot
    /// double-decrement.
    async fn quota_release(&self, run_id: &str) -> Result<bool, String>;

    /// A user's current durable usage. `runs_today` reports 0 when the
    /// stored 24 h window has elapsed (the window is lazily restarted on
    /// the next admission, so a read never writes).
    async fn quota_usage(
        &self,
        user_id: &str,
        now_ms: i64,
    ) -> Result<crate::infra::quota::QuotaUsage, String>;

    /// Persist the quota limits (single row) so every instance enforces the
    /// same configuration after restart.
    async fn save_quota_config(
        &self,
        config: &crate::infra::quota::QuotaConfig,
    ) -> Result<(), String>;

    /// Load the persisted quota limits; `None` means the database has never
    /// stored a configuration (callers fall back to the process default).
    async fn load_quota_config(&self) -> Result<Option<crate::infra::quota::QuotaConfig>, String>;

    /// Reservations whose run row is terminal or missing — released at
    /// startup so a crash between admission and the run-row insert cannot
    /// pin quota forever.
    async fn quota_stale_reservations(&self) -> Result<Vec<String>, String>;
}
