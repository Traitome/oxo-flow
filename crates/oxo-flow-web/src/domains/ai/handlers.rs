//! HTTP handlers for AI domain.
//!
//! Thin adapters: parse HTTP request -> call service -> serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Json, http::StatusCode};

use crate::domains::ai::types::*;
use crate::domains::execution::types::DiagnosticsResponse;
use crate::domains::workflow::handlers::{ApiError, err};
use crate::infra::db::models;

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

/// POST /api/ai/translate — standard JSON response.
pub async fn translate(Json(req): Json<TranslateRequest>) -> ApiResult<TranslateResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    let templates: Vec<String> = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY usage_count DESC LIMIT 20",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
    } else {
        vec![]
    };

    super::service::translate_intent(&provider, &req.intent, None, &templates)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_TRANSLATE_ERROR", e))
}

/// POST /api/ai/translate?stream=true — SSE streaming response.
///
/// Streams progress events: intent → data → match → generate → validate → done.
/// Each event has `type` and `data` fields. The final event contains the full
/// TranslateResponse as JSON.
pub async fn translate_sse(
    Json(req): Json<TranslateRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use std::convert::Infallible;

    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    let templates: Vec<String> = if let Ok(pool) = get_pool() {
        sqlx::query_as::<_, models::TemplateRow>(
            "SELECT * FROM templates ORDER BY usage_count DESC LIMIT 20",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
    } else {
        vec![]
    };

    let intent = req.intent.clone();
    let stream = async_stream::stream! {
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

        let result = super::service::translate_intent(&provider, &intent, None, &templates).await;

        // Step 4: validate + done
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
                yield Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": e}).to_string()));
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// POST /api/ai/explain
///
/// Looks up the run from the database to get real diagnostics data.
pub async fn explain(Json(req): Json<ExplainRequest>) -> ApiResult<ExplainResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    // Try to look up run diagnostics from DB
    let (diagnostics, log_output) = if let Ok(pool) = get_pool() {
        let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
            .bind(&req.run_id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

        if let Some(run) = run {
            let nodes: Vec<models::RunNodeRow> =
                sqlx::query_as("SELECT * FROM run_nodes WHERE run_id = ? ORDER BY attempt ASC")
                    .bind(&req.run_id)
                    .fetch_all(pool)
                    .await
                    .unwrap_or_default();

            let node_items: Vec<crate::domains::execution::types::NodeStatusItem> = nodes
                .iter()
                .map(|n| {
                    use crate::domains::execution::types::NodeStatus;
                    let status = match n.status.as_str() {
                        "success" => NodeStatus::Success,
                        "failed" => NodeStatus::Failed,
                        "running" => NodeStatus::Running,
                        "skipped" => NodeStatus::Skipped,
                        _ => NodeStatus::Pending,
                    };
                    crate::domains::execution::types::NodeStatusItem {
                        rule: n.rule_name.clone(),
                        status,
                        started_at: n.started_at.clone(),
                        duration_ms: None,
                        exit_code: n.exit_code,
                        progress_pct: None,
                    }
                })
                .collect();

            let log = run
                .workdir
                .as_ref()
                .and_then(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).ok())
                .unwrap_or_default();

            let diagnostics = crate::domains::execution::service::diagnose_run(&node_items, &log);
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
        }
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

/// POST /api/ai/interpret
///
/// Looks up run results from DB and workdir for real data.
pub async fn interpret(Json(req): Json<InterpretRequest>) -> ApiResult<InterpretResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    // Try to get output summary from run
    let output_summary = if let Ok(pool) = get_pool() {
        let run: Option<models::RunRow> = sqlx::query_as("SELECT * FROM runs WHERE id = ?")
            .bind(&req.run_id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

        run.and_then(|r| {
            r.workdir.as_ref().and_then(|wd| {
                // Read result files for summary
                let mut summary = String::new();
                if let Ok(entries) = std::fs::read_dir(wd) {
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

/// POST /api/ai/optimize
///
/// Accepts TOML directly or loads from DB by pipeline_id.
pub async fn optimize(Json(req): Json<OptimizeRequest>) -> ApiResult<OptimizeResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    // Use provided TOML, or load from DB
    let toml_content = if let Some(ref toml) = req.toml_content {
        toml.clone()
    } else if let Ok(pool) = get_pool() {
        let pipeline: Option<models::PipelineRow> =
            sqlx::query_as("SELECT * FROM pipelines WHERE id = ?")
                .bind(&req.pipeline_id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
        pipeline.map(|p| p.toml_content).unwrap_or_default()
    } else {
        String::new()
    };

    super::service::optimize_pipeline(&provider, &toml_content, &req.goal, req.constraints)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_OPTIMIZE_ERROR", e))
}

/// GET /api/ai/config
pub async fn get_ai_config() -> ApiResult<AiConfigResponse> {
    let config = crate::ai_provider::AiProviderRegistry::global().get_config();
    Ok(Json(AiConfigResponse {
        provider: config.provider,
        model: config.model,
        api_url: config.api_url,
        is_configured: config.is_configured,
    }))
}

/// POST /api/ai/config
pub async fn update_ai_config(Json(req): Json<AiConfigRequest>) -> ApiResult<AiConfigResponse> {
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

/// POST /api/ai/test
pub async fn test_ai_config(Json(_req): Json<AiConfigRequest>) -> ApiResult<AiTestResponse> {
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
// AI Config - Three-tier priority (v0.9)
// ---------------------------------------------------------------------------

/// GET /api/ai/config/effective
pub async fn get_ai_config_effective() -> ApiResult<serde_json::Value> {
    use crate::ai_provider::AiProviderRegistry;

    let registry = AiProviderRegistry::global();
    let config = registry.get_config();
    let env_provider = std::env::var("OXO_FLOW_AI_PROVIDER").ok();
    let env_model = std::env::var("OXO_FLOW_AI_MODEL").ok();
    let env_url = std::env::var("OXO_FLOW_AI_API_URL").ok();

    Ok(Json(serde_json::json!({
        "effective": {
            "provider": config.provider,
            "model": config.model,
            "api_url": config.api_url,
            "is_configured": config.is_configured,
        },
        "tiers": {
            "env_provider": env_provider,
            "env_model": env_model,
            "env_url": env_url,
            "server_provider": config.provider,
            "server_model": config.model,
            "user_provider": serde_json::Value::Null,
        },
        "resolution_order": ["user_settings", "server_config", "environment", "defaults"],
    })))
}

/// GET /api/ai/config/server
pub async fn get_server_ai_config() -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().ok();
    let config = if let Some(p) = pool {
        let row: Option<serde_json::Value> = sqlx::query_as::<_, (String, String, String, String, i64)>(
            "SELECT provider, api_url, model, api_key, search_enabled FROM ai_provider_config WHERE user_id IS NULL ORDER BY updated_at DESC LIMIT 1"
        )
        .fetch_optional(p)
        .await
        .ok()
        .flatten()
        .map(|(provider, api_url, model, _api_key, search_enabled)| {
            serde_json::json!({
                "provider": provider,
                "api_url": api_url,
                "model": model,
                "search_enabled": search_enabled == 1,
            })
        });
        row
    } else {
        None
    };

    Ok(Json(serde_json::json!({
        "server_config": config,
        "configured": config.is_some(),
    })))
}

/// PUT /api/ai/config/server
pub async fn update_server_ai_config(
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let provider = req
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled");
    let api_url = req.get("api_url").and_then(|v| v.as_str()).unwrap_or("");
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = req.get("api_key").and_then(|v| v.as_str()).unwrap_or("");

    // Upsert server config (user_id IS NULL)
    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_url, model, api_key, search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, created_at, updated_at) VALUES (?, NULL, ?, ?, ?, ?, 1, 1, 0, 3, ?, ?) ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider, api_url=excluded.api_url, model=excluded.model, api_key=excluded.api_key, updated_at=excluded.updated_at"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(api_url)
    .bind(model)
    .bind(api_key)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    // Apply to runtime
    let _ = crate::ai_provider::AiProviderRegistry::global().reconfigure(
        provider,
        Some(api_key.into()),
        Some(api_url.into()),
        Some(model.into()),
    );

    Ok(Json(
        serde_json::json!({"status": "updated", "provider": provider}),
    ))
}

/// GET /api/ai/config/user
pub async fn get_user_ai_config() -> ApiResult<serde_json::Value> {
    let config = crate::ai_provider::AiProviderRegistry::global().get_config();
    Ok(Json(serde_json::json!({
        "user_config": {
            "provider": config.provider,
            "api_url": config.api_url,
            "model": config.model,
            "is_configured": config.is_configured,
        },
        "configured": config.is_configured,
    })))
}

/// PUT /api/ai/config/user
pub async fn update_user_ai_config(
    Json(req): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    let pool = crate::infra::db::sqlite::try_pool().map_err(|_| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_ERROR",
            "Database not available".into(),
        )
    })?;

    let provider = req
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled");
    let api_url = req.get("api_url").and_then(|v| v.as_str()).unwrap_or("");
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = req.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
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

    sqlx::query(
        "INSERT INTO ai_provider_config (id, user_id, provider, api_url, model, api_key, search_enabled, monitor_enabled, auto_retry_enabled, max_correction_rounds, created_at, updated_at) VALUES (?, 'default', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider, api_url=excluded.api_url, model=excluded.model, api_key=excluded.api_key, search_enabled=excluded.search_enabled, monitor_enabled=excluded.monitor_enabled, auto_retry_enabled=excluded.auto_retry_enabled, max_correction_rounds=excluded.max_correction_rounds, updated_at=excluded.updated_at"
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(api_url)
    .bind(model)
    .bind(api_key)
    .bind(search_enabled as i64)
    .bind(monitor_enabled as i64)
    .bind(auto_retry_enabled as i64)
    .bind(max_correction_rounds)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let _ = crate::ai_provider::AiProviderRegistry::global().reconfigure(
        provider,
        Some(api_key.into()),
        Some(api_url.into()),
        Some(model.into()),
    );

    Ok(Json(
        serde_json::json!({"status": "updated", "provider": provider}),
    ))
}
