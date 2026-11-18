//! Per-identity sliding-window budget for run creation (issue #213).
//!
//! Expensive spawns must not share the global request limiter's budget with
//! health checks and reads. State lives in the limiter *instance*, not in
//! process-wide statics: the production router installs the process-global
//! instance as a request extension, while router-level tests install their
//! own instance with a fixed budget and an advanceable clock (WEB-18,
//! issue #1142) — no process-global override involved.

use std::collections::HashMap;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Sliding window for the run-creation budget.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);
/// Allowance when `OXO_FLOW_RUNS_RATE_LIMIT` is unset.
pub(crate) const DEFAULT_LIMIT: u32 = 5;

/// Source of monotonic time for the sliding window.
///
/// Production uses the wall clock; tests implement this trait with a clock
/// they can advance, so window refill is provable without sleeping.
pub trait Clock: Send + Sync {
    /// Current monotonic timestamp.
    fn now(&self) -> Instant;
}

/// Production clock: [`Instant::now`].
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Where each check's per-window allowance comes from.
enum BudgetSource {
    /// `OXO_FLOW_RUNS_RATE_LIMIT`, re-read on every check (production).
    Environment,
    /// Fixed allowance baked into the instance (router-level tests only).
    #[cfg(debug_assertions)]
    Fixed(u32),
}

/// A per-identity sliding-window budget for run creation.
pub struct RunRateLimiter {
    budget: BudgetSource,
    window: Duration,
    clock: Arc<dyn Clock>,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RunRateLimiter {
    fn new(budget: BudgetSource, window: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            budget,
            window,
            clock,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// The process-global production limiter: environment budget, wall clock.
    pub(crate) fn production() -> Self {
        Self::new(BudgetSource::Environment, WINDOW, Arc::new(SystemClock))
    }

    /// A limiter with a fixed allowance and injectable clock.
    ///
    /// Router-level tests (WEB-18 / #1142) use this to drive the real
    /// composition with a small budget and an advanceable clock. The seam
    /// is omitted from release builds.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn for_tests(max: u32, clock: Arc<dyn Clock>) -> Self {
        Self::new(BudgetSource::Fixed(max), WINDOW, clock)
    }

    /// The allowance in force for the next check (0 = disabled).
    pub(crate) fn limit(&self) -> u32 {
        match self.budget {
            BudgetSource::Environment => environment_limit(),
            #[cfg(debug_assertions)]
            BudgetSource::Fixed(max) => max,
        }
    }

    /// Record one attempt for `identity`.
    ///
    /// Returns `Err(seconds_until_next_slot)` when the identity already
    /// spent its allowance inside the window.
    pub(crate) fn check(&self, identity: &str) -> Result<(), u64> {
        // The global kill switch (Playwright/tests, documented as disabling
        // the login/API limiters) covers this limiter too — "off means off".
        if crate::rate_limit::rate_limit_disabled() {
            return Ok(());
        }
        let max = self.limit();
        if max == 0 {
            return Ok(()); // disabled
        }
        let now = self.clock.now();
        let mut map = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stamps = map.entry(identity.to_string()).or_default();
        stamps.retain(|t| now.duration_since(*t) < self.window);
        if stamps.len() >= max as usize {
            let oldest = stamps.first().copied().unwrap_or(now);
            let wait = self
                .window
                .saturating_sub(now.duration_since(oldest))
                .as_secs()
                + 1;
            return Err(wait);
        }
        stamps.push(now);
        Ok(())
    }
}

/// `OXO_FLOW_RUNS_RATE_LIMIT` (allowed runs per minute, default 5; `0`
/// disables the limiter entirely).
fn environment_limit() -> u32 {
    #[cfg(debug_assertions)]
    {
        let test_limit = TEST_LIMIT.load(Ordering::Relaxed);
        if test_limit != NO_TEST_OVERRIDE {
            return test_limit as u32;
        }
    }
    std::env::var("OXO_FLOW_RUNS_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LIMIT)
}

#[cfg(debug_assertions)]
const NO_TEST_OVERRIDE: u64 = u64::MAX;
#[cfg(debug_assertions)]
static TEST_LIMIT: AtomicU64 = AtomicU64::new(NO_TEST_OVERRIDE);

/// Override the process-global limiter's budget (debug builds only).
///
/// Integration suites that create many runs share one process-wide window;
/// they zero their budget so unrelated contract coverage does not trip it.
/// The limiter's real router composition is pinned separately by
/// `tests/run_limiter_composition.rs`, which installs a test-scoped
/// instance instead of touching this override.
#[cfg(debug_assertions)]
pub(crate) fn set_limit_for_tests(limit: u32) {
    TEST_LIMIT.store(u64::from(limit), Ordering::Relaxed);
}

/// The process-global production limiter the router installs.
///
/// One instance per process, so every router built in this process (and
/// every handler call) shares the same window — exactly the old static map
/// the limiter used before it became an instance.
pub(crate) fn global() -> Arc<RunRateLimiter> {
    static LIMITER: LazyLock<Arc<RunRateLimiter>> =
        LazyLock::new(|| Arc::new(RunRateLimiter::production()));
    LIMITER.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// An advanceable clock: `base` is real, the offset is ours.
    struct TestClock {
        base: Instant,
        offset_ms: AtomicU64,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset_ms: AtomicU64::new(0),
            }
        }

        fn advance(&self, delta: Duration) {
            self.offset_ms
                .fetch_add(delta.as_millis() as u64, Ordering::SeqCst);
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
        }
    }

    fn limiter(max: u32, clock: Arc<TestClock>) -> RunRateLimiter {
        RunRateLimiter::new(BudgetSource::Fixed(max), WINDOW, clock)
    }

    #[test]
    fn fixed_budget_allows_up_to_limit_then_blocks_with_wait() {
        let clock = Arc::new(TestClock::new());
        let limiter = limiter(3, clock);
        for _ in 0..3 {
            assert!(limiter.check("id").is_ok());
        }
        let wait = limiter.check("id").expect_err("expected budget breach");
        assert!(wait > 0 && wait <= 61, "wait out of window: {wait}");
    }

    #[test]
    fn budgets_are_keyed_per_identity() {
        let clock = Arc::new(TestClock::new());
        let limiter = limiter(1, clock);
        assert!(limiter.check("alice").is_ok());
        assert!(limiter.check("alice").is_err());
        assert!(
            limiter.check("bob").is_ok(),
            "a different identity owns a separate budget"
        );
    }

    #[test]
    fn advancing_the_clock_refills_the_window() {
        let clock = Arc::new(TestClock::new());
        let limiter = limiter(1, clock.clone());
        assert!(limiter.check("id").is_ok());
        assert!(limiter.check("id").is_err());
        clock.advance(WINDOW + Duration::from_secs(1));
        assert!(
            limiter.check("id").is_ok(),
            "timestamps older than the window must be evicted"
        );
    }

    #[test]
    fn zero_budget_disables_the_limiter() {
        let clock = Arc::new(TestClock::new());
        let limiter = limiter(0, clock);
        for _ in 0..10 {
            assert!(limiter.check("id").is_ok());
        }
    }
}
