use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::migrate::MigrateDatabase;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::sync::OnceLock;
use uuid::Uuid;

/// Global SQLite connection pool.
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Initialize the database, run migrations, and seed the default admin user.
///
/// When called a second time (e.g., from a later `#[tokio::test]`) this
/// function is a no-op: the existing pool is reused.  For in-memory SQLite
/// the pool is configured with a single persistent connection so all
/// concurrent queries share the same schema and data.
pub async fn init_db(database_url: &str) -> Result<()> {
    // Pool already initialized — no-op.
    if DB_POOL.get().is_some() {
        return Ok(());
    }

    if !sqlx::Sqlite::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        sqlx::Sqlite::create_database(database_url).await?;
    }

    // In-memory SQLite databases are per-connection: use a single persistent
    // connection so all queries share the same schema and data.
    let is_memory = database_url.contains(":memory:");
    let mut opts = SqlitePoolOptions::new();
    if is_memory {
        opts = opts
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None);
    } else {
        opts = opts.max_connections(5);
    }
    let pool = opts.connect(database_url).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            role TEXT NOT NULL,
            auth_type TEXT NOT NULL,
            os_user TEXT NOT NULL,
            created_at DATETIME NOT NULL
        );

        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            workflow_name TEXT NOT NULL,
            status TEXT NOT NULL,
            pid INTEGER,
            started_at DATETIME,
            finished_at DATETIME,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            timestamp DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
        CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);
        CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);

        CREATE TABLE IF NOT EXISTS workflows (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            toml_content TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            updated_at DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            expires_at DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(&pool)
    .await?;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;

    if count.0 == 0 {
        let admin_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(admin_id)
        .bind("admin")
        .bind("admin")
        .bind("sudo")
        .bind("oxo-flow")
        .bind(now)
        .execute(&pool)
        .await?;
    }

    // `set` will fail if another thread beat us here; that is fine since we
    // already checked `is_some()` above and both pools point to the same DB.
    let _ = DB_POOL.set(pool);
    Ok(())
}

/// Obtain a reference to the global pool.
///
/// # Panics
/// Panics if `init_db` has not been called yet.
pub fn pool() -> &'static SqlitePool {
    DB_POOL
        .get()
        .expect("Database pool not initialized — call init_db() first")
}

/// Recover runs left in the 'running' state after a server crash.
pub async fn recover_orphaned_runs() -> Result<()> {
    let now = Utc::now();
    let result =
        sqlx::query("UPDATE runs SET status = 'failed', finished_at = ? WHERE status = 'running'")
            .bind(now)
            .execute(pool())
            .await?;

    if result.rows_affected() > 0 {
        tracing::warn!(
            "Recovered {} orphaned runs and marked them as failed.",
            result.rows_affected()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: String,
    pub auth_type: String,
    pub os_user: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Run {
    pub id: String,
    pub user_id: String,
    pub workflow_name: String,
    pub status: String,
    pub pid: Option<i64>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Repository Functions
// ---------------------------------------------------------------------------

pub async fn get_user_by_username(username: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool())
        .await?;
    Ok(user)
}

pub async fn get_user_by_id(id: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool())
        .await?;
    Ok(user)
}

pub async fn create_session(session: &Session) -> Result<()> {
    sqlx::query(
        "INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&session.token)
    .bind(&session.user_id)
    .bind(session.created_at)
    .bind(session.expires_at)
    .execute(pool())
    .await?;
    Ok(())
}

pub async fn get_session(token: &str) -> Result<Option<Session>> {
    let session =
        sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE token = ? AND expires_at > ?")
            .bind(token)
            .bind(Utc::now())
            .fetch_optional(pool())
            .await?;
    Ok(session)
}

pub async fn delete_session(token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(pool())
        .await?;
    Ok(())
}

pub async fn insert_run(run: &Run) -> Result<()> {
    sqlx::query(
        "INSERT INTO runs (id, user_id, workflow_name, status, pid, started_at, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&run.id)
    .bind(&run.user_id)
    .bind(&run.workflow_name)
    .bind(&run.status)
    .bind(run.pid)
    .bind(run.started_at)
    .bind(run.finished_at)
    .execute(pool())
    .await?;
    Ok(())
}

pub async fn log_action(user_id: &str, action: &str, target: &str) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, timestamp) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(action)
    .bind(target)
    .bind(now)
    .execute(pool())
    .await?;
    Ok(())
}
