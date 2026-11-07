//! HTTP handlers for AI domain.
//!
//! Thin adapters: parse HTTP request -> call service -> serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use crate::extract::ApiJson;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Extension, Json, http::StatusCode};

use crate::domains::ai::types::*;
use crate::domains::auth::current_user;
use crate::domains::auth::current_user::{CurrentUser, resolve};
use crate::domains::execution::types::DiagnosticsResponse;
use crate::domains::workflow::handlers::{
    ApiError, ai_not_configured_error, can_read_pipeline, err, error_event, get_backend,
};
use crate::infra::db::{global_backend, models};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// Shared-provider writes are admin-only outside personal mode (issue #82
/// P0-5): reconfiguring the runtime AI provider affects every user's AI
/// calls and their cost.
fn require_ai_admin(user: &CurrentUser) -> Result<(), (StatusCode, Json<ApiError>)> {
    current_user::require_role_outside_personal(
        user,
        "admin",
        "Admin role required to change the server AI provider",
    )
}

/// The one `TranslateError` → HTTP contract shared by the JSON and SSE
/// translate routes: JSON routes pair the `ApiError` with its status code,
/// the SSE route serializes the same `ApiError` into its terminal `error`
/// event so clients can branch on `code` on both surfaces.
fn translate_error_parts(error: &super::service::TranslateError) -> (StatusCode, ApiError) {
    match error {
        super::service::TranslateError::Unavailable => {
            (StatusCode::SERVICE_UNAVAILABLE, ai_not_configured_error())
        }
        super::service::TranslateError::Failed(message) => (
            StatusCode::BAD_REQUEST,
            ApiError {
                code: "AI_TRANSLATE_ERROR".into(),
                message: message.clone(),
                detail: None,
                suggestion: None,
            },
        ),
    }
}

/// One owner for every AI-provider storage failure so logs and HTTP errors
/// cannot drift between config endpoints.
fn ai_db_error(operation: &'static str, error: String) -> (StatusCode, Json<ApiError>) {
    tracing::error!("DB error {operation}: {error}");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "DB_ERROR",
        "Internal database error".to_string(),
    )
}

fn runtime_optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn runtime_api_key(stored: Option<&str>) -> Option<String> {
    runtime_optional(&crate::infra::crypto::open(stored.unwrap_or_default()))
}

/// Fail fast with the same structured `AI_NOT_CONFIGURED` chat and translate
/// answer when the acting user resolves to no usable provider — an explicit
/// `OXO_FLOW_AI_PROVIDER=disabled` pin, a disabled settings row, or an
/// unconfigured instance. No outbound HTTP is attempted in that state.
fn require_usable_provider(
    provider: &crate::ai_provider::AiProvider,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if provider.is_usable() {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ai_not_configured_error()),
        ))
    }
}

/// The structured refusal for the runtime-only AI configuration endpoint
/// (`POST /api/ai/config`) while `OXO_FLOW_AI_PROVIDER=disabled` pins the
/// process off.
///
/// That endpoint has no durable tier — applying to the live registry is its
/// only effect — so under the pin there is nothing meaningful to do but
/// refuse. The persisted PUT server/user tiers instead save normally and
/// suppress only activation (`applied_to_runtime: false`); see their
/// handlers. `pinned_disabled` is injected so the contract is unit-testable
/// without process-global env mutation (mirrors `provider_candidates_for`
/// in the AI service).
fn reject_config_update_when_pinned(
    pinned_disabled: bool,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if !pinned_disabled {
        return Ok(());
    }
    Err((
        StatusCode::CONFLICT,
        Json(ApiError {
            code: "AI_DISABLED_BY_ENV".into(),
            message: "AI is disabled by OXO_FLOW_AI_PROVIDER=disabled".into(),
            detail: Some(
                "The server was started with the explicit AI off switch; configuration \
                 updates cannot take effect while it is set."
                    .into(),
            ),
            suggestion: Some(
                "Unset OXO_FLOW_AI_PROVIDER (or set it to a provider) and restart the \
                 server, then save the AI configuration."
                    .into(),
            ),
        }),
    ))
}

/// The honest `/api/ai/test` answer while pinned: no provider is resolved
/// and no HTTP client is constructed, so the probe cannot dial out. The
/// shape matches the endpoint's ordinary failure response (200,
/// `success: false`) and names the same state `/api/health` reports
/// (`disabled`).
fn pinned_test_response(pinned_disabled: bool) -> Option<AiTestResponse> {
    pinned_disabled.then(|| AiTestResponse {
        success: false,
        message: "AI is disabled by OXO_FLOW_AI_PROVIDER=disabled — no connection attempted".into(),
        provider: "disabled".into(),
        model: None,
    })
}

