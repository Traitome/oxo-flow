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
/// `global_backend()` already falls back to this pool internally, so this
/// is only for call sites that need the pool handle itself. There is no
/// second body under `#[cfg(feature = "postgres")]` — both feature states
/// resolved to the same call, and byte-identical cfg twins always drift.
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

/// The #207 migration surface: one storage abstraction the domains route
/// through, so a PostgreSQL backend becomes a matter of wiring, not rewrites.
///
/// Scope — users, auth sessions, API keys, OAuth CSRF state, pipelines and
/// revisions, templates, shares, run-record metadata, clusters, chat history,
/// audit logging, webhook configuration, and AI provider configuration all
/// consume this trait. Actual run execution remains SQLite-only and gates
/// `/api/runs*` with `RUNS_REQUIRE_SQLITE`; PostgreSQL deployments advertise
/// that split through `PG_CAPABILITY_MATRIX`. A new domain handler must call
/// the trait, not add another raw-SQL path.
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
    async fn get_pipeline(&self, id: &str) -> Result<Option<models::PipelineRow>, String>;
    async fn list_pipelines(
        &self,
        user_id: &str,
        pagination: Pagination,
    ) -> Result<Paginated<models::PipelineRow>, String>;
    async fn delete_pipeline(&self, id: &str) -> Result<(), String>;

    async fn create_run(&self, run: &models::RunRow) -> Result<models::RunRow, String>;
    async fn create_run_if_workdir_idle(&self, run: &models::RunRow) -> Result<bool, String>;
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

    async fn get_ai_provider_config(
        &self,
        user_id: Option<&str>,
    ) -> Result<Option<models::AiProviderConfigRow>, String>;
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

    async fn store_oauth_state(&self, state: &str, created_at: &str) -> Result<(), String>;
    async fn consume_oauth_state(&self, state: &str) -> Result<Option<String>, String>;
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
    async fn log_action_with_metadata(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
        metadata: Option<&str>,
    ) -> Result<(), String>;

    async fn record_audit_log(
        &self,
        user_id: &str,
        action: &str,
        target: &str,
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
    async fn search_pipelines(
        &self,
        query: &str,
        user_id: Option<&str>,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String>;
    async fn list_pipelines_visible(
        &self,
        user_id: &str,
        is_admin: bool,
        limit: u32,
    ) -> Result<Vec<models::PipelineRow>, String>;
}
