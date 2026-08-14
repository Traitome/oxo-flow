//! HTTP handlers for workflow domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Extension, Json, extract::Path, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::observability::types::*;
use crate::infra::db::models;

/// Pipeline read permission (issue #82 P0-4): admins and the owner always
/// pass; `workspace`-visibility pipelines are readable by any
/// authenticated user; everything else is private.
fn can_read_pipeline(user: &CurrentUser, row: &models::PipelineRow) -> bool {
    user.is_admin() || row.user_id == user.id || row.visibility == "workspace"
}

/// Pipeline write permission: owner or admin only.
fn can_write_pipeline(user: &CurrentUser, row: &models::PipelineRow) -> bool {
    user.is_admin() || row.user_id == user.id
}

/// Snapshot a pipeline into `pipeline_revisions` (issue #82 P1-14). Keeps
/// the last 50 revisions per pipeline; older snapshots are pruned.
async fn record_revision(
    pool: &sqlx::SqlitePool,
    pipeline_id: &str,
    user_id: &str,
    version: &str,
    toml_content: &str,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "INSERT INTO pipeline_revisions (id, pipeline_id, user_id, version, toml_content, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(pipeline_id)
    .bind(user_id)
    .bind(version)
    .bind(toml_content)
    .bind(&now)
    .execute(pool)
    .await;
    // Keep the history bounded: drop everything beyond the newest 50.
    let _ = sqlx::query(
        "DELETE FROM pipeline_revisions WHERE pipeline_id = ? AND id NOT IN \
         (SELECT id FROM pipeline_revisions WHERE pipeline_id = ? \
          ORDER BY created_at DESC, rowid DESC LIMIT 50)",
    )
    .bind(pipeline_id)
    .bind(pipeline_id)
    .execute(pool)
    .await;
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

pub fn err(status: StatusCode, code: &str, msg: String) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: msg,
            detail: None,
            suggestion: None,
        }),
    )
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

// ---------------------------------------------------------------------------
// Pipeline lifecycle
// ---------------------------------------------------------------------------

/// POST /api/pipelines/parse
pub async fn parse_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<ParseResponse> {
    service::parse_pipeline(&req.toml_content, req.format_version.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PARSE_ERROR", e))
}

/// POST /api/pipelines/validate
///
/// Accepts TOML content directly so the endpoint is self-contained.
pub async fn validate_pipeline(Json(req): Json<serde_json::Value>) -> ApiResult<ValidateResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    service::validate_pipeline(toml)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "VALIDATE_ERROR", e))
}

/// POST /api/pipelines/prepare
pub async fn prepare_pipeline(Json(req): Json<serde_json::Value>) -> ApiResult<PrepareResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    let resolve = req
        .get("resolve_wildcards")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let apply = req
        .get("apply_defaults")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    service::prepare_pipeline(toml, resolve, apply)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PREPARE_ERROR", e))
}

/// POST /api/pipelines/dag
pub async fn build_dag(Json(req): Json<serde_json::Value>) -> ApiResult<DagJsonResponse> {
    let toml = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING_FIELD",
                "toml_content is required".into(),
            )
        })?;
    service::build_dag(toml)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DAG_ERROR", e))
}

/// POST /api/pipelines/format
pub async fn format_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<FormatResponse> {
    service::format_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "FORMAT_ERROR", e))
}

/// POST /api/pipelines/lint
pub async fn lint_pipeline(Json(req): Json<ParseRequest>) -> ApiResult<ValidateResponse> {
    service::lint_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "LINT_ERROR", e))
}

/// POST /api/pipelines/stats
pub async fn pipeline_stats(Json(req): Json<ParseRequest>) -> ApiResult<WorkflowStatsResponse> {
    service::workflow_stats(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "STATS_ERROR", e))
}

/// POST /api/pipelines/diff
pub async fn diff_pipelines(Json(req): Json<DiffRequest>) -> ApiResult<DiffResponse> {
    service::diff_workflows(&req.toml_a, &req.toml_b)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DIFF_ERROR", e))
}

