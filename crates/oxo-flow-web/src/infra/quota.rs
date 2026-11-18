//! Durable, atomic resource quota enforcement for team-mode deployments.
//!
//! Per-user limits on concurrent runs, total CPU threads, and total memory.
//! Quotas are hard limits — exceeding one rejects the spawn with
//! `429 QUOTA_EXCEEDED`.
//!
//! # Durable authority (RB-09 / issue #1133)
//!
//! Usage lives in the unified [`StorageBackend`] (`quota_usage` /
//! `quota_reservations` tables), not in a process-local map: counters survive
//! restart and are shared by every server instance pointed at the same
//! database. Admission is ONE storage operation — the limits check and the
//! resource reservation commit (or fail) together — so two concurrent
//! requests can never both pass the check and both be recorded, and a
//! reservation that is never released is reclaimed by the startup
//! reconciliation ([`reconcile_stale`]).
//!
//! [`QuotaTracker`] remains as an in-process fallback (a server running with
//! no storage backend) and as the cache of the limits this process last
//! loaded; when a backend is present the durable row is authoritative.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::infra::db::StorageBackend;

/// Length of the `max_runs_per_day` window.
pub const QUOTA_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// Per-user resource quota configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QuotaConfig {
    /// Maximum concurrent runs per user.
    pub max_concurrent_runs: u32,
    /// Maximum total CPU threads across all active runs.
    pub max_total_threads: u32,
    /// Maximum total memory (MB) across all active runs.
    pub max_total_memory_mb: u64,
    /// Maximum runs per day (24h rolling window).
    pub max_runs_per_day: u32,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 10,
            max_total_threads: 64,
            max_total_memory_mb: 262144, // 256 GB
            max_runs_per_day: 100,
        }
    }
}

/// Current resource usage for a user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaUsage {
    /// Number of currently running workflows.
    pub active_runs: u32,
    /// Total CPU threads used by active runs.
    pub used_threads: u32,
    /// Total memory (MB) used by active runs.
    pub used_memory_mb: u64,
    /// Number of runs started in the current 24h window.
    pub runs_today: u32,
}

/// Result of a quota check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaCheckResult {
    /// Whether all quotas are within limits.
    pub allowed: bool,
    /// Per-limit violation messages (empty if allowed).
    pub violations: Vec<String>,
    /// Current usage snapshot.
    pub usage: QuotaUsage,
    /// Configured limits.
    pub limits: QuotaConfig,
}

/// Evaluate the configured limits against `usage` plus the requested
/// resources. Shared by the in-memory tracker and the durable read-after-
/// rejection path so both describe a breach with the same wording.
fn evaluate(usage: &QuotaUsage, limits: &QuotaConfig, threads: u32, memory_mb: u64) -> Vec<String> {
    let mut violations = Vec::new();
    if usage.active_runs >= limits.max_concurrent_runs {
        violations.push(format!(
            "Concurrent run limit reached: {}/{}",
            usage.active_runs, limits.max_concurrent_runs
        ));
    }
    if usage.used_threads.saturating_add(threads) > limits.max_total_threads {
        violations.push(format!(
            "Thread limit would be exceeded: {}+{} > {}",
            usage.used_threads, threads, limits.max_total_threads
        ));
    }
    if usage.used_memory_mb.saturating_add(memory_mb) > limits.max_total_memory_mb {
        violations.push(format!(
            "Memory limit would be exceeded: {}+{} MB > {} MB",
            usage.used_memory_mb, memory_mb, limits.max_total_memory_mb
        ));
    }
    if usage.runs_today >= limits.max_runs_per_day {
        violations.push(format!(
            "Daily run limit reached: {}/{}",
            usage.runs_today, limits.max_runs_per_day
        ));
    }
    violations
}

/// In-memory quota tracker — the fallback used only when no storage backend
/// is configured, plus the process-local cache of the active limits.
pub struct QuotaTracker {
    config: std::sync::RwLock<QuotaConfig>,
    /// Per-user usage counters.
    usage: Mutex<HashMap<String, QuotaUsage>>,
}

impl QuotaTracker {
    /// Create a new quota tracker with the given config.
    pub fn new(config: QuotaConfig) -> Self {
        Self {
            config: std::sync::RwLock::new(config),
            usage: Mutex::new(HashMap::new()),
        }
    }

    /// Update the configured limits at runtime (admin only).
    pub fn update_config(&self, config: QuotaConfig) {
        if let Ok(mut c) = self.config.write() {
            *c = config;
        }
    }

    /// Check whether a user can start a new run with the given resources.
    ///
    /// Read-only: production admission goes through [`admit`], which commits
    /// the check and the reservation together.
    pub fn check(&self, user_id: &str, threads: u32, memory_mb: u64) -> QuotaCheckResult {
        let config = self.config();
        let usage_map = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let usage = usage_map.get(user_id).cloned().unwrap_or_default();
        let violations = evaluate(&usage, &config, threads, memory_mb);
        QuotaCheckResult {
            allowed: violations.is_empty(),
            violations,
            usage,
            limits: config,
        }
    }

