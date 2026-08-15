use async_trait::async_trait;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::OnceLock;
use uuid::Uuid;

use super::models;
use super::{Paginated, Pagination, StorageBackend};

/// Global SQLite connection pool for backward compatibility.
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Initialize the global pool and run schema migrations.
///
/// Can safely be called multiple times — subsequent calls are no-ops.
pub async fn init_pool(database_url: &str) {
    // Serialize concurrent callers — the runs-table migration drops and
    // renames tables, which must not interleave with another init.
    static INIT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = INIT_LOCK.lock().await;

    if DB_POOL.get().is_some() {
        return;
    }
    // Transient filesystem errors (SQLITE_IOERR) happen on busy ephemeral
    // disks (CI runners under parallel test load); retry a few times before
    // giving up.
    let mut last_err = String::new();
    for attempt in 1..=3 {
        match SqliteBackend::new(database_url).await {
            Ok(backend) => match backend.init().await {
                Ok(()) => {
                    let _ = DB_POOL.set(backend.pool.clone());
                    return;
                }
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }
        tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
    }
    panic!("Failed to initialize database after 3 attempts: {last_err}");
}

/// Obtain a reference to the global pool.
///
/// # Panics
/// Panics if `init_pool` has not been called yet.
pub fn pool() -> &'static SqlitePool {
    DB_POOL
        .get()
        .expect("DB pool not initialized — call init_pool() first")
}

/// Try to obtain a reference to the global pool, returning None if not initialized.
pub fn try_pool() -> Result<&'static SqlitePool, String> {
    DB_POOL
        .get()
        .ok_or_else(|| "DB pool not initialized — call init_pool() first".to_string())
}

/// SQLite-backed implementation of [`StorageBackend`].
#[derive(Debug, Clone)]
pub struct SqliteBackend {
    pool: SqlitePool,
}

impl SqliteBackend {
    /// Create a new `SqliteBackend` with a connection pool.
    ///
    /// For in-memory databases (`:memory:`) the pool uses a single persistent
    /// connection so that all queries share the same schema and data.
    /// Otherwise the pool is configured with up to 5 connections.
    pub async fn new(database_url: &str) -> Result<Self, String> {
        let is_memory = database_url.contains(":memory:");
        let mut opts = SqlitePoolOptions::new().max_connections(if is_memory { 1 } else { 5 });
        if is_memory {
            opts = opts
                .min_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }
        let pool = opts
            .connect(database_url)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { pool })
    }

    /// Return a reference to the inner pool.
    pub fn inner_pool(&self) -> &SqlitePool {
        &self.pool
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Rebuild the legacy `runs` table (where `pipeline_id` is NOT NULL)
    /// with the current nullable schema, normalizing invalid '' values.
    /// SQLite cannot relax a NOT NULL constraint in place. Individual
    /// statements (no multi-statement batches — unsupported by sqlx).
    async fn rebuild_runs_table(&self) -> Result<(), String> {
        // Run every statement on ONE dedicated connection: with a pool, DDL
        // statements can land on different connections, and a connection
        // with a stale schema cache fails the rename with "there is already
        // another table or index with this name".
        let mut conn = self.pool.acquire().await.map_err(|e| e.to_string())?;
        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query(
            r#"CREATE TABLE runs_migrated (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                pipeline_id TEXT,
                pipeline_snapshot TEXT NOT NULL,
                workflow_name TEXT,
                status TEXT NOT NULL DEFAULT 'queued',
                phase TEXT NOT NULL DEFAULT 'parsing',
                pid INTEGER,
                workdir TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE)"#,
        )
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
        sqlx::query(
            r#"INSERT INTO runs_migrated
                (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at)
                SELECT id, user_id, NULLIF(pipeline_id, ''), pipeline_snapshot, NULL, status, phase, pid, workdir, started_at, finished_at, created_at
                FROM runs"#,
        )
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
        sqlx::query("DROP TABLE runs")
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("ALTER TABLE runs_migrated RENAME TO runs")
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Recover runs that were left in the 'running' state after a crash.
    ///
    /// Only marks runs whose `started_at` is older than 60 seconds, to avoid
    /// killing runs that are still initialising after a quick restart.
    pub async fn recover_orphaned_runs(&self) -> Result<u64, String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(60);
        let cutoff_str = cutoff.to_rfc3339();
        let now_str = Self::now();
        let result = sqlx::query(
            "UPDATE runs SET status = 'failed', finished_at = ? WHERE status = 'running' AND started_at < ?",
        )
        .bind(&now_str)
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let count = result.rows_affected();
        if count > 0 {
            tracing::warn!(
                "Recovered {} orphaned run(s) (started before {}). Runs started within the last 60s were left untouched.",
                count,
                cutoff_str,
            );
        }
        Ok(count)
    }
}

