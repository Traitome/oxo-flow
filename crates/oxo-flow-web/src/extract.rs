//! Shared request extractors.
//!
//! [`ApiQuery`] replaces [`axum::extract::Query`] on the domain handlers so a
//! query string that does not fit the target type is answered in the site's
//! structured error envelope (see `domains::workflow::handlers::ApiError`)
//! instead of axum's plain-text rejection.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::domains::workflow::handlers::ApiError;

/// Typed query-string extractor whose rejection carries the site error shape.
///
/// `axum::extract::Query` answers an unusable query string with a bare-text
/// 400 (`Failed to deserialize query string: ...`) — no `code` to branch on,
/// no `suggestion`. This wrapper turns exactly that case into
/// `{"code": "INVALID_QUERY", ...}`, keeping the offending parameter in
/// `detail` (axum's message names it) and pointing at the spec in
/// `suggestion`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

/// JSON-body extractor that answers with a structured error envelope.
///
/// Axum's built-in `Json` rejects a malformed body with a plain-text
/// `422/400` — the SPA's `toApiError` cannot surface it as anything but
/// `UNKNOWN`. This wrapper maps the rejection into the house
/// `{code, message, detail, suggestion}` shape so every endpoint that swaps
/// `Json(x): Json<T>` for `ApiJson(x): ApiJson<T>` keeps the AI-native error
/// contract (issue #907 P2-9).
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<ApiError>);

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|axum::Json(value)| Self(value))
            .map_err(|rejection| {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(ApiError {
                        code: "INVALID_BODY".into(),
                        message: "Request body does not match this endpoint's schema".into(),
                        detail: Some(rejection.body_text()),
                        suggestion: Some("Check the request body against /api/openapi.json".into()),
                    }),
                )
            })
    }
}

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<ApiError>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| Self(value))
            .map_err(|rejection| {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(ApiError {
                        code: "INVALID_QUERY".into(),
                        message: "Query string does not match this endpoint's parameters".into(),
                        detail: Some(rejection.body_text()),
                        suggestion: Some(
                            "Check the query parameters against /api/openapi.json".into(),
                        ),
                    }),
                )
            })
    }
}
