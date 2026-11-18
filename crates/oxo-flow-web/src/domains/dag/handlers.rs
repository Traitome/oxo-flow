//! HTTP handlers for DAG Edit domain.

use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::workflow::handlers::{ApiError, err};
use crate::extract::ApiJson;
use axum::{Extension, Json, extract::Path, http::StatusCode};

type AR<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// Fail-closed code for an undo/redo whose history stack does not exist for
/// this identity — first use before any edit, or lost to a server restart or
/// LRU eviction (issue #1137).
pub const EDIT_HISTORY_UNAVAILABLE: &str = "EDIT_HISTORY_UNAVAILABLE";

/// Fail-closed code for an undo/redo whose supplied TOML is not the tip of
/// the recorded history (stale or racing client). Nothing is applied.
pub const EDIT_HISTORY_STALE: &str = "EDIT_HISTORY_STALE";

/// A history refusal carries a remediation suggestion, unlike the generic
/// [`err`] envelope — the caller must know *how* to recover (reload the
/// pipeline / re-apply the edit), not just that undo failed.
fn history_closed(
    status: StatusCode,
    code: &str,
    message: String,
    suggestion: &str,
) -> (StatusCode, Json<ApiError>) {
    let (status, Json(mut body)) = err(status, code, message);
    body.suggestion = Some(suggestion.into());
    (status, Json(body))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct EditPayload {
    pub toml_content: String,
    #[serde(flatten)]
    pub cmd: super::service::DagEditCommand,
}

#[utoipa::path(
    post,
    path = "/api/pipeline/{id}/command",
    tag = "dag",
    responses(
        (status = 200, description = "Success", body = super::service::DagEditResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipeline/{id}/command
///
/// The edit is recorded in the undo history of the acting identity (the
/// resolved session user, or the `default` pseudo-user in personal mode).
pub async fn edit_command(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(payload): ApiJson<EditPayload>,
) -> AR<super::service::DagEditResponse> {
    let user = resolve(authenticated.as_ref());
    super::service::execute_edit(&user.id, &payload.toml_content, &id, &payload.cmd)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DAG_EDIT_ERROR", e))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CurrentToml {
    pub toml_content: String,
}

#[utoipa::path(
    post,
    path = "/api/pipeline/{id}/undo",
    tag = "dag",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
        (status = 404, description = "History exists but there is nothing to undo (NO_UNDO)", body = ApiError),
        (status = 409, description = "Fail-closed: EDIT_HISTORY_UNAVAILABLE (no stack for this identity/pipeline, e.g. after a server restart) or EDIT_HISTORY_STALE (the supplied TOML is not the history tip)", body = ApiError),
    )
)]
/// POST /api/pipeline/{id}/undo
///
/// Undo is scoped to the acting identity: a stack recorded by another user
/// is invisible here. History is in-memory only, so an undo after a server
/// restart fails closed with `EDIT_HISTORY_UNAVAILABLE` instead of guessing.
pub async fn undo_command(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CurrentToml>,
) -> AR<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    match super::service::undo(&user.id, &id, &body.toml_content) {
        Ok(super::service::HistoryOutcome::Applied(toml)) => {
            Ok(Json(serde_json::json!({"toml_content": toml})))
        }
        Ok(super::service::HistoryOutcome::Unavailable) => Err(history_closed(
            StatusCode::CONFLICT,
            EDIT_HISTORY_UNAVAILABLE,
            "No DAG edit history is available for this pipeline".into(),
            "Edit history is in-memory and is lost on server restart. Re-apply the change you want to reverse; the document you send is authoritative.",
        )),
        Ok(super::service::HistoryOutcome::Stale) => Err(history_closed(
            StatusCode::CONFLICT,
            EDIT_HISTORY_STALE,
            "The supplied toml_content is not the tip of the recorded edit history".into(),
            "Reload the pipeline (or re-send the current document) and retry; a stale document is never reverted blindly.",
        )),
        Ok(super::service::HistoryOutcome::Exhausted) => Err(err(
            StatusCode::NOT_FOUND,
            "NO_UNDO",
            "Nothing to undo".into(),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "UNDO_ERROR", e)),
    }
}

