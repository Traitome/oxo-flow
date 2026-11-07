//! Total wall-clock budgets for the non-streaming AI JSON routes.
//!
//! Streaming routes can legitimately run for many minutes (a chat generation
//! takes 8–14 minutes). The JSON routes cannot: every provider request is
//! bounded by `OXO_FLOW_AI_TIMEOUT_SECS`, but a multi-round generation
//! (knowledge lookups plus correction rounds) can still outlive a reverse
//! proxy's idle limit, which then kills the connection with no structured
//! answer. These helpers cap the WHOLE generation at the same documented
//! knob and answer `504 AI_TIMEOUT` instead (issue #1040 P3-5).

use std::future::Future;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;

use crate::domains::workflow::handlers::ApiError;

/// The JSON routes' total generation budget: the same
/// `OXO_FLOW_AI_TIMEOUT_SECS` knob that bounds one provider request (default
/// 300 s), read per request so an operator's setting is honored without a
/// rebuild or provider reconstruction.
pub fn json_route_timeout() -> Duration {
    Duration::from_secs(oxo_flow_ai::provider::request_timeout_secs())
}

/// The structured timeout contract shared by `/api/chat/send/json` and
/// `/api/ai/translate`. The suggestion names the knob so the caller can act
/// instead of guessing.
pub fn ai_timeout_error(budget: Duration) -> ApiError {
    ApiError {
        code: "AI_TIMEOUT".into(),
        message: format!(
            "AI generation exceeded its {}-second budget",
            budget.as_secs()
        ),
        detail: Some(
            "The non-streaming AI routes cap the WHOLE generation (all tool and \
             validation rounds) at OXO_FLOW_AI_TIMEOUT_SECS."
                .into(),
        ),
        suggestion: Some(
            "Raise OXO_FLOW_AI_TIMEOUT_SECS, use the streaming endpoint \
             (/api/chat/send or /api/ai/translate/stream) for long generations, \
             or pick a faster model."
                .into(),
        ),
    }
}

/// Status + body for an expired budget, matching the shared structured-error
/// shape (`{code, message, detail, suggestion}`).
pub fn timeout_response(budget: Duration) -> (StatusCode, Json<ApiError>) {
    (StatusCode::GATEWAY_TIMEOUT, Json(ai_timeout_error(budget)))
}

/// Await `future` under an explicit budget; `None` means it expired. Kept
/// budget-parameterised so tests prove the bound in milliseconds instead of
/// sleeping out the production default.
pub async fn with_budget<F: Future>(budget: Duration, future: F) -> Option<F::Output> {
    tokio::time::timeout(budget, future).await.ok()
}

/// Await `future` under [`json_route_timeout`].
pub async fn with_total_timeout<F: Future>(future: F) -> Option<F::Output> {
    with_budget(json_route_timeout(), future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slow_generation_is_cut_off_at_the_budget() {
        // Arrange: a provider round that would outlive any gateway.
        let started = std::time::Instant::now();

        // Act: bound it at 50 ms instead of the 300 s production default.
        let result = with_budget(Duration::from_millis(50), async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            "never produced"
        })
        .await;

        // Assert: the route answers instead of hanging.
        assert!(result.is_none(), "the budget must expire");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the timeout must fire at the budget, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_generation_inside_the_budget_passes_through() {
        let result = with_budget(Duration::from_secs(5), async { "pipeline" }).await;
        assert_eq!(result, Some("pipeline"));
    }

    #[tokio::test]
    async fn total_timeout_uses_the_documented_knob() {
        // The helper must read the same knob as the provider client; with the
        // knob unset both report the shipped 300 s default. (This test does
        // not mutate the process environment — other tests run concurrently.)
        if std::env::var("OXO_FLOW_AI_TIMEOUT_SECS").is_err() {
            assert_eq!(json_route_timeout(), Duration::from_secs(300));
        }
        let result = with_total_timeout(async { 42 }).await;
        assert_eq!(result, Some(42));
    }

    #[test]
    fn timeout_error_is_a_504_that_names_the_knob() {
        let budget = Duration::from_secs(300);
        let error = ai_timeout_error(budget);

        assert_eq!(error.code, "AI_TIMEOUT");
        assert!(error.message.contains("300-second"), "{:?}", error.message);
        let detail = error.detail.as_deref().unwrap_or_default();
        assert!(detail.contains("OXO_FLOW_AI_TIMEOUT_SECS"), "{detail}");
        let suggestion = error.suggestion.as_deref().unwrap_or_default();
        assert!(
            suggestion.contains("OXO_FLOW_AI_TIMEOUT_SECS"),
            "{suggestion}"
        );
        assert!(suggestion.contains("streaming"), "{suggestion}");

        let (status, Json(body)) = timeout_response(budget);
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(body.code, "AI_TIMEOUT");
    }
}
