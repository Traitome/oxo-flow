//! Rate limiting middleware for oxo-flow-web.
//!
//! Provides per-IP rate limiting using a sliding window algorithm.

use axum::{
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use axum::extract::Request;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration for the in-memory rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum number of requests allowed within the window.
    pub max_requests: u64,
    /// Sliding window duration.
    pub window: Duration,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window: Duration::from_secs(60),
        }
    }
}

/// Simple in-memory rate limiter that tracks request timestamps per key (IP).
#[derive(Debug, Clone)]
pub struct RateLimiter {
    config: RateLimiterConfig,
    /// Maps a client key to a list of request timestamps within the current window.
    entries: Arc<DashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Check whether a request from `key` is allowed.
    ///
    /// Returns `Ok(())` when the request is within the limit, or
    /// `Err(remaining_secs)` with the number of seconds until the oldest
    /// entry expires when the limit is exceeded.
    pub fn check_rate_limit(&self, key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let window_start = now - self.config.window;

        let mut timestamps = self.entries.entry(key.to_owned()).or_default();

        // Evict timestamps outside the sliding window.
        timestamps.retain(|t| *t > window_start);

        if timestamps.len() as u64 >= self.config.max_requests {
            let retry_after = timestamps
                .first()
                .map(|t| {
                    self.config
                        .window
                        .saturating_sub(now.duration_since(*t))
                        .as_secs()
                        + 1
                })
                .unwrap_or(1);
            return Err(retry_after);
        }

        timestamps.push(now);
        Ok(())
    }
}

/// Response returned when the rate limit is exceeded.
#[derive(Serialize, Deserialize)]
pub struct RateLimitResponse {
    pub error: String,
    pub retry_after_secs: u64,
}

/// Axum middleware that enforces per-IP rate limiting.
///
/// The IP is extracted from the `X-Forwarded-For` header (for reverse-proxy
/// deployments), then `X-Real-IP`, then falls back to a fixed key.  The
/// [`RateLimiter`] instance must be available via request extensions.
pub async fn rate_limit_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    use axum::Json;

    // A missing limiter is a wiring bug (layer order), not a license to skip
    // rate limiting silently — fail loud and closed (issue #79 P1-04).
    // Note the Arc: the Extension layer inserts an `Arc<RateLimiter>`, and
    // request extensions are typed — a lookup by the bare type never matches
    // (the second half of the original bug, alongside the layer order).
    let limiter = request
        .extensions()
        .get::<std::sync::Arc<RateLimiter>>()
        .cloned();
    let Some(limiter) = limiter else {
        tracing::error!(
            "RateLimiter missing from request extensions — the rate-limit \
             middleware must sit inside the Extension layer in server.rs"
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RateLimitResponse {
                error: "Server misconfiguration: rate limiter unavailable".to_string(),
                retry_after_secs: 0,
            }),
        )
            .into_response();
    };

    // Derive client key: X-Forwarded-For > X-Real-IP > fallback
    let key = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            request
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    if let Err(retry_after) = limiter.check_rate_limit(&key) {
        let body = RateLimitResponse {
            error: "Rate limit exceeded".to_string(),
            retry_after_secs: retry_after,
        };
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("60")),
            )],
            Json(body),
        )
            .into_response();
    }

    next.run(request).await
}