/// POST /api/pipelines/export
///
/// Prefers inline `toml_content`; falls back to loading the saved pipeline
/// when only `pipeline_id` is given.
pub async fn export_pipeline(Json(req): Json<ExportRequest>) -> ApiResult<ExportResponse> {
    let toml_content = if !req.toml_content.trim().is_empty() {
        req.toml_content
    } else if let Some(id) = req.pipeline_id.as_deref() {
        let pool = get_pool()?;
        let row: Option<(String,)> =
            sqlx::query_as("SELECT toml_content FROM pipelines WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| {
                    err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DB_ERROR",
                        format!("Failed to load pipeline '{id}': {e}"),
                    )
                })?;
        match row {
            Some((toml,)) => toml,
            None => {
                return Err(err(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("pipeline '{id}' not found"),
                ));
            }
        }
    } else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "MISSING",
            "toml_content or pipeline_id required".into(),
        ));
    };
    service::export_pipeline(&toml_content, req.format.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "EXPORT_ERROR", e))
}

/// POST /api/pipelines/search
pub async fn search_pipelines(Json(req): Json<SearchRequest>) -> ApiResult<SearchResponse> {
    let pool = get_pool()?;

    // Search saved pipelines from DB
    let pipeline_rows: Vec<models::PipelineRow> = sqlx::query_as(
        "SELECT * FROM pipelines WHERE name LIKE ? OR toml_content LIKE ? ORDER BY updated_at DESC LIMIT 50",
    )
    .bind(format!("%{}%", req.query))
    .bind(format!("%{}%", req.query))
    .fetch_all(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error searching pipelines: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let pipelines: Vec<Pipeline> = pipeline_rows
        .into_iter()
        .map(|r| Pipeline {
            id: r.id,
            user_id: r.user_id,
            name: r.name,
            version: r.version,
            toml_content: r.toml_content,
            rules_count: r.rules_count as usize,
            forked_from: r.forked_from,
            visibility: r.visibility,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect();

    // Search templates from DB
    let template_rows: Vec<models::TemplateRow> = sqlx::query_as(
        "SELECT * FROM templates WHERE name LIKE ? OR description LIKE ? OR category LIKE ? ORDER BY usage_count DESC LIMIT 20",
    )
    .bind(format!("%{}%", req.query))
    .bind(format!("%{}%", req.query))
    .bind(format!("%{}%", req.query))
    .fetch_all(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error searching templates: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let templates: Vec<Template> = template_rows
        .into_iter()
        .map(|r| Template {
            id: r.id,
            name: r.name,
            category: r.category,
            description: r.description,
            tags: serde_json::from_str(&r.tags).unwrap_or_default(),
            toml_content: Some(r.toml_content),
            is_system: r.is_system != 0,
            created_by: r.created_by,
            usage_count: r.usage_count as u64,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect();

    Ok(Json(service::search_pipelines(
        &req.query, &pipelines, &templates,
    )))
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

/// POST /api/pipelines — create a new pipeline from TOML
pub async fn save_pipeline(
    authenticated: Option<axum::Extension<CurrentUser>>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Pipeline> {
    let pool = get_pool()?;
    let toml_content = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "toml_content required".into(),
            )
        })?;
    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("untitled");
    let version = req
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0");
    let visibility = req
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("private");
    if !matches!(visibility, "private" | "workspace" | "link") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_VISIBILITY",
            format!("visibility must be private, workspace, or link — got '{visibility}'"),
        ));
    }
    // Attribute the pipeline to the acting user (team/hpc mode) or the
    // 'default' pseudo-user (personal mode). The auth middleware resolves
    // the session into a canonical users.id; client-supplied user ids are
    // never trusted (issue #82 P0-4).
    let user_id = crate::domains::auth::current_user::resolve(authenticated.as_ref()).id;

    let rules_count = oxo_flow_core::WorkflowConfig::parse(toml_content)
        .map(|wf| wf.rules.len() as i64)
        .unwrap_or(0);

    let now = now_iso();
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO pipelines (id, user_id, name, version, toml_content, rules_count, forked_from, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id).bind(&user_id).bind(name).bind(version).bind(toml_content)
    .bind(rules_count).bind(None::<String>).bind(visibility).bind(&now).bind(&now)
    .execute(pool).await
    .map_err(|e| {
        tracing::error!("DB error creating pipeline: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    // The initial save is revision 1 of the pipeline's history.
    record_revision(pool, &id, &user_id, version, toml_content).await;

    Ok(Json(Pipeline {
        id,
        user_id: user_id.clone(),
        name: name.to_string(),
        version: version.to_string(),
        toml_content: toml_content.to_string(),
        rules_count: rules_count as usize,
        forked_from: None,
        visibility: visibility.to_string(),
        created_at: now.clone(),
        updated_at: now,
    }))
}

/// GET /api/pipelines
pub async fn list_pipelines(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<Vec<Pipeline>> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    // Ownership scoping (issue #82 P0-4): non-admins see their own
    // pipelines plus workspace-visible ones; admins see everything.
    let rows: Vec<models::PipelineRow> = if user.is_admin() {
        sqlx::query_as("SELECT * FROM pipelines ORDER BY updated_at DESC LIMIT 100")
            .fetch_all(pool)
            .await
    } else {
        sqlx::query_as(
            "SELECT * FROM pipelines WHERE user_id = ? OR visibility = 'workspace' \
             ORDER BY updated_at DESC LIMIT 100",
        )
        .bind(&user.id)
        .fetch_all(pool)
        .await
    }
    .map_err(|e| {
        tracing::error!("DB error listing pipelines: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let list: Vec<Pipeline> = rows
        .into_iter()
        .map(|r| Pipeline {
            id: r.id,
            user_id: r.user_id,
            name: r.name,
            version: r.version,
            toml_content: r.toml_content,
            rules_count: r.rules_count as usize,
            forked_from: r.forked_from,
            visibility: r.visibility,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect();

    Ok(Json(list))
}

/// GET /api/pipelines/{id}
pub async fn get_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<Pipeline> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let row: Option<models::PipelineRow> = sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching pipeline {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    // Enforce read permission; foreign private pipelines 404 (existence
    // must not leak — issue #82 P0-4).
    if let Some(r) = &row
        && !can_read_pipeline(&user, r)
    {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    match row {
        Some(r) => Ok(Json(Pipeline {
            id: r.id,
            user_id: r.user_id,
            name: r.name,
            version: r.version,
            toml_content: r.toml_content,
            rules_count: r.rules_count as usize,
            forked_from: r.forked_from,
            visibility: r.visibility,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })),
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )),
    }
}

/// PUT /api/pipelines/{id}
pub async fn update_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Pipeline> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let existing: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error fetching pipeline {id} for update: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let existing = existing.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;

    // Write permission: owner or admin. Foreign pipelines 404 so ownership
    // itself stays private (issue #82 P0-4).
    if !can_write_pipeline(&user, &existing) {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.name)
        .to_string();
    let toml_content = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.toml_content)
        .to_string();
    let visibility = req
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or(&existing.visibility)
        .to_string();
    if !matches!(visibility.as_str(), "private" | "workspace" | "link") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_VISIBILITY",
            format!("visibility must be private, workspace, or link — got '{visibility}'"),
        ));
    }

    let rules_count = oxo_flow_core::WorkflowConfig::parse(&toml_content)
        .map(|wf| wf.rules.len() as i64)
        .unwrap_or(existing.rules_count);

    let now = now_iso();
    // Snapshot the pre-update content so rollback can restore it.
    record_revision(pool, &id, &user.id, &existing.version, &existing.toml_content).await;
    sqlx::query(
        "UPDATE pipelines SET name = ?, toml_content = ?, visibility = ?, rules_count = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(&toml_content)
    .bind(&visibility)
    .bind(rules_count)
    .bind(&now)
    .bind(&id)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error updating pipeline {id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(Pipeline {
        id,
        user_id: existing.user_id,
        name,
        version: existing.version,
        toml_content,
        rules_count: rules_count as usize,
        forked_from: existing.forked_from,
        visibility,
        created_at: existing.created_at,
        updated_at: now,
    }))
}

/// DELETE /api/pipelines/{id}
pub async fn delete_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let existing: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error fetching pipeline {id} for deletion: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let existing = existing.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;

    // Write permission: owner or admin (issue #82 P0-4).
    if !can_write_pipeline(&user, &existing) {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    sqlx::query("DELETE FROM pipelines WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error deleting pipeline {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// GET /api/templates
pub async fn list_templates() -> ApiResult<Vec<Template>> {
    let pool = get_pool()?;

    let rows: Vec<models::TemplateRow> =
        sqlx::query_as("SELECT * FROM templates ORDER BY category, name ASC")
            .fetch_all(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error listing templates: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let list: Vec<Template> = rows
        .into_iter()
        .map(|r| Template {
            id: r.id,
            name: r.name,
            category: r.category,
            description: r.description,
            tags: serde_json::from_str(&r.tags).unwrap_or_default(),
            toml_content: Some(r.toml_content),
            is_system: r.is_system != 0,
            created_by: r.created_by,
            usage_count: r.usage_count as u64,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect();

    Ok(Json(list))
}

/// GET /api/templates/{id}
pub async fn get_template(Path(id): Path<String>) -> ApiResult<Template> {
    let pool = get_pool()?;

    let row: Option<models::TemplateRow> = sqlx::query_as("SELECT * FROM templates WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching template {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    match row {
        Some(r) => Ok(Json(Template {
            id: r.id,
            name: r.name,
            category: r.category,
            description: r.description,
            tags: serde_json::from_str(&r.tags).unwrap_or_default(),
            toml_content: Some(r.toml_content),
            is_system: r.is_system != 0,
            created_by: r.created_by,
            usage_count: r.usage_count as u64,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })),
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Template {id} not found"),
        )),
    }
}

/// POST /api/templates
pub async fn save_template(
    authenticated: Option<Extension<CurrentUser>>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Template> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "MISSING", "name required".into()))?;
    let category = req
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let description = req
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let toml_content = req
        .get("toml_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "toml_content required".into(),
            )
        })?;
    let tags: Vec<String> = req
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let template_id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let is_system = req
        .get("is_system")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // System templates are shared resources — only admins may create or
    // promote them (issue #81 template-DELETE-auth companion fix).
    if is_system && !user.is_admin() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Admin role required to create system templates".into(),
        ));
    }

    let now = now_iso();
    let id = if template_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        template_id.to_string()
    };

    // Updating an existing system template is an admin operation too.
    if !template_id.is_empty() && !user.is_admin() {
        let existing_is_system: Option<i64> =
            sqlx::query_scalar("SELECT is_system FROM templates WHERE id = ?")
                .bind(&id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
        if existing_is_system == Some(1) {
            return Err(err(
                StatusCode::FORBIDDEN,
                "ACCESS_DENIED",
                "Only admins may modify system templates".into(),
            ));
        }
    }

    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());

    sqlx::query(
        "INSERT INTO templates (id, name, category, description, tags, toml_content, is_system, created_by, usage_count, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, category = excluded.category, description = excluded.description, tags = excluded.tags, toml_content = excluded.toml_content, is_system = excluded.is_system, updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(name)
    .bind(category)
    .bind(description)
    .bind(&tags_json)
    .bind(toml_content)
    .bind(if is_system { 1_i64 } else { 0_i64 })
    .bind(Some(&user.id))
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error saving template: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(Template {
        id,
        name: name.to_string(),
        category: category.to_string(),
        description: description.to_string(),
        tags,
        toml_content: Some(toml_content.to_string()),
        is_system,
        created_by: Some(user.id),
        usage_count: 0_u64,
        created_at: now.clone(),
        updated_at: now,
    }))
}

