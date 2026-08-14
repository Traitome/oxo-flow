//! HTTP handlers for collaboration domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.

use axum::{Extension, Json, extract::Path, http::StatusCode};

use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::collaboration::types::*;
use crate::domains::workflow::handlers::{ApiError, err, get_pool};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// POST /api/pipelines/{id}/fork
pub async fn fork_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<ForkResponse> {
    let pool = get_pool()?;
    // Ownership comes from the session, never from the request body
    // (issue #82 P0-4: client-supplied user ids were trusted before).
    let user = resolve(authenticated.as_ref());

    // Fetch the source pipeline
    let source: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error fetching source pipeline {id} for fork: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let source = source.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;

    // Read permission applies to forking too: owners, admins, and
    // workspace-visible pipelines may be forked.
    if !(user.is_admin() || source.user_id == user.id || source.visibility == "workspace") {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        ));
    }

    // Create a new pipeline as a fork
    let forked_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let name = format!("{} (fork)", source.name);

    let new_pipeline = models::PipelineRow {
        id: forked_id.clone(),
        user_id: user.id.clone(),
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
    .map_err(|e| {
        tracing::error!("DB error creating forked pipeline: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    // Log the fork action
    let _ = sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, metadata, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&user.id)
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
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(body): Json<ShareRequest>,
) -> ApiResult<ShareResponse> {
    let pool = get_pool()?;
    let user = resolve(authenticated.as_ref());

    // Verify pipeline exists and the caller may share it (owner or admin).
    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error fetching pipeline {id} to share: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )
    })?;
    if !(user.is_admin() || pipeline.user_id == user.id) {
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

    // Save share to DB, attributed to the acting user.
    let share = models::ShareRow {
        id: uuid::Uuid::new_v4().to_string(),
        pipeline_id: id.clone(),
        owner_id: user.id.clone(),
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
    .map_err(|e| {
        tracing::error!("DB error creating share link for pipeline {id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let host = std::env::var("OXO_FLOW_HOST").unwrap_or_else(|_| "localhost".into());
    // Use the port the server actually bound (issue #82 P0-6: the URL was
    // hardcoded to 3000 even when serving on 8080 or any -p value).
    let port = crate::server::bound_port().unwrap_or(3000);
    Ok(Json(ShareResponse {
        share_url: format!("oxo+https://{host}:{port}/share/{token}"),
        access_token: token,
        expires_at,
    }))
}

/// POST /api/pipelines/import
pub async fn import_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Json(body): Json<ImportRequest>,
) -> ApiResult<ImportResponse> {
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
    let user = resolve(authenticated.as_ref());

    // Look up the share by token
    let share: Option<models::ShareRow> = sqlx::query_as("SELECT * FROM shares WHERE token = ?")
        .bind(&token)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error looking up share by token: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;

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
            .map_err(|e| {
                tracing::error!("DB error fetching source pipeline for import: {e}");
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;

    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Source pipeline no longer exists".into(),
        )
    })?;

    // Create imported copy, attributed to the acting user (issue #82 P0-4).
    let import_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();

    sqlx::query(
        "INSERT INTO pipelines (id, user_id, name, version, toml_content, rules_count, forked_from, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&import_id)
    .bind(&user.id)
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
    .map_err(|e| {
        tracing::error!("DB error creating imported pipeline: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(ImportResponse {
        pipeline_id: import_id,
    }))
}

/// GET /api/share/{token} — public read-only landing payload for a share
/// link (issue #82 P0-6): pipeline identity + DAG summary + the TOML +
/// the most recent run's outcome. The share row IS the authorization —
/// this endpoint deliberately sits on the anonymous whitelist so a share
/// URL opens without a session.
pub async fn get_share_landing(Path(token): Path<String>) -> ApiResult<serde_json::Value> {
    let pool = get_pool()?;

    let share: Option<models::ShareRow> = sqlx::query_as("SELECT * FROM shares WHERE token = ?")
        .bind(&token)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error looking up share by token: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;
    let share = share.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Share not found or expired".into(),
        )
    })?;
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

    let pipeline: Option<models::PipelineRow> =
        sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
            .bind(&share.pipeline_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "Internal database error".into(),
                )
            })?;
    let pipeline = pipeline.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Source pipeline no longer exists".into(),
        )
    })?;

    // DAG summary: rule names in execution order (no full layout — the
    // landing page shows the shape, the importer gets the TOML).
    let dag_names: Vec<String> = oxo_flow_core::WorkflowConfig::parse(&pipeline.toml_content)
        .ok()
        .and_then(|wf| oxo_flow_core::dag::WorkflowDag::from_rules(&wf.rules).ok())
        .and_then(|d| d.execution_order().ok())
        .unwrap_or_default();

    // Most recent terminal run as provenance evidence.
    let recent: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, finished_at FROM runs WHERE pipeline_id = ? \
         AND status IN ('completed', 'failed', 'cancelled') \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&share.pipeline_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    let owner_username: Option<String> = sqlx::query_scalar(
        "SELECT username FROM users WHERE id = ?",
    )
    .bind(&share.owner_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    Ok(Json(serde_json::json!({
        "pipeline": {
            "name": pipeline.name,
            "version": pipeline.version,
            "rules_count": pipeline.rules_count,
            "visibility": pipeline.visibility,
        },
        "dag": dag_names,
        "toml_content": pipeline.toml_content,
        "owner": owner_username,
        "created_at": share.created_at,
        "expires_at": share.expires_at,
        "recent_run": recent.map(|(status, finished_at)| {
            serde_json::json!({"status": status, "finished_at": finished_at})
        }),
    })))
}
