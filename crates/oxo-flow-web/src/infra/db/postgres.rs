//! PostgreSQL-backed implementation of [`StorageBackend`].
//!
//! Available behind the `postgres` feature flag. Provides the same
//! interface as [`SqliteBackend`] but backed by PostgreSQL for team
//! deployments with >15 concurrent users or multi-server setups.
//!
//! # Feature flag
//!
//! ```toml
//! [features]
//! postgres = ["sqlx/postgres"]
//! ```

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::models;
use super::{Paginated, Pagination, StorageBackend};

/// Global PostgreSQL connection pool.
static PG_POOL: std::sync::OnceLock<PgPool> = std::sync::OnceLock::new();

/// Advisory-lock key serializing schema bootstrap across concurrent
/// callers (parallel test binaries, multi-instance server startup).
/// Any fixed value works as long as every caller of this module
/// shares it.
const PG_INIT_LOCK_KEY: i64 = 0x6F78_6F66; // 'oxof'

/// Initialize the global PostgreSQL pool and run schema migrations.
///
/// Can safely be called multiple times — subsequent calls are no-ops.
///
/// Returns `Err` (instead of panicking) when the connection string is
/// invalid or the initial migration fails, so the server can emit a
/// structured error instead of crashing at startup.
pub async fn init_pool(database_url: &str) -> Result<(), String> {
    if PG_POOL.get().is_some() {
        return Ok(());
    }
    let backend = PostgresBackend::new(database_url).await?;
    backend.init().await?;
    let _ = PG_POOL.set(backend.pool.clone());
    super::set_global_backend(std::sync::Arc::new(backend));
    Ok(())
}

/// Try to obtain a reference to the global PostgreSQL pool.
pub fn try_pool() -> Result<&'static PgPool, String> {
    PG_POOL
        .get()
        .ok_or_else(|| "PostgreSQL pool not initialized — call init_pool() first".to_string())
}

/// PostgreSQL-backed implementation of [`StorageBackend`].
#[derive(Debug, Clone)]
pub struct PostgresBackend {
    pool: PgPool,
}

impl PostgresBackend {
    /// Create a new `PostgresBackend` with a connection pool.
    pub async fn new(database_url: &str) -> Result<Self, String> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| format!("Failed to connect to PostgreSQL: {e}"))?;
        Ok(Self { pool })
    }

    /// Construct a `PostgresBackend` from an existing connection pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }
}