/// DELETE /api/templates/{id}
pub async fn delete_template(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let existing: Option<models::TemplateRow> =
        sqlx::query_as("SELECT * FROM templates WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error fetching template {id} for deletion: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let existing = existing.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Template {id} not found"),
        )
    })?;

    // System templates are admin-only; user templates belong to their
    // creator (issue #81: template DELETE had no authorization at all).
    let may_delete = if existing.is_system != 0 {
        user.is_admin()
    } else {
        user.is_admin() || existing.created_by.as_deref() == Some(user.id.as_str())
    };
    if !may_delete {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Only the template owner or an admin may delete this template".into(),
        ));
    }

    sqlx::query("DELETE FROM templates WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error deleting template {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

// ---------------------------------------------------------------------------
// Data discovery
// ---------------------------------------------------------------------------

/// POST /api/data/analyze
pub async fn analyze_data(Json(req): Json<DataAnalysisRequest>) -> ApiResult<DataAnalysisResponse> {
    super::data::analyze_files(&req.paths, req.max_depth)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DATA_ERROR", e))
}

/// POST /api/data/reference
pub async fn discover_reference(Json(req): Json<ReferenceRequest>) -> ApiResult<ReferenceResponse> {
    super::data::discover_reference(&req.genome, &req.components)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "REF_ERROR", e))
}

