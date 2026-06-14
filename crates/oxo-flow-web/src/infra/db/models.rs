use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub role: String,      // "admin" | "user" | "viewer"
    pub auth_type: String, // "password" | "oauth"
    pub os_user: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PipelineRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub version: String,
    pub toml_content: String,
    pub rules_count: i64,
    pub forked_from: Option<String>,
    pub visibility: String, // "private" | "workspace" | "link"
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RunRow {
    pub id: String,
    pub user_id: String,
    pub pipeline_id: String,
    pub pipeline_snapshot: String,
    pub status: String, // "queued"|"running"|"completed"|"failed"|"cancelled"
    pub phase: String,  // "parsing"|"validating"|"preparing"|"executing"|"reporting"
    pub pid: Option<i64>,
    pub workdir: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RunNodeRow {
    pub run_id: String,
    pub rule_name: String,
    pub status: String, // "pending"|"running"|"success"|"failed"|"skipped"
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub attempt: i64,
    pub error_pattern: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SessionRow {
    pub token: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TemplateRow {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tags: String, // JSON array stored as string
    pub toml_content: String,
    pub is_system: i64,
    pub created_by: Option<String>,
    pub usage_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ShareRow {
    pub id: String,
    pub pipeline_id: String,
    pub owner_id: String,
    pub token: String,
    pub visibility: String,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuditLogRow {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    pub metadata: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatSessionRow {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}
// v0.8 AI Companion models

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatMessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PipelineDraftRow {
    pub id: String,
    pub user_id: String,
    pub session_id: Option<String>,
    pub toml_content: String,
    pub dag_json: Option<String>,
    pub data_report_json: Option<String>,
    pub status: String, // "draft" | "validated" | "running" | "completed"
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DagEditOperationRow {
    pub id: String,
    pub draft_id: String,
    pub user_id: String,
    pub operation: String,
    pub payload: String,
    pub sequence_num: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AiProviderConfigRow {
    pub id: String,
    pub user_id: Option<String>,
    pub provider: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub search_enabled: Option<i64>,
    pub monitor_enabled: Option<i64>,
    pub auto_retry_enabled: Option<i64>,
    pub max_correction_rounds: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MonitorAlertRow {
    pub id: String,
    pub run_id: String,
    pub level: String,
    pub rule_name: Option<String>,
    pub prediction: String,
    pub suggestion: String,
    pub auto_fixable: Option<i64>,
    pub acknowledged: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ReportSessionRow {
    pub id: String,
    pub run_id: String,
    pub narrative_md: String,
    pub findings: Option<String>,
    pub caveats: Option<String>,
    pub suggested_next: Option<String>,
    pub created_at: String,
}
