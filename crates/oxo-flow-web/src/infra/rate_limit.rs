//! Rate limiting middleware for oxo-flow-web.
//!
//! Re-exports from the canonical rate limiting implementation in `crate::rate_limit`.
//! Provides per-IP rate limiting using a sliding window algorithm,
//! configurable via environment variables or server configuration.

// Re-export the canonical implementations
pub use crate::rate_limit::RateLimitResponse;
pub use crate::rate_limit::RateLimiter;
pub use crate::rate_limit::RateLimiterConfig;
pub use crate::rate_limit::rate_limit_middleware;
