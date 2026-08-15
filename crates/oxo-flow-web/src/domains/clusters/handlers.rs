//! HTTP handlers for cluster connections (SSH endpoints).

use axum::extract::Path;
use axum::{Extension, Json, http::StatusCode};

use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::workflow::handlers::{ApiError, err};
use crate::infra::db::models;

use super::service;
use super::types::{ClusterInfo, ClusterProbeResult, ClusterUpsertRequest};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

fn get_pool() -> Result<&'static sqlx::SqlitePool, (StatusCode, Json<ApiError>)> {
    crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Cluster connections hold SSH credentials shared by the whole platform.
/// Managing them (upsert/delete) is admin-only outside personal mode
/// (issue #82 P0-5); listing and probing stay available to any
/// authenticated user so they can pick a cluster for their runs.
fn require_cluster_admin(user: &CurrentUser) -> Result<(), (StatusCode, Json<ApiError>)> {
    if crate::server::running_mode() != "personal" && !user.is_admin() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ACCESS_DENIED",
            "Admin role required to manage cluster connections".into(),
        ));
    }
    Ok(())
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
    let pool = get_pool()?;
    let rows: Vec<models::ClusterRow> = sqlx::query_as("SELECT * FROM clusters ORDER BY name")
        .fetch_all(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error listing clusters: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;
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
    Json(req): Json<ClusterUpsertRequest>,
) -> ApiResult<ClusterInfo> {
    let user = resolve(authenticated.as_ref());
    require_cluster_admin(&user)?;
    service::validate(&req).map_err(|e| err(StatusCode::BAD_REQUEST, "INVALID_CLUSTER", e))?;
    let pool = get_pool()?;
    let now = now_iso();
    sqlx::query(
        "INSERT INTO clusters (id, name, ssh_host, ssh_port, ssh_user, ssh_key, scheduler, remote_dir, enabled, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, ssh_host = excluded.ssh_host, \
         ssh_port = excluded.ssh_port, ssh_user = excluded.ssh_user, ssh_key = excluded.ssh_key, \
         scheduler = excluded.scheduler, remote_dir = excluded.remote_dir, enabled = excluded.enabled",
    )
    .bind(&req.id)
    .bind(&req.name)
    .bind(&req.ssh_host)
    .bind(req.ssh_port as i64)
    .bind(req.ssh_user.as_deref())
    .bind(req.ssh_key.as_deref())
    .bind(req.scheduler.as_deref())
    .bind(req.remote_dir.as_deref())
    .bind(req.enabled)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!("DB error upserting cluster {}: {e}", req.id);
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "Internal database error".into(),
        )
    })?;

    let row: models::ClusterRow = sqlx::query_as("SELECT * FROM clusters WHERE id = ?")
        .bind(&req.id)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error re-reading cluster {}: {e}", req.id);
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;
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
    let pool = get_pool()?;
    let deleted = sqlx::query("DELETE FROM clusters WHERE id = ?")
        .bind(&id)
        .execute(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error deleting cluster {id}: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?
        .rows_affected();
    if deleted == 0 {
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
pub async fn probe_cluster(Path(id): Path<String>) -> ApiResult<ClusterProbeResult> {
    let pool = get_pool()?;
    let row: Option<models::ClusterRow> = sqlx::query_as("SELECT * FROM clusters WHERE id = ?")
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("DB error fetching cluster {id} for probe: {e}");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "Internal database error".into(),
            )
        })?;
    let row = row.ok_or_else(|| {
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
        let pool = match crate::infra::db::sqlite::try_pool() {
            Ok(pool) => pool,
            Err(_) => return,
        };
        let now = now_iso();
        // INSERT OR IGNORE: existing rows win over the config file.
        let result = sqlx::query(
            "INSERT OR IGNORE INTO clusters (id, name, ssh_host, ssh_port, ssh_user, ssh_key, scheduler, remote_dir, enabled, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&req.id)
        .bind(&req.name)
        .bind(&req.ssh_host)
        .bind(req.ssh_port as i64)
        .bind(req.ssh_user.as_deref())
        .bind(req.ssh_key.as_deref())
        .bind(req.scheduler.as_deref())
        .bind(req.remote_dir.as_deref())
        .bind(req.enabled)
        .bind(&now)
        .execute(pool)
        .await;
        match result {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::info!("Imported cluster '{}' from platform config", req.id);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Failed to import cluster '{}': {e}", req.id),
        }
    }
}
