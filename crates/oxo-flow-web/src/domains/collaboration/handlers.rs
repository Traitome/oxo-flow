//! HTTP handlers for collaboration domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.

use axum::{Json, extract::Path, http::StatusCode};

use crate::domains::collaboration::types::*;
use crate::domains::workflow::handlers::{ApiError, err};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

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

async fn get_or_default_user(pool: &sqlx::SqlitePool, fallback: &str) -> String {
    sqlx::query_as::<_, crate::infra::db::models::UserRow>(
        "SELECT * FROM users WHERE role = 'admin' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None)
    .map(|u| u.id)
    .unwrap_or_else(|| fallback.to_string())
}

/// POST /api/pipelines/{id}/fork
pub async fn fork_pipeline(
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<ForkResponse> {
    let pool = get_pool()?;
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "default".into());
    let user_id = get_or_default_user(pool, &user_id).await;

    // Fetch the source pipeline
    let source: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let source = source.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;

    // Create a new pipeline as a fork
    let forked_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let name = format!("{} (fork)", source.name);

    let new_pipeline = models::PipelineRow {
        id: forked_id.clone(),
        user_id: user_id.clone(),
        name: name.clone(),
        version: source.version.clone(),
        toml_content: source.toml_content.clone(),
        rules_count: source.rules_count,
        forked_from: Some(id.clone()),
        visibility: "private".to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };

    sqlx::query(
        "INSERT INTO pipelines (id, user_id, name, version, toml_content, rules_count, forked_from, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&new_pipeline.id)
    .bind(&new_pipeline.user_id)
    .bind(&new_pipeline.name)
    .bind(&new_pipeline.version)
    .bind(&new_pipeline.toml_content)
    .bind(new_pipeline.rules_count)
    .bind(&new_pipeline.forked_from)
    .bind(&new_pipeline.visibility)
    .bind(&new_pipeline.created_at)
    .bind(&new_pipeline.updated_at)
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    // Log the fork action
    let _ = sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, metadata, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user_id)
    .bind("fork_pipeline")
    .bind(&forked_id)
    .bind(Some(format!("{{\"forked_from\": \"{id}\"}}")))
    .bind(&now)
    .execute(pool)
    .await;

    Ok(Json(ForkResponse { forked_id, name }))
}

/// POST /api/pipelines/{id}/share
pub async fn share_pipeline(
    Path(id): Path<String>,
    Json(body): Json<ShareRequest>,
) -> ApiResult<ShareResponse> {
    let pool = get_pool()?;

    // Verify pipeline exists
    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if pipeline.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    let token = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let expires_at = body
        .expires_in_days
        .map(|d| (chrono::Utc::now() + chrono::Duration::days(d as i64)).to_rfc3339());

    // Save share to DB
    let owner_id = get_or_default_user(pool, "default").await;
    let share = models::ShareRow {
        id: uuid::Uuid::new_v4().to_string(),
        pipeline_id: id.clone(),
        owner_id,
        token: token.clone(),
        visibility: body.visibility.clone(),
        expires_at: expires_at.clone(),
        created_at: now,
    };

    sqlx::query(
        "INSERT INTO shares (id, pipeline_id, owner_id, token, visibility, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&share.id)
    .bind(&share.pipeline_id)
    .bind(&share.owner_id)
    .bind(&share.token)
    .bind(&share.visibility)
    .bind(&share.expires_at)
    .bind(&share.created_at)
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(ShareResponse {
        share_url: format!("oxo+https://localhost:8777/share/{token}"),
        access_token: token,
        expires_at,
    }))
}

/// POST /api/pipelines/import
pub async fn import_pipeline(Json(body): Json<ImportRequest>) -> ApiResult<ImportResponse> {
    // Validate URL format
    if !body.url.starts_with("oxo+https://") && !body.url.starts_with("oxo+http://") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_URL",
            "URL must use oxo+https:// format".into(),
        ));
    }

    // Extract token from URL: oxo+https://host/share/{token}
    let token = body.url.rsplit('/').next().unwrap_or("").to_string();

    let pool = get_pool()?;

    // Look up the share by token
    let share: Option<models::ShareRow> = sqlx::query_as("SELECT * FROM shares WHERE token = ?")
        .bind(&token)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let share = share.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Share not found or expired".into(),
        )
    })?;

    // Check expiration
    if let Some(ref expires) = share.expires_at
        && let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires)
        && chrono::Utc::now() > exp
    {
        return Err(err(
            StatusCode::GONE,
            "EXPIRED",
            "Share link has expired".into(),
        ));
    }

    // Fetch the shared pipeline and import as a copy
    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&share.pipeline_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Source pipeline no longer exists".into(),
        )
    })?;

    // Create imported copy
    let import_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let owner_id = get_or_default_user(pool, "default").await;

    sqlx::query(
        "INSERT INTO pipelines (id, user_id, name, version, toml_content, rules_count, forked_from, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&import_id)
    .bind(&owner_id)
    .bind(format!("{} (imported)", pipeline.name))
    .bind(&pipeline.version)
    .bind(&pipeline.toml_content)
    .bind(pipeline.rules_count)
    .bind(Some(share.pipeline_id))
    .bind("private")
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(ImportResponse {
        pipeline_id: import_id,
    }))
}
