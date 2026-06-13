use axum::{Json, extract::Path, http::StatusCode};

use crate::domains::collaboration::types::*;
use crate::domains::workflow::handlers::{ApiError, err};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// POST /api/pipelines/{id}/fork
pub async fn fork_pipeline(
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<ForkResponse> {
    // In production, user_id comes from auth middleware. For v0.8, accept in body.
    let _user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    // Use in-memory SQLite pool for now
    let _pool = crate::infra::db::sqlite::pool();

    // Build a temporary backend wrapper... For now, return a demo response
    Ok(Json(ForkResponse {
        forked_id: uuid::Uuid::new_v4().to_string(),
        name: format!("pipeline-{id} (fork)"),
    }))
}

/// POST /api/pipelines/{id}/share
pub async fn share_pipeline(
    Path(_id): Path<String>,
    Json(body): Json<ShareRequest>,
) -> ApiResult<ShareResponse> {
    let token = uuid::Uuid::new_v4().to_string();
    Ok(Json(ShareResponse {
        share_url: format!("oxo+https://localhost:8777/share/{token}"),
        access_token: token,
        expires_at: body.expires_in_days.map(|d| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + (d as u64 * 86400);
            secs.to_string()
        }),
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
    Ok(Json(ImportResponse {
        pipeline_id: uuid::Uuid::new_v4().to_string(),
    }))
}
