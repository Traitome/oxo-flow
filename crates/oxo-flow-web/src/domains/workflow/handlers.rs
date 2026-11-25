//! HTTP handlers for workflow domain.
//!
//! Thin adapters: parse HTTP request → call service → serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::extract::ApiJson;
use axum::{
    Extension, Json,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
};

use super::service;
use super::types::*;
use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::observability::types::*;
use crate::infra::db::models;

/// The pipeline visibility predicates live in [`super::visibility`] — the
/// single home for the WEB-13 / issue #1138 policy. They are re-exported
/// here because `domains/execution/` imports `can_read_pipeline` from this
/// module; the re-export is the SAME item, not a second definition.
pub(crate) use super::visibility::{can_read_pipeline, can_write_pipeline};

/// A pipeline response that also carries its optimistic-concurrency token
/// (issue #1136). The token is an HTTP `ETag`, not a JSON field, so the body
/// contract mirrored by `frontend/src/api/types.ts` stays unchanged.
type PipelineResult = Result<(HeaderMap, Json<Pipeline>), (StatusCode, Json<ApiError>)>;

/// Build the `ETag: "<revision>"` response header (a strong validator).
/// Revisions are positive integers, so the quoting has no escaping hazards.
fn pipeline_response(pipeline: Pipeline, revision: i64) -> (HeaderMap, Json<Pipeline>) {
    let mut headers = HeaderMap::new();
    if let Ok(value) = format!("\"{revision}\"").parse() {
        headers.insert(header::ETAG, value);
    }
    (headers, Json(pipeline))
}

/// Parse the optional `If-Match` optimistic-concurrency token from a
/// pipeline write request (issue #1136). The value is the `ETag` a previous
/// GET/PUT returned: a revision in the form `"7"` (quotes optional for
/// tolerance). A present but malformed guard is rejected instead of
/// silently dropped — dropping it would turn an intended conditional write
/// into an unconditional one. `*`, weak validators (`W/"7"`), and
/// multi-token lists are not supported and answer the same 400.
fn if_match_revision(headers: &HeaderMap) -> Result<Option<i64>, (StatusCode, Json<ApiError>)> {
    let Some(raw) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let invalid = || {
        err(
            StatusCode::BAD_REQUEST,
            "INVALID_IF_MATCH",
            "If-Match must carry a single strong pipeline revision, e.g. \"7\"".into(),
        )
    };
    let raw = raw.to_str().map_err(|_| invalid())?.trim();
    let raw = raw
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(raw);
    raw.parse::<i64>()
        .ok()
        .filter(|revision| *revision >= 1)
        .map(Some)
        .ok_or_else(invalid)
}

/// Turn a compare-and-swap outcome into the new revision or the structured
/// HTTP error (issue #1136): `409 PIPELINE_VERSION_CONFLICT` when the token
/// is stale, `404 NOT_FOUND` when the row vanished. One definition so the
/// update and rollback paths cannot drift apart.
fn applied_revision(
    id: &str,
    expected_revision: Option<i64>,
    outcome: crate::infra::db::PipelineWriteOutcome,
) -> Result<i64, (StatusCode, Json<ApiError>)> {
    use crate::infra::db::PipelineWriteOutcome;
    match outcome {
        PipelineWriteOutcome::Applied { revision } => Ok(revision),
        PipelineWriteOutcome::Conflict { current_revision } => Err((
            StatusCode::CONFLICT,
            Json(ApiError {
                code: "PIPELINE_VERSION_CONFLICT".into(),
                message: format!("Pipeline {id} was modified by another writer"),
                detail: Some(match expected_revision {
                    Some(expected) => format!(
                        "expected revision {expected} but the stored revision is {current_revision}"
                    ),
                    None => format!("the stored revision is {current_revision}"),
                }),
                suggestion: Some(format!(
                    "Re-read GET /api/pipelines/{id} for its current ETag and retry with If-Match: \"{current_revision}\"."
                )),
            }),
        )),
        PipelineWriteOutcome::NotFound => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

/// The one structured-error shape every JSON route returns:
/// `{code, message, detail, suggestion}`. `detail` and `suggestion` are
/// ALWAYS serialized — as `null` when the error has no extra context — so
/// the documented four-field contract holds on the common paths instead of
/// varying per endpoint. Both fields are nullable in the schema, so
/// generated clients already accept the null form.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
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

/// One owner for the shared "storage failure → 500 DB_ERROR" mapping: log
/// the context (what the handler was doing + the backend error), answer the
/// generic structured response, and never let either half drift per call
/// site. Callers pass a `format!`-built context so each log line keeps its
/// own interpolation.
pub fn db_err(context: impl std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    tracing::error!("{context}");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "DB_ERROR",
        "Internal database error".into(),
    )
}

/// The one `AI_NOT_CONFIGURED` contract shared by every AI surface: JSON
/// routes wrap it in `Json`, streaming routes serialize the same struct
/// into their `error` event, so clients can branch on `code` everywhere.
pub fn ai_not_configured_error() -> ApiError {
    ApiError {
        code: "AI_NOT_CONFIGURED".into(),
        message: "AI assistant is not configured".into(),
        detail: Some("no usable AI provider for this user".into()),
        suggestion: Some("Set OXO_FLOW_AI_PROVIDER and its API key, or ask your admin.".into()),
    }
}

/// Serialize a structured error into the payload of an SSE `error` event,
/// mirroring the JSON routes' `ApiError` shape.
pub fn error_event(error: &ApiError) -> axum::response::sse::Event {
    let data = serde_json::to_string(error).unwrap_or_else(|_| error.code.clone());
    axum::response::sse::Event::default()
        .event("error")
        .data(data)
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn get_backend()
-> Result<std::sync::Arc<dyn crate::infra::db::StorageBackend>, (StatusCode, Json<ApiError>)> {
    crate::infra::db::global_backend().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })
}