    /// Atomically check the limits and record the start in one lock scope —
    /// the no-backend fallback for the durable admission path. A rejected
    /// request leaves every counter untouched.
    pub fn try_admit(&self, user_id: &str, threads: u32, memory_mb: u64) -> QuotaCheckResult {
        let config = self.config();
        let mut usage_map = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let usage = usage_map.entry(user_id.to_string()).or_default();
        let violations = evaluate(usage, &config, threads, memory_mb);
        if violations.is_empty() {
            usage.active_runs = usage.active_runs.saturating_add(1);
            usage.used_threads = usage.used_threads.saturating_add(threads);
            usage.used_memory_mb = usage.used_memory_mb.saturating_add(memory_mb);
            usage.runs_today = usage.runs_today.saturating_add(1);
        }
        QuotaCheckResult {
            allowed: violations.is_empty(),
            violations,
            usage: usage.clone(),
            limits: config,
        }
    }

    /// Record a run completing (decrement counters).
    pub fn record_complete(&self, user_id: &str, threads: u32, memory_mb: u64) {
        let mut usage_map = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(usage) = usage_map.get_mut(user_id) {
            usage.active_runs = usage.active_runs.saturating_sub(1);
            usage.used_threads = usage.used_threads.saturating_sub(threads);
            usage.used_memory_mb = usage.used_memory_mb.saturating_sub(memory_mb);
        }
    }

    /// Get current usage for a user.
    pub fn get_usage(&self, user_id: &str) -> QuotaUsage {
        let usage_map = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        usage_map.get(user_id).cloned().unwrap_or_default()
    }

    /// Reset daily counters (fallback path only — the durable read applies
    /// the rolling window itself, so a restarted server never needs this).
    pub fn reset_daily(&self) {
        let mut usage_map = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for usage in usage_map.values_mut() {
            usage.runs_today = 0;
        }
    }

