//! Total wall-clock budgets for the non-streaming AI JSON routes.
//!
//! Streaming routes can legitimately run for many minutes (a chat generation
//! takes 8–14 minutes). The JSON routes cannot: every provider request is
//! bounded by `OXO_FLOW_AI_TIMEOUT_SECS`, but a multi-round generation
//! (knowledge lookups plus correction rounds) can still outlive a reverse
//! proxy's idle limit, which then kills the connection with no structured
//! answer. These helpers cap the WHOLE generation at the same documented
//! knob and answer `504 AI_TIMEOUT` instead (issue #1040 P3-5).
//!
//! The route budget fires `ROUTE_TIMEOUT_MARGIN_SECS` before the provider
//! client's per-request timeout, so the structured 504 normally wins the
//! race. Timeout-class provider errors are additionally classified into the
//! same response by the handlers, so the documented contract holds even when
//! the two deadlines coincide (review wave 2 HIGH-1).

use std::future::Future;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;

use crate::domains::workflow::handlers::ApiError;

/// Seconds reserved between the route budget and the provider client's
/// per-request timeout. Both are configured from the same
/// `OXO_FLOW_AI_TIMEOUT_SECS` knob, and with equal deadlines the inner
/// provider error used to win and surface as `400 CHAT_ERROR` /
/// `AI_TRANSLATE_ERROR` instead of the documented 504 (review HIGH-1). The
/// margin only orders the timers; it barely shortens real budgets (295 s of
/// the 300 s default).
const ROUTE_TIMEOUT_MARGIN_SECS: u64 = 1;

/// The JSON routes' total generation budget: one second below the same
/// `OXO_FLOW_AI_TIMEOUT_SECS` knob that bounds one provider request (default
/// 300 s), read per request so an operator's setting is honored without a
/// rebuild or provider reconstruction. Never drops below one second: at the
/// knob's minimum the timeout-class classification covers the tie.
pub fn json_route_timeout() -> Duration {
    route_budget_for(oxo_flow_ai::provider::request_timeout_secs())
}

/// Derive the route budget from the provider's per-request timeout.
///
/// Pure core of [`json_route_timeout`] so the margin math is testable
/// without mutating the process environment.
fn route_budget_for(provider_secs: u64) -> Duration {
    Duration::from_secs(
        provider_secs
            .saturating_sub(ROUTE_TIMEOUT_MARGIN_SECS)
            .max(1),
    )
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
             validation rounds) at OXO_FLOW_AI_TIMEOUT_SECS. The route budget \
             fires one second before the provider's per-request timeout, and a \
             timeout-class provider error maps to this same answer."
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
        // knob unset the provider gets the shipped 300 s default and the
        // route budget sits one second below it. (This test does not mutate
        // the process environment — other tests run concurrently.)
        if std::env::var("OXO_FLOW_AI_TIMEOUT_SECS").is_err() {
            assert_eq!(json_route_timeout(), Duration::from_secs(299));
        }
        let result = with_total_timeout(async { 42 }).await;
        assert_eq!(result, Some(42));
    }

    #[test]
    fn route_budget_fires_before_the_provider_timeout() {
        // The margin orders the two timers that share one knob; the floor
        // keeps the documented minimum (1 s) usable, where the handler's
        // timeout-class classification covers the tie.
        assert_eq!(route_budget_for(300), Duration::from_secs(299));
        assert_eq!(route_budget_for(5), Duration::from_secs(4));
        assert_eq!(route_budget_for(2), Duration::from_secs(1));
        assert_eq!(route_budget_for(1), Duration::from_secs(1));
        for provider_secs in 1..=600 {
            assert!(route_budget_for(provider_secs) <= Duration::from_secs(provider_secs));
        }
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
