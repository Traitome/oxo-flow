//! HTTP handlers for cluster connections (SSH endpoints).

use crate::extract::ApiJson;
use axum::extract::Path;
use axum::{Extension, Json, http::StatusCode};

use crate::domains::auth::current_user;
use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::workflow::handlers::{ApiError, db_err, err, get_backend, now_iso};
use crate::infra::db::models;

use super::service;
use super::types::{ClusterInfo, ClusterProbeResult, ClusterUpsertRequest};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// Cluster connections hold SSH credentials shared by the whole platform.
/// Managing and probing them (upsert/delete/probe) is admin-only outside
/// personal mode (issues #82 P0-5, #517); listing stays available to any
/// authenticated user so they can see what a run could target.
fn require_cluster_admin(user: &CurrentUser) -> Result<(), (StatusCode, Json<ApiError>)> {
    current_user::require_role_outside_personal(
        user,
        "admin",
        "Admin role required to manage cluster connections",
    )
}

#[utoipa::path(
    get,
    path = "/api/clusters",
    tag = "clusters",
    responses(
        (status = 200, description = "Success", body = Vec<ClusterInfo>),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/clusters — list configured cluster connections.
pub async fn list_clusters() -> ApiResult<Vec<ClusterInfo>> {
    let backend = get_backend()?;
    let rows = backend
        .list_clusters()
        .await
        .map_err(|e| db_err(format!("DB error listing clusters: {e}")))?;
    Ok(Json(rows.into_iter().map(cluster_from_row).collect()))
}

#[utoipa::path(
    post,
    path = "/api/clusters",
    tag = "clusters",
    responses(
        (status = 200, description = "Success", body = ClusterInfo),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// POST /api/clusters — create or update a cluster connection (upsert by id).
pub async fn upsert_cluster(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<ClusterUpsertRequest>,
) -> ApiResult<ClusterInfo> {
    let user = resolve(authenticated.as_ref());
    require_cluster_admin(&user)?;
    service::validate(&req).map_err(|e| err(StatusCode::BAD_REQUEST, "INVALID_CLUSTER", e))?;
    let backend = get_backend()?;
    let now = now_iso();
    let row = models::ClusterRow {
        id: req.id.clone(),
        name: req.name.clone(),
        ssh_host: req.ssh_host.clone(),
        ssh_port: req.ssh_port as i64,
        ssh_user: req.ssh_user.clone(),
        ssh_key: req.ssh_key.as_deref().map(crate::infra::crypto::seal),
        scheduler: req.scheduler.clone(),
        remote_dir: req.remote_dir.clone(),
        enabled: req.enabled,
        created_at: now,
    };
    backend
        .save_cluster(&row)
        .await
        .map_err(|e| db_err(format!("DB error upserting cluster {}: {e}", req.id)))?;
    Ok(Json(cluster_from_row(row)))
}

#[utoipa::path(
    delete,
    path = "/api/clusters/{id}",
    tag = "clusters",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// DELETE /api/clusters/{id}
pub async fn delete_cluster(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    require_cluster_admin(&user)?;
    let backend = get_backend()?;
    let deleted = backend
        .delete_cluster(&id)
        .await
        .map_err(|e| db_err(format!("DB error deleting cluster {id}: {e}")))?;
    if !deleted {
        return Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Cluster {id} not found"),
        ));
    }
    Ok(Json(serde_json::json!({"deleted": id})))
}

#[utoipa::path(
    post,
    path = "/api/clusters/{id}/probe",
    tag = "clusters",
    responses(
        (status = 200, description = "Success", body = ClusterProbeResult),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// POST /api/clusters/{id}/probe — SSH connectivity + scheduler detection.
///
/// Admin-only outside personal mode (#517): a probe actively uses the
/// stored SSH credential against an arbitrary endpoint.
pub async fn probe_cluster(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<ClusterProbeResult> {
    let user = resolve(authenticated.as_ref());
    require_cluster_admin(&user)?;
    let backend = get_backend()?;
    let row = backend
        .get_cluster(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching cluster {id} for probe: {e}")))?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                format!("Cluster {id} not found"),
            )
        })?;
    Ok(Json(service::probe(&cluster_from_row(row)).await))
}

fn cluster_from_row(row: models::ClusterRow) -> ClusterInfo {
    ClusterInfo {
        id: row.id,
        name: row.name,
        ssh_host: row.ssh_host,
        ssh_port: row.ssh_port as u16,
        ssh_user: row.ssh_user,
        // The stored (possibly sealed) value stays in-memory for the
        // probe/submit paths; `skip_serializing` keeps it out of every API
        // response and `ssh_key_set` tells clients whether it exists (#517).
        ssh_key_set: row.ssh_key.is_some(),
        ssh_key: row.ssh_key,
        scheduler: row.scheduler,
        remote_dir: row.remote_dir,
        enabled: row.enabled,
        created_at: row.created_at,
    }
}

/// Import cluster definitions from the platform config file at startup
/// (idempotent upsert by id; existing DB rows win — the UI is the
/// runtime source of truth).
pub async fn import_from_config(definitions: &[crate::config::ClusterDefinition]) {
    for def in definitions {
        let req = ClusterUpsertRequest {
            id: def.id.clone(),
            name: def.name.clone(),
            ssh_host: def.ssh_host.clone(),
            ssh_port: def.ssh_port,
            ssh_user: def.ssh_user.clone(),
            ssh_key: def.ssh_key.clone(),
            scheduler: def.scheduler.clone(),
            remote_dir: def.remote_dir.clone(),
            enabled: def.enabled,
        };
        if service::validate(&req).is_err() {
            tracing::warn!("platform config cluster '{}' is invalid — skipped", def.id);
            continue;
        }
        let now = now_iso();
        let Some(backend) = crate::infra::db::global_backend() else {
            continue;
        };
        match backend.get_cluster(&req.id).await {
            Ok(None) => {
                let row = models::ClusterRow {
                    id: req.id.clone(),
                    name: req.name.clone(),
                    ssh_host: req.ssh_host.clone(),
                    ssh_port: req.ssh_port as i64,
                    ssh_user: req.ssh_user.clone(),
                    ssh_key: req.ssh_key.as_deref().map(crate::infra::crypto::seal),
                    scheduler: req.scheduler.clone(),
                    remote_dir: req.remote_dir.clone(),
                    enabled: req.enabled,
                    created_at: now,
                };
                if let Err(error) = backend.save_cluster(&row).await {
                    tracing::warn!(
                        cluster_id = %req.id,
                        error = %error,
                        "failed to import cluster from platform config"
                    );
                }
            }
            Ok(Some(_)) => {}
            Err(error) => {
                tracing::warn!(
                    cluster_id = %req.id,
                    error = %error,
                    "could not check for an existing cluster; config import skipped"
                );
            }
        }
    }
}
