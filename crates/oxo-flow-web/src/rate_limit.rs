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

/// Whether the deployment sits behind a reverse proxy we control.
///
/// Only then may `X-Forwarded-For` / `X-Real-IP` be believed — they are
/// client-supplied headers, so an untrusted deployment that keys on them
/// hands every caller an unlimited supply of fresh rate-limit buckets.
fn trusted_proxy_enabled() -> bool {
    std::env::var("OXO_FLOW_TRUSTED_PROXY")
        .map(|v| !v.is_empty() && v != "0" && v != "false")
        .unwrap_or(false)
}

/// Derive the rate-limit bucket key for a request.
///
/// With a trusted proxy configured the left-most `X-Forwarded-For` value (or
/// `X-Real-IP`) is the client. Without one the key is the transport peer
/// address from [`axum::extract::ConnectInfo`] — spoofable headers are
/// ignored — falling back to a single shared bucket when the transport does
/// not supply a peer address (in-process routers, tests).
fn client_key(request: &Request, trusted_proxy: bool) -> String {
    if trusted_proxy {
        let forwarded = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                request
                    .headers()
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            });
        if let Some(client) = forwarded {
            return client.to_string();
        }
    }

    request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Axum middleware that enforces per-IP rate limiting.
///
/// The client key is the transport peer address unless
/// `OXO_FLOW_TRUSTED_PROXY` marks the deployment as sitting behind a reverse
/// proxy, in which case the forwarded headers are honoured.  The
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

    let key = client_key(&request, trusted_proxy_enabled());

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

    /// The login limiter must not be bypassable by rotating a spoofed
    /// `X-Forwarded-For` header when no trusted proxy is configured.
    #[test]
    fn forwarded_headers_are_ignored_without_a_trusted_proxy() {
        let mut request = Request::builder()
            .uri("/api/auth/login")
            .body(axum::body::Body::empty())
            .unwrap();
        request.headers_mut().insert(
            "x-forwarded-for",
            HeaderValue::from_static("10.0.0.7, 10.0.0.8"),
        );

        // No trusted proxy and no peer address: one shared bucket, so
        // rotating the header cannot mint new buckets.
        assert_eq!(client_key(&request, false), "unknown");

        // With a peer address the key is the peer, header or not.
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:4242".parse::<std::net::SocketAddr>().unwrap(),
        ));
        assert_eq!(client_key(&request, false), "127.0.0.1");

        // A configured trusted proxy may believe the left-most entry.
        assert_eq!(client_key(&request, true), "10.0.0.7");
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

/// Brute-force guard for `POST /api/auth/login` (issue #907 T5: 23
/// consecutive bad logins answered plain 401s with no throttle at all).
///
/// Separate from the global per-IP [`RateLimiter`]: that one allows 100
/// requests/min per IP, far more than a credential-stuffing run needs, and
/// it cannot see the username (the middleware runs before the body is
/// parsed). This limiter is keyed **per account + per IP** inside the login
/// handler, so an attacker hammering one account is throttled even when
/// rotating source IPs is impossible to detect, and a victim account is not
/// locked out by other users' traffic.
#[derive(Debug, Clone)]
pub struct LoginRateLimiter {
    /// Failed-attempt timestamps per bucket key.
    attempts: Arc<DashMap<String, Vec<Instant>>>,
    /// Number of lockout checks since the last idle-bucket purge.
    checks_since_purge: Arc<AtomicU64>,
    max_attempts: u32,
    window: Duration,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginRateLimiter {
    /// Window and attempt budget: 10 failed attempts per 5 minutes per
    /// (account, IP) bucket — well above any human's typo rate, far below
    /// an online guessing run (~50k/5min).
    const MAX_ATTEMPTS: u32 = 10;
    const WINDOW: Duration = Duration::from_secs(300);

    pub fn new() -> Self {
        Self {
            attempts: Arc::new(DashMap::new()),
            checks_since_purge: Arc::new(AtomicU64::new(0)),
            max_attempts: Self::MAX_ATTEMPTS,
            window: Self::WINDOW,
        }
    }

    /// Constructor with explicit tuning (tests).
    #[cfg(test)]
    fn with_config(max_attempts: u32, window: Duration) -> Self {
        Self {
            attempts: Arc::new(DashMap::new()),
            checks_since_purge: Arc::new(AtomicU64::new(0)),
            max_attempts,
            window,
        }
    }

    /// Record a failed login for `bucket` and report whether further
    /// attempts are now blocked (`Some(retry_after_secs)`).
    fn record_failure(&self, bucket: &str) -> Option<u64> {
        let now = Instant::now();
        let window_start = now - self.window;
        let mut timestamps = self.attempts.entry(bucket.to_owned()).or_default();
        timestamps.retain(|t| *t > window_start);
        timestamps.push(now);
        if timestamps.len() as u32 >= self.max_attempts {
            let retry_after = timestamps
                .first()
                .map(|t| self.window.saturating_sub(now.duration_since(*t)).as_secs() + 1)
                .unwrap_or(1);
            Some(retry_after)
        } else {
            None
        }
    }

    /// Whether `bucket` is currently locked out, and for how long.
    fn is_blocked(&self, bucket: &str) -> Option<u64> {
        let now = Instant::now();
        let window_start = now - self.window;
        let mut timestamps = self.attempts.entry(bucket.to_owned()).or_default();
        timestamps.retain(|t| *t > window_start);
        if (timestamps.len() as u32) < self.max_attempts {
            return None;
        }
        Some(
            timestamps
                .first()
                .map(|t| self.window.saturating_sub(now.duration_since(*t)).as_secs() + 1)
                .unwrap_or(1),
        )
    }

    /// Drop the failure history for `bucket` (successful login).
    fn clear(&self, bucket: &str) {
        self.attempts.remove(bucket);
    }

    /// Opportunistic eviction of fully-expired buckets, mirroring the global
    /// limiter's purge: attacker-controlled usernames would otherwise grow
    /// the map one key per sprayed name, forever.
    fn maybe_purge(&self) {
        if self.checks_since_purge.fetch_add(1, Ordering::Relaxed) >= PURGE_EVERY_CHECKS {
            self.checks_since_purge.store(0, Ordering::Relaxed);
            let window_start = Instant::now() - self.window;
            self.attempts
                .retain(|_, timestamps| timestamps.last().is_some_and(|t| *t > window_start));
        }
    }
}

/// The process-wide login limiter. A single static instance is deliberate:
/// the throttle must be shared by every request the server answers,
/// including concurrent ones.
fn login_limiter() -> &'static LoginRateLimiter {
    static LIMITER: std::sync::OnceLock<LoginRateLimiter> = std::sync::OnceLock::new();
    LIMITER.get_or_init(LoginRateLimiter::new)
}

