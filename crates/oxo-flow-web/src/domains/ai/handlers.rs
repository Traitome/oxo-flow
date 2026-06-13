//! HTTP handlers for AI domain.
//!
//! Thin adapters: parse HTTP request -> call service -> serialize response.
//! Zero business logic here — all logic lives in `service.rs`.

use axum::{Json, http::StatusCode};

use crate::domains::ai::types::*;
use crate::domains::execution::types::DiagnosticsResponse;
use crate::domains::workflow::handlers::{ApiError, err};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

/// POST /api/ai/translate
pub async fn translate(Json(req): Json<TranslateRequest>) -> ApiResult<TranslateResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    let templates: Vec<String> = vec![]; // will be loaded from DB in future
    super::service::translate_intent(&provider, &req.intent, None, &templates)
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_TRANSLATE_ERROR", e))
}

/// POST /api/ai/explain
pub async fn explain(Json(req): Json<ExplainRequest>) -> ApiResult<ExplainResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    let diagnostics = DiagnosticsResponse {
        failed_nodes: vec![],
        warnings: vec![],
        resource_bottlenecks: vec![],
    };
    super::service::explain_failure(
        &provider,
        &diagnostics,
        "",
        req.language.as_deref().unwrap_or("en"),
    )
    .await
    .map(Json)
    .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_EXPLAIN_ERROR", e))
}

/// POST /api/ai/interpret
pub async fn interpret(Json(req): Json<InterpretRequest>) -> ApiResult<InterpretResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    super::service::interpret_results(
        &provider,
        &req.run_id,
        req.result_type.as_deref().unwrap_or("general"),
        "",
    )
    .await
    .map(Json)
    .map_err(|e| err(StatusCode::BAD_REQUEST, "AI_INTERPRET_ERROR", e))
}

/// POST /api/ai/optimize
pub async fn optimize(Json(req): Json<OptimizeRequest>) -> ApiResult<OptimizeResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    super::service::optimize_pipeline(&provider, "", &req.goal, req.constraints)
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