#[utoipa::path(
    post,
    path = "/api/pipeline/{id}/redo",
    tag = "dag",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
        (status = 404, description = "History exists but there is nothing to redo (NO_REDO)", body = ApiError),
        (status = 409, description = "Fail-closed: EDIT_HISTORY_UNAVAILABLE or EDIT_HISTORY_STALE (see undo)", body = ApiError),
    )
)]
/// POST /api/pipeline/{id}/redo
///
/// Same identity scope and in-memory lifetime contract as `undo`.
pub async fn redo_command(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CurrentToml>,
) -> AR<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    match super::service::redo(&user.id, &id, &body.toml_content) {
        Ok(super::service::HistoryOutcome::Applied(toml)) => {
            Ok(Json(serde_json::json!({"toml_content": toml})))
        }
        Ok(super::service::HistoryOutcome::Unavailable) => Err(history_closed(
            StatusCode::CONFLICT,
            EDIT_HISTORY_UNAVAILABLE,
            "No DAG edit history is available for this pipeline".into(),
            "Edit history is in-memory and is lost on server restart. Re-apply the change you want to restore; the document you send is authoritative.",
        )),
        Ok(super::service::HistoryOutcome::Stale) => Err(history_closed(
            StatusCode::CONFLICT,
            EDIT_HISTORY_STALE,
            "The supplied toml_content is not the tip of the recorded edit history".into(),
            "Reload the pipeline (or re-send the current document) and retry; a stale document is never re-applied blindly.",
        )),
        Ok(super::service::HistoryOutcome::Exhausted) => Err(err(
            StatusCode::NOT_FOUND,
            "NO_REDO",
            "Nothing to redo".into(),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "REDO_ERROR", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str =
        "[workflow]\nname = \"history\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\n";

    fn user(id: &str) -> Option<Extension<CurrentUser>> {
        Some(Extension(CurrentUser {
            id: id.into(),
            role: "user".into(),
        }))
    }

    fn edit(shell: &str) -> ApiJson<EditPayload> {
        ApiJson(EditPayload {
            toml_content: TOML.into(),
            cmd: crate::domains::dag::service::DagEditCommand {
                source: "dag_editor".into(),
                operation: "update_rule".into(),
                payload: serde_json::json!({"name": "s1", "patch": {"shell": shell}}),
            },
        })
    }

    async fn apply_edit(user_id: &str, pipeline_id: &str, shell: &str) -> String {
        match edit_command(user(user_id), Path(pipeline_id.into()), edit(shell)).await {
            Ok(Json(response)) => response.toml_content,
            Err(_) => panic!("edit must succeed"),
        }
    }

    #[tokio::test]
    async fn undo_is_scoped_to_the_acting_user() {
        let pid = "handler-owner-scope";
        let edited = apply_edit("handler-alice", pid, "echo alice").await;

        // Bob cannot see Alice's stack for the same pipeline id.
        let (status, Json(body)) = undo_command(
            user("handler-bob"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: edited.clone(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.code, EDIT_HISTORY_UNAVAILABLE);
        assert!(body.suggestion.is_some());

        // Alice's own undo still applies.
        let Ok(Json(value)) = undo_command(
            user("handler-alice"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: edited,
            }),
        )
        .await
        else {
            panic!("Alice must be able to undo her own edit");
        };
        assert_eq!(value["toml_content"], TOML);
    }

    #[tokio::test]
    async fn undo_after_restart_fails_closed_with_structured_code() {
        // A fresh (identity, pipeline) pair sees an empty stack — exactly
        // the post-restart state.
        let (status, Json(body)) = undo_command(
            user("handler-restart"),
            Path("handler-restart-pipeline".into()),
            ApiJson(CurrentToml {
                toml_content: TOML.into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.code, EDIT_HISTORY_UNAVAILABLE);
        assert!(
            body.suggestion
                .as_deref()
                .is_some_and(|s| s.contains("restart")),
            "the refusal must explain the in-memory lifetime"
        );
    }

    #[tokio::test]
    async fn redo_after_restart_fails_closed_with_structured_code() {
        let (status, Json(body)) = redo_command(
            user("handler-restart-redo"),
            Path("handler-restart-redo-pipeline".into()),
            ApiJson(CurrentToml {
                toml_content: TOML.into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.code, EDIT_HISTORY_UNAVAILABLE);
        assert!(body.suggestion.is_some());
    }

    #[tokio::test]
    async fn stale_undo_fails_closed_without_consuming_history() {
        let pid = "handler-stale-pipeline";
        let edited = apply_edit("handler-stale", pid, "echo stale").await;

        let (status, Json(body)) = undo_command(
            user("handler-stale"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: TOML.into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.code, EDIT_HISTORY_STALE);
        assert!(body.suggestion.is_some());

        // The refused undo changed nothing: the matching state still undoes.
        let Ok(Json(value)) = undo_command(
            user("handler-stale"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: edited,
            }),
        )
        .await
        else {
            panic!("a matched undo must still apply");
        };
        assert_eq!(value["toml_content"], TOML);
    }

    #[tokio::test]
    async fn undo_with_no_remaining_transitions_keeps_no_undo() {
        let pid = "handler-exhausted-pipeline";
        let edited = apply_edit("handler-exhausted", pid, "echo once").await;
        let first = undo_command(
            user("handler-exhausted"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: edited,
            }),
        )
        .await;
        assert!(first.is_ok(), "first undo applies");

        let (status, Json(body)) = undo_command(
            user("handler-exhausted"),
            Path(pid.into()),
            ApiJson(CurrentToml {
                toml_content: TOML.into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body.code, "NO_UNDO");
    }
}