#[utoipa::path(
    post,
    path = "/api/ai/translate",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = TranslateResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/translate — standard JSON response.
pub async fn translate(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<TranslateRequest>,
) -> ApiResult<TranslateResponse> {
    let user = resolve(authenticated.as_ref());
    // Boundary checks before any I/O: an empty intent would otherwise come
    // back as a fully hallucinated pipeline.
    if req.intent.trim().is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "INVALID_INTENT",
            "Intent must not be empty".into(),
        ));
    }

    let provider = crate::ai_provider::provider_for(&user.id).await;
    let templates = super::super::workflow::handlers::top_template_names().await;

    super::service::translate_intent(&provider, &user.id, &req.intent, None, &templates)
        .await
        .map(Json)
        .map_err(|e| {
            let (status, error) = translate_error_parts(&e);
            (status, Json(error))
        })
}

#[utoipa::path(
    post,
    path = "/api/ai/translate/stream",
    tag = "ai",
    responses(
        (status = 200, description = "Success", content_type = "text/event-stream"),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/translate?stream=true — SSE streaming response.
///
/// Streams progress events: intent → data → match → generate → validate → done.
/// Each event has `type` and `data` fields. The final event contains the full
/// TranslateResponse as JSON.
pub async fn translate_sse(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<TranslateRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use std::convert::Infallible;

    let user = resolve(authenticated.as_ref());
    // Boundary check before any I/O: an empty intent must fail cheaply
    // instead of paying for a hallucinated pipeline.
    let invalid_intent = req.intent.trim().is_empty();

    let provider = if invalid_intent {
        None
    } else {
        Some(crate::ai_provider::provider_for(&user.id).await)
    };

    let templates = if invalid_intent {
        vec![]
    } else {
        super::super::workflow::handlers::top_template_names().await
    };

    let intent = req.intent.clone();
    let stream = async_stream::stream! {
        if invalid_intent {
            yield Ok::<_, Infallible>(error_event(&err(
                StatusCode::BAD_REQUEST,
                "INVALID_INTENT",
                "Intent must not be empty".into(),
            ).1));
            return;
        }
        let provider = provider.expect("provider resolved for non-empty intent");

        // Step 1: intent received
        yield Ok::<_, Infallible>(Event::default()
            .event("progress")
            .data(serde_json::json!({"step": "intent", "message": "Intent received", "intent": &intent}).to_string()));

        // Step 2: matching templates
        yield Ok::<_, Infallible>(Event::default()
            .event("progress")
            .data(serde_json::json!({"step": "match", "message": "Matching templates...", "templates_count": templates.len()}).to_string()));

        // Step 3: AI generation (with fallback chain)
        yield Ok::<_, Infallible>(Event::default()
            .event("progress")
            .data(serde_json::json!({"step": "generate", "message": "Generating pipeline via AI..."}).to_string()));

        let result =
            super::service::translate_intent(&provider, &user.id, &intent, None, &templates).await;

        // Step 4: validate + done. Errors share the JSON routes' shape and
        // always terminate with `done`, so an SSE client can branch on
        // `code` and never hangs waiting for a terminal event.
        match result {
            Ok(response) => {
                let json = serde_json::to_string(&response).unwrap_or_default();
                yield Ok::<_, Infallible>(Event::default()
                    .event("progress")
                    .data(serde_json::json!({"step": "validate", "message": "Pipeline validated", "pipeline_id": &response.pipeline_id}).to_string()));
                yield Ok::<_, Infallible>(Event::default()
                    .event("done")
                    .data(json));
            }
            Err(e) => {
                let (_, error) = translate_error_parts(&e);
                yield Ok::<_, Infallible>(error_event(&error));
                yield Ok::<_, Infallible>(Event::default().event("done").data("{}"));
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

#[utoipa::path(
    post,
    path = "/api/ai/explain",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = ExplainResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/explain
///
/// Looks up the run from the database to get real diagnostics data.
pub async fn explain(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<ExplainRequest>,
) -> ApiResult<ExplainResponse> {
    let user = resolve(authenticated.as_ref());
    let provider = crate::ai_provider::provider_for(&user.id).await;

    // Try to look up run diagnostics from DB. Ownership is enforced exactly
    // like the execution handlers (#515): foreign and unknown runs both 404
    // — the run's execution log must not be readable through the AI surface.
    let (diagnostics, log_output) = if let Some(backend) = crate::infra::db::global_backend() {
        let run = crate::domains::execution::handlers::load_owned_run(
            backend.as_ref(),
            &user,
            &req.run_id,
        )
        .await?;

        // Node status comes from the engine's checkpoint state. The
        // checkpoint/log reads are synchronous file I/O — run the
        // combined block on the blocking pool (#268 item 4, web minor).
        let workdir = run.workdir.clone();
        let run_status = run.status.clone();
        let (node_items, log, benchmarks) = tokio::task::spawn_blocking(move || {
            let wd = workdir.as_deref().unwrap_or("");
            let node_items = crate::domains::execution::checkpoint_status::load_node_statuses(
                std::path::Path::new(wd),
                &run_status,
            );
            let log = crate::domains::execution::checkpoint_status::read_tail_bounded(
                std::path::Path::new(&format!("{wd}/execution.log")),
                1024 * 1024,
            );
            let benchmarks = crate::domains::execution::checkpoint_status::load_benchmarks(
                std::path::Path::new(wd),
            );
            (node_items, log, benchmarks)
        })
        .await
        .unwrap_or_default();
        let diagnostics =
            crate::domains::execution::service::diagnose_run(&node_items, &log, &benchmarks);
        (diagnostics, log)
    } else {
        (
            DiagnosticsResponse {
                failed_nodes: vec![],
                warnings: vec![],
                resource_bottlenecks: vec![],
            },
            String::new(),
        )
    };

    require_usable_provider(&provider)?;
    super::service::explain_failure(
        &provider,
        &diagnostics,
        &log_output,
        req.language.as_deref().unwrap_or("en"),
    )
    .await
    .map(Json)
    .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_EXPLAIN_ERROR", e))
}

/// Run a blocking filesystem walk off the async worker thread.
///
/// `block_in_place` tells the multi-threaded runtime this worker is about
/// to block so it can move other tasks elsewhere — but it PANICS on a
/// current-thread runtime (unit tests, embedded callers), so the flavor is
/// checked first and the walk runs inline there (same contract as
/// `auth::service::blocking_crypto`; issue #297 item 5).
fn blocking_dir_walk<T>(op: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(op)
        }
        _ => op(),
    }
}

#[utoipa::path(
    post,
    path = "/api/ai/interpret",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = InterpretResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/interpret
///
/// Looks up run results from DB and workdir for real data.
pub async fn interpret(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<InterpretRequest>,
) -> ApiResult<InterpretResponse> {
    let user = resolve(authenticated.as_ref());
    let provider = crate::ai_provider::provider_for(&user.id).await;

    // Try to get output summary from run. Ownership is enforced exactly
    // like the execution handlers (#515): foreign and unknown runs both 404
    // — the workdir file listing must not be readable through the AI surface.
    let output_summary = if let Some(backend) = crate::infra::db::global_backend() {
        let run = crate::domains::execution::handlers::load_owned_run(
            backend.as_ref(),
            &user,
            &req.run_id,
        )
        .await?;

        run.workdir
            .as_ref()
            .and_then(|wd| {
                // Read result files for summary — synchronous directory
                // walk, kept off the async runtime (#268 item 4, web
                // minor). The blocking-pool hop is only worth it when a
                // workdir exists; otherwise stay on the cheap path.
                let wd = wd.to_string();
                blocking_dir_walk(|| {
                    let mut summary = String::new();
                    if let Ok(entries) = std::fs::read_dir(&wd) {
                        for entry in entries.filter_map(|e| e.ok()) {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if let Ok(meta) = entry.metadata() {
                                summary.push_str(&format!("{} ({} bytes)\n", name, meta.len()));
                            }
                        }
                    }
                    if summary.is_empty() {
                        None
                    } else {
                        Some(summary)
                    }
                })
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    require_usable_provider(&provider)?;
    super::service::interpret_results(
        &provider,
        &req.run_id,
        req.result_type.as_deref().unwrap_or("general"),
        &output_summary,
    )
    .await
    .map(Json)
    .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_INTERPRET_ERROR", e))
}

#[utoipa::path(
    post,
    path = "/api/ai/optimize",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = OptimizeResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/optimize
///
/// Accepts TOML directly or loads from DB by pipeline_id.
pub async fn optimize(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<OptimizeRequest>,
) -> ApiResult<OptimizeResponse> {
    let user = resolve(authenticated.as_ref());
    let provider = crate::ai_provider::provider_for(&user.id).await;

    // Use provided TOML, or load from DB. A `pipeline_id` lookup enforces
    // the same read permission as the pipeline library (#515): private
    // pipelines must not be exfiltrated through the AI surface. Foreign and
    // unknown ids both 404 to avoid existence probing.
    let toml_content = if let Some(ref toml) = req.toml_content {
        toml.clone()
    } else if let Some(backend) = crate::infra::db::global_backend() {
        let pipeline: Option<models::PipelineRow> =
            backend.get_pipeline(&req.pipeline_id).await.unwrap_or(None);
        match pipeline.filter(|p| can_read_pipeline(&user, p)) {
            Some(p) => p.toml_content,
            None => {
                return Err(err(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!("Pipeline {} not found", req.pipeline_id),
                ));
            }
        }
    } else {
        String::new()
    };

    require_usable_provider(&provider)?;
    super::service::optimize_pipeline(&provider, &toml_content, &req.goal, req.constraints)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_OPTIMIZE_ERROR", e))
}

#[utoipa::path(
    get,
    path = "/api/ai/config",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = AiConfigResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/ai/config
pub async fn get_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<AiConfigResponse> {
    let config = crate::ai_provider::AiProviderRegistry::global().get_config();
    // Anonymous discoverability stays public, but the deployment's internal
    // AI endpoint and model are not world-readable: outside personal mode
    // (loopback, single local user) only `is_configured` is returned unless
    // the caller is an authenticated admin.
    if crate::server::running_mode() != "personal"
        && !authenticated.as_ref().is_some_and(|u| u.0.is_admin())
    {
        return Ok(Json(AiConfigResponse {
            provider: String::new(),
            model: None,
            api_url: None,
            is_configured: config.is_configured,
        }));
    }
    Ok(Json(AiConfigResponse {
        provider: config.provider,
        model: config.model,
        api_url: config.api_url,
        is_configured: config.is_configured,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ai/config",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = AiConfigResponse),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/config
pub async fn update_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<AiConfigRequest>,
) -> ApiResult<AiConfigResponse> {
    let user = resolve(authenticated.as_ref());
    require_ai_admin(&user)?;
    reject_config_update_when_pinned(oxo_flow_ai::provider::provider_pinned_disabled())?;
    // The shared runtime changed — every cached per-user fallback is stale.
    crate::ai_provider::invalidate_provider_cache();
    crate::ai_provider::AiProviderRegistry::global()
        .reconfigure(
            req.provider.as_deref().unwrap_or("noop"),
            req.api_key.clone(),
            req.api_url.clone(),
            req.model.clone(),
        )
        .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_CONFIG_ERROR", e))?;
    let config = crate::ai_provider::AiProviderRegistry::global().get_config();
    Ok(Json(AiConfigResponse {
        provider: config.provider,
        model: config.model,
        api_url: config.api_url,
        is_configured: config.is_configured,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ai/test",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = AiTestResponse),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// POST /api/ai/test
pub async fn test_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(_req): ApiJson<AiConfigRequest>,
) -> ApiResult<AiTestResponse> {
    let user = resolve(authenticated.as_ref());
    // Testing hits the real provider (costs money; also a DoS vector) —
    // admin-only outside personal mode.
    require_ai_admin(&user)?;
    // Return before resolving a provider: a pinned process must answer
    // "disabled" without constructing an HTTP client, let alone dialing out.
    if let Some(response) = pinned_test_response(oxo_flow_ai::provider::provider_pinned_disabled())
    {
        return Ok(Json(response));
    }
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    match provider
        .chat("You are a helpful assistant.", "Reply with exactly: OK")
        .await
    {
        Ok(response) => Ok(Json(AiTestResponse {
            success: true,
            message: format!("Test successful: {response}"),
            provider: provider.name().to_string(),
            model: None,
        })),
        Err(e) => Ok(Json(AiTestResponse {
            success: false,
            message: format!("Test failed: {e}"),
            provider: provider.name().to_string(),
            model: None,
        })),
    }
}

// ---------------------------------------------------------------------------
// AI Config - Three-tier priority
// ---------------------------------------------------------------------------

/// Restore the AI registry from the DB-persisted config at startup.
///
/// Env vars (via `init_from_env`) take precedence; the DB tier (user row,
/// then server row) fills in when env did not configure a provider. Without
/// this, a key saved through the settings UI would be lost on restart.
pub async fn restore_ai_config_from_db() {
    // Startup restoration is best-effort: an uninitialized database leaves
    // env/default resolution in control rather than blocking server boot.
    let Some(backend) = global_backend() else {
        return;
    };
    // Env explicitly configured a provider and remains the top tier.
    if std::env::var("OXO_FLOW_AI_PROVIDER").is_ok() {
        return;
    }
    let user_row = backend
        .get_ai_provider_config(Some("default"))
        .await
        .ok()
        .flatten();
    let row = match user_row {
        Some(row) => Some(row),
        None => backend.get_ai_provider_config(None).await.ok().flatten(),
    };
    let Some(row) = row else { return };
    let api_key = row.api_key.unwrap_or_default();
    if api_key.is_empty() {
        return;
    }
    // A sealed credential that cannot be read back (missing or rotated
    // OXO_FLOW_MASTER_KEY) must not be registered as an empty-key provider:
    // the config would look configured while every AI call fails. Leaving it
    // unset keeps the "needs re-entry" state visible.
    if !crate::infra::crypto::is_recoverable(&api_key) {
        tracing::error!(
            "AI provider '{}' is saved but its credential cannot be decrypted with \
             the current OXO_FLOW_MASTER_KEY; re-enter the API key in the settings UI \
             to restore AI features",
            row.provider
        );
        return;
    }
    if let Err(error) = crate::ai_provider::AiProviderRegistry::global().reconfigure(
        &row.provider,
        Some(crate::infra::crypto::open(&api_key)),
        Some(row.api_url.unwrap_or_default()),
        Some(row.model.unwrap_or_default()),
    ) {
        tracing::error!(
            provider = %row.provider,
            error = %error,
            "stored AI configuration could not be restored into the runtime registry"
        );
    } else {
        tracing::info!(
            "AI registry restored from DB tier: provider={}",
            row.provider
        );
    }
}

#[utoipa::path(
    get,
    path = "/api/ai/config/effective",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/ai/config/effective
pub async fn get_ai_config_effective(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    use crate::ai_provider::AiProviderRegistry;

    let user = resolve(authenticated.as_ref());
    let registry = AiProviderRegistry::global();
    let config = registry.get_config();
    let user_row = get_backend()?
        .get_ai_provider_config(Some(&user.id))
        .await
        .map_err(|e| ai_db_error("loading effective AI provider config", e))?;
    let (user_provider, user_model, user_url) = match user_row {
        Some(row) => (Some(row.provider), row.model, row.api_url),
        None => (None, None, None),
    };

    Ok(Json(serde_json::json!({
        "effective": {
            "provider": config.provider,
            "model": config.model,
            "api_url": config.api_url,
            "is_configured": config.is_configured,
        },
        "disabled_by_env": oxo_flow_ai::provider::provider_pinned_disabled(),
        "tiers": {
            "env_provider": std::env::var("OXO_FLOW_AI_PROVIDER").ok(),
            "env_model": std::env::var("OXO_FLOW_AI_MODEL").ok(),
            "env_url": std::env::var("OXO_FLOW_AI_API_URL").ok(),
            "server_provider": config.provider,
            "server_model": config.model,
            "user_provider": user_provider,
            "user_model": user_model,
            "user_api_url": user_url,
        },
        "resolution_order": ["user_settings", "server_config", "environment", "defaults"],
    })))
}

#[utoipa::path(
    get,
    path = "/api/ai/config/server",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// GET /api/ai/config/server
pub async fn get_server_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    // The server read is admin-only outside personal mode (web-api.md's
    // contract): the configured endpoint URL identifies the internal AI
    // proxy/gateway — exactly the reconnaissance input a hardened team
    // deployment must not expose to unauthenticated callers.
    let user = resolve(authenticated.as_ref());
    current_user::require_role_outside_personal(
        &user,
        "admin",
        "Admin role required to read the server AI provider config",
    )?;
    let row = get_backend()?
        .get_ai_provider_config(None)
        .await
        .map_err(|e| ai_db_error("loading server AI provider config", e))?;
    // Split "a row exists" from "that row is usable": the api_key is sealed
    // and only readable with the same OXO_FLOW_MASTER_KEY. After a key
    // rotation the ciphertext stays in place but cannot be opened.
    let (stored_configured, requires_reauth) =
        match row.as_ref().and_then(|row| row.api_key.as_deref()) {
            None => (false, false),
            Some(api_key) => {
                let usable = api_key.is_empty() || crate::infra::crypto::is_recoverable(api_key);
                (usable, !usable)
            }
        };
    // `configured` is the effective runtime state, not just "a row exists
    // and is readable": a pinned process answers providers with nothing.
    let pinned_disabled = oxo_flow_ai::provider::provider_pinned_disabled();
    let configured = stored_configured && !pinned_disabled;
    let server_config = row.map(|row| {
        serde_json::json!({
            "provider": row.provider,
            "api_url": row.api_url.unwrap_or_default(),
            "model": row.model.unwrap_or_default(),
            "search_enabled": row.search_enabled != 0,
        })
    });

    let mut body = serde_json::json!({
        "server_config": server_config,
        "configured": configured,
        "disabled_by_env": pinned_disabled,
        "requires_reauth": requires_reauth,
    });
    if requires_reauth {
        body["message"] = serde_json::json!(
            "The stored AI credential cannot be decrypted with the current \
             OXO_FLOW_MASTER_KEY — re-enter the API key to restore AI features."
        );
    }

    Ok(Json(body))
}

#[utoipa::path(
    put,
    path = "/api/ai/config/server",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 403, description = "Error", body = ApiError),
    )
)]
/// PUT /api/ai/config/server
pub async fn update_server_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    require_ai_admin(&user)?;
    // Contract (issue #1040 follow-up): under `OXO_FLOW_AI_PROVIDER=disabled`
    // the save still persists — Settings must stay usable, and the stored
    // config applies after an unpinned restart — but the runtime is NEVER
    // reconfigured, so a pinned process cannot be resurrected by an API call.
    let pinned_disabled = oxo_flow_ai::provider::provider_pinned_disabled();
    let backend = get_backend()?;
    let existing = backend
        .get_ai_provider_config(None)
        .await
        .map_err(|e| ai_db_error("loading server AI provider config", e))?;

    let provider = req
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled")
        .trim()
        .to_ascii_lowercase();
    let api_url = req.get("api_url").and_then(|v| v.as_str()).unwrap_or("");
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = req.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
    crate::ai_provider::validate_provider_kind(&provider).map_err(|error| {
        err(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_PROVIDER",
            format!("AI provider '{provider}' is not supported: {error}"),
        )
    })?;
    // An absent/empty key means "keep the stored one" — the API never returns
    // the key, so a settings save that omits it must not wipe it.
    let stored_api_key = crate::infra::crypto::seal(api_key);
    let now = chrono::Utc::now().to_rfc3339();
    let row = models::AiProviderConfigRow {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: None,
        provider: provider.clone(),
        api_key: Some(stored_api_key),
        api_url: Some(api_url.to_string()),
        model: Some(model.to_string()),
        search_enabled: existing.as_ref().map_or(1, |row| row.search_enabled),
        monitor_enabled: existing.as_ref().map_or(1, |row| row.monitor_enabled),
        auto_retry_enabled: existing.as_ref().map_or(0, |row| row.auto_retry_enabled),
        max_correction_rounds: existing.as_ref().map_or(3, |row| row.max_correction_rounds),
        created_at: existing
            .as_ref()
            .map_or_else(|| now.clone(), |row| row.created_at.clone()),
        updated_at: now,
    };
    let saved = backend
        .save_ai_provider_config(&row, api_key.is_empty())
        .await
        .map_err(|e| ai_db_error("saving server AI provider config", e))?;

    // Apply only after durable persistence so a failed save cannot leave the
    // runtime pointing at a configuration that disappears on restart.
    if pinned_disabled {
        tracing::warn!(
            "server AI configuration persisted but not activated: \
             OXO_FLOW_AI_PROVIDER=disabled pins this process off"
        );
    } else {
        crate::ai_provider::invalidate_provider_cache();
        if let Err(error) = crate::ai_provider::AiProviderRegistry::global().reconfigure(
            &saved.provider,
            runtime_api_key(saved.api_key.as_deref()),
            runtime_optional(saved.api_url.as_deref().unwrap_or_default()),
            runtime_optional(saved.model.as_deref().unwrap_or_default()),
        ) {
            tracing::error!(
                provider = %saved.provider,
                error = %error,
                "server AI configuration was persisted but could not be activated"
            );
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "AI_RECONFIGURE_FAILED",
                "AI configuration was saved but could not be activated; correct the provider settings or restart the server.".into(),
            ));
        }
    }

    Ok(Json(serde_json::json!({
        "status": "updated",
        "provider": saved.provider,
        "applied_to_runtime": !pinned_disabled,
    })))
}

#[utoipa::path(
    get,
    path = "/api/ai/config/user",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/ai/config/user
///
/// Returns the ACTING USER's stored row (#545): the four advanced fields
/// PUT persists must round-trip, or the Settings form silently reverted
/// them and the next save wiped the stored values. The API key never
/// leaves the server — only `api_key_set` travels.
pub async fn get_user_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    let row = get_backend()?
        .get_ai_provider_config(Some(&user.id))
        .await
        .map_err(|e| ai_db_error("loading user AI provider config", e))?;

    let user_config = match row {
        Some(row) => {
            let key_present = row
                .api_key
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty());
            serde_json::json!({
                "provider": row.provider,
                "api_url": row.api_url.unwrap_or_default(),
                "model": row.model.unwrap_or_default(),
                "api_key_set": key_present,
                "is_configured": row.provider != "disabled" && key_present,
                "search_enabled": row.search_enabled != 0,
                "monitor_enabled": row.monitor_enabled != 0,
                "auto_retry_enabled": row.auto_retry_enabled != 0,
                "max_correction_rounds": row.max_correction_rounds,
            })
        }
        None => {
            // No per-user row yet — mirror the shared runtime and the
            // PUT defaults so a first save does not surprise.
            let config = crate::ai_provider::AiProviderRegistry::global().get_config();
            serde_json::json!({
                "provider": config.provider,
                "api_url": config.api_url,
                "model": config.model,
                "api_key_set": false,
                "is_configured": config.is_configured,
                "search_enabled": true,
                "monitor_enabled": true,
                "auto_retry_enabled": false,
                "max_correction_rounds": 3,
            })
        }
    };
    // The stored row is reported as stored, but `configured` describes the
    // EFFECTIVE state: a pinned process runs no provider regardless of what
    // the row says, and must say so the way /api/health does.
    let pinned_disabled = oxo_flow_ai::provider::provider_pinned_disabled();
    let configured = !pinned_disabled && user_config["is_configured"].as_bool().unwrap_or(false);
    Ok(Json(serde_json::json!({
        "user_config": user_config,
        "configured": configured,
        "disabled_by_env": pinned_disabled,
    })))
}

#[utoipa::path(
    put,
    path = "/api/ai/config/user",
    tag = "ai",
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// PUT /api/ai/config/user
pub async fn update_user_ai_config(
    authenticated: Option<Extension<CurrentUser>>,
    ApiJson(req): ApiJson<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let user = resolve(authenticated.as_ref());
    // Same contract as the server tier: persist the row (Settings stays
    // usable; GET reports `disabled_by_env` so the effective state is never
    // misrepresented), but never reconfigure the runtime under the pin.
    let pinned_disabled = oxo_flow_ai::provider::provider_pinned_disabled();
    let backend = get_backend()?;

    let provider = req
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled")
        .trim()
        .to_ascii_lowercase();
    let api_url = req.get("api_url").and_then(|v| v.as_str()).unwrap_or("");
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = req.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
    crate::ai_provider::validate_provider_kind(&provider).map_err(|error| {
        err(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_PROVIDER",
            format!("AI provider '{provider}' is not supported: {error}"),
        )
    })?;
    // An absent/empty key means "keep the stored one". Compared as a flag,
    // not by the sealed value: with a master key configured seal("") is a
    // non-empty ciphertext.
    let stored_api_key = crate::infra::crypto::seal(api_key);
    let search_enabled = req
        .get("search_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let monitor_enabled = req
        .get("monitor_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let auto_retry_enabled = req
        .get("auto_retry_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_correction_rounds = req
        .get("max_correction_rounds")
        .and_then(|v| v.as_i64())
        .unwrap_or(3);
    let now = chrono::Utc::now().to_rfc3339();
    let row = models::AiProviderConfigRow {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: Some(user.id.clone()),
        provider: provider.clone(),
        api_key: Some(stored_api_key),
        api_url: Some(api_url.to_string()),
        model: Some(model.to_string()),
        search_enabled: i64::from(search_enabled),
        monitor_enabled: i64::from(monitor_enabled),
        auto_retry_enabled: i64::from(auto_retry_enabled),
        max_correction_rounds,
        created_at: now.clone(),
        updated_at: now,
    };
    let saved = backend
        .save_ai_provider_config(&row, api_key.is_empty())
        .await
        .map_err(|e| ai_db_error("saving user AI provider config", e))?;

    // The runtime serves ONE provider registry per process: in personal
    // mode (or for admins) applying it immediately is expected. Non-admin
    // team users' rows are resolved per user by provider_for(). The pin
    // overrides all of it: never apply while `OXO_FLOW_AI_PROVIDER=disabled`.
    let applied =
        !pinned_disabled && (crate::server::running_mode() == "personal" || user.is_admin());
    crate::ai_provider::invalidate_provider_for(&user.id);
    if applied
        && let Err(error) = crate::ai_provider::AiProviderRegistry::global().reconfigure(
            &saved.provider,
            runtime_api_key(saved.api_key.as_deref()),
            runtime_optional(saved.api_url.as_deref().unwrap_or_default()),
            runtime_optional(saved.model.as_deref().unwrap_or_default()),
        )
    {
        tracing::error!(
            provider = %saved.provider,
            user_id = %user.id,
            error = %error,
            "user AI configuration was persisted but could not be activated"
        );
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AI_RECONFIGURE_FAILED",
            "AI configuration was saved but could not be activated; correct the provider settings or restart the server.".into(),
        ));
    }

    Ok(Json(serde_json::json!({
        "status": "updated",
        "provider": saved.provider,
        "applied_to_runtime": applied,
    })))
}

// ---------------------------------------------------------------------------
// Knowledge search — the editor palette's grounded tool source
// ---------------------------------------------------------------------------

/// Query parameters for the knowledge search endpoints.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KnowledgeQuery {
    #[serde(default)]
    pub q: String,
    pub limit: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/api/knowledge/tools",
    tag = "ai",
    params(KnowledgeQuery),
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/knowledge/tools — search the embedded Bioconda tool database.
pub async fn knowledge_tools(
    crate::extract::ApiQuery(params): crate::extract::ApiQuery<KnowledgeQuery>,
) -> ApiResult<serde_json::Value> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let tools = oxo_flow_ai::knowledge::bioconda::search_tools(&params.q, limit);
    Ok(Json(serde_json::json!({
        "total": oxo_flow_ai::knowledge::bioconda::tool_count(),
        "tools": tools.iter().map(|t| serde_json::json!({
            "name": t.name,
            "version": t.version,
            "summary": t.summary,
            "platforms": t.platforms,
        })).collect::<Vec<_>>(),
    })))
}

#[utoipa::path(
    get,
    path = "/api/knowledge/skills",
    tag = "ai",
    params(KnowledgeQuery),
    responses(
        (status = 200, description = "Success", body = serde_json::Value),
        (status = 400, description = "Error", body = ApiError),
    )
)]
/// GET /api/knowledge/skills — search the embedded bioSkills database.
pub async fn knowledge_skills(
    crate::extract::ApiQuery(params): crate::extract::ApiQuery<KnowledgeQuery>,
) -> ApiResult<serde_json::Value> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let skills = oxo_flow_ai::knowledge::skills::search_skills(&params.q, limit);
    Ok(Json(serde_json::json!({
        "total": oxo_flow_ai::knowledge::skills::skill_count(),
        "skills": skills.iter().map(|s| serde_json::json!({
            "name": s.name,
            "description": s.description,
            "domain": s.domain,
            "primary_tool": s.primary_tool,
        })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        pinned_test_response, reject_config_update_when_pinned, runtime_api_key, runtime_optional,
    };
    use axum::http::StatusCode;

    #[test]
    fn pinned_runtime_config_endpoint_is_refused_with_a_structured_conflict() {
        // Unpinned: the endpoint proceeds to its normal reconfigure path.
        assert!(reject_config_update_when_pinned(false).is_ok());

        let (status, body) = reject_config_update_when_pinned(true).expect_err("pin must refuse");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.0.code, "AI_DISABLED_BY_ENV");
        assert!(body.0.message.contains("OXO_FLOW_AI_PROVIDER=disabled"));
        assert!(body.0.detail.is_some(), "structured error carries detail");
        assert!(
            body.0.suggestion.is_some(),
            "structured error carries a remedy"
        );
    }

    #[test]
    fn pinned_test_probe_answers_disabled_without_a_provider() {
        // Regression: after a config update under the pin, this endpoint
        // used to run a real chat round-trip (3 outbound connections). The
        // pinned answer is produced before any provider/client is built.
        assert!(pinned_test_response(false).is_none());
        let response = pinned_test_response(true).expect("pinned answer");
        assert!(!response.success);
        assert_eq!(response.provider, "disabled");
        assert!(response.message.contains("no connection attempted"));
    }

    #[test]
    fn runtime_config_omits_blank_overrides() {
        assert_eq!(runtime_optional("  "), None);
        assert_eq!(runtime_optional(" gpt-5 "), Some("gpt-5".to_string()));
        assert_eq!(runtime_api_key(None), None);
        assert_eq!(runtime_api_key(Some("  ")), None);
        assert_eq!(
            runtime_api_key(Some(" sk-stored ")),
            Some("sk-stored".to_string())
        );
    }
}
