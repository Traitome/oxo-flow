use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::migrate::MigrateDatabase;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::sync::OnceLock;
use uuid::Uuid;

/// Global SQLite connection pool.
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Initialize the database and run migrations.
pub async fn init_db(database_url: &str) -> Result<()> {
    if !sqlx::Sqlite::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        sqlx::Sqlite::create_database(database_url).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

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
            FOREIGN KEY(user_id) REFERENCES users(id)
        );

        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            timestamp DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id)
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

    DB_POOL
        .set(pool)
        .map_err(|_| anyhow::anyhow!("DB pool already initialized"))?;
    Ok(())
}

pub fn pool() -> &'static SqlitePool {
    DB_POOL.get().expect("Database pool not initialized")
}

/// Recovers runs that were left in the 'running' state after a server crash.
pub async fn recover_orphaned_runs() -> Result<()> {
    // We mark any run that was 'running' as 'failed' (interrupted by server restart)
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
