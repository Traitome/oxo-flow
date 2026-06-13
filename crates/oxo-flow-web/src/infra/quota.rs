//! Resource quota enforcement for team-mode deployments.
//!
//! Per-user limits on concurrent runs, total CPU threads, and total memory.
//! Quotas are soft limits — exceeding them returns a warning rather than
//! rejecting the request (admins can override).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-user resource quota configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// In-memory quota tracker.
pub struct QuotaTracker {
    config: QuotaConfig,
    /// Per-user usage counters.
    usage: Mutex<HashMap<String, QuotaUsage>>,
}

impl QuotaTracker {
    /// Create a new quota tracker with the given config.
    pub fn new(config: QuotaConfig) -> Self {
        Self { config, usage: Mutex::new(HashMap::new()) }
    }

    /// Check whether a user can start a new run with the given resources.
    pub fn check(&self, user_id: &str, threads: u32, memory_mb: u64) -> QuotaCheckResult {
        let mut usage_map = self.usage.lock().unwrap();
        let usage = usage_map.entry(user_id.to_string()).or_default();
        let mut violations = Vec::new();

        if usage.active_runs >= self.config.max_concurrent_runs {
            violations.push(format!(
                "Concurrent run limit reached: {}/{}",
                usage.active_runs, self.config.max_concurrent_runs
            ));
        }
        if usage.used_threads + threads > self.config.max_total_threads {
            violations.push(format!(
                "Thread limit would be exceeded: {}+{} > {}",
                usage.used_threads, threads, self.config.max_total_threads
            ));
        }
        if usage.used_memory_mb + memory_mb > self.config.max_total_memory_mb {
            violations.push(format!(
                "Memory limit would be exceeded: {}+{} MB > {} MB",
                usage.used_memory_mb, memory_mb, self.config.max_total_memory_mb
            ));
        }
        if usage.runs_today >= self.config.max_runs_per_day {
            violations.push(format!(
                "Daily run limit reached: {}/{}",
                usage.runs_today, self.config.max_runs_per_day
            ));
        }

        QuotaCheckResult {
            allowed: violations.is_empty(),
            violations,
            usage: usage.clone(),
            limits: self.config.clone(),
        }
    }

    /// Record a new run starting (increment counters).
    pub fn record_start(&self, user_id: &str, threads: u32, memory_mb: u64) {
        let mut usage_map = self.usage.lock().unwrap();
        let usage = usage_map.entry(user_id.to_string()).or_default();
        usage.active_runs += 1;
        usage.used_threads += threads;
        usage.used_memory_mb += memory_mb;
        usage.runs_today += 1;
    }

    /// Record a run completing (decrement counters).
    pub fn record_complete(&self, user_id: &str, threads: u32, memory_mb: u64) {
        let mut usage_map = self.usage.lock().unwrap();
        if let Some(usage) = usage_map.get_mut(user_id) {
            usage.active_runs = usage.active_runs.saturating_sub(1);
            usage.used_threads = usage.used_threads.saturating_sub(threads);
            usage.used_memory_mb = usage.used_memory_mb.saturating_sub(memory_mb);
        }
    }

    /// Get current usage for a user.
    pub fn get_usage(&self, user_id: &str) -> QuotaUsage {
        let usage_map = self.usage.lock().unwrap();
        usage_map.get(user_id).cloned().unwrap_or_default()
    }

    /// Reset daily counters (call at midnight or on demand).
    pub fn reset_daily(&self) {
        let mut usage_map = self.usage.lock().unwrap();
        for usage in usage_map.values_mut() {
            usage.runs_today = 0;
        }
    }

    /// Get the configured limits.
    pub fn config(&self) -> &QuotaConfig {
        &self.config
    }
}

/// Global quota tracker singleton.
static QUOTA_TRACKER: std::sync::LazyLock<QuotaTracker> =
    std::sync::LazyLock::new(|| QuotaTracker::new(QuotaConfig::default()));

/// Get a reference to the global quota tracker.
pub fn global_quota_tracker() -> &'static QuotaTracker {
    &QUOTA_TRACKER
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
        let mut config = QuotaConfig::default();
        config.max_concurrent_runs = 1;
        let tracker = QuotaTracker::new(config);
        tracker.record_start("user1", 1, 1024);
        let result = tracker.check("user1", 1, 1024);
        assert!(!result.allowed);
        assert!(result.violations.iter().any(|v| v.contains("Concurrent")));
    }

    #[test]
    fn test_quota_record_and_release() {
        let tracker = QuotaTracker::new(QuotaConfig::default());
        tracker.record_start("user1", 4, 8192);
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
        tracker.record_start("user1", 1, 1024);
        assert_eq!(tracker.get_usage("user1").runs_today, 1);
        tracker.reset_daily();
        assert_eq!(tracker.get_usage("user1").runs_today, 0);
    }

    #[test]
    fn test_default_config() {
        let config = QuotaConfig::default();
        assert_eq!(config.max_concurrent_runs, 10);
        assert_eq!(config.max_total_threads, 64);
        assert_eq!(config.max_runs_per_day, 100);
    }
}