    /// Get the configured limits.
    pub fn config(&self) -> QuotaConfig {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Global quota tracker singleton (fallback path + limits cache).
static QUOTA_TRACKER: std::sync::LazyLock<QuotaTracker> =
    std::sync::LazyLock::new(|| QuotaTracker::new(QuotaConfig::default()));

/// A fallback reservation: (user_id, threads, memory_mb).
type Reservation = (String, u32, u64);

/// Per-run reservations for the NO-BACKEND fallback path. With a storage
/// backend, reservations live in the `quota_reservations` table instead
/// (durable and shared); this map never holds production state.
static RUN_RESERVATIONS: std::sync::LazyLock<Mutex<HashMap<String, Reservation>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn take_fallback_reservation(run_id: &str) -> Option<Reservation> {
    RUN_RESERVATIONS
        .lock()
        .ok()
        .and_then(|mut map| map.remove(run_id))
}

/// Milliseconds since the Unix epoch — the quota window arithmetic runs on
/// integers so no timestamp string formatting can perturb the comparison.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Get a reference to the global quota tracker.
pub fn global_quota_tracker() -> &'static QuotaTracker {
    &QUOTA_TRACKER
}

/// Effective quota limits: the durable row when one exists (the authority
/// across instances), otherwise the process-local configuration (the
/// default, or the last admin update on a backend-less server).
pub async fn limits(backend: Option<&dyn StorageBackend>) -> Result<QuotaConfig, String> {
    match backend {
        Some(backend) => Ok(backend
            .load_quota_config()
            .await?
            .unwrap_or_else(|| global_quota_tracker().config())),
        None => Ok(global_quota_tracker().config()),
    }
}

/// Persist new limits — durable when a backend is present, process-local
/// otherwise — and refresh the local cache.
pub async fn save_limits(
    backend: Option<&dyn StorageBackend>,
    config: QuotaConfig,
) -> Result<(), String> {
    if let Some(backend) = backend {
        backend.save_quota_config(&config).await?;
    }
    global_quota_tracker().update_config(config);
    Ok(())
}

/// Atomically admit a new run: check the limits and record its resource
/// reservation in one storage operation.
///
/// On a backend this is a single-statement compare-and-swap plus the
/// reservation insert in one transaction — concurrent requests cannot both
/// pass, and a crash between check and record is impossible. Without a
/// backend it falls back to the in-process tracker's single-lock admission.
pub async fn admit(
    backend: Option<&dyn StorageBackend>,
    run_id: &str,
    user_id: &str,
    threads: u32,
    memory_mb: u64,
) -> Result<QuotaCheckResult, String> {
    match backend {
        Some(backend) => {
            let limits = limits(Some(backend)).await?;
            let (allowed, usage) = backend
                .quota_admit(run_id, user_id, threads, memory_mb, &limits, now_ms())
                .await?;
            let violations = if allowed {
                Vec::new()
            } else {
                evaluate(&usage, &limits, threads, memory_mb)
            };
            Ok(QuotaCheckResult {
                allowed,
                violations,
                usage,
                limits,
            })
        }
        None => {
            let result = global_quota_tracker().try_admit(user_id, threads, memory_mb);
            if result.allowed
                && let Ok(mut map) = RUN_RESERVATIONS.lock()
            {
                map.insert(
                    run_id.to_string(),
                    (user_id.to_string(), threads, memory_mb),
                );
            }
            Ok(result)
        }
    }
}

/// Release a run's reservation (idempotent — a second release is a no-op).
pub async fn release(backend: Option<&dyn StorageBackend>, run_id: &str) -> Result<(), String> {
    match backend {
        Some(backend) => backend.quota_release(run_id).await.map(|_| ()),
        None => {
            if let Some((user_id, threads, memory_mb)) = take_fallback_reservation(run_id) {
                global_quota_tracker().record_complete(&user_id, threads, memory_mb);
            }
            Ok(())
        }
    }
}

/// Release a run's reservation through the active storage backend.
///
/// Every terminal path needs exactly this call: the durable release is
/// idempotent, so cancel-vs-failure races cannot double-decrement.
pub async fn release_and_complete(run_id: &str) {
    let backend = crate::infra::db::global_backend();
    if let Err(e) = release(backend.as_deref(), run_id).await {
        tracing::warn!("Failed to release quota reservation for run {run_id}: {e}");
    }
}

/// A user's current usage (durable when a backend is present).
pub async fn usage(
    backend: Option<&dyn StorageBackend>,
    user_id: &str,
) -> Result<QuotaUsage, String> {
    match backend {
        Some(backend) => backend.quota_usage(user_id, now_ms()).await,
        None => Ok(global_quota_tracker().get_usage(user_id)),
    }
}

/// Release reservations whose run row is terminal or missing — done once at
/// startup so a crash between admission and the run row insert (or between a
/// terminal write and its release) cannot pin quota forever.
pub async fn reconcile_stale(backend: &dyn StorageBackend) -> Result<usize, String> {
    let stale = backend.quota_stale_reservations().await?;
    let mut released = 0usize;
    for run_id in stale {
        if backend.quota_release(&run_id).await? {
            released += 1;
        }
    }
    Ok(released)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_allowed() {
        let tracker = QuotaTracker::new(QuotaConfig::default());
        let result = tracker.check("user1", 4, 8192);
        assert!(result.allowed);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn test_quota_concurrent_limit() {
        let config = QuotaConfig {
            max_concurrent_runs: 1,
            ..QuotaConfig::default()
        };
        let tracker = QuotaTracker::new(config);
        assert!(tracker.try_admit("user1", 1, 1024).allowed);
        let result = tracker.try_admit("user1", 1, 1024);
        assert!(!result.allowed);
        assert!(result.violations.iter().any(|v| v.contains("Concurrent")));
    }

    #[test]
    fn try_admit_is_all_or_nothing() {
        let config = QuotaConfig {
            max_concurrent_runs: 10,
            max_total_threads: 8,
            ..QuotaConfig::default()
        };
        let tracker = QuotaTracker::new(config);
        // Over the thread limit: the rejected request must not bump ANY
        // counter (the pre-RB-09 separate check/record pair did).
        let rejected = tracker.try_admit("user1", 9, 1024);
        assert!(!rejected.allowed);
        let usage = tracker.get_usage("user1");
        assert_eq!(usage.active_runs, 0);
        assert_eq!(usage.used_threads, 0);
        assert_eq!(usage.runs_today, 0);
        // A fitting request starts from a clean slate.
        assert!(tracker.try_admit("user1", 8, 1024).allowed);
        assert_eq!(tracker.get_usage("user1").used_threads, 8);
    }

    #[test]
    fn test_quota_record_and_release() {
        let tracker = QuotaTracker::new(QuotaConfig::default());
        assert!(tracker.try_admit("user1", 4, 8192).allowed);
        let usage = tracker.get_usage("user1");
        assert_eq!(usage.active_runs, 1);
        assert_eq!(usage.used_threads, 4);
        tracker.record_complete("user1", 4, 8192);
        let usage = tracker.get_usage("user1");
        assert_eq!(usage.active_runs, 0);
        assert_eq!(usage.used_threads, 0);
    }

    #[test]
    fn test_quota_reset_daily() {
        let tracker = QuotaTracker::new(QuotaConfig::default());
        assert!(tracker.try_admit("user1", 1, 1024).allowed);
        assert_eq!(tracker.get_usage("user1").runs_today, 1);
        tracker.reset_daily();
        assert_eq!(tracker.get_usage("user1").runs_today, 0);
    }

    #[test]
    fn test_default_config() {
        let config = QuotaConfig::default();
        assert_eq!(config.max_concurrent_runs, 10);
        assert_eq!(config.max_total_threads, 64);
        assert_eq!(config.max_total_memory_mb, 262144);
        assert_eq!(config.max_runs_per_day, 100);
    }
}
