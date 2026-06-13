//! HTTP handlers for DAG Edit domain.

use crate::domains::workflow::handlers::{ApiError, err};
use axum::{Json, extract::Path, http::StatusCode};

type AR<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// POST /api/pipeline/{id}/command
pub async fn edit_command(
    Path(id): Path<String>,
    Json(cmd): Json<super::service::DagEditCommand>,
) -> AR<super::service::DagEditResponse> {
    let default_toml = "[workflow]\nname = \"edit\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\n\n[[rules]]\nname = \"s2\"\nshell = \"echo s2\"\ndepends_on = [\"s1\"]\n";
    super::service::execute_edit(default_toml, &id, &cmd)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DAG_EDIT_ERROR", e))
}

/// POST /api/pipeline/{id}/undo
pub async fn undo_command(Path(id): Path<String>) -> AR<serde_json::Value> {
    match super::service::undo(&id) {
        Ok(Some(toml)) => Ok(Json(serde_json::json!({"toml_content": toml}))),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            "NO_UNDO",
            "Nothing to undo".into(),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "UNDO_ERROR", e)),
    }
}

/// POST /api/pipeline/{id}/redo
pub async fn redo_command(Path(id): Path<String>) -> AR<serde_json::Value> {
    match super::service::redo(&id) {
        Ok(Some(toml)) => Ok(Json(serde_json::json!({"toml_content": toml}))),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            "NO_REDO",
            "Nothing to redo".into(),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "REDO_ERROR", e)),
    }
}
