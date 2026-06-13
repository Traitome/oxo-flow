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

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }
}

#[async_trait]
impl StorageBackend for PostgresBackend {
    async fn init(&self) -> Result<(), String> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                role TEXT NOT NULL,
                auth_type TEXT NOT NULL,
                os_user TEXT,
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
                pipeline_id TEXT NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                pipeline_snapshot TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                phase TEXT NOT NULL DEFAULT 'parsing',
                pid BIGINT,
                workdir TEXT,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS run_nodes (
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                rule_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                started_at TEXT,
                finished_at TEXT,
                exit_code INTEGER,
                attempt BIGINT NOT NULL DEFAULT 1,
                error_pattern TEXT,
                PRIMARY KEY (run_id, rule_name)
            );

            CREATE TABLE IF NOT EXISTS sessions (
                token TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
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
                metadata TEXT,
                timestamp TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("PG init schema: {e}"))?;

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

    async fn create_user(&self, username: &str, role: &str) -> Result<models::UserRow, String> {
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
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
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
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
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
            .map(|opt| opt.expect("pipeline should exist after upsert"))
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
            "INSERT INTO runs (id, user_id, pipeline_id, pipeline_snapshot, status, phase, pid, workdir, started_at, finished_at, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
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

    async fn cancel_run(&self, id: &str) -> Result<(), String> {
        let now = Self::now();
        sqlx::query("UPDATE runs SET status = 'cancelled', finished_at = $1 WHERE id = $2")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn create_run_node(&self, node: &models::RunNodeRow) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO run_nodes (run_id, rule_name, status, started_at, finished_at, exit_code, attempt, error_pattern) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&node.run_id)
        .bind(&node.rule_name)
        .bind(&node.status)
        .bind(&node.started_at)
        .bind(&node.finished_at)
        .bind(node.exit_code)
        .bind(node.attempt)
        .bind(&node.error_pattern)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn update_run_node(
        &self,
        run_id: &str,
        rule_name: &str,
        status: &str,
        exit_code: Option<i32>,
        error_pattern: Option<&str>,
    ) -> Result<(), String> {
        let now = Self::now();
        let is_terminal = status == "success" || status == "failed" || status == "skipped";
        if is_terminal {
            sqlx::query(
                "UPDATE run_nodes SET status = $1, finished_at = $2, exit_code = $3, error_pattern = $4 WHERE run_id = $5 AND rule_name = $6",
            )
            .bind(status)
            .bind(&now)
            .bind(exit_code)
            .bind(error_pattern)
            .bind(run_id)
            .bind(rule_name)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        } else if status == "running" {
            sqlx::query(
                "UPDATE run_nodes SET status = $1, started_at = $2, exit_code = $3, error_pattern = $4 WHERE run_id = $5 AND rule_name = $6",
            )
            .bind(status)
            .bind(&now)
            .bind(exit_code)
            .bind(error_pattern)
            .bind(run_id)
            .bind(rule_name)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        } else {
            sqlx::query(
                "UPDATE run_nodes SET status = $1, exit_code = $2, error_pattern = $3 WHERE run_id = $4 AND rule_name = $5",
            )
            .bind(status)
            .bind(exit_code)
            .bind(error_pattern)
            .bind(run_id)
            .bind(rule_name)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn get_run_nodes(&self, run_id: &str) -> Result<Vec<models::RunNodeRow>, String> {
        sqlx::query_as::<_, models::RunNodeRow>(
            "SELECT * FROM run_nodes WHERE run_id = $1 ORDER BY attempt ASC",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())
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
        sqlx::query_as::<_, models::TemplateRow>("SELECT * FROM templates WHERE id = $1")
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
            .map(|opt| opt.expect("template should exist after upsert"))
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
        let now = Self::now();
        sqlx::query_as::<_, models::ShareRow>(
            "SELECT * FROM shares WHERE token = $1 AND (expires_at IS NULL OR expires_at > $2)",
        )
        .bind(token)
        .bind(&now)
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
        let id = Uuid::new_v4().to_string();
        let now = Self::now();
        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, action, target, metadata, timestamp) VALUES ($1, $2, $3, $4, $5, $6)",
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
