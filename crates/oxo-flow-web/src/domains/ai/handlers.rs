//! HTTP handlers for AI domain.
//!
//! Thin adapters: parse HTTP request -> call service -> serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

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

/// POST /api/ai/translate
pub async fn translate(Json(req): Json<TranslateRequest>) -> ApiResult<TranslateResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    // Load template names from DB for fallback matching
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
/// Loads the actual pipeline TOML from DB for optimization.
pub async fn optimize(Json(req): Json<OptimizeRequest>) -> ApiResult<OptimizeResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();

    // Load pipeline TOML from DB if pipeline_id is provided
    let toml_content = if let Ok(pool) = get_pool() {
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