/// Longest username prefix kept in a bucket key. The check runs before the
/// username is validated, so the raw body text becomes a map key — a giant
/// string must not be retained verbatim.
const LOGIN_BUCKET_PREFIX_CHARS: usize = 128;

fn login_bucket(account: &str, ip: &str) -> String {
    let account: String = account.chars().take(LOGIN_BUCKET_PREFIX_CHARS).collect();
    format!("login:{account}@{ip}")
}

fn rate_limit_disabled() -> bool {
    std::env::var("OXO_FLOW_DISABLE_RATE_LIMIT").as_deref() == Ok("1")
}

/// Throttle check for one login attempt. Returns `Err(retry_after_secs)` if
/// the (account, ip) bucket is locked out. Successful logins clear the
/// account's failure history; failures are recorded by the handler.
///
/// The global `OXO_FLOW_DISABLE_RATE_LIMIT=1` escape hatch (Playwright)
/// disables this too — brute-force protection off means off.
pub fn check_login_attempt(account: &str, ip: &str) -> Result<(), u64> {
    if rate_limit_disabled() {
        return Ok(());
    }
    let limiter = login_limiter();
    limiter.maybe_purge();
    limiter
        .is_blocked(&login_bucket(account, ip))
        .map_or(Ok(()), Err)
}

/// Record a failed login against the (account, ip) bucket.
pub fn record_login_failure(account: &str, ip: &str) {
    if rate_limit_disabled() {
        return;
    }
    let bucket = login_bucket(account, ip);
    if let Some(retry_after) = login_limiter().record_failure(&bucket) {
        tracing::warn!(
            "login lockout engaged for account '{}' from {ip}: retry in {retry_after}s",
            account
                .chars()
                .take(LOGIN_BUCKET_PREFIX_CHARS)
                .collect::<String>()
        );
    }
}

/// Clear the (account, ip) failure history after a successful login.
pub fn clear_login_failures(account: &str, ip: &str) {
    login_limiter().clear(&login_bucket(account, ip));
}

#[cfg(test)]
mod login_tests {
    use super::*;

    #[test]
    fn lockout_after_budget_of_failures() {
        let limiter = LoginRateLimiter::with_config(3, Duration::from_secs(60));
        assert!(limiter.record_failure("a@ip").is_none());
        assert!(limiter.record_failure("a@ip").is_none());
        assert!(
            limiter.record_failure("a@ip").is_some(),
            "3rd failure locks"
        );
        assert!(limiter.is_blocked("a@ip").is_some());
        // A different bucket is unaffected.
        assert!(limiter.is_blocked("a@other").is_none());
    }

    #[test]
    fn success_clears_history() {
        let limiter = LoginRateLimiter::with_config(2, Duration::from_secs(60));
        let _ = limiter.record_failure("u@ip");
        limiter.clear("u@ip");
        assert!(limiter.is_blocked("u@ip").is_none());
    }

    #[test]
    fn failed_attempts_expire_with_the_window() {
        let limiter = LoginRateLimiter::with_config(1, Duration::from_millis(5));
        let _ = limiter.record_failure("u@ip");
        assert!(limiter.is_blocked("u@ip").is_some());
        std::thread::sleep(Duration::from_millis(10));
        assert!(limiter.is_blocked("u@ip").is_none());
    }
}
