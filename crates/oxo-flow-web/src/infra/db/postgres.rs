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
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("PG init schema: {e}"))?;

        // Migration for pre-existing postgres databases: the runs table
        // gained `config_overrides` after #930. Run execution remains
        // SQLite-only (#207), but keeping the storage contract healthy lets
        // metadata tools and pre-execution checks work on legacy tables.
        sqlx::query("ALTER TABLE runs ADD COLUMN IF NOT EXISTS config_overrides TEXT")
            .execute(&self.pool)
            .await
            .map_err(|e| format!("PG migration config_overrides: {e}"))?;

        // AI config flags are part of the public model as i64 (matching
        // SQLite); PostgreSQL INT4 columns cannot decode into that type.
        sqlx::raw_sql(
            "ALTER TABLE ai_provider_config ALTER COLUMN search_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN monitor_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN auto_retry_enabled TYPE BIGINT;
             ALTER TABLE ai_provider_config ALTER COLUMN max_correction_rounds TYPE BIGINT;",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("PG migration AI config integer widths: {e}"))?;

        // Seed admin user
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
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
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }

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
            sqlx::query("UPDATE runs SET status = $1, phase = $2, started_at = $3 WHERE id = $4")
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
    ) -> Result<(), String> {
        sqlx::query("UPDATE runs SET status = $1, phase = $2, finished_at = $3 WHERE id = $4")
            .bind(status)
            .bind(phase)
            .bind(finished_at)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
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
        let mut where_clauses: Vec<String> = Vec::new();
        let mut binds: Vec<String> = Vec::new();
        let mut param_idx = 1;

        if let Some(user_id) = &filters.user_id {
            where_clauses.push(format!("user_id = ${param_idx}"));
            binds.push(user_id.clone());
            param_idx += 1;
        }
        if let Some(status) = &filters.status
            && matches!(
                status.as_str(),
                "queued" | "running" | "paused" | "completed" | "failed" | "cancelled"
            )
        {
            where_clauses.push(format!("status = ${param_idx}"));
            binds.push(status.clone());
            param_idx += 1;
        }
        if let Some(q) = &filters.q {
            let trimmed = q.trim();
            if !trimmed.is_empty() {
                where_clauses.push(format!(
                    "(workflow_name LIKE ${} OR id LIKE ${})",
                    param_idx,
                    param_idx + 1
                ));
                let pattern = format!("%{trimmed}%");
                binds.push(pattern.clone());
                binds.push(pattern);
                param_idx += 2;
            }
        }

        let filter_where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_clauses.join(" AND "))
        };
        let filter_binds = binds.clone();

        if let Some(cursor) = &filters.cursor {
            where_clauses.push(format!("created_at < ${param_idx}"));
            binds.push(cursor.clone());
        }

        let page_where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_clauses.join(" AND "))
        };

        let limit = filters.limit.clamp(1, 500);
        let sql = format!(
            "SELECT * FROM runs{} ORDER BY created_at DESC LIMIT {}",
            page_where_sql,
            limit + 1
        );
        let mut query = sqlx::query_as::<_, models::RunRow>(sqlx::AssertSqlSafe(sql));
        for bind in &binds {
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

        let count_sql = format!("SELECT COUNT(*) FROM runs{}", filter_where_sql);
        let mut count_query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
        for bind in &filter_binds {
            count_query = count_query.bind(bind);
        }
        let total: i64 = count_query.fetch_one(&self.pool).await.unwrap_or(0);

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
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at)              VALUES ($1, $2, 'viewer', 'audit', NULL, $3)              ON CONFLICT (id) DO NOTHING",
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
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE status IN ('running', 'queued')")
                .fetch_one(&self.pool)
                .await
                .unwrap_or(0);
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
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
