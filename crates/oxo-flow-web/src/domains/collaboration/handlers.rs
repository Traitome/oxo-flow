//! HTTP handlers for collaboration domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.

use crate::extract::ApiJson;
use axum::{Extension, Json, extract::Path, http::StatusCode};

use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::collaboration::types::*;
use crate::domains::workflow::handlers::{ApiError, err, now_iso};
use crate::infra::db::models;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn get_backend()
-> Result<std::sync::Arc<dyn crate::infra::db::StorageBackend>, (StatusCode, Json<ApiError>)> {
    crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

#[utoipa::path(
    post,
    path = "/api/pipelines/{id}/fork",
    tag = "collaboration",
    responses(
        (status = 200, description = "Success", body = ForkResponse),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/{id}/fork
pub async fn fork_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<ForkResponse> {
    let backend = get_backend()?;
    // Ownership comes from the session, never from the request body
    // (issue #82 P0-4: client-supplied user ids were trusted before).
    let user = resolve(authenticated.as_ref());

    // Fetch the source pipeline
    let source = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching source pipeline {id} for fork: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
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

    backend.save_pipeline(&new_pipeline).await.map_err(|e| {
        tracing::error!("DB error creating forked pipeline: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    // Log the fork action
    if let Err(error) = backend
        .log_action(&user.id, "fork_pipeline", &forked_id)
        .await
    {
        tracing::error!(
            user_id = %user.id,
            forked_id,
            error = %error,
            "fork succeeded but its audit record could not be persisted"
        );
    }

    Ok(Json(ForkResponse { forked_id, name }))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/{id}/share",
    tag = "collaboration",
    responses(
        (status = 200, description = "Success", body = ShareResponse),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/{id}/share
pub async fn share_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ShareRequest>,
) -> ApiResult<ShareResponse> {
    let backend = get_backend()?;
    let user = resolve(authenticated.as_ref());

    // Verify pipeline exists and the caller may share it (owner or admin).
    let pipeline = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching pipeline {id} to share: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
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

    // A share link is an anonymous read-only landing page: the token IS the
    // authorization, so "anyone holding the link" is the only access mode
    // this endpoint can honour. Storing "private"/"workspace" would persist
    // a restriction nothing enforces, so unsupported values are rejected
    // instead of silently ignored.
    if !matches!(body.visibility.as_str(), "link" | "public") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_VISIBILITY",
            format!(
                "visibility must be \"link\" (alias \"public\") — a share token grants \
                 anonymous read access to anyone holding it; got \x27{}\x27",
                body.visibility
            ),
        ));
    }

    let token = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    // chrono's `DateTime + TimeDelta` PANICS on overflow, and u32::MAX days
    // (~11.7M years) is far past chrono's year-262143 ceiling — bound the
    // value so a bad payload gets a structured 400, not a dropped
    // connection (audit #660). 10 years is a generous share lifetime.
    const MAX_EXPIRES_IN_DAYS: u32 = 3650;
    if let Some(d) = body.expires_in_days
        && d > MAX_EXPIRES_IN_DAYS
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_EXPIRY",
            format!("expires_in_days must be at most {MAX_EXPIRES_IN_DAYS}, got {d}"),
        ));
    }
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

    backend.create_share(&share).await.map_err(|e| {
        tracing::error!("DB error creating share link for pipeline {id}: {e}");
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    Ok(Json(ShareResponse {
        // Single source of truth for the URL shape (server.rs): emits
        // oxo+http:// unless OXO_FLOW_PUBLIC_URL declares an HTTPS origin —
        // the server itself has no TLS, so a hardcoded oxo+https:// was a
        // transport lie on plain-HTTP deployments (#907 R16).
        share_url: crate::server::share_url_for(&token),
        access_token: token,
        expires_at,
    }))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/import",
    tag = "collaboration",
    responses(
        (status = 200, description = "Success", body = ImportResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/import
pub async fn import_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(body): ApiJson<ImportRequest>,
) -> ApiResult<ImportResponse> {
    // Validate URL format (both schemes accepted: the share handler mints
    // oxo+http:// by default and oxo+https:// behind a public HTTPS origin)
    if !body.url.starts_with("oxo+https://") && !body.url.starts_with("oxo+http://") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_URL",
            "URL must use oxo+https:// or oxo+http:// format".into(),
        ));
    }

    // Extract token from URL: oxo+https://host[:port]/share/{token} or
    // oxo+http://host[:port]/share/{token}
    let token = body.url.rsplit('/').next().unwrap_or("").to_string();

    let backend = get_backend()?;
    let user = resolve(authenticated.as_ref());

    // Look up the share by token
    let share = backend
        .get_share_by_token(&token)
        .await
        .map_err(|e| {
            tracing::error!("DB error looking up share by token: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
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
    let pipeline = backend
        .get_pipeline(&share.pipeline_id)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching source pipeline for import: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                "Source pipeline no longer exists".into(),
            )
        })?;

    // Create imported copy, attributed to the acting user (issue #82 P0-4).
    let import_id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();

    let imported_pipeline = models::PipelineRow {
        id: import_id.clone(),
        user_id: user.id.clone(),
        name: format!("{} (imported)", pipeline.name),
        version: pipeline.version.clone(),
        toml_content: pipeline.toml_content.clone(),
        rules_count: pipeline.rules_count,
        forked_from: Some(share.pipeline_id),
        visibility: "private".to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };

    backend
        .save_pipeline(&imported_pipeline)
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

#[utoipa::path(
    get,
    path = "/api/share/{token}",
    tag = "collaboration",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// GET /api/share/{token} — public read-only landing payload for a share
/// link (issue #82 P0-6): pipeline identity + DAG summary + the TOML +
/// the most recent run's outcome. The share row IS the authorization —
/// this endpoint deliberately sits on the anonymous whitelist so a share
/// URL opens without a session.
pub async fn get_share_landing(Path(token): Path<String>) -> ApiResult<serde_json::Value> {
    let backend = get_backend()?;

    let share = backend
        .get_share_by_token(&token)
        .await
        .map_err(|e| {
            tracing::error!("DB error looking up share by token: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
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

    let pipeline = backend
        .get_pipeline(&share.pipeline_id)
        .await
        .map_err(|_| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                "Source pipeline no longer exists".into(),
            )
        })?;

    // DAG summary: rule names in execution order (no full layout — the
    // landing page shows the shape, the importer gets the TOML).
    // Config expansion matches `run` (issue #707): {config.*}-routed
    // producer/consumer pairs only match after placeholder expansion.
    let dag_names: Vec<String> = oxo_flow_core::WorkflowConfig::parse(&pipeline.toml_content)
        .ok()
        .and_then(|wf| {
            oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
                &wf.rules,
                &wf.config_placeholder_values(),
            )
            .ok()
        })
        .and_then(|d| d.execution_order().ok())
        .unwrap_or_default();

    // Most recent terminal run as provenance evidence.
    let recent: Option<(String, Option<String>)> = backend
        .get_latest_terminal_run_for_pipeline(&share.pipeline_id)
        .await
        .unwrap_or(None);

    let owner_username: Option<String> = backend
        .get_user_by_id(&share.owner_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username);

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