/// Top-20 template names feeding AI matching (translate, chat).
///
/// Single point of sync for the lookup: read through the storage backend
/// and degrade to an empty list rather than failing the AI call. The
/// names inherit `list_templates`' category/name ordering.
pub async fn top_template_names() -> Vec<String> {
    if let Some(backend) = crate::infra::db::global_backend() {
        backend
            .list_templates()
            .await
            .unwrap_or_default()
            .into_iter()
            .take(20)
            .map(|t| t.name)
            .collect()
    } else {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Pipeline lifecycle
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/pipelines/parse",
    tag = "workflow",
    request_body = ParseRequest,
    responses(
        (status = 200, description = "Success", body = ParseResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/parse
pub async fn parse_pipeline(ApiJson(req): ApiJson<ParseRequest>) -> ApiResult<ParseResponse> {
    service::parse_pipeline(&req.toml_content, req.format_version.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PARSE_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/validate",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = ValidateResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/validate
///
/// Accepts TOML content directly so the endpoint is self-contained.
pub async fn validate_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<ValidateResponse> {
    let user = resolve(authenticated.as_ref());
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
    // Missing-input existence checks resolve against the caller's base_dir
    // (issue #81 parity: the CLI checks against the workflow's directory).
    // Sandbox (#521): base_dir must be relative (no `..`) and resolves
    // inside the acting user's workspace — an arbitrary base_dir turned the
    // missing-input checks into a directory-existence oracle.
    let base_dir = match req.get("base_dir").and_then(|v| v.as_str()) {
        Some(raw) => Some(
            crate::workspace::resolve_user_scoped_path(&user.id, raw)
                .map_err(|e| err(StatusCode::BAD_REQUEST, "INVALID_BASE_DIR", e))?,
        ),
        None => None,
    };
    service::validate_pipeline(toml, base_dir.as_deref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "VALIDATE_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/prepare",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = PrepareResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/prepare
pub async fn prepare_pipeline(
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<PrepareResponse> {
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

#[utoipa::path(
    post,
    path = "/api/pipelines/dag",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = DagJsonResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/dag
pub async fn build_dag(ApiJson(req): ApiJson<serde_json::Value>) -> ApiResult<DagJsonResponse> {
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

#[utoipa::path(
    post,
    path = "/api/pipelines/format",
    tag = "workflow",
    request_body = ParseRequest,
    responses(
        (status = 200, description = "Success", body = FormatResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/format
pub async fn format_pipeline(ApiJson(req): ApiJson<ParseRequest>) -> ApiResult<FormatResponse> {
    service::format_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "FORMAT_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/lint",
    tag = "workflow",
    request_body = ParseRequest,
    responses(
        (status = 200, description = "Success", body = ValidateResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/lint
pub async fn lint_pipeline(ApiJson(req): ApiJson<ParseRequest>) -> ApiResult<ValidateResponse> {
    service::lint_workflow(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "LINT_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/stats",
    tag = "workflow",
    request_body = ParseRequest,
    responses(
        (status = 200, description = "Success", body = WorkflowStatsResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/stats
pub async fn pipeline_stats(
    ApiJson(req): ApiJson<ParseRequest>,
) -> ApiResult<WorkflowStatsResponse> {
    service::workflow_stats(&req.toml_content)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "STATS_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/diff",
    tag = "workflow",
    request_body = DiffRequest,
    responses(
        (status = 200, description = "Success", body = DiffResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/diff
///
/// Accepts inline TOML (`toml_a`/`toml_b`) or saved-pipeline ids
/// (`pipeline_a_id`/`pipeline_b_id`), resolving ids server-side — the
/// contract the client always assumed (issue #82 P1-10).
pub async fn diff_pipelines(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<DiffRequest>,
) -> ApiResult<DiffResponse> {
    let user = resolve(authenticated.as_ref());
    let resolve_side = async |inline: &Option<String>,
                              id: &Option<String>|
           -> Result<String, (StatusCode, Json<ApiError>)> {
        if let Some(toml) = inline
            && !toml.trim().is_empty()
        {
            return Ok(toml.clone());
        }
        if let Some(pid) = id {
            let backend = get_backend()?;
            let row = backend
                .get_pipeline(pid)
                .await
                .map_err(|e| db_err(format!("DB error loading pipeline {pid}: {e}")))?;
            // A diff of another user's private pipeline leaks its TOML —
            // apply the same read scope as every other pipeline endpoint.
            let Some(row) = row.filter(|r| can_read_pipeline(&user, r)) else {
                return Err(err(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("Pipeline {pid} not found"),
                ));
            };
            return Ok(row.toml_content);
        }
        Err(err(
            StatusCode::BAD_REQUEST,
            "MISSING",
            "Provide toml_a/toml_b or pipeline_a_id/pipeline_b_id".into(),
        ))
    };
    let toml_a = resolve_side(&req.toml_a, &req.pipeline_a_id).await?;
    let toml_b = resolve_side(&req.toml_b, &req.pipeline_b_id).await?;
    service::diff_workflows(&toml_a, &toml_b)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DIFF_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/export",
    tag = "workflow",
    request_body = ExportRequest,
    responses(
        (status = 200, description = "Success", body = ExportResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/export
///
/// Prefers inline `toml_content`; falls back to loading the saved pipeline
/// when only `pipeline_id` is given — scoped by read permission.
pub async fn export_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<ExportRequest>,
) -> ApiResult<ExportResponse> {
    let user = resolve(authenticated.as_ref());
    let toml_content = if !req.toml_content.trim().is_empty() {
        req.toml_content
    } else if let Some(id) = req.pipeline_id.as_deref() {
        let backend = get_backend()?;
        let row = backend
            .get_pipeline(id)
            .await
            .map_err(|e| db_err(format!("DB error loading pipeline {id}: {e}")))?;
        match row.filter(|r| can_read_pipeline(&user, r)) {
            Some(row) => row.toml_content,
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

#[utoipa::path(
    post,
    path = "/api/pipelines/search",
    tag = "workflow",
    request_body = SearchRequest,
    responses(
        (status = 200, description = "Success", body = SearchResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines/search
pub async fn search_pipelines(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<SearchRequest>,
) -> ApiResult<SearchResponse> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    // Search saved pipelines from DB. The visibility predicate rides in the
    // SQL as a candidate window (audit #664): filtering AFTER `LIMIT 50` let
    // foreign private rows crowd the caller's own results out of the window
    // entirely. `can_read_pipeline` (WEB-13 / issue #1138) is the
    // authoritative filter and is applied here on every returned row.
    let pipeline_rows = backend
        .search_pipelines(&req.query, Some(&user.id), user.is_admin(), 50)
        .await
        .map_err(|e| db_err(format!("DB error searching pipelines: {e}")))?;

    let pipelines: Vec<Pipeline> = pipeline_rows
        .into_iter()
        .filter(|r| can_read_pipeline(&user, r))
        .map(Pipeline::from)
        .collect();

    // Search templates from DB via backend
    let query_lower = req.query.to_lowercase();
    let template_rows: Vec<models::TemplateRow> = backend
        .list_templates()
        .await
        .map_err(|e| db_err(format!("DB error searching templates: {e}")))?
        .into_iter()
        .filter(|r| {
            r.name.to_lowercase().contains(&query_lower)
                || r.description.to_lowercase().contains(&query_lower)
                || r.category.to_lowercase().contains(&query_lower)
        })
        .take(20)
        .collect();

    let templates: Vec<Template> = template_rows.into_iter().map(Template::from).collect();

    Ok(Json(service::search_pipelines(
        &req.query, &pipelines, &templates,
    )))
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/pipelines",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = Pipeline),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/pipelines — create a new pipeline from TOML.
///
/// The response carries `ETag: "1"` — the optimistic-concurrency token for
/// the new pipeline (issue #1136).
pub async fn save_pipeline(
    authenticated: Option<axum::Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> PipelineResult {
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

    // One parse drives both derived columns: the rule count and the version.
    // TOML is the truth — an explicit `version` JSON field is no longer
    // honored (issue #907 P2-5): a stored version that disagrees with the
    // content silently poisons fork/import snapshots and provenance.
    let parsed = oxo_flow_core::WorkflowConfig::parse(toml_content).map_err(|e| {
        err(
            StatusCode::BAD_REQUEST,
            "PARSE_ERROR",
            format!("toml_content is not a valid workflow: {e}"),
        )
    })?;
    let rules_count = parsed.rules.len() as i64;
    let version = parsed.workflow.version;

    let now = now_iso();
    let id = uuid::Uuid::new_v4().to_string();

    let pipeline_row = models::PipelineRow {
        id: id.clone(),
        user_id: user_id.clone(),
        name: name.to_string(),
        version: version.clone(),
        toml_content: toml_content.to_string(),
        rules_count,
        forked_from: None,
        visibility: visibility.to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
        // The initial save is revision 1 of the pipeline's history (issue
        // #1136); the row and its first snapshot commit in one transaction,
        // so a create can never report success without its history row.
        revision: 1,
    };
    let backend = get_backend()?;
    backend
        .create_pipeline_with_revision(&pipeline_row)
        .await
        .map_err(|e| db_err(format!("DB error creating pipeline: {e}")))?;

    Ok(pipeline_response(
        Pipeline {
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
        },
        1,
    ))
}

#[utoipa::path(
    get,
    path = "/api/pipelines",
    tag = "workflow",
    security(("bearerAuth" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<Pipeline>),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/pipelines
pub async fn list_pipelines(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<Vec<Pipeline>> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    // The storage query is a candidate window: it scopes by owner/shared
    // visibility BEFORE `LIMIT` so foreign private rows cannot crowd the
    // caller's own results out of the page (audit #664). The authoritative
    // filter is `can_read_pipeline` (WEB-13 / issue #1138) — every list path
    // filters with the SAME predicate the read paths 404 on.
    let rows = backend
        .list_pipelines_visible(&user.id, user.is_admin(), 100)
        .await
        .map_err(|e| db_err(format!("DB error listing pipelines: {e}")))?;

    let list: Vec<Pipeline> = rows
        .into_iter()
        .filter(|row| can_read_pipeline(&user, row))
        .map(Pipeline::from)
        .collect();

    Ok(Json(list))
}

#[utoipa::path(
    get,
    path = "/api/pipelines/{id}",
    tag = "workflow",
    responses(
        (status = 200, description = "Success", body = Pipeline),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// GET /api/pipelines/{id}
///
/// The response carries `ETag: "<revision>"` (issue #1136); send that value
/// back in `If-Match` on PUT/rollback to make the write conditional.
pub async fn get_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> PipelineResult {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    let row = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id}: {e}")))?;

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
        Some(r) => {
            let revision = r.revision;
            Ok(pipeline_response(Pipeline::from(r), revision))
        }
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Pipeline {id} not found"),
        )),
    }
}

#[utoipa::path(
    put,
    path = "/api/pipelines/{id}",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success (ETag carries the new revision)", body = Pipeline),
        (status = 400, description = "Malformed If-Match (INVALID_IF_MATCH)", body = ApiError),
        (status = 404, description = "Error", body = ApiError),
        (status = 409, description = "Stale If-Match (PIPELINE_VERSION_CONFLICT)", body = ApiError),
    )
)]
/// PUT /api/pipelines/{id}
///
/// Optimistic concurrency (issue #1136): send the `ETag` from a previous
/// GET/PUT back in `If-Match` to make the write conditional. A stale token
/// answers `409 PIPELINE_VERSION_CONFLICT` and nothing is written. Omitting
/// the header keeps the historical unconditional update (backward
/// compatibility); the stored revision still advances, so a later
/// conditional write can detect it. The response's `ETag` carries the new
/// revision. The pre-update snapshot and the row update commit in one
/// storage transaction — success always means both landed.
pub async fn update_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> PipelineResult {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;
    let expected_revision = if_match_revision(&headers)?;

    let existing = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id} for update: {e}")))?;

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

    // A name-only rename keeps the stored rule count; supplied content must
    // parse (same contract as POST /api/pipelines) or the stale count would
    // mask an unrunnable definition. Content-bearing updates re-derive the
    // version from the new TOML so the stored column tracks `[workflow]`
    // version (issue #907 P2-5); renames keep the existing version.
    let parsed = match req.get("toml_content").and_then(|v| v.as_str()) {
        Some(_) => Some(
            oxo_flow_core::WorkflowConfig::parse(&toml_content).map_err(|e| {
                err(
                    StatusCode::BAD_REQUEST,
                    "PARSE_ERROR",
                    format!("toml_content is not a valid workflow: {e}"),
                )
            })?,
        ),
        None => None,
    };
    let rules_count = match &parsed {
        Some(wf) => wf.rules.len() as i64,
        None => existing.rules_count,
    };
    let version = match &parsed {
        Some(wf) => wf.workflow.version.clone(),
        None => existing.version.clone(),
    };

    let now = now_iso();
    let updated_pipeline = models::PipelineRow {
        id: id.clone(),
        user_id: existing.user_id.clone(),
        name: name.clone(),
        version: version.clone(),
        toml_content: toml_content.clone(),
        rules_count,
        forked_from: existing.forked_from.clone(),
        visibility: visibility.clone(),
        created_at: existing.created_at.clone(),
        updated_at: now.clone(),
        // The stored revision, not a caller value: the storage layer
        // increments it under the compare-and-swap guard.
        revision: existing.revision,
    };
    // The pre-update snapshot rides in the SAME transaction as the row
    // write (issue #1136). If either fails — including the snapshot insert —
    // the request fails and neither is visible.
    let snapshot = models::RevisionSnapshot {
        user_id: &user.id,
        version: &existing.version,
        toml_content: &existing.toml_content,
    };
    let outcome = backend
        .update_pipeline_cas(&updated_pipeline, expected_revision, Some(&snapshot))
        .await
        .map_err(|e| db_err(format!("DB error updating pipeline {id}: {e}")))?;
    let revision = applied_revision(&id, expected_revision, outcome)?;

    Ok(pipeline_response(
        Pipeline {
            id,
            user_id: existing.user_id,
            name,
            version,
            toml_content,
            rules_count: rules_count as usize,
            forked_from: existing.forked_from,
            visibility,
            created_at: existing.created_at,
            updated_at: now,
        },
        revision,
    ))
}

#[utoipa::path(
    delete,
    path = "/api/pipelines/{id}",
    tag = "workflow",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// DELETE /api/pipelines/{id}
pub async fn delete_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    let existing = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id} for deletion: {e}")))?;

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

    backend
        .delete_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error deleting pipeline {id}: {e}")))?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Single conversion from the storage row to the API pipeline type — the
/// search, list, and get handlers all surface rows, so they share this
/// mapping (reuse, never retype) instead of three hand-copied field lists
/// that drift when the row gains a column (issue #1136 added `revision`).
impl From<models::PipelineRow> for Pipeline {
    fn from(r: models::PipelineRow) -> Self {
        Pipeline {
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
        }
    }
}

/// Single conversion from the storage row to the API template type —
/// every handler that surfaces `templates` rows shares this mapping
/// (reuse, never retype).
impl From<models::TemplateRow> for Template {
    fn from(r: models::TemplateRow) -> Self {
        Template {
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
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/templates",
    tag = "templates",
    responses(
        (status = 200, description = "Success", body = Vec<Template>),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/templates
pub async fn list_templates() -> ApiResult<Vec<Template>> {
    let backend = get_backend()?;

    let rows = backend
        .list_templates()
        .await
        .map_err(|e| db_err(format!("DB error listing templates: {e}")))?;

    let list: Vec<Template> = rows.into_iter().map(Template::from).collect();

    Ok(Json(list))
}

#[utoipa::path(
    get,
    path = "/api/templates/{id}",
    tag = "templates",
    responses(
        (status = 200, description = "Success", body = Template),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// GET /api/templates/{id}
///
/// Accepts the template UUID OR its name (`?template=` supports both —
/// issue #81: the editor only understood UUIDs before).
pub async fn get_template(Path(id): Path<String>) -> ApiResult<Template> {
    let backend = get_backend()?;

    let row = backend
        .get_template(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching template {id}: {e}")))?;

    match row {
        Some(r) => Ok(Json(Template::from(r))),
        None => Err(err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Template {id} not found"),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/api/templates",
    tag = "templates",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = Template),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/templates
pub async fn save_template(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<Template> {
    let user = resolve(authenticated.as_ref());

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

    // Updating an existing template requires owner-or-admin (#519) — the
    // upsert used to let any user rewrite anyone's template (suggested to
    // every user via LIMIT 20, so planted content was widely visible).
    // Updating a system template is an admin operation (issue #81).
    let backend = get_backend()?;
    if !template_id.is_empty() {
        let existing = backend.get_template(&id).await.unwrap_or(None);
        let Some(existing) = existing else {
            return Err(err(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                format!("Template {id} not found"),
            ));
        };
        if existing.is_system == 1 && !user.is_admin() {
            return Err(err(
                StatusCode::FORBIDDEN,
                "ACCESS_DENIED",
                "Only admins may modify system templates".into(),
            ));
        }
        if existing.created_by.as_deref() != Some(&user.id) && !user.is_admin() {
            return Err(err(
                StatusCode::FORBIDDEN,
                "ACCESS_DENIED",
                "Only the template owner or an admin may modify it".into(),
            ));
        }
    }

    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());

    let template_row = models::TemplateRow {
        id: id.clone(),
        name: name.to_string(),
        category: category.to_string(),
        description: description.to_string(),
        tags: tags_json,
        toml_content: toml_content.to_string(),
        is_system: if is_system { 1 } else { 0 },
        created_by: Some(user.id.clone()),
        usage_count: 0,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    backend
        .save_template(&template_row)
        .await
        .map_err(|e| db_err(format!("DB error saving template: {e}")))?;

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

#[utoipa::path(
    delete,
    path = "/api/templates/{id}",
    tag = "templates",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// DELETE /api/templates/{id}
pub async fn delete_template(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    let existing = backend
        .get_template(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching template {id} for deletion: {e}")))?;

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

    backend
        .delete_template(&id)
        .await
        .map_err(|e| db_err(format!("DB error deleting template {id}: {e}")))?;

    Ok(Json(serde_json::json!({"deleted": id})))
}

// ---------------------------------------------------------------------------
// Data discovery
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/data/analyze",
    tag = "data",
    request_body = DataAnalysisRequest,
    responses(
        (status = 200, description = "Success", body = DataAnalysisResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/data/analyze
///
/// Paths are sandboxed to the acting user's workspace (#521) — the size
/// probe previously stat-ed any client-supplied path on the server.
pub async fn analyze_data(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<DataAnalysisRequest>,
) -> ApiResult<DataAnalysisResponse> {
    let user = resolve(authenticated.as_ref());
    let scope = crate::workspace::user_root(&user.id)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DATA_ERROR", e.to_string()))?;
    super::data::analyze_files(&req.paths, req.max_depth, Some(&scope))
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DATA_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/data/reference",
    tag = "data",
    request_body = ReferenceRequest,
    responses(
        (status = 200, description = "Success", body = ReferenceResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/data/reference
pub async fn discover_reference(
    ApiJson(req): ApiJson<ReferenceRequest>,
) -> ApiResult<ReferenceResponse> {
    super::data::discover_reference(&req.genome, &req.components)
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "REF_ERROR", e))
}

// ---------------------------------------------------------------------------
// Plugin validation
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/plugins/validate",
    tag = "workflow",
    request_body = ValidatePluginRequest,
    responses(
        (status = 200, description = "Success", body = ValidatePluginResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/plugins/validate
pub async fn validate_plugin(
    ApiJson(req): ApiJson<ValidatePluginRequest>,
) -> ApiResult<ValidatePluginResponse> {
    service::validate_plugin_manifest(&req.manifest, req.trusted_keys.as_ref())
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "PLUGIN_ERROR", e))
}

// ---------------------------------------------------------------------------
// Data Perception API (v0.8 AI Companion)
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/api/data/perceive",
    tag = "data",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/data/perceive
pub async fn perceive_data(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    use crate::domains::ai::agents::data_agent;

    let user = resolve(authenticated.as_ref());
    let scope = crate::workspace::user_root(&user.id)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "DATA_ERROR", e.to_string()))?;

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
            data_agent::analyze_paths(p, &scope)
        } else if let Some(ref desc) = description {
            data_agent::analyze_description(desc)
        } else {
            data_agent::analyze_paths(&[], &scope)
        }
    } else if let Some(ref desc) = description {
        data_agent::analyze_description(desc)
    } else {
        data_agent::analyze_paths(&[], &scope)
    };

    Ok(Json(serde_json::json!(report)))
}

#[utoipa::path(
    get,
    path = "/api/data/reference/status",
    tag = "data",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/data/reference/status
///
/// Deterministic filesystem probe of the standard `/data/references/`
/// layout for a few common genome builds (issue #907 P1-4). A genome only
/// counts as `installed` when at least one expected component actually
/// exists — a build with zero files on disk is reported under `missing`
/// genomes, never as installed-and-Complete.
///
/// `download_commands` carries *guidance*, not fetchable URLs: oxo-flow
/// ships no reference mirror, so each entry states where to stage the files
/// (or to use a workflow's `reference_dir` + `[[references]]` auto-build).
/// The per-genome `found`/`components` lists what exists on disk (absolute
/// paths), so clients can render partial state honestly; `missing_components`
/// carries the probed-but-absent paths.
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
        // mm39 deliberately probes only genome.fa + star: the auto-build
        // ([[references]]) does not need a gene annotation for index
        // construction. Keep docs/guide/src/reference/web-api.md in sync.
        (
            "mm39",
            vec![
                "/data/references/mm39/genome.fa",
                "/data/references/mm39/star",
            ],
        ),
    ];

    let mut installed = Vec::new();
    let mut missing_genomes = Vec::new();
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

        if !found.is_empty() {
            installed.push(serde_json::json!({
                "genome": genome,
                // Legacy alias of `found` kept for API stability — frontend
                // Settings.tsx renders `components`, while its badge logic
                // reads `missing_components`. Always identical to `found`;
                // do not diverge them.
                "components": found.iter().map(String::as_str).collect::<Vec<_>>(),
                "found": found,
                "missing_components": not_found,
            }));
        } else {
            missing_genomes.push(genome);
        }
        for m in &not_found {
            missing.push(m.clone());
        }
        if !not_found.is_empty() {
            // No fetchable URL exists — oxo-flow does not host or mirror
            // reference data. Point operators at the staging layout and the
            // workflow-level auto-build instead of a placeholder host. Name
            // only the components this genome actually probes (mm39 has no
            // genes.gtf entry).
            let expected: Vec<String> = not_found
                .iter()
                .map(|p| {
                    let base = std::path::Path::new(p)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(p);
                    if base == "star" {
                        "STAR index".to_string()
                    } else {
                        base.to_string()
                    }
                })
                .collect();
            download_commands.push(format!(
                "# {genome}: stage the expected files under /data/references/{genome} \
                 ({}) — or set reference_dir in a workflow and let [[references]] \
                 auto-build the indexes",
                expected.join(", ")
            ));
        }
    }

    Ok(Json(serde_json::json!({
        "installed": installed,
        "missing_genomes": missing_genomes,
        "missing": missing,
        "download_commands": download_commands,
    })))
}

#[utoipa::path(
    post,
    path = "/api/data/samplesheet/parse",
    tag = "data",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/data/samplesheet/parse
pub async fn parse_samplesheet(
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
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

#[utoipa::path(
    get,
    path = "/api/pipelines/{id}/revisions",
    tag = "workflow",
    responses(
        (status = 200, description = "Success", body = Vec<serde_json::Value>),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// GET /api/pipelines/{id}/revisions — snapshot list, newest first.
pub async fn list_revisions(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> ApiResult<Vec<serde_json::Value>> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    let pipeline = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id}: {e}")))?;
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

    let rows = backend
        .list_pipeline_revisions(&id, 50)
        .await
        .map_err(|e| db_err(format!("DB error listing revisions: {e}")))?;

    let revisions: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "version": r.version,
                "actor": r.user_id,
                "created_at": r.created_at,
            })
        })
        .collect();
    Ok(Json(revisions))
}

#[utoipa::path(
    get,
    path = "/api/pipelines/{id}/revisions/{rev}",
    tag = "workflow",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 404, description = "Error", body = ApiError),
    )
)]
/// GET /api/pipelines/{id}/revisions/{rev} — one snapshot's full TOML.
pub async fn get_revision(
    authenticated: Option<Extension<CurrentUser>>,
    Path((id, rev)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;

    let pipeline = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id}: {e}")))?;
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

    let row = backend
        .get_pipeline_revision(&id, &rev)
        .await
        .map_err(|e| db_err(format!("DB error fetching revision: {e}")))?;
    let r = row.ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("Revision {rev} not found"),
        )
    })?;
    Ok(Json(serde_json::json!({
        "id": r.id,
        "pipeline_id": id,
        "version": r.version,
        "actor": r.user_id,
        "toml_content": r.toml_content,
    })))
}

#[utoipa::path(
    post,
    path = "/api/pipelines/{id}/rollback",
    tag = "workflow",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Success (ETag carries the new revision)", body = Pipeline),
        (status = 400, description = "Unparsable revision / malformed If-Match", body = ApiError),
        (status = 404, description = "Error", body = ApiError),
        (status = 409, description = "Stale If-Match (PIPELINE_VERSION_CONFLICT)", body = ApiError),
    )
)]
/// POST /api/pipelines/{id}/rollback — restore a revision as the current
/// pipeline (creating a new revision, so nothing is lost). The restored
/// revision must parse; its version and rules_count are derived from the
/// revision TOML itself (TOML-is-truth, #907 P2-5).
///
/// Rollback is a pipeline write, so it shares the update endpoint's
/// optimistic-concurrency contract (issue #1136): an optional `If-Match`
/// guards it with the same `409 PIPELINE_VERSION_CONFLICT` answer and the
/// response carries the new `ETag`, and the pre-rollback snapshot commits
/// in the same transaction as the restore.
pub async fn rollback_pipeline(
    authenticated: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> PipelineResult {
    let user = resolve(authenticated.as_ref());
    let backend = get_backend()?;
    let expected_revision = if_match_revision(&headers)?;

    let pipeline = backend
        .get_pipeline(&id)
        .await
        .map_err(|e| db_err(format!("DB error fetching pipeline {id}: {e}")))?;
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
    let rev_row = backend
        .get_pipeline_revision(&id, revision_id)
        .await
        .map_err(|e| db_err(format!("DB error fetching revision: {e}")))?;
    let toml_content = rev_row.map(|r| r.toml_content).ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Revision not found".into(),
        )
    })?;

    // The restored content must parse — the same contract as save/update. A
    // stale rules_count kept from the current pipeline would mask an
    // unrunnable revision, and the snapshot below must not be written for a
    // rollback that is about to be rejected. rules_count AND version are both
    // derived from the revision's TOML (TOML-is-truth, #907 P2-5): writing the
    // old TOML back under the pre-rollback version would recreate exactly the
    // divergence P2-5 removed.
    let parsed = oxo_flow_core::WorkflowConfig::parse(&toml_content).map_err(|e| {
        err(
            StatusCode::BAD_REQUEST,
            "PARSE_ERROR",
            format!("revision {revision_id} is not a valid workflow: {e}"),
        )
    })?;
    let rules_count = parsed.rules.len() as i64;
    let version = parsed.workflow.version.clone();

    // Restore = snapshot current + write the old content back, in one
    // transaction (issue #1136).
    let now = now_iso();
    let updated = models::PipelineRow {
        id: id.clone(),
        user_id: pipeline.user_id.clone(),
        name: pipeline.name.clone(),
        version: version.clone(),
        toml_content: toml_content.clone(),
        rules_count,
        forked_from: pipeline.forked_from.clone(),
        visibility: pipeline.visibility.clone(),
        created_at: pipeline.created_at.clone(),
        updated_at: now.clone(),
        revision: pipeline.revision,
    };
    let snapshot = models::RevisionSnapshot {
        user_id: &user.id,
        version: &pipeline.version,
        toml_content: &pipeline.toml_content,
    };
    let outcome = backend
        .update_pipeline_cas(&updated, expected_revision, Some(&snapshot))
        .await
        .map_err(|e| db_err(format!("DB error rolling back pipeline {id}: {e}")))?;
    let revision = applied_revision(&id, expected_revision, outcome)?;

    Ok(pipeline_response(
        Pipeline {
            id,
            user_id: pipeline.user_id,
            name: pipeline.name,
            version,
            toml_content,
            rules_count: rules_count as usize,
            forked_from: pipeline.forked_from,
            visibility: pipeline.visibility,
            created_at: pipeline.created_at,
            updated_at: now,
        },
        revision,
    ))
}

#[cfg(test)]
mod error_shape_tests {
    use super::*;

    /// Report finding P-F5: the documented contract is
    /// `{code, message, detail, suggestion}`. The shared constructor must
    /// emit all four keys on every path — absent context is `null`, never a
    /// missing field — so clients can parse the shape unconditionally.
    #[test]
    fn err_serializes_all_four_contract_fields() {
        let (status, Json(error)) = err(StatusCode::NOT_FOUND, "NOT_FOUND", "Run missing".into());
        assert_eq!(status, StatusCode::NOT_FOUND);
        let value = serde_json::to_value(&error).expect("ApiError serializes");
        assert_eq!(value["code"], "NOT_FOUND");
        assert_eq!(value["message"], "Run missing");
        assert!(value["detail"].is_null(), "detail must be null: {value}");
        assert!(
            value["suggestion"].is_null(),
            "suggestion must be null: {value}"
        );
        assert_eq!(
            value.as_object().map(|o| o.len()),
            Some(4),
            "exactly the four contract fields: {value}"
        );
    }

    /// Populated hints keep their string types (the AI surface relies on
    /// both fields being actionable advice, not just present).
    #[test]
    fn populated_hints_serialize_as_strings() {
        let error = ai_not_configured_error();
        let value = serde_json::to_value(&error).expect("ApiError serializes");
        assert!(value["detail"].is_string());
        assert!(value["suggestion"].is_string());
    }
}