// ---------------------------------------------------------------------------
// Plugin validation
// ---------------------------------------------------------------------------

/// POST /api/plugins/validate
pub async fn validate_plugin(
    Json(req): Json<ValidatePluginRequest>,
) -> ApiResult<ValidatePluginResponse> {
    service::validate_plugin_manifest(&req.manifest, req.trusted_keys.as_ref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PLUGIN_ERROR", e))
}

// ---------------------------------------------------------------------------
// Data Perception API (v0.8 AI Companion)
// ---------------------------------------------------------------------------

/// POST /api/data/perceive
pub async fn perceive_data(Json(req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    use crate::domains::ai::agents::data_agent;

    let paths = req.get("paths").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>()
    });
    let description = req
        .get("description")
        .and_then(|v| v.as_str().map(String::from));

    let report = if let Some(ref p) = paths {
        if !p.is_empty() {
            data_agent::analyze_paths(p)
        } else if let Some(ref desc) = description {
            data_agent::analyze_description(desc)
        } else {
            data_agent::analyze_paths(&[])
        }
    } else if let Some(ref desc) = description {
        data_agent::analyze_description(desc)
    } else {
        data_agent::analyze_paths(&[])
    };

    Ok(Json(serde_json::json!(report)))
}

/// GET /api/data/reference/status
pub async fn reference_status() -> ApiResult<serde_json::Value> {
    let common_refs: Vec<(&str, Vec<&str>)> = vec![
        (
            "hg38",
            vec![
                "/data/references/hg38/genome.fa",
                "/data/references/hg38/star",
                "/data/references/hg38/genes.gtf",
            ],
        ),
        (
            "mm10",
            vec![
                "/data/references/mm10/genome.fa",
                "/data/references/mm10/star",
                "/data/references/mm10/genes.gtf",
            ],
        ),
        (
            "mm39",
            vec![
                "/data/references/mm39/genome.fa",
                "/data/references/mm39/star",
            ],
        ),
    ];

    let mut installed = Vec::new();
    let mut missing = Vec::new();
    let mut download_commands = Vec::new();

    for (genome, required_paths) in common_refs {
        let mut found = Vec::new();
        let mut not_found = Vec::new();

        for p in required_paths {
            if std::path::Path::new(p).exists() {
                found.push(p.to_string());
            } else {
                not_found.push(p.to_string());
            }
        }

        if !found.is_empty() || !not_found.is_empty() {
            installed.push(serde_json::json!({
                "genome": genome,
                "found": found,
                "missing": not_found,
            }));
        }
        for m in &not_found {
            missing.push(m.clone());
        }
        if !not_found.is_empty() {
            download_commands.push(format!(
                "wget -P /data/references/{genome} https://genome-idx.example.com/{genome}.tar.gz"
            ));
        }
    }

    Ok(Json(serde_json::json!({
        "installed": installed,
        "missing": missing,
        "download_commands": download_commands,
    })))
}