#[async_trait]
impl StorageBackend for SqliteBackend {
    async fn init(&self) -> Result<(), String> {
        // Enable WAL mode and foreign keys
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        // Create all 8 tables
        sqlx::query(
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

            CREATE TABLE IF NOT EXISTS pipelines (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                toml_content TEXT NOT NULL,
                rules_count INTEGER NOT NULL DEFAULT 0,
                forked_from TEXT,
                visibility TEXT NOT NULL DEFAULT 'private',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                pipeline_id TEXT,
                pipeline_snapshot TEXT NOT NULL,
                workflow_name TEXT,
                status TEXT NOT NULL DEFAULT 'queued',
                phase TEXT NOT NULL DEFAULT 'parsing',
                pid INTEGER,
                workdir TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE
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
                is_system INTEGER NOT NULL DEFAULT 0,
                created_by TEXT,
                usage_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ai_provider_config (
                id TEXT PRIMARY KEY,
                user_id TEXT UNIQUE,
                provider TEXT NOT NULL,
                api_key TEXT,
                api_url TEXT,
                model TEXT,
                search_enabled INTEGER NOT NULL DEFAULT 1,
                monitor_enabled INTEGER NOT NULL DEFAULT 1,
                auto_retry_enabled INTEGER NOT NULL DEFAULT 0,
                max_correction_rounds INTEGER NOT NULL DEFAULT 3,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS shares (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                token TEXT UNIQUE NOT NULL,
                visibility TEXT NOT NULL,
                expires_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE,
                FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS audit_logs (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                action TEXT NOT NULL,
                target TEXT NOT NULL,
                result TEXT NOT NULL DEFAULT 'success',
                metadata TEXT,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS oauth_states (
                state TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                meta TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL DEFAULT 'default',
                title TEXT NOT NULL DEFAULT 'New Chat',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_used_at TEXT,
                revoked INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS webhook_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                url TEXT NOT NULL DEFAULT '',
                secret TEXT,
                enabled INTEGER NOT NULL DEFAULT 0,
                events TEXT NOT NULL DEFAULT '[]',
                signature_scheme TEXT NOT NULL DEFAULT 'sha256-keyed',
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pipeline_revisions (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL,
                user_id TEXT NOT NULL DEFAULT 'default',
                version TEXT NOT NULL DEFAULT '0.1.0',
                toml_content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        // Indexes
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_pipelines_user_id ON pipelines(user_id);
            CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
            CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);
            CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
            CREATE INDEX IF NOT EXISTS idx_templates_category ON templates(category);
            CREATE INDEX IF NOT EXISTS idx_shares_pipeline_id ON shares(pipeline_id);
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_user_id ON chat_sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_pipeline_revisions_pipeline ON pipeline_revisions(pipeline_id, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_shares_token ON shares(token);
            CREATE INDEX IF NOT EXISTS idx_audit_logs_user_id ON audit_logs(user_id);

            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                ssh_host TEXT NOT NULL,
                ssh_port INTEGER NOT NULL DEFAULT 22,
                ssh_user TEXT,
                ssh_key TEXT,
                scheduler TEXT,
                remote_dir TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_audit_logs_timestamp ON audit_logs(timestamp DESC);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        // Migrations for v0.8: add columns that may be missing from old schema
        for col in [
            "pipeline_id TEXT NOT NULL DEFAULT ''",
            "pipeline_snapshot TEXT NOT NULL DEFAULT ''",
            "phase TEXT NOT NULL DEFAULT 'parsing'",
            "workdir TEXT",
            "created_at TEXT NOT NULL DEFAULT ''",
        ] {
            sqlx::query(&format!("ALTER TABLE runs ADD COLUMN {col}"))
                .execute(&self.pool)
                .await
                .ok();
        }

        // Migration: workflow_name for ad-hoc runs (nullable)
        sqlx::query("ALTER TABLE runs ADD COLUMN workflow_name TEXT")
            .execute(&self.pool)
            .await
            .ok();

        // Migration: legacy DBs have `pipeline_id TEXT NOT NULL` (ad-hoc runs
        // have no saved pipeline, so the column must be nullable). SQLite
        // cannot relax a NOT NULL constraint in place — rebuild the table,
        // normalizing the legacy invalid '' value to NULL.
        let pipeline_notnull: Option<(i64,)> = match sqlx::query_as(
            r#"SELECT "notnull" FROM pragma_table_info('runs') WHERE name = 'pipeline_id'"#,
        )
        .fetch_optional(&self.pool)
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("runs schema check failed ({e}); skipping runs migration");
                None
            }
        };
        if pipeline_notnull == Some((1,))
            && let Err(e) = self.rebuild_runs_table().await
        {
            tracing::warn!(
                "runs table migration failed ({}); ad-hoc runs may not persist",
                e
            );
        }
        // Migration: add usage_count to templates if missing
        sqlx::query("ALTER TABLE templates ADD COLUMN usage_count INTEGER NOT NULL DEFAULT 0")
            .execute(&self.pool)
            .await
            .ok();

        // Migration: webhook signature scheme. Defaults to the legacy
        // keyed-sha256 so v0.10 consumers keep verifying after upgrade;
        // the UI can opt into hmac-sha256 explicitly.
        sqlx::query(
            "ALTER TABLE webhook_config ADD COLUMN signature_scheme TEXT NOT NULL DEFAULT 'sha256-keyed'",
        )
        .execute(&self.pool)
        .await
        .ok();

        // Migration (issue #79 P1-06): user password hashes for DB-created
        // accounts. Idempotent — safe to run on every init.
        sqlx::query("ALTER TABLE users ADD COLUMN password_hash TEXT")
            .execute(&self.pool)
            .await
            .ok();

        // Vocabulary migration: legacy executors wrote `success` for completed
        // runs; the canonical terminal set is completed|failed|cancelled.
        // Idempotent — safe to run on every init.
        sqlx::query("UPDATE runs SET status = 'completed' WHERE status = 'success'")
            .execute(&self.pool)
            .await
            .ok();

        // Seed admin + default users if users table is empty. `default` is
        // the pseudo-user for personal-mode runs (runs.user_id is a
        // foreign key, so it must exist).
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        if count.0 == 0 {
            let admin_id = Uuid::new_v4().to_string();
            let now = Self::now();
            sqlx::query(
                "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&admin_id)
            .bind("admin")
            .bind("admin")
            .bind("password")
            .bind(Some("oxo-flow"))
            .bind(&now)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }

        sqlx::query(
            "INSERT OR IGNORE INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("default")
        .bind("default")
        .bind("user")
        .bind("local")
        .bind(Some("oxo-flow"))
        .bind(Self::now())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn health(&self) -> Result<bool, String> {
        let result: Result<(i64,), _> = sqlx::query_as("SELECT 1").fetch_one(&self.pool).await;
        match result {
            Ok(_) => Ok(true),
            Err(e) => Err(e.to_string()),
        }
    }

    // -----------------------------------------------------------------------
    // Users
    // -----------------------------------------------------------------------

    async fn create_user(&self, username: &str, role: &str) -> Result<models::UserRow, String> {
        // The username becomes a workspace path component — reject anything
        // that could escape the per-user tree before it reaches the DB.
        crate::workspace::validate_username(username).map_err(|e| e.to_string())?;
        let id = Uuid::new_v4().to_string();
        let now = Self::now();
        let row = models::UserRow {
            id: id.clone(),
            username: username.to_string(),
            role: role.to_string(),
            auth_type: "password".to_string(),
            os_user: None,
            created_at: now.clone(),
        };
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.username)
        .bind(&row.role)
        .bind(&row.auth_type)
        .bind(&row.os_user)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(row)
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<models::UserRow>, String> {
        sqlx::query_as::<_, models::UserRow>("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<models::UserRow>, String> {
        sqlx::query_as::<_, models::UserRow>("SELECT * FROM users WHERE username = ?")
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
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Pipelines
    // -----------------------------------------------------------------------

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
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                toml_content = excluded.toml_content,
                rules_count = excluded.rules_count,
                forked_from = excluded.forked_from,
                visibility = excluded.visibility,
                updated_at = excluded.updated_at
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

        // Return the saved row
        self.get_pipeline(&id)
            .await
            .map(|opt| opt.expect("pipeline should exist after upsert"))
    }

    async fn get_pipeline(&self, id: &str) -> Result<Option<models::PipelineRow>, String> {
        sqlx::query_as::<_, models::PipelineRow>("SELECT * FROM pipelines WHERE id = ?")
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
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pipelines WHERE user_id = ?")
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
            "SELECT * FROM pipelines WHERE user_id = ? ORDER BY updated_at DESC LIMIT ? OFFSET ?",
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
        sqlx::query("DELETE FROM pipelines WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Runs
    // -----------------------------------------------------------------------

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
            pid: run.pid,
            workdir: run.workdir.clone(),
            started_at: run.started_at.clone(),
            finished_at: run.finished_at.clone(),
            created_at: if run.created_at.is_empty() {
                now
            } else {
                run.created_at.clone()
            },
        };

        sqlx::query(
            "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.user_id)
        .bind(&row.pipeline_id)
        .bind(&row.pipeline_snapshot)
        .bind(&row.status)
        .bind(&row.phase)
        .bind(row.pid)
        .bind(&row.workdir)
        .bind(&row.started_at)
        .bind(&row.finished_at)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(row)
    }

    async fn update_run_status(&self, id: &str, status: &str, phase: &str) -> Result<(), String> {
        let now = Self::now();
        // If transitioning to a terminal state, set finished_at
        let is_terminal = status == "completed" || status == "failed" || status == "cancelled";
        if is_terminal {
            sqlx::query("UPDATE runs SET status = ?, phase = ?, finished_at = ? WHERE id = ?")
                .bind(status)
                .bind(phase)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        } else if status == "running" {
            // Set started_at when run begins
            sqlx::query("UPDATE runs SET status = ?, phase = ?, started_at = ? WHERE id = ?")
                .bind(status)
                .bind(phase)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            sqlx::query("UPDATE runs SET status = ?, phase = ? WHERE id = ?")
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
        sqlx::query_as::<_, models::RunRow>("SELECT * FROM runs WHERE id = ?")
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
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM runs WHERE user_id = ?")
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
            "SELECT * FROM runs WHERE user_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?",
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

    async fn cancel_run(&self, id: &str) -> Result<(), String> {
        let now = Self::now();
        sqlx::query("UPDATE runs SET status = 'cancelled', finished_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Sessions
    // -----------------------------------------------------------------------

    async fn create_session(
        &self,
        user_id: &str,
        token: &str,
        expires_at: &str,
    ) -> Result<(), String> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
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
            "SELECT * FROM sessions WHERE token = ? AND expires_at > ?",
        )
        .bind(token)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn delete_session(&self, token: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn cleanup_expired_sessions(&self) -> Result<u64, String> {
        let now = Self::now();
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(&now)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    // -----------------------------------------------------------------------
    // Templates
    // -----------------------------------------------------------------------

    async fn list_templates(&self) -> Result<Vec<models::TemplateRow>, String> {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY category, name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn get_template(&self, id: &str) -> Result<Option<models::TemplateRow>, String> {
        sqlx::query_as::<_, models::TemplateRow>("SELECT * FROM templates WHERE id = ?")
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
            "INSERT INTO templates (id, name, category, description, tags, toml_content, is_system, created_by, usage_count, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, category = excluded.category, description = excluded.description, tags = excluded.tags, toml_content = excluded.toml_content, is_system = excluded.is_system, created_by = excluded.created_by, usage_count = excluded.usage_count, updated_at = excluded.updated_at",
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
            .map(|opt| opt.expect("template should exist after upsert"))
    }

    async fn delete_template(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM templates WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Shares
    // -----------------------------------------------------------------------

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
            "INSERT INTO shares (id, pipeline_id, owner_id, token, visibility, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        let now = Self::now();
        sqlx::query_as::<_, models::ShareRow>(
            "SELECT * FROM shares WHERE token = ? AND (expires_at IS NULL OR expires_at > ?)",
        )
        .bind(token)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn list_shares(&self, pipeline_id: &str) -> Result<Vec<models::ShareRow>, String> {
        sqlx::query_as::<_, models::ShareRow>(
            "SELECT * FROM shares WHERE pipeline_id = ? ORDER BY created_at DESC",
        )
        .bind(pipeline_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
    }

    async fn revoke_share(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM shares WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Audit Logs
    // -----------------------------------------------------------------------

    async fn log_action(&self, user_id: &str, action: &str, target: &str) -> Result<(), String> {
        let id = Uuid::new_v4().to_string();
        let now = Self::now();
        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, action, target, result, metadata, timestamp) VALUES (?, ?, ?, ?, 'success', ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(action)
        .bind(target)
        .bind(None::<String>)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db::Pagination;
    use crate::infra::db::models;
    use uuid::Uuid;

    /// A pre-fix database must be migrated on init(): runs.pipeline_id
    /// becomes nullable and workflow_name appears.
    #[tokio::test]
    async fn test_legacy_runs_schema_migrated() {
        let backend = SqliteBackend::new("sqlite::memory:")
            .await
            .expect("Failed to create in-memory backend");
        // Legacy schema: pipeline_id NOT NULL, no workflow_name.
        // Individual statements (sqlx does not batch multi-statement SQL).
        for stmt in [
            "CREATE TABLE users (id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, role TEXT NOT NULL, auth_type TEXT NOT NULL, os_user TEXT, password_hash TEXT, created_at TEXT NOT NULL)",
            "CREATE TABLE pipelines (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, version TEXT NOT NULL, toml_content TEXT NOT NULL, rules_count INTEGER NOT NULL DEFAULT 0, forked_from TEXT, visibility TEXT NOT NULL DEFAULT 'private', created_at TEXT NOT NULL, updated_at TEXT NOT NULL, FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE)",
            "CREATE TABLE runs (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, pipeline_id TEXT NOT NULL, pipeline_snapshot TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued', phase TEXT NOT NULL DEFAULT 'parsing', pid INTEGER, workdir TEXT, started_at TEXT, finished_at TEXT, created_at TEXT NOT NULL, FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE, FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE)",
        ] {
            sqlx::query(stmt)
                .execute(backend.inner_pool())
                .await
                .expect("legacy schema setup");
        }

        backend.init().await.expect("migration should succeed");

        // Ad-hoc run insert (the shape the handler uses) must now work.
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("run-1")
        .bind("default")
        .bind(Option::<String>::None)
        .bind("[workflow]")
        .bind(Some("legacy-test"))
        .bind("queued")
        .bind("parsing")
        .bind(None::<i64>)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(&now)
        .execute(backend.inner_pool())
        .await
        .expect("ad-hoc run insert must work after migration");

        let row: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT pipeline_id, workflow_name FROM runs WHERE id = 'run-1'")
                .fetch_one(backend.inner_pool())
                .await
                .unwrap();
        assert_eq!(row.0, None);
        assert_eq!(row.1.as_deref(), Some("legacy-test"));
    }

    async fn create_backend() -> SqliteBackend {
        let backend = SqliteBackend::new("sqlite::memory:")
            .await
            .expect("Failed to create in-memory backend");
        backend.init().await.expect("Failed to init schema");
        backend
    }

    /// Create a simple user row for testing.
    async fn create_test_user(backend: &SqliteBackend, username: &str) -> models::UserRow {
        let role = if username == "admin" { "admin" } else { "user" };
        backend
            .create_user(username, role)
            .await
            .unwrap_or_else(|e| panic!("Failed to create user '{username}': {e}"))
    }

    /// Create a simple pipeline row for testing.
    async fn create_test_pipeline(
        backend: &SqliteBackend,
        user_id: &str,
        name: &str,
    ) -> models::PipelineRow {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let pipeline = models::PipelineRow {
            id,
            user_id: user_id.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            toml_content: format!("[workflow]\nname = \"{name}\"\nversion = \"1.0.0\""),
            rules_count: 2,
            forked_from: None,
            visibility: "private".to_string(),
            created_at: now.clone(),
            updated_at: now,
        };
        backend
            .save_pipeline(&pipeline)
            .await
            .expect("Failed to save pipeline")
    }

    // -----------------------------------------------------------------------
    // Test: init creates all 8 tables
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_init_creates_tables() {
        let backend = SqliteBackend::new("sqlite::memory:")
            .await
            .expect("Failed to create backend");
        backend.init().await.expect("Failed to init");

        // Verify all 7 tables exist by querying sqlite_master
        let expected_tables = [
            "users",
            "pipelines",
            "runs",
            "sessions",
            "templates",
            "shares",
            "audit_logs",
        ];

        for table in &expected_tables {
            let row: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(table)
                    .fetch_one(backend.inner_pool())
                    .await
                    .unwrap_or_else(|_| panic!("Failed to query for table '{table}'"));
            assert_eq!(
                row.0, 1,
                "Table '{table}' should exist after init but was not found"
            );
        }

        // Verify admin user was seeded
        let admin = backend.get_user_by_username("admin").await.unwrap();
        assert!(admin.is_some(), "Admin user should be seeded");
        assert_eq!(admin.unwrap().role, "admin");
    }

    // -----------------------------------------------------------------------
    // Test: create_user + get_user_by_id roundtrip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_user_roundtrip() {
        let backend = create_backend().await;

        let user = create_test_user(&backend, "alice").await;
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, "user");
        assert_eq!(user.auth_type, "password");
        assert!(user.os_user.is_none());
        assert!(!user.id.is_empty());
        assert!(!user.created_at.is_empty());

        // Fetch by id
        let fetched = backend.get_user_by_id(&user.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.username, "alice");
        assert_eq!(fetched.role, "user");

        // Fetch by username
        let by_username = backend.get_user_by_username("alice").await.unwrap();
        assert!(by_username.is_some());
        assert_eq!(by_username.unwrap().id, user.id);

        // List users (seeded admin + default pseudo-user + alice)
        let users = backend.list_users().await.unwrap();
        assert_eq!(users.len(), 3, "Expected admin + default + alice");

        // Delete user
        backend.delete_user(&user.id).await.unwrap();
        let deleted = backend.get_user_by_id(&user.id).await.unwrap();
        assert!(deleted.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: save_pipeline + get_pipeline roundtrip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_save_pipeline_roundtrip() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "bob").await;

        let pipeline = create_test_pipeline(&backend, &user.id, "my-pipeline").await;
        assert_eq!(pipeline.name, "my-pipeline");
        assert!(!pipeline.id.is_empty());

        // Fetch by id
        let fetched = backend.get_pipeline(&pipeline.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "my-pipeline");

        // Upsert: update name
        let updated = models::PipelineRow {
            name: "my-pipeline-v2".to_string(),
            ..pipeline.clone()
        };
        let saved = backend.save_pipeline(&updated).await.unwrap();
        assert_eq!(saved.name, "my-pipeline-v2");
        // updated_at should have changed
        assert!(saved.updated_at > pipeline.updated_at || saved.updated_at != pipeline.updated_at);

        // Fetch again
        let fetched = backend.get_pipeline(&pipeline.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "my-pipeline-v2");

        // Delete
        backend.delete_pipeline(&pipeline.id).await.unwrap();
        let deleted = backend.get_pipeline(&pipeline.id).await.unwrap();
        assert!(deleted.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: list_pipelines pagination
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_pipelines_pagination() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "paginator").await;

        // Create 5 pipelines
        for i in 0..5 {
            create_test_pipeline(&backend, &user.id, &format!("pipeline-{i}")).await;
        }

        // Page 1: 3 items per page
        let pag = Pagination {
            page: 1,
            per_page: 3,
        };
        let result = backend.list_pipelines(&user.id, pag).await.unwrap();
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total_items, 5);
        assert_eq!(result.total_pages, 2);
        assert_eq!(result.page, 1);

        // Page 2: 3 items per page (should have 2 remaining)
        let pag = Pagination {
            page: 2,
            per_page: 3,
        };
        let result = backend.list_pipelines(&user.id, pag).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.page, 2);

        // Page beyond results
        let pag = Pagination {
            page: 10,
            per_page: 3,
        };
        let result = backend.list_pipelines(&user.id, pag).await.unwrap();
        assert_eq!(result.items.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Test: create_run + update_run_status
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_run_and_status() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "runner").await;
        let pipeline = create_test_pipeline(&backend, &user.id, "run-test").await;

        // Create a run
        let run = models::RunRow {
            id: String::new(),
            user_id: user.id.clone(),
            pipeline_id: Some(pipeline.id.clone()),
            pipeline_snapshot: pipeline.toml_content.clone(),
            workflow_name: Some("run-test".to_string()),
            status: "queued".to_string(),
            phase: "parsing".to_string(),
            pid: None,
            workdir: Some("/tmp/workdir".to_string()),
            started_at: None,
            finished_at: None,
            created_at: String::new(),
        };
        let created_run = backend.create_run(&run).await.unwrap();
        assert!(!created_run.id.is_empty());
        assert_eq!(created_run.status, "queued");
        assert_eq!(created_run.phase, "parsing");

        // Update status to running
        backend
            .update_run_status(&created_run.id, "running", "executing")
            .await
            .unwrap();

        let running = backend.get_run(&created_run.id).await.unwrap().unwrap();
        assert_eq!(running.status, "running");
        assert!(running.started_at.is_some());

        // List runs for user
        let pag = Pagination::default();
        let runs = backend.list_runs(&user.id, pag).await.unwrap();
        assert_eq!(runs.items.len(), 1);
        assert_eq!(runs.total_items, 1);
    }

    // -----------------------------------------------------------------------
    // Test: recover_orphaned_runs marks old running runs as failed
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_recover_orphaned_runs() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "orphan-test").await;
        let pipeline = create_test_pipeline(&backend, &user.id, "orphan-pipeline").await;

        // Create a run with an old started_at (more than 60 seconds ago)
        let old_time = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
        let run_id = Uuid::new_v4().to_string();
        let run = models::RunRow {
            id: run_id.clone(),
            user_id: user.id.clone(),
            pipeline_id: Some(pipeline.id.clone()),
            pipeline_snapshot: pipeline.toml_content.clone(),
            workflow_name: None,
            status: "running".to_string(),
            phase: "executing".to_string(),
            pid: Some(12345),
            workdir: None,
            started_at: Some(old_time),
            finished_at: None,
            created_at: String::new(),
        };
        backend.create_run(&run).await.unwrap();

        // Mark the run as running with old started_at via direct SQL
        // (create_run already set it correctly, so proceed directly to recovery)
        let recovered = backend.recover_orphaned_runs().await.unwrap();
        assert_eq!(recovered, 1, "Should recover 1 orphaned run");

        // Verify the run was marked as failed
        let fetched = backend.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(fetched.status, "failed");
        assert!(fetched.finished_at.is_some());
    }

    // -----------------------------------------------------------------------
    // Test: template CRUD
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_template_crud() {
        let backend = create_backend().await;

        let now = chrono::Utc::now().to_rfc3339();
        let template = models::TemplateRow {
            id: String::new(),
            name: "test-template".to_string(),
            category: "test".to_string(),
            description: "A test template".to_string(),
            tags: "[\"test\", \"demo\"]".to_string(),
            toml_content: "[workflow]\nname = \"test\"\nversion = \"1.0.0\"".to_string(),
            is_system: 0,
            created_by: None,
            usage_count: 0,
            created_at: now.clone(),
            updated_at: now,
        };

        // Save template
        let saved = backend.save_template(&template).await.unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.name, "test-template");

        // Get template
        let fetched = backend.get_template(&saved.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "test-template");

        // List templates
        let templates = backend.list_templates().await.unwrap();
        let our_template = templates.iter().find(|t| t.id == saved.id);
        assert!(our_template.is_some());

        // Update template
        let updated = models::TemplateRow {
            name: "updated-template".to_string(),
            usage_count: 5,
            ..saved
        };
        backend.save_template(&updated).await.unwrap();
        let fetched = backend.get_template(&updated.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "updated-template");
        assert_eq!(fetched.usage_count, 5);

        // Delete template
        backend.delete_template(&updated.id).await.unwrap();
        let deleted = backend.get_template(&updated.id).await.unwrap();
        assert!(deleted.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: session create + get + delete + cleanup
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_session_flow() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "session-test").await;

        let token = Uuid::new_v4().to_string();
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();

        // Create valid session
        backend
            .create_session(&user.id, &token, &future)
            .await
            .unwrap();

        // Get valid session
        let session = backend.get_session(&token).await.unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().user_id, user.id);

        // Create expired session
        let expired_token = Uuid::new_v4().to_string();
        backend
            .create_session(&user.id, &expired_token, &past)
            .await
            .unwrap();

        // Expired session should not be retrievable
        let expired = backend.get_session(&expired_token).await.unwrap();
        assert!(
            expired.is_none(),
            "Expired session should not be retrievable"
        );

        // Cleanup expired sessions
        let cleaned = backend.cleanup_expired_sessions().await.unwrap();
        assert_eq!(cleaned, 1, "Should clean up 1 expired session");

        // Delete valid session
        backend.delete_session(&token).await.unwrap();
        let deleted = backend.get_session(&token).await.unwrap();
        assert!(deleted.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: share CRUD
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_share_crud() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "share-test").await;
        let pipeline = create_test_pipeline(&backend, &user.id, "shared-pipeline").await;

        // Create share
        let share_token = Uuid::new_v4().to_string();
        let share = models::ShareRow {
            id: String::new(),
            pipeline_id: pipeline.id.clone(),
            owner_id: user.id.clone(),
            token: share_token.clone(),
            visibility: "link".to_string(),
            expires_at: None,
            created_at: String::new(), // will be overwritten
        };
        let created = backend.create_share(&share).await.unwrap();
        assert!(!created.id.is_empty());
        assert_eq!(created.token, share_token);

        // Get share by token
        let by_token = backend.get_share_by_token(&share_token).await.unwrap();
        assert!(by_token.is_some());
        assert_eq!(by_token.unwrap().pipeline_id, pipeline.id);

        // List shares for pipeline
        let shares = backend.list_shares(&pipeline.id).await.unwrap();
        assert_eq!(shares.len(), 1);

        // Revoke share
        backend.revoke_share(&created.id).await.unwrap();
        let shares = backend.list_shares(&pipeline.id).await.unwrap();
        assert_eq!(shares.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Test: audit log
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_audit_log() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "audit-test").await;

        backend
            .log_action(&user.id, "run_workflow", "pipeline-123")
            .await
            .unwrap();

        // Verify by querying directly
        let rows: Vec<models::AuditLogRow> =
            sqlx::query_as("SELECT * FROM audit_logs WHERE user_id = ? ORDER BY timestamp DESC")
                .bind(&user.id)
                .fetch_all(backend.inner_pool())
                .await
                .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "run_workflow");
        assert_eq!(rows[0].target, "pipeline-123");
    }

    // -----------------------------------------------------------------------
    // Test: health
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_health() {
        let backend = create_backend().await;
        let healthy = backend.health().await.unwrap();
        assert!(healthy);
    }

    // -----------------------------------------------------------------------
    // Test: cancel_run
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_run() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "cancel-test").await;
        let pipeline = create_test_pipeline(&backend, &user.id, "cancel-pipeline").await;

        let run = models::RunRow {
            id: String::new(),
            user_id: user.id.clone(),
            pipeline_id: Some(pipeline.id.clone()),
            pipeline_snapshot: pipeline.toml_content.clone(),
            workflow_name: None,
            status: "running".to_string(),
            phase: "executing".to_string(),
            pid: None,
            workdir: None,
            started_at: Some(chrono::Utc::now().to_rfc3339()),
            finished_at: None,
            created_at: String::new(),
        };
        let created = backend.create_run(&run).await.unwrap();

        backend.cancel_run(&created.id).await.unwrap();
        let cancelled = backend.get_run(&created.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.finished_at.is_some());
    }

    // -----------------------------------------------------------------------
    // Test: list_runs pagination
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_runs_pagination() {
        let backend = create_backend().await;
        let user = create_test_user(&backend, "list-runs").await;
        let pipeline = create_test_pipeline(&backend, &user.id, "list-runs-pipeline").await;

        for _i in 0..7 {
            let run = models::RunRow {
                id: String::new(),
                user_id: user.id.clone(),
                pipeline_id: Some(pipeline.id.clone()),
                pipeline_snapshot: pipeline.toml_content.clone(),
                workflow_name: None,
                status: "completed".to_string(),
                phase: "reporting".to_string(),
                pid: None,
                workdir: None,
                started_at: None,
                finished_at: None,
                created_at: String::new(),
            };
            backend.create_run(&run).await.unwrap();
        }

        let pag = Pagination {
            page: 1,
            per_page: 4,
        };
        let result = backend.list_runs(&user.id, pag).await.unwrap();
        assert_eq!(result.items.len(), 4);
        assert_eq!(result.total_items, 7);
        assert_eq!(result.total_pages, 2);

        let pag = Pagination {
            page: 2,
            per_page: 4,
        };
        let result = backend.list_runs(&user.id, pag).await.unwrap();
        assert_eq!(result.items.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Test: legacy 'success' status migrates to 'completed'
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_legacy_success_status_migrates_to_completed() {
        let backend = SqliteBackend::new("sqlite::memory:")
            .await
            .expect("Failed to create in-memory backend");
        backend.init().await.expect("schema init");

        sqlx::query(
            "INSERT INTO runs (id, user_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at)
             VALUES ('legacy-1', 'default', '', 'wf', 'success', 'executing', NULL, '', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(backend.inner_pool())
        .await
        .unwrap();

        // Re-init is idempotent and re-runs the migration.
        backend.init().await.expect("re-init");
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = 'legacy-1'")
            .fetch_one(backend.inner_pool())
            .await
            .unwrap();
        assert_eq!(status, "completed");

        // Non-success rows are untouched.
        sqlx::query("UPDATE runs SET status = 'failed' WHERE id = 'legacy-1'")
            .execute(backend.inner_pool())
            .await
            .unwrap();
        backend.init().await.expect("re-init again");
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = 'legacy-1'")
            .fetch_one(backend.inner_pool())
            .await
            .unwrap();
        assert_eq!(status, "failed");
    }

    // -----------------------------------------------------------------------
    // Test: init_pool global singleton
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_global_pool() {
        // Reset: use a fresh in-memory URL
        let url = "sqlite::memory:";
        init_pool(url).await;
        let p = pool();
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(p).await.unwrap();
        assert_eq!(row.0, 1);
    }
}