#[async_trait]
impl StorageBackend for PostgresBackend {
    async fn init(&self) -> Result<(), String> {
        // `sqlx::query` uses the extended protocol, which PostgreSQL refuses
        // to prepare when it contains multiple commands. `raw_sql` uses the
        // simple protocol and is the supported path for this idempotent
        // schema bootstrap batch.
        //
        // Concurrent callers (parallel test binaries against one CI
        // postgres, multi-instance server startup) serialize on a
        // transaction-scoped advisory lock: `CREATE TABLE IF NOT EXISTS` is
        // only check-then-create per session, so two sessions racing the
        // same table can fail with `duplicate key value violates unique
        // constraint "pg_type_typname_nsp_index"` (table creation inserts a
        // composite row type into pg_type). The lock is released at commit.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("PG init begin: {e}"))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(PG_INIT_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("PG init lock: {e}"))?;
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                role TEXT NOT NULL,
                auth_type TEXT NOT NULL,
                os_user TEXT,
                password_hash TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                ssh_host TEXT NOT NULL,
                ssh_port BIGINT NOT NULL DEFAULT 22,
                ssh_user TEXT,
                ssh_key TEXT,
                scheduler TEXT,
                remote_dir TEXT,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pipelines (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                toml_content TEXT NOT NULL,
                rules_count BIGINT NOT NULL DEFAULT 0,
                forked_from TEXT,
                visibility TEXT NOT NULL DEFAULT 'private',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                pipeline_id TEXT REFERENCES pipelines(id) ON DELETE CASCADE,
                pipeline_snapshot TEXT NOT NULL,
                workflow_name TEXT,
                status TEXT NOT NULL DEFAULT 'queued',
                phase TEXT NOT NULL DEFAULT 'parsing',
                dry_run BOOLEAN NOT NULL DEFAULT FALSE,
                pid BIGINT,
                workdir TEXT,
                config_overrides TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS run_nodes (
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                rule_name TEXT NOT NULL,
                status TEXT NOT NULL,
                exit_code INTEGER,
                started_at TEXT,
                finished_at TEXT,
                PRIMARY KEY (run_id, rule_name)
            );

            

            CREATE TABLE IF NOT EXISTS sessions (
                token TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS templates (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                description TEXT NOT NULL,
                tags TEXT NOT NULL,
                toml_content TEXT NOT NULL,
                is_system BIGINT NOT NULL DEFAULT 0,
                created_by TEXT,
                usage_count BIGINT NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS shares (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                token TEXT UNIQUE NOT NULL,
                visibility TEXT NOT NULL,
                expires_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS audit_logs (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                action TEXT NOT NULL,
                target TEXT NOT NULL,
                result TEXT NOT NULL DEFAULT 'success',
                metadata TEXT,
                timestamp TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ai_provider_config (
                id TEXT PRIMARY KEY,
                user_id TEXT UNIQUE,
                provider TEXT NOT NULL,
                api_key TEXT DEFAULT '',
                api_url TEXT DEFAULT '',
                model TEXT DEFAULT '',
                search_enabled BIGINT NOT NULL DEFAULT 1,
                monitor_enabled BIGINT NOT NULL DEFAULT 1,
                auto_retry_enabled BIGINT NOT NULL DEFAULT 1,
                max_correction_rounds BIGINT NOT NULL DEFAULT 3,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT 'New Chat',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                meta TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_used_at TEXT,
                revoked BIGINT NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);
            CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);

            CREATE TABLE IF NOT EXISTS oauth_states (
                state TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS webhook_config (
                id BIGINT PRIMARY KEY CHECK (id = 1),
                url TEXT NOT NULL DEFAULT '',
                secret TEXT,
                enabled BIGINT NOT NULL DEFAULT 0,
                events TEXT NOT NULL DEFAULT '[]',
                signature_scheme TEXT NOT NULL DEFAULT 'sha256-keyed',
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pipeline_revisions (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                user_id TEXT NOT NULL DEFAULT 'default',
                version TEXT NOT NULL DEFAULT '0.1.0',
                toml_content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_pipeline_revisions_pipeline ON pipeline_revisions(pipeline_id, created_at DESC);

            -- Indexes (parity with the SQLite init in sqlite.rs — keep the
            -- index list in sync there when adding one here).
            CREATE INDEX IF NOT EXISTS idx_pipelines_user_id ON pipelines(user_id);
            CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
            CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);
            CREATE INDEX IF NOT EXISTS idx_runs_created_at ON runs(created_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
            CREATE INDEX IF NOT EXISTS idx_templates_category ON templates(category);
            CREATE INDEX IF NOT EXISTS idx_shares_pipeline_id ON shares(pipeline_id);
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_user_id ON chat_sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_shares_token ON shares(token);
            CREATE INDEX IF NOT EXISTS idx_audit_logs_user_id ON audit_logs(user_id);
            CREATE INDEX IF NOT EXISTS idx_audit_logs_timestamp ON audit_logs(timestamp DESC);

            -- One-time repair for databases created before seeding was
            -- name-guarded (same repair as the SQLite init, in PG dialect:
            -- no `rowid`, so dedup by `ctid` — keep the oldest physical row
            -- per name). Must run BEFORE the partial unique index below.
            DELETE FROM templates a
             USING templates b
             WHERE a.is_system = 1
               AND b.is_system = 1
               AND a.name = b.name
               AND a.ctid > b.ctid;

            -- Parity with the SQLite schema: system-template seeding is
            -- name-guarded, so "one row per template name" is a schema
            -- invariant on every backend (partial unique index on system
            -- templates only — user templates may still share names).
            CREATE UNIQUE INDEX IF NOT EXISTS idx_templates_name
                ON templates(name) WHERE is_system = 1;
            "#,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("PG init schema: {e}"))?;

        // Migration for pre-existing postgres databases: the runs table
        // gained `config_overrides` after #930. Run execution remains
        // SQLite-only (#207), but keeping the storage contract healthy lets
        // metadata tools and pre-execution checks work on legacy tables.
        sqlx::query("ALTER TABLE runs ADD COLUMN IF NOT EXISTS config_overrides TEXT")
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("PG migration config_overrides: {e}"))?;

        // Vocabulary migration (parity with the SQLite init): legacy
        // executors wrote `success` for completed runs; the canonical
        // terminal set is completed|failed|cancelled. Idempotent — safe to
        // run on every init.
        sqlx::query("UPDATE runs SET status = 'completed' WHERE status = 'success'")
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("PG migration legacy run status: {e}"))?;

        // AI config flags are part of the public model as i64 (matching
        // SQLite); PostgreSQL INT4 columns cannot decode into that type.
        sqlx::raw_sql(
            "ALTER TABLE ai_provider_config ALTER COLUMN search_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN monitor_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN auto_retry_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN max_correction_rounds TYPE BIGINT;",
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("PG migration AI config integer widths: {e}"))?;

        // Seed admin user — inside the locked transaction so two concurrent
        // inits cannot both observe COUNT(*) = 0 and race the UNIQUE
        // username insert.
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        if count.0 == 0 {
            let admin_id = Uuid::new_v4().to_string();
            let now = Self::now();
            sqlx::query(
                "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&admin_id)
            .bind("admin")
            .bind("admin")
            .bind("password")
            .bind(Some("oxo-flow"))
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        // Seed the `default` pseudo-user (parity with the SQLite init):
        // personal-mode runs carry `user_id = 'default'` and runs.user_id is
        // a foreign key, so the row must exist. `ON CONFLICT DO NOTHING`
        // without a target is the PG equivalent of SQLite's INSERT OR
        // IGNORE — any unique violation skips the insert.
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind("default")
        .bind("default")
        .bind("user")
        .bind("local")
        .bind(Some("oxo-flow"))
        .bind(Self::now())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit()
            .await
            .map_err(|e| format!("PG init commit: {e}"))?;

        Ok(())
    }

    async fn health(&self) -> Result<bool, String> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| true)
            .map_err(|e| e.to_string())
    }

    async fn create_user(
        &self,
        username: &str,
        role: &str,
        password_hash: Option<&str>,
    ) -> Result<models::UserRow, String> {
        // Same rule as the SQLite backend: the username becomes a workspace
        // path component.
        crate::workspace::validate_username(username).map_err(|e| e.to_string())?;
        let id = Uuid::new_v4().to_string();
        let now = Self::now();
        let row = models::UserRow {
            id: id.clone(),
            username: username.to_string(),
            role: role.to_string(),
            auth_type: "password".to_string(),
            os_user: None,
            password_hash: password_hash.map(|s| s.to_string()),
            created_at: now.clone(),
        };
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, password_hash, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&row.id)
        .bind(&row.username)
        .bind(&row.role)
        .bind(&row.auth_type)
        .bind(&row.os_user)
        .bind(password_hash)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(row)
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<models::UserRow>, String> {
        sqlx::query_as::<_, models::UserRow>("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<models::UserRow>, String> {
        sqlx::query_as::<_, models::UserRow>("SELECT * FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_users(&self) -> Result<Vec<models::UserRow>, String> {
        sqlx::query_as::<_, models::UserRow>("SELECT * FROM users ORDER BY created_at ASC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete_user(&self, id: &str) -> Result<(), String> {
        // Cascade the user's live credentials (#520) atomically. If any step
        // fails, keep the user row so a retry can finish the cleanup instead
        // of leaving orphaned sessions or API keys behind.
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM api_keys WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn save_pipeline(&self, p: &models::PipelineRow) -> Result<models::PipelineRow, String> {
        let now = Self::now();
        let id = if p.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            p.id.clone()
        };

        sqlx::query(
            r#"
            INSERT INTO pipelines (id, user_id, name, version, toml_content, rules_count, forked_from, visibility, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT(id) DO UPDATE SET
                name = EXCLUDED.name,
                version = EXCLUDED.version,
                toml_content = EXCLUDED.toml_content,
                rules_count = EXCLUDED.rules_count,
                forked_from = EXCLUDED.forked_from,
                visibility = EXCLUDED.visibility,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&id)
        .bind(&p.user_id)
        .bind(&p.name)
        .bind(&p.version)
        .bind(&p.toml_content)
        .bind(p.rules_count)
        .bind(&p.forked_from)
        .bind(&p.visibility)
        .bind(&p.created_at)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        self.get_pipeline(&id)
            .await
            .and_then(|opt| opt.ok_or_else(|| format!("pipeline {id} disappeared after upsert")))
    }

    async fn get_pipeline(&self, id: &str) -> Result<Option<models::PipelineRow>, String> {
        sqlx::query_as::<_, models::PipelineRow>("SELECT * FROM pipelines WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_pipelines(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::PipelineRow>, String> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pipelines WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let total_items = count.0 as u64;
        let per_page = pagination.per_page.max(1);
        let total_pages = if total_items == 0 {
            1
        } else {
            total_items.div_ceil(per_page as u64)
        };
        let offset = (pagination.page.saturating_sub(1)) * per_page;

        let items = sqlx::query_as::<_, models::PipelineRow>(
            "SELECT * FROM pipelines WHERE user_id = $1 ORDER BY updated_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(Paginated {
            items,
            page: pagination.page,
            per_page,
            total_items,
            total_pages,
        })
    }

    async fn delete_pipeline(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM pipelines WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn create_run(&self, run: &models::RunRow) -> Result<models::RunRow, String> {
        let id = if run.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            run.id.clone()
        };
        let now = Self::now();

        let row = models::RunRow {
            id: id.clone(),
            user_id: run.user_id.clone(),
            pipeline_id: run.pipeline_id.clone(),
            pipeline_snapshot: run.pipeline_snapshot.clone(),
            workflow_name: run.workflow_name.clone(),
            status: run.status.clone(),
            phase: run.phase.clone(),
            dry_run: run.dry_run,
            pid: run.pid,
            workdir: run.workdir.clone(),
            config_overrides: run.config_overrides.clone(),
            started_at: run.started_at.clone(),
            finished_at: run.finished_at.clone(),
            created_at: if run.created_at.is_empty() {
                now
            } else {
                run.created_at.clone()
            },
        };

        sqlx::query(
            "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(&row.id)
        .bind(&row.user_id)
        .bind(&row.pipeline_id)
        .bind(&row.pipeline_snapshot)
        .bind(&row.workflow_name)
        .bind(&row.status)
        .bind(&row.phase)
        .bind(row.dry_run)
        .bind(row.pid)
        .bind(&row.workdir)
        .bind(&row.config_overrides)
        .bind(&row.started_at)
        .bind(&row.finished_at)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(row)
    }

    async fn create_run_if_workdir_idle(&self, run: &models::RunRow) -> Result<bool, String> {
        let result = sqlx::query(
            "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, dry_run, pid, workdir, config_overrides, started_at, finished_at, created_at) \
             SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14 \
             WHERE NOT EXISTS (SELECT 1 FROM runs WHERE workdir = $15 AND status IN ('queued', 'running', 'paused'))",
        )
        .bind(&run.id)
        .bind(&run.user_id)
        .bind(&run.pipeline_id)
        .bind(&run.pipeline_snapshot)
        .bind(&run.workflow_name)
        .bind(&run.status)
        .bind(&run.phase)
        .bind(run.dry_run)
        .bind(run.pid)
        .bind(&run.workdir)
        .bind(&run.config_overrides)
        .bind(&run.started_at)
        .bind(&run.finished_at)
        .bind(&run.created_at)
        .bind(&run.workdir)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() == 1)
    }

    async fn update_run_status(&self, id: &str, status: &str, phase: &str) -> Result<(), String> {
        let now = Self::now();
        let is_terminal = status == "completed" || status == "failed" || status == "cancelled";
        if is_terminal {
            sqlx::query("UPDATE runs SET status = $1, phase = $2, finished_at = $3 WHERE id = $4")
                .bind(status)
                .bind(phase)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        } else if status == "running" {
            // COALESCE keeps the original start time across pause/resume
            // (see the SQLite twin for the rationale).
            sqlx::query(
                "UPDATE runs SET status = $1, phase = $2, started_at = COALESCE(started_at, $3) WHERE id = $4",
            )
                .bind(status)
                .bind(phase)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            sqlx::query("UPDATE runs SET status = $1, phase = $2 WHERE id = $3")
                .bind(status)
                .bind(phase)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn get_run(&self, id: &str) -> Result<Option<models::RunRow>, String> {
        sqlx::query_as::<_, models::RunRow>("SELECT * FROM runs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_runs(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::RunRow>, String> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM runs WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let total_items = count.0 as u64;
        let per_page = pagination.per_page.max(1);
        let total_pages = if total_items == 0 {
            1
        } else {
            total_items.div_ceil(per_page as u64)
        };
        let offset = (pagination.page.saturating_sub(1)) * per_page;

        let items = sqlx::query_as::<_, models::RunRow>(
            "SELECT * FROM runs WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(Paginated {
            items,
            page: pagination.page,
            per_page,
            total_items,
            total_pages,
        })
    }

    async fn mark_run_running(&self, id: &str, started_at: &str) -> Result<bool, String> {
        let res = sqlx::query(
            "UPDATE runs SET status = 'running', phase = 'executing', started_at = $1 WHERE id = $2 AND status = 'queued'",
        )
        .bind(started_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(res.rows_affected() > 0)
    }

    async fn update_run_pid(&self, id: &str, pid: i64) -> Result<(), String> {
        sqlx::query("UPDATE runs SET pid = $1 WHERE id = $2")
            .bind(pid)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn is_run_cancelled(&self, id: &str) -> Result<bool, String> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM runs WHERE id = $1 AND status = 'cancelled'")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        Ok(count.0 > 0)
    }

    async fn record_run_terminal(
        &self,
        id: &str,
        status: &str,
        phase: &str,
        finished_at: &str,
    ) -> Result<bool, String> {
        // First terminal write wins (mirrors models::is_terminal_run_status):
        // cancel-vs-failed races must never rewrite an already-final row.
        let res = sqlx::query(
            "UPDATE runs SET status = $1, phase = $2, finished_at = $3 \
             WHERE id = $4 AND status NOT IN ('completed', 'failed', 'cancelled')",
        )
        .bind(status)
        .bind(phase)
        .bind(finished_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(res.rows_affected() > 0)
    }

    async fn get_run_pid(&self, id: &str) -> Result<Option<i64>, String> {
        let row: Option<(Option<i64>,)> = sqlx::query_as("SELECT pid FROM runs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.and_then(|r| r.0))
    }

    async fn list_runs_cursor(
        &self,
        filters: &models::RunFilters,
    ) -> Result<models::CursorPage<models::RunRow>, String> {
        // Same shared assembler as SQLite, rendered with PostgreSQL's
        // numbered `$N` binds — filter semantics live in one place.
        let q = models::assemble_run_list_query_pg(filters);

        let limit = filters.limit.clamp(1, 500);
        let sql = format!(
            "SELECT * FROM runs{} ORDER BY created_at DESC LIMIT {}",
            q.page_where_sql,
            limit + 1
        );
        let mut query = sqlx::query_as::<_, models::RunRow>(sqlx::AssertSqlSafe(sql));
        for bind in &q.page_binds {
            query = query.bind(bind);
        }
        let rows: Vec<models::RunRow> = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let has_more = rows.len() as i64 > limit;
        let items = if has_more {
            rows[..limit as usize].to_vec()
        } else {
            rows
        };
        let next_cursor = if has_more {
            items.last().map(|r| r.created_at.clone())
        } else {
            None
        };

        let count_sql = format!("SELECT COUNT(*) FROM runs{}", q.filter_where_sql);
        let mut count_query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
        for bind in &q.filter_binds {
            count_query = count_query.bind(bind);
        }
        // Propagate, never swallow: a failed COUNT must not degrade into a
        // fabricated `total: 0` that the client reads as valid data.
        let total: i64 = count_query
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("count runs: {e}"))?;

        Ok(models::CursorPage {
            items,
            next_cursor,
            total: total as u64,
        })
    }

    async fn upsert_run_node(&self, node: &models::RunNodeRow) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO run_nodes (run_id, rule_name, status, exit_code, started_at, finished_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (run_id, rule_name) DO UPDATE SET \
                status = EXCLUDED.status, \
                exit_code = EXCLUDED.exit_code, \
                started_at = COALESCE(EXCLUDED.started_at, run_nodes.started_at), \
                finished_at = EXCLUDED.finished_at",
        )
        .bind(&node.run_id)
        .bind(&node.rule_name)
        .bind(&node.status)
        .bind(node.exit_code)
        .bind(&node.started_at)
        .bind(&node.finished_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list_run_nodes(&self, run_id: &str) -> Result<Vec<models::RunNodeRow>, String> {
        sqlx::query_as::<_, models::RunNodeRow>(
            "SELECT * FROM run_nodes WHERE run_id = $1 ORDER BY rule_name",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn get_cluster(&self, id: &str) -> Result<Option<models::ClusterRow>, String> {
        sqlx::query_as::<_, models::ClusterRow>("SELECT * FROM clusters WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_ai_provider_config(
        &self,
        user_id: Option<&str>,
    ) -> Result<Option<models::AiProviderConfigRow>, String> {
        match user_id {
            Some(user_id) => sqlx::query_as::<_, models::AiProviderConfigRow>(
                "SELECT * FROM ai_provider_config WHERE user_id = $1 \
                     ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string()),
            None => sqlx::query_as::<_, models::AiProviderConfigRow>(
                "SELECT * FROM ai_provider_config WHERE user_id IS NULL \
                     ORDER BY updated_at DESC LIMIT 1",
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string()),
        }
    }

    async fn save_ai_provider_config(
        &self,
        config: &models::AiProviderConfigRow,
        preserve_api_key: bool,
    ) -> Result<models::AiProviderConfigRow, String> {
        let id = if config.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            config.id.clone()
        };
        let now = Self::now();
        let saved = models::AiProviderConfigRow {
            id,
            updated_at: now,
            ..config.clone()
        };

        if let Some(user_id) = &saved.user_id {
            sqlx::query(
                r#"
                INSERT INTO ai_provider_config (
                    id, user_id, provider, api_key, api_url, model,
                    search_enabled, monitor_enabled, auto_retry_enabled,
                    max_correction_rounds, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                ON CONFLICT(user_id) DO UPDATE SET
                    provider = excluded.provider,
                    api_key = CASE WHEN $13 = 1 THEN ai_provider_config.api_key
                                   ELSE excluded.api_key END,
                    api_url = excluded.api_url,
                    model = excluded.model,
                    search_enabled = excluded.search_enabled,
                    monitor_enabled = excluded.monitor_enabled,
                    auto_retry_enabled = excluded.auto_retry_enabled,
                    max_correction_rounds = excluded.max_correction_rounds,
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(&saved.id)
            .bind(user_id)
            .bind(&saved.provider)
            .bind(&saved.api_key)
            .bind(&saved.api_url)
            .bind(&saved.model)
            .bind(saved.search_enabled)
            .bind(saved.monitor_enabled)
            .bind(saved.auto_retry_enabled)
            .bind(saved.max_correction_rounds)
            .bind(&saved.created_at)
            .bind(&saved.updated_at)
            .bind(i64::from(preserve_api_key))
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        } else {
            let updated = sqlx::query(
                "UPDATE ai_provider_config SET provider = $1, \
                 api_key = CASE WHEN $2 = 1 THEN ai_provider_config.api_key ELSE $3 END, \
                 api_url = $4, model = $5, search_enabled = $6, monitor_enabled = $7, \
                 auto_retry_enabled = $8, max_correction_rounds = $9, updated_at = $10 \
                 WHERE user_id IS NULL",
            )
            .bind(&saved.provider)
            .bind(i64::from(preserve_api_key))
            .bind(&saved.api_key)
            .bind(&saved.api_url)
            .bind(&saved.model)
            .bind(saved.search_enabled)
            .bind(saved.monitor_enabled)
            .bind(saved.auto_retry_enabled)
            .bind(saved.max_correction_rounds)
            .bind(&saved.updated_at)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

            if updated.rows_affected() == 0 {
                sqlx::query(
                    "INSERT INTO ai_provider_config (id, user_id, provider, api_key, api_url, model, \
                     search_enabled, monitor_enabled, auto_retry_enabled, \
                     max_correction_rounds, created_at, updated_at) \
                     VALUES ($1, NULL, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                )
                .bind(&saved.id)
                .bind(&saved.provider)
                .bind(&saved.api_key)
                .bind(&saved.api_url)
                .bind(&saved.model)
                .bind(saved.search_enabled)
                .bind(saved.monitor_enabled)
                .bind(saved.auto_retry_enabled)
                .bind(saved.max_correction_rounds)
                .bind(&saved.created_at)
                .bind(&saved.updated_at)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
            }
        }

        self.get_ai_provider_config(saved.user_id.as_deref())
            .await?
            .ok_or_else(|| "AI provider config disappeared after save".to_string())
    }

    async fn create_session(
        &self,
        user_id: &str,
        token: &str,
        expires_at: &str,
    ) -> Result<(), String> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(token)
        .bind(user_id)
        .bind(&now)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn get_session(&self, token: &str) -> Result<Option<models::SessionRow>, String> {
        let now = Self::now();
        sqlx::query_as::<_, models::SessionRow>(
            "SELECT * FROM sessions WHERE token = $1 AND expires_at > $2",
        )
        .bind(token)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn delete_session(&self, token: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn cleanup_expired_sessions(&self) -> Result<u64, String> {
        let now = Self::now();
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
            .bind(&now)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    async fn list_templates(&self) -> Result<Vec<models::TemplateRow>, String> {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY category, name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn get_template(&self, id: &str) -> Result<Option<models::TemplateRow>, String> {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates WHERE id = $1 OR name = $1 LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn save_template(&self, t: &models::TemplateRow) -> Result<models::TemplateRow, String> {
        let now = Self::now();
        let id = if t.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            t.id.clone()
        };

        sqlx::query(
            "INSERT INTO templates (id, name, category, description, tags, toml_content, is_system, created_by, usage_count, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT(id) DO UPDATE SET name = EXCLUDED.name, category = EXCLUDED.category, description = EXCLUDED.description, tags = EXCLUDED.tags, toml_content = EXCLUDED.toml_content, is_system = EXCLUDED.is_system, created_by = EXCLUDED.created_by, usage_count = EXCLUDED.usage_count, updated_at = EXCLUDED.updated_at",
        )
        .bind(&id)
        .bind(&t.name)
        .bind(&t.category)
        .bind(&t.description)
        .bind(&t.tags)
        .bind(&t.toml_content)
        .bind(t.is_system)
        .bind(&t.created_by)
        .bind(t.usage_count)
        .bind(&t.created_at)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        self.get_template(&id)
            .await
            .and_then(|opt| opt.ok_or_else(|| format!("template {id} disappeared after upsert")))
    }

    async fn delete_template(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM templates WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn create_share(&self, share: &models::ShareRow) -> Result<models::ShareRow, String> {
        let id = if share.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            share.id.clone()
        };
        let token = if share.token.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            share.token.clone()
        };

        let now = Self::now();
        let row = models::ShareRow {
            id,
            pipeline_id: share.pipeline_id.clone(),
            owner_id: share.owner_id.clone(),
            token,
            visibility: share.visibility.clone(),
            expires_at: share.expires_at.clone(),
            created_at: now,
        };

        sqlx::query(
            "INSERT INTO shares (id, pipeline_id, owner_id, token, visibility, expires_at, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&row.id)
        .bind(&row.pipeline_id)
        .bind(&row.owner_id)
        .bind(&row.token)
        .bind(&row.visibility)
        .bind(&row.expires_at)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(row)
    }

    async fn get_share_by_token(&self, token: &str) -> Result<Option<models::ShareRow>, String> {
        sqlx::query_as::<_, models::ShareRow>("SELECT * FROM shares WHERE token = $1")
            .bind(token)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_shares(&self, pipeline_id: &str) -> Result<Vec<models::ShareRow>, String> {
        sqlx::query_as::<_, models::ShareRow>(
            "SELECT * FROM shares WHERE pipeline_id = $1 ORDER BY created_at DESC",
        )
        .bind(pipeline_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn revoke_share(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM shares WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn log_action(&self, user_id: &str, action: &str, target: &str) -> Result<(), String> {
        self.log_action_with_metadata(user_id, action, target, None)
            .await
    }

    async fn log_action_with_metadata(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
        metadata: Option<&str>,
    ) -> Result<(), String> {
        self.record_audit_log(user_id, action, target, "success", metadata)
            .await
    }

    // -----------------------------------------------------------------------
    // API Keys
    // -----------------------------------------------------------------------

    async fn create_api_key(&self, key: &models::ApiKeyRow) -> Result<models::ApiKeyRow, String> {
        sqlx::query(
            "INSERT INTO api_keys (id, user_id, name, key_hash, created_at, last_used_at, revoked)              VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&key.id)
        .bind(&key.user_id)
        .bind(&key.name)
        .bind(&key.key_hash)
        .bind(&key.created_at)
        .bind(&key.last_used_at)
        .bind(key.revoked)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(key.clone())
    }

    async fn list_api_keys(&self, user_id: &str) -> Result<Vec<models::ApiKeyRow>, String> {
        let rows = sqlx::query_as::<_, models::ApiKeyRow>(
            "SELECT id, user_id, name, key_hash, created_at, last_used_at, revoked              FROM api_keys              WHERE user_id = $1 AND revoked = 0              ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(rows)
    }

    async fn revoke_api_key(&self, id: &str, user_id: &str) -> Result<bool, String> {
        let result = sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(result.rows_affected() > 0)
    }

    async fn resolve_api_key(
        &self,
        key_hash: &str,
        touched_at: &str,
    ) -> Result<Option<(String, String)>, String> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT k.user_id, u.role FROM api_keys k              LEFT JOIN users u ON u.id = k.user_id              WHERE k.key_hash = $1 AND k.revoked = 0",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let Some((user_id, role)) = row else {
            return Ok(None);
        };

        if let Err(error) = sqlx::query("UPDATE api_keys SET last_used_at = $1 WHERE key_hash = $2")
            .bind(touched_at)
            .bind(key_hash)
            .execute(&self.pool)
            .await
        {
            tracing::warn!(
                error = %error,
                "API key authenticated but last_used_at could not be updated"
            );
        }

        Ok(Some((user_id, role)))
    }

    async fn delete_api_keys_for_user(&self, user_id: &str) -> Result<u64, String> {
        let res = sqlx::query("DELETE FROM api_keys WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(res.rows_affected())
    }

    // -----------------------------------------------------------------------
    // OAuth CSRF State
    // -----------------------------------------------------------------------

    async fn store_oauth_state(&self, state: &str, created_at: &str) -> Result<(), String> {
        sqlx::query("INSERT INTO oauth_states (state, created_at) VALUES ($1, $2)")
            .bind(state)
            .bind(created_at)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn consume_oauth_state(&self, state: &str) -> Result<Option<String>, String> {
        // DELETE ... RETURNING consumes the state atomically: concurrent
        // callbacks cannot both validate the same CSRF token. A delete error
        // fails the callback closed instead of accepting a reusable state.
        let row: Option<(String,)> =
            sqlx::query_as("DELETE FROM oauth_states WHERE state = $1 RETURNING created_at")
                .bind(state)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        Ok(row.map(|(c,)| c))
    }

    async fn cleanup_stale_oauth_states(&self, older_than: &str) -> Result<u64, String> {
        let res = sqlx::query("DELETE FROM oauth_states WHERE created_at < $1")
            .bind(older_than)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(res.rows_affected())
    }

    // -----------------------------------------------------------------------
    // Auth Identity Provisioning & OAuth User Synchronization
    // -----------------------------------------------------------------------

    async fn ensure_env_user(
        &self,
        username: &str,
        role: &str,
    ) -> Result<Option<(String, String)>, String> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at)              VALUES ($1, $2, $3, 'password', '', $4)              ON CONFLICT (username) DO NOTHING",
        )
        .bind(username)
        .bind(username)
        .bind(role)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let row: Option<(String, String)> =
            sqlx::query_as("SELECT id, role FROM users WHERE id = $1")
                .bind(username)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?;

        Ok(row)
    }

    async fn ensure_oauth_user(
        &self,
        oauth_user_id: &str,
        oauth_username: &str,
        fallback_username: &str,
    ) -> Result<String, String> {
        if oauth_user_id.is_empty() || fallback_username.is_empty() {
            return Err("OAuth identity is missing its unique id".into());
        }

        let existing = sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id = $1")
            .bind(oauth_user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("OAuth user lookup failed: {e}"))?;
        if let Some(name) = existing {
            return Ok(name);
        }

        let now = Self::now();
        match sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at)              VALUES ($1, $2, 'user', 'oauth', '', $3)",
        )
        .bind(oauth_user_id)
        .bind(oauth_username)
        .bind(&now)
        .execute(&self.pool)
        .await
        {
            Ok(_) => Ok(oauth_username.to_string()),
            Err(first_err) => {
                let retry =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = $1")
                        .bind(oauth_username)
                        .fetch_one(&self.pool)
                        .await
                        .map(|n| n > 0)
                        .unwrap_or(false);
                if !retry {
                    return Err(format!("Failed to create OAuth user record: {first_err}"));
                }
                sqlx::query(
                    "INSERT INTO users (id, username, role, auth_type, os_user, created_at)                      VALUES ($1, $2, 'user', 'oauth', '', $3)",
                )
                .bind(oauth_user_id)
                .bind(fallback_username)
                .bind(&now)
                .execute(&self.pool)
                .await
                .map_err(|e| format!("Failed to create OAuth user record: {e}"))?;
                Ok(fallback_username.to_string())
            }
        }
    }

    async fn record_audit_log(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
        result: &str,
        metadata: Option<&str>,
    ) -> Result<(), String> {
        let id = Uuid::new_v4().to_string();
        let now = Self::now();
        // The audit_logs.user_id FK (REFERENCES users(id)) rejects any id
        // without a users row — and authenticated ids are not limited to the
        // seeded 'default' user (OAuth/env-provisioned users never pass
        // through here). The synthetic viewer row is therefore functionally
        // required, not bookkeeping garnish; ON CONFLICT (id) DO NOTHING
        // keeps repeat audits of the same id race-tolerant. (SQLite
        // record_audit_log mirrors this with INSERT OR IGNORE — keep the
        // two in step.)
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES ($1, $2, 'viewer', 'audit', NULL, $3) ON CONFLICT (id) DO NOTHING",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, action, target, result, metadata, timestamp) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(action)
        .bind(target)
        .bind(result)
        .bind(metadata)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list_audit_logs(
        &self,
        cutoff: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<models::AuditLogRow>, i64), String> {
        let rows: Vec<models::AuditLogRow> = sqlx::query_as(
            "SELECT id, user_id, action, target, result, metadata, timestamp FROM audit_logs WHERE timestamp >= $1 ORDER BY timestamp DESC LIMIT $2 OFFSET $3",
        )
        .bind(cutoff)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE timestamp >= $1")
                .bind(cutoff)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;

        Ok((rows, total))
    }

    async fn list_recent_audit_logs(&self, limit: u32) -> Result<Vec<models::AuditLogRow>, String> {
        sqlx::query_as(
            "SELECT id, user_id, action, target, result, metadata, timestamp FROM audit_logs ORDER BY timestamp DESC LIMIT $1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn get_run_metrics(&self) -> Result<(i64, u64), String> {
        // Both COUNTs propagate: the trait already returns Result, and the
        // caller owns the display-tier degrade — swallowing here would make
        // that fallback dead code.
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE status IN ('running', 'queued')")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| format!("count active runs: {e}"))?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("count runs: {e}"))?;
        Ok((active, total as u64))
    }

    async fn get_webhook_config(&self) -> Result<Option<models::WebhookConfigRow>, String> {
        sqlx::query_as(
            "SELECT id, url, secret, enabled, events, signature_scheme, updated_at FROM webhook_config WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn save_webhook_config(
        &self,
        url: &str,
        secret: Option<&str>,
        enabled: bool,
        events: &str,
        signature_scheme: &str,
    ) -> Result<(), String> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO webhook_config (id, url, secret, enabled, events, signature_scheme, updated_at)              VALUES (1, $1, $2, $3, $4, $5, $6)              ON CONFLICT(id) DO UPDATE SET              url = EXCLUDED.url,              secret = COALESCE(EXCLUDED.secret, webhook_config.secret),              enabled = EXCLUDED.enabled,              events = EXCLUDED.events,              signature_scheme = EXCLUDED.signature_scheme,              updated_at = EXCLUDED.updated_at",
        )
        .bind(url)
        .bind(secret)
        .bind(enabled as i64)
        .bind(events)
        .bind(signature_scheme)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn is_chat_session_writable(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<bool, String> {
        sqlx::query_scalar::<_, bool>(
            "SELECT NOT EXISTS(SELECT 1 FROM chat_sessions WHERE id = $1 AND user_id != $2)",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn save_chat_user_message(
        &self,
        session_id: &str,
        user_id: &str,
        title: &str,
        message: &str,
    ) -> Result<bool, String> {
        let now = Self::now();
        let msg_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let session = sqlx::query(
            "INSERT INTO chat_sessions (id, user_id, title, created_at, updated_at)              VALUES ($1, $2, $3, $4, $5)              ON CONFLICT(id) DO UPDATE SET title = EXCLUDED.title, updated_at = EXCLUDED.updated_at              WHERE chat_sessions.user_id = EXCLUDED.user_id",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(title)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if session.rows_affected() == 0 {
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO chat_messages (id, session_id, role, content, meta, created_at)              VALUES ($1, $2, 'user', $3, NULL, $4)",
        )
        .bind(&msg_id)
        .bind(session_id)
        .bind(message)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(true)
    }

    async fn save_chat_assistant_message(
        &self,
        session_id: &str,
        user_id: &str,
        content: &str,
    ) -> Result<bool, String> {
        let now = Self::now();
        let msg_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let session =
            sqlx::query("UPDATE chat_sessions SET updated_at = $1 WHERE id = $2 AND user_id = $3")
                .bind(&now)
                .bind(session_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        if session.rows_affected() == 0 {
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO chat_messages (id, session_id, role, content, meta, created_at)              VALUES ($1, $2, 'assistant', $3, NULL, $4)",
        )
        .bind(&msg_id)
        .bind(session_id)
        .bind(content)
        .bind(&now)
        .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(true)
    }

    async fn list_chat_sessions(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Vec<models::ChatSessionRow>, String> {
        sqlx::query_as(
            "SELECT id, user_id, title, created_at, updated_at FROM chat_sessions WHERE user_id = $1 ORDER BY updated_at DESC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn list_clusters(&self) -> Result<Vec<models::ClusterRow>, String> {
        sqlx::query_as("SELECT * FROM clusters ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn save_cluster(&self, cluster: &models::ClusterRow) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO clusters (id, name, ssh_host, ssh_port, ssh_user, ssh_key, scheduler, remote_dir, enabled, created_at)              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)              ON CONFLICT(id) DO UPDATE SET name = EXCLUDED.name, ssh_host = EXCLUDED.ssh_host,              ssh_port = EXCLUDED.ssh_port, ssh_user = EXCLUDED.ssh_user, ssh_key = EXCLUDED.ssh_key,              scheduler = EXCLUDED.scheduler, remote_dir = EXCLUDED.remote_dir, enabled = EXCLUDED.enabled",
        )
        .bind(&cluster.id)
        .bind(&cluster.name)
        .bind(&cluster.ssh_host)
        .bind(cluster.ssh_port)
        .bind(&cluster.ssh_user)
        .bind(&cluster.ssh_key)
        .bind(&cluster.scheduler)
        .bind(&cluster.remote_dir)
        .bind(cluster.enabled)
        .bind(&cluster.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn delete_cluster(&self, id: &str) -> Result<bool, String> {
        let affected = sqlx::query("DELETE FROM clusters WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
        Ok(affected > 0)
    }

    async fn get_latest_terminal_run_for_pipeline(
        &self,
        pipeline_id: &str,
    ) -> Result<Option<(String, Option<String>)>, String> {
        sqlx::query_as(
            "SELECT status, finished_at FROM runs WHERE pipeline_id = $1               AND status IN ('completed', 'failed', 'cancelled')              ORDER BY created_at DESC LIMIT 1",
        )
        .bind(pipeline_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn record_pipeline_revision(
        &self,
        pipeline_id: &str,
        user_id: &str,
        version: &str,
        toml_content: &str,
    ) -> Result<(), String> {
        let now = Self::now();
        let rev_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO pipeline_revisions (id, pipeline_id, user_id, version, toml_content, created_at)              VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&rev_id)
        .bind(pipeline_id)
        .bind(user_id)
        .bind(version)
        .bind(toml_content)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "DELETE FROM pipeline_revisions WHERE pipeline_id = $1 AND id NOT IN              (SELECT id FROM pipeline_revisions WHERE pipeline_id = $2                ORDER BY created_at DESC, id DESC LIMIT 50)",
        )
        .bind(pipeline_id)
        .bind(pipeline_id)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn list_pipeline_revisions(
        &self,
        pipeline_id: &str,
        limit: u32,
    ) -> Result<Vec<models::PipelineRevisionRow>, String> {
        sqlx::query_as(
            "SELECT id, pipeline_id, user_id, version, toml_content, created_at FROM pipeline_revisions              WHERE pipeline_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
        )
        .bind(pipeline_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn get_pipeline_revision(
        &self,
        pipeline_id: &str,
        revision_id: &str,
    ) -> Result<Option<models::PipelineRevisionRow>, String> {
        sqlx::query_as(
            "SELECT id, pipeline_id, user_id, version, toml_content, created_at FROM pipeline_revisions              WHERE pipeline_id = $1 AND id = $2",
        )
        .bind(pipeline_id)
        .bind(revision_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn search_pipelines(
        &self,
        query: &str,
        user_id: Option<&str>,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String> {
        let pattern = format!("%{}%", query);
        // ILIKE here (not LIKE, as in the SQLite backend) is a deliberate
        // dialect choice: SQLite's LIKE is case-insensitive for ASCII but
        // PostgreSQL's is case-sensitive — ILIKE keeps the search behavior
        // equivalent across backends. Do not "normalize" one to the other.
        if is_admin {
            sqlx::query_as(
                "SELECT * FROM pipelines WHERE name ILIKE $1 OR toml_content ILIKE $2 ORDER BY updated_at DESC LIMIT $3",
            )
            .bind(&pattern)
            .bind(&pattern)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())
        } else {
            let uid = user_id.unwrap_or("");
            sqlx::query_as(
                "SELECT * FROM pipelines WHERE (user_id = $1 OR visibility = 'workspace')                  AND (name ILIKE $2 OR toml_content ILIKE $3) ORDER BY updated_at DESC LIMIT $4",
            )
            .bind(uid)
            .bind(&pattern)
            .bind(&pattern)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())
        }
    }

    async fn list_pipelines_visible(
        &self,
        user_id: &str,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String> {
        if is_admin {
            sqlx::query_as("SELECT * FROM pipelines ORDER BY updated_at DESC LIMIT $1")
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| e.to_string())
        } else {
            sqlx::query_as(
                "SELECT * FROM pipelines WHERE user_id = $1 OR visibility = 'workspace' ORDER BY updated_at DESC LIMIT $2",
            )
            .bind(user_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())
        }
    }
}

// Live-PostgreSQL smoke tests (issue #207 evidence upgrade). Compiled only
// with `--features postgres`, and skipped unless `OXO_PG_TEST_URL` points at
// a scratch database — CI wires that env to a `services: postgres` container,
// while a local `make ci` without the env var stays green because the tests
// return early. They pin the schema bootstrap + core StorageBackend CRUD
// against a real server; concurrency, crash-recovery, and multi-instance
// behavior remain out of scope here.
#[cfg(all(test, feature = "postgres"))]
mod tests {
    use super::*;
    use crate::infra::db::models;
    use uuid::Uuid;

    /// Connect to the smoke-test database, or skip the test cleanly when
    /// `OXO_PG_TEST_URL` is unset (local `make ci` has no PostgreSQL).
    async fn connect() -> Option<PostgresBackend> {
        let url = match std::env::var("OXO_PG_TEST_URL") {
            Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
            _ => {
                eprintln!("skipping live-PostgreSQL smoke: OXO_PG_TEST_URL is not set");
                return None;
            }
        };
        let backend = PostgresBackend::new(&url)
            .await
            .expect("connect to live PostgreSQL");
        backend.init().await.expect("schema bootstrap");
        Some(backend)
    }

    /// Usernames become workspace path components (validate_username), so
    /// stay inside the allowed charset; the uuid suffix keeps re-runs against
    /// a persistent database collision-free.
    fn unique_username() -> String {
        format!("smoke-u-{}", Uuid::new_v4().simple())
    }

    fn sample_run(user_id: &str) -> models::RunRow {
        models::RunRow {
            id: String::new(),
            user_id: user_id.to_string(),
            pipeline_id: None,
            pipeline_snapshot: "snapshot-for-smoke".to_string(),
            workflow_name: Some("smoke.oxoflow".to_string()),
            status: "queued".to_string(),
            phase: "parsing".to_string(),
            dry_run: true,
            pid: None,
            workdir: Some("/tmp/smoke-run".to_string()),
            config_overrides: Some(r#"["threads=4"]"#.to_string()),
            started_at: None,
            finished_at: None,
            created_at: String::new(),
        }
    }

    #[tokio::test]
    async fn test_users_runs_and_nodes_smoke() {
        let Some(backend) = connect().await else {
            return;
        };

        assert!(
            backend.health().await.expect("health check"),
            "health returns true against a live server"
        );

        let username = unique_username();
        let user = backend
            .create_user(&username, "user", None)
            .await
            .expect("create_user");
        assert_eq!(user.username, username);
        assert_eq!(user.role, "user");

        let fetched = backend
            .get_user_by_username(&username)
            .await
            .expect("get_user_by_username")
            .expect("created user roundtrips");
        assert_eq!(fetched.id, user.id);

        let run = backend
            .create_run(&sample_run(&user.id))
            .await
            .expect("create_run");
        assert!(!run.id.is_empty(), "create_run fills an empty id");
        assert!(!run.created_at.is_empty(), "create_run fills created_at");

        let stored = backend
            .get_run(&run.id)
            .await
            .expect("get_run")
            .expect("run roundtrips");
        assert!(stored.dry_run, "dry_run BOOLEAN roundtrips");
        assert_eq!(
            stored.config_overrides.as_deref(),
            Some(r#"["threads=4"]"#),
            "config_overrides TEXT roundtrips"
        );

        assert!(
            backend
                .mark_run_running(&run.id, "2026-01-01T00:00:00+00:00")
                .await
                .expect("mark_run_running"),
            "queued → running transition claims the row"
        );
        assert!(
            !backend
                .mark_run_running(&run.id, "2026-01-01T00:00:01+00:00")
                .await
                .expect("second mark_run_running"),
            "a running run is no longer claimable"
        );

        let filters = models::RunFilters {
            user_id: Some(user.id.clone()),
            status: Some("running".to_string()),
            limit: 10,
            ..Default::default()
        };
        let page = backend
            .list_runs_cursor(&filters)
            .await
            .expect("list_runs_cursor");
        assert!(page.total >= 1, "COUNT total propagates");
        assert!(
            page.items.iter().any(|r| r.id == run.id),
            "filtered cursor listing contains the run"
        );

        backend
            .upsert_run_node(&models::RunNodeRow {
                run_id: run.id.clone(),
                rule_name: "align".to_string(),
                status: "running".to_string(),
                exit_code: None,
                started_at: Some("2026-01-01T00:00:00+00:00".to_string()),
                finished_at: None,
            })
            .await
            .expect("upsert_run_node insert");
        backend
            .upsert_run_node(&models::RunNodeRow {
                run_id: run.id.clone(),
                rule_name: "align".to_string(),
                status: "completed".to_string(),
                exit_code: Some(0),
                started_at: None,
                finished_at: Some("2026-01-01T00:05:00+00:00".to_string()),
            })
            .await
            .expect("upsert_run_node update");

        let nodes = backend
            .list_run_nodes(&run.id)
            .await
            .expect("list_run_nodes");
        assert_eq!(nodes.len(), 1, "ON CONFLICT upserts instead of duplicating");
        assert_eq!(nodes[0].status, "completed");
        assert_eq!(nodes[0].exit_code, Some(0));
        assert_eq!(
            nodes[0].started_at.as_deref(),
            Some("2026-01-01T00:00:00+00:00"),
            "COALESCE keeps the original started_at"
        );
        assert_eq!(
            nodes[0].finished_at.as_deref(),
            Some("2026-01-01T00:05:00+00:00")
        );
    }

    #[tokio::test]
    async fn test_audit_webhook_chat_sessions_smoke() {
        let Some(backend) = connect().await else {
            return;
        };

        // record_audit_log self-seeds a synthetic viewer row to satisfy the
        // audit_logs.user_id FK (see the impl comment) — an arbitrary id must
        // be accepted, not rejected.
        backend
            .record_audit_log(
                &format!("smoke-audit-{}", Uuid::new_v4().simple()),
                "test.action",
                "test/target",
                "ok",
                Some(r#"{"k":"v"}"#),
            )
            .await
            .expect("record_audit_log with synthetic user");

        let cutoff = "2000-01-01T00:00:00+00:00";
        let (logs, total) = backend
            .list_audit_logs(cutoff, 100, 0)
            .await
            .expect("list_audit_logs");
        assert!(total >= 1, "audit total counts the recorded row");
        assert!(
            logs.iter().any(|l| l.action == "test.action"),
            "recorded audit row is listed"
        );

        backend
            .save_webhook_config(
                "https://hooks.example/1",
                Some("secret-1"),
                true,
                "run.completed",
                "hmac-sha256",
            )
            .await
            .expect("save_webhook_config");
        let hook = backend
            .get_webhook_config()
            .await
            .expect("get_webhook_config")
            .expect("webhook row exists after save");
        assert_eq!(hook.url, "https://hooks.example/1");
        assert_eq!(hook.secret.as_deref(), Some("secret-1"));
        assert_eq!(hook.enabled, 1);
        assert_eq!(hook.signature_scheme, "hmac-sha256");

        // Save with secret=None must keep the existing secret (COALESCE).
        backend
            .save_webhook_config(
                "https://hooks.example/2",
                None,
                false,
                "run.failed",
                "hmac-sha256",
            )
            .await
            .expect("second save_webhook_config");
        let hook = backend
            .get_webhook_config()
            .await
            .expect("get_webhook_config after update")
            .expect("webhook row still exists");
        assert_eq!(hook.url, "https://hooks.example/2");
        assert_eq!(hook.secret.as_deref(), Some("secret-1"), "secret preserved");
        assert_eq!(hook.enabled, 0);

        let session_id = format!("smoke-s-{}", Uuid::new_v4().simple());
        let owner = backend
            .create_user(&unique_username(), "user", None)
            .await
            .expect("create chat owner");
        assert!(
            backend
                .save_chat_user_message(&session_id, &owner.id, "Title", "hello")
                .await
                .expect("save_chat_user_message"),
            "first message creates the session"
        );
        // A different user hitting the same session id must be refused by
        // the ownership guard (rows_affected == 0 → Ok(false)).
        let outsider = backend
            .create_user(&unique_username(), "user", None)
            .await
            .expect("create chat intruder");
        assert!(
            !backend
                .save_chat_user_message(&session_id, &outsider.id, "Hijack", "no")
                .await
                .expect("cross-user save returns false"),
            "ownership guard rejects a different user on an existing session"
        );

        // Expired session is swept; sessions table is exercised end-to-end.
        backend
            .create_session(
                &owner.id,
                "smoke-expired-token",
                "2000-01-01T00:00:00+00:00",
            )
            .await
            .expect("create_session");
        let swept = backend
            .cleanup_expired_sessions()
            .await
            .expect("cleanup_expired_sessions");
        assert!(swept >= 1, "expired session swept (swept {swept})");
        assert!(
            backend
                .get_session("smoke-expired-token")
                .await
                .expect("get_session")
                .is_none(),
            "swept session no longer resolves"
        );
    }
}