/// POST /api/data/samplesheet/parse
pub async fn parse_samplesheet(Json(req): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    let content = req.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let mut samples = Vec::new();
    let mut format = "unknown";

    let lines: Vec<&str> = content.lines().collect();
    if lines.len() >= 2 {
        let header = lines[0].to_lowercase();
        if header.contains("sample") || header.contains("sample_name") {
            format = "standard";
            for line in &lines[1..] {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split(',').collect();
                if !parts.is_empty() {
                    let mut entry = serde_json::json!({"sample": parts[0].trim()});
                    if parts.len() >= 2 {
                        entry["fastq_r1"] = serde_json::json!(parts[1].trim());
                    }
                    if parts.len() >= 3 {
                        entry["fastq_r2"] = serde_json::json!(parts[2].trim());
                    }
                    if parts.len() >= 4 {
                        entry["condition"] = serde_json::json!(parts[3].trim());
                    }
                    samples.push(entry);
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "format": format,
        "samples_count": samples.len(),
        "samples": samples,
        "detected_headers": if lines.is_empty() { Vec::<String>::new() } else {
            lines[0].split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
        }
    })))
}

// ---------------------------------------------------------------------------
// Pipeline version history (issue #82 P1-14)
// ---------------------------------------------------------------------------

/// GET /api/pipelines/{id}/revisions — snapshot list, newest first.
pub async fn list_revisions(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<Vec<serde_json::Value>> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    format!("DB error fetching pipeline {id}: {e}"),
                )
            })?;
    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;
    if !can_read_pipeline(&user, &pipeline) {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, version, user_id, created_at FROM pipeline_revisions \
         WHERE pipeline_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 50",
    )
    .bind(&id)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            format!("DB error listing revisions: {e}"),
        )
    })?;

    let revisions: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(rid, version, user_id, created_at)| {
            serde_json::json!({
                "id": rid,
                "version": version,
                "actor": user_id,
                "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(revisions))
}

/// GET /api/pipelines/{id}/revisions/{rev} — one snapshot's full TOML.
pub async fn get_revision(
    authenticated: Option<Extension<CurrentUser>>,
    Path((id, rev)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    format!("DB error fetching pipeline {id}: {e}"),
                )
            })?;
    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;
    if !can_read_pipeline(&user, &pipeline) {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, version, user_id, toml_content FROM pipeline_revisions \
         WHERE id = ? AND pipeline_id = ?",
    )
    .bind(&rev)
    .bind(&id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            format!("DB error fetching revision: {e}"),
        )
    })?;
    let (rid, version, actor, toml_content) = row.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Revision {rev} not found"),
        )
    })?;
    Ok(Json(serde_json::json!({
        "id": rid,
        "pipeline_id": id,
        "version": version,
        "actor": actor,
        "toml_content": toml_content,
    })))
}

