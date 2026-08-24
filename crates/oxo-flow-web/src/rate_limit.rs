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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::domains::workflow::handlers::ApiError;

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
    /// Number of `check_rate_limit` calls since the last idle-key purge.
    checks_since_purge: Arc<AtomicU64>,
}

/// Purge keys whose timestamps all expired every this many checks, so
/// rotating client keys (NAT pools, scripted probes) cannot grow the map
/// without bound.
const PURGE_EVERY_CHECKS: u64 = 1024;

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            entries: Arc::new(DashMap::new()),
            checks_since_purge: Arc::new(AtomicU64::new(0)),
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

        // Opportunistic eviction: drop keys with no timestamps left in the
        // window. Bounded by the check counter, not a background task, so
        // idle deployments pay nothing.
        if self.checks_since_purge.fetch_add(1, Ordering::Relaxed) >= PURGE_EVERY_CHECKS {
            self.checks_since_purge.store(0, Ordering::Relaxed);
            self.entries
                .retain(|_, timestamps| timestamps.last().is_some_and(|t| *t > window_start));
        }

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

    /// Number of tracked client keys (test/diagnostic introspection).
    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.entries.len()
    }
}

/// Legacy response body returned when the rate limit is exceeded.
///
/// Retained for API compatibility of the `infra::rate_limit` re-export; the
/// middleware itself now answers with the crate-wide structured [`ApiError`]
/// (`{code, message, detail, suggestion}`) contract.
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
            Json(ApiError {
                code: "INTERNAL_ERROR".into(),
                message: "Server misconfiguration: rate limiter unavailable".into(),
                detail: None,
                suggestion: None,
            }),
        )
            .into_response();
    };

    // Derive client key: X-Forwarded-For > X-Real-IP > fallback.
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

    // Explicit test/dev escape hatch (Playwright webServer sets it): the
    // browser e2e suite legitimately exceeds the 100 req/min budget from
    // localhost and was otherwise throttled into 52/63 failures. NOT for
    // production — brute-force protection is the point of this limiter.
    if std::env::var("OXO_FLOW_DISABLE_RATE_LIMIT").as_deref() == Ok("1") {
        return next.run(request).await;
    }

    if let Err(retry_after) = limiter.check_rate_limit(&key) {
        let body = ApiError {
            code: "RATE_LIMITED".into(),
            message: "Rate limit exceeded".into(),
            detail: Some(format!("retry in {retry_after}s")),
            suggestion: Some("Wait for the Retry-After window before retrying".into()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_keys_are_purged_after_threshold() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            max_requests: 100,
            window: Duration::from_millis(1),
        });
        limiter.check_rate_limit("stale-key").unwrap();
        assert_eq!(limiter.tracked_keys(), 1);

        // Let the window expire, then push the check counter past the purge
        // threshold with fresh keys (over-limit denials still count).
        std::thread::sleep(Duration::from_millis(5));
        for _ in 0..PURGE_EVERY_CHECKS {
            let _ = limiter.check_rate_limit("fresh-key");
        }
        let _ = limiter.check_rate_limit("fresh-key");

        assert!(
            !limiter.entries.contains_key("stale-key"),
            "fully-expired keys must be evicted by the opportunistic purge"
        );
        assert!(limiter.entries.contains_key("fresh-key"));
    }

    #[test]
    fn over_limit_reports_retry_after() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
        });
        assert!(limiter.check_rate_limit("k").is_ok());
        assert!(limiter.check_rate_limit("k").is_ok());
        let retry_after = limiter.check_rate_limit("k").unwrap_err();
        assert!(retry_after >= 1);
    }
}
