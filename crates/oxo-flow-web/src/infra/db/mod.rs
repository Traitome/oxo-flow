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

/// Unified pool accessor: returns a reference to the SQLite pool.
///
/// The handlers are SQLite-backed; even with the `postgres` feature enabled,
/// `init_pool` (main.rs) decides which backend is live and handlers read
/// through this SQLite pool accessor.
#[cfg(feature = "postgres")]
pub fn try_pool() -> Result<&'static sqlx::SqlitePool, String> {
    sqlite::try_pool()
}

#[cfg(not(feature = "postgres"))]
pub fn try_pool() -> Result<&'static sqlx::SqlitePool, String> {
    sqlite::try_pool()
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

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn init(&self) -> Result<(), String>;
    async fn health(&self) -> Result<bool, String>;

    async fn create_user(&self, username: &str, role: &str) -> Result<models::UserRow, String>;
    async fn get_user_by_id(&self, id: &str) -> Result<Option<models::UserRow>, String>;
    async fn get_user_by_username(&self, username: &str)
    -> Result<Option<models::UserRow>, String>;
    async fn list_users(&self) -> Result<Vec<models::UserRow>, String>;
    async fn delete_user(&self, id: &str) -> Result<(), String>;

    async fn save_pipeline(&self, p: &models::PipelineRow) -> Result<models::PipelineRow, String>;
    async fn get_pipeline(&self, id: &str) -> Result<Option<models::PipelineRow>, String>;
    async fn list_pipelines(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::PipelineRow>, String>;
    async fn delete_pipeline(&self, id: &str) -> Result<(), String>;

    async fn create_run(&self, run: &models::RunRow) -> Result<models::RunRow, String>;
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
    async fn record_run_terminal(
        &self,
        id: &str,
        status: &str,
        phase: &str,
        finished_at: &str,
    ) -> Result<(), String>;
    async fn get_run_pid(&self, id: &str) -> Result<Option<i64>, String>;
    async fn cancel_run(&self, id: &str) -> Result<(), String>;
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
    async fn delete_session(&self, token: &str) -> Result<(), String>;
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
}
