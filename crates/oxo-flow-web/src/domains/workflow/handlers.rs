//! HTTP handlers for workflow domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, extract::Path, http::StatusCode};

use super::service;
use super::types::*;
use crate::domains::observability::types::*;
use crate::infra::db::models;

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

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
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
pub async fn export_pipeline(Json(req): Json<ExportRequest>) -> ApiResult<ExportResponse> {
    let id = req.pipeline_id.as_deref().unwrap_or("");
    service::export_pipeline(id, req.format.as_deref())
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
pub async fn save_pipeline(Json(req): Json<serde_json::Value>) -> ApiResult<Pipeline> {
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
    // Find admin user ID for FK constraint
    let admin_row: Option<models::UserRow> =
        sqlx::query_as("SELECT * FROM users WHERE role = 'admin' LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    let user_id = admin_row.map(|u| u.id).unwrap_or_else(|| "default".into());

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
pub async fn list_pipelines() -> ApiResult<Vec<Pipeline>> {
    let pool = get_pool()?;

    let rows: Vec<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines ORDER BY updated_at DESC LIMIT 100")
            .fetch_all(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
pub async fn get_pipeline(Path(id): Path<String>) -> ApiResult<Pipeline> {
    let pool = get_pool()?;

    let row: Option<models::PipelineRow> = sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
    Path(id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Pipeline> {
    let pool = get_pool()?;

    let existing: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let existing = existing.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;

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

    let rules_count = oxo_flow_core::WorkflowConfig::parse(&toml_content)
        .map(|wf| wf.rules.len() as i64)
        .unwrap_or(existing.rules_count);

    let now = now_iso();
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
pub async fn delete_pipeline(Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let pool = get_pool()?;

    let existing: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if existing.is_none() {
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
pub async fn save_template(Json(req): Json<serde_json::Value>) -> ApiResult<Template> {
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

    let now = now_iso();
    let id = if template_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        template_id.to_string()
    };

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
    .bind(None::<String>)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(Template {
        id,
        name: name.to_string(),
        category: category.to_string(),
        description: description.to_string(),
        tags,
        toml_content: Some(toml_content.to_string()),
        is_system,
        created_by: None,
        usage_count: 0_u64,
        created_at: now.clone(),
        updated_at: now,
    }))
}

/// DELETE /api/templates/{id}
pub async fn delete_template(Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let pool = get_pool()?;

    let existing: Option<models::TemplateRow> =
        sqlx::query_as("SELECT * FROM templates WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if existing.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Template {id} not found"),
        ));
    }

    sqlx::query("DELETE FROM templates WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

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