/// POST /api/pipelines/{id}/rollback — restore a revision as the current
/// pipeline (creating a new revision, so nothing is lost).
pub async fn rollback_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Pipeline> {
    let user = resolve(authenticated.as_ref());
    let pool = get_pool()?;

    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    format!("DB error fetching pipeline {id}: {e}"),
                )
            })?;
    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;
    if !can_write_pipeline(&user, &pipeline) {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    let revision_id = req
        .get("revision_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "MISSING",
                "revision_id required".into(),
            )
        })?;
    let rev: Option<(String,)> = sqlx::query_as(
        "SELECT toml_content FROM pipeline_revisions WHERE id = ? AND pipeline_id = ?",
    )
    .bind(revision_id)
    .bind(&id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            format!("DB error fetching revision: {e}"),
        )
    })?;
    let toml_content = rev
        .map(|r| r.0)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "NOT_FOUND", "Revision not found".into()))?;

    // Restore = snapshot current + write the old content back.
    record_revision(pool, &id, &user.id, &pipeline.version, &pipeline.toml_content).await;
    let rules_count = oxo_flow_core::WorkflowConfig::parse(&toml_content)
        .map(|wf| wf.rules.len() as i64)
        .unwrap_or(pipeline.rules_count);
    let now = now_iso();
    sqlx::query(
        "UPDATE pipelines SET toml_content = ?, rules_count = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&toml_content)
    .bind(rules_count)
    .bind(&now)
    .bind(&id)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error rolling back pipeline {id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(Pipeline {
        id,
        user_id: pipeline.user_id,
        name: pipeline.name,
        version: pipeline.version,
        toml_content,
        rules_count: rules_count as usize,
        forked_from: pipeline.forked_from,
        visibility: pipeline.visibility,
        created_at: pipeline.created_at,
        updated_at: now,
    }))
}
