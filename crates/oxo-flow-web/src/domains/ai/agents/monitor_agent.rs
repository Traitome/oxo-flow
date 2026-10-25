//! Monitor Agent — watches pipeline execution, predicts failures, generates alerts.
//!
//! Resource monitoring: CPU/Mem/Disk trends, OOM prediction, timeout prediction
//! Progress tracking: Done/pending/failed, ETA computation
//! Log analysis: stderr real-time, known error pattern matching
//! Decision engine: info -> warn -> alert -> critical with escalation

use super::types::*;
use crate::domains::execution::diagnostics::{DiagnosticsEngine, ErrorCategory};
use chrono::Utc;

/// Execution status of a single rule/node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeExecutionStatus {
    pub rule: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
    /// Bounded stderr tail of a failed rule (checkpoint `rule_runs`) — lets
    /// the monitor classify the failure via the shared DiagnosticsEngine
    /// instead of guessing from the exit code alone (issue #907 P1-3).
    #[serde(default)]
    pub stderr_tail: Option<String>,
    /// Bounded stdout tail of a failed rule — some tools print their root
    /// cause on stdout (issue #765); analyzed alongside stderr.
    #[serde(default)]
    pub stdout_tail: Option<String>,
}

/// Current resource usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResourceUsage {
    pub cpu_pct: f64,
    pub memory_pct: f64,
    pub memory_mb: f64,
    pub memory_total_mb: f64,
    pub disk_pct: f64,
    pub disk_mb: f64,
}

impl Default for ResourceUsage {
    fn default() -> Self {
        Self {
            cpu_pct: 50.0,
            memory_pct: 60.0,
            memory_mb: 12000.0,
            memory_total_mb: 16000.0,
            disk_pct: 45.0,
            disk_mb: 500000.0,
        }
    }
}

/// Analyze current execution status and resource usage.
pub fn analyze_run_status(
    nodes: &[NodeExecutionStatus],
    resources: &ResourceUsage,
) -> MonitorStatus {
    let mut alerts = Vec::new();
    let now = Utc::now().to_rfc3339();

    // Resource checks
    if resources.memory_pct > 80.0 {
        let level = if resources.memory_pct > 95.0 {
            AlertLevel::Critical
        } else if resources.memory_pct > 90.0 {
            AlertLevel::Alert
        } else {
            AlertLevel::Warn
        };
        alerts.push(MonitorAlert {
            alert_id: "resource:memory".into(),
            level: level.clone(),
            rule_name: None,
            prediction: format!("Memory at {:.1}% -- risk of OOM", resources.memory_pct),
            suggestion: if resources.memory_pct > 95.0 {
                "Auto-pausing. Reduce parallel jobs or increase memory limit.".into()
            } else if resources.memory_pct > 90.0 {
                "Consider reducing parallel jobs or increasing swap.".into()
            } else {
                "Monitor memory -- consider adjusting resource allocation.".into()
            },
            auto_fixable: level == AlertLevel::Critical,
            needs_approval: level == AlertLevel::Critical,
            timestamp: now.clone(),
        });
    }

    if resources.disk_pct > 85.0 {
        let level = if resources.disk_pct > 95.0 {
            AlertLevel::Critical
        } else {
            AlertLevel::Warn
        };
        alerts.push(MonitorAlert {
            alert_id: "resource:disk".into(),
            level: level.clone(),
            rule_name: None,
            prediction: format!("Disk at {:.1}% -- may run out of space", resources.disk_pct),
            suggestion: "Clean intermediate files or increase disk quota.".into(),
            auto_fixable: false,
            needs_approval: level == AlertLevel::Critical,
            timestamp: now.clone(),
        });
    }

    // Slow rule detection
    let durations: Vec<f64> = nodes
        .iter()
        .filter_map(|n| n.duration_ms)
        .filter(|&d| d > 0)
        .map(|d| d as f64)
        .collect();
    let avg = if durations.is_empty() {
        0.0
    } else {
        durations.iter().sum::<f64>() / durations.len() as f64
    };

    for node in nodes {
        if node.status == "running"
            && let Some(dur) = node.duration_ms
            && avg > 0.0
            && dur as f64 > avg * 3.0
        {
            alerts.push(MonitorAlert {
                alert_id: format!("slow:{}", node.rule),
                level: AlertLevel::Warn,
                rule_name: Some(node.rule.clone()),
                prediction: format!(
                    "Rule '{}' is running {:.1}x slower than average",
                    node.rule,
                    dur as f64 / avg
                ),
                suggestion: "Check for resource contention or I/O bottleneck.".into(),
                auto_fixable: false,
                needs_approval: false,
                timestamp: now.clone(),
            });
        }

        if node.status == "failed" {
            let (prediction, suggestion, auto_fixable, level, pattern_id) = classify_failure(node);
            alerts.push(MonitorAlert {
                alert_id: format!("failed:{}:{}", node.rule, pattern_id),
                level,
                rule_name: Some(node.rule.clone()),
                prediction,
                suggestion,
                auto_fixable,
                needs_approval: true,
                timestamp: now.clone(),
            });
        }
    }

    // ETA computation. Only nodes still to do (running + pending) count as
    // remaining — failed/skipped nodes are not pending work, so a
    // terminal-failed run reports no ETA instead of a permanently positive
    // one (review finding on #908).
    let completed: Vec<&NodeExecutionStatus> =
        nodes.iter().filter(|n| n.status == "success").collect();
    let remaining_count = nodes
        .iter()
        .filter(|n| n.status == "running" || n.status == "pending")
        .count();
    let estimated_completion = if !completed.is_empty() && remaining_count > 0 {
        let total_done: i64 = completed.iter().filter_map(|n| n.duration_ms).sum();
        let avg_per_step = total_done as f64 / completed.len() as f64;
        let eta_secs = (avg_per_step * remaining_count as f64 / 1000.0) as u64;
        Some(format!("~{}m {}s remaining", eta_secs / 60, eta_secs % 60))
    } else if !nodes.is_empty() && completed.len() == nodes.len() {
        Some("Complete".into())
    } else {
        None
    };

    // Overall status
    let overall = if alerts.iter().any(|a| a.level == AlertLevel::Critical) {
        "critical"
    } else if alerts.iter().any(|a| a.level == AlertLevel::Alert) {
        "alert"
    } else if alerts.iter().any(|a| a.level == AlertLevel::Warn) {
        "warning"
    } else {
        "normal"
    };

    MonitorStatus {
        overall: overall.into(),
        alerts,
        resource_forecast: ResourceForecast {
            cpu_trend: if resources.cpu_pct > 80.0 {
                "high".into()
            } else {
                "normal".into()
            },
            memory_trend: if resources.memory_pct > 80.0 {
                format!("rising ({:.1}%)", resources.memory_pct)
            } else {
                "stable".into()
            },
            disk_trend: if resources.disk_pct > 85.0 {
                format!("critical ({:.1}%)", resources.disk_pct)
            } else {
                "normal".into()
            },
            oom_risk: (resources.memory_pct / 100.0).min(1.0),
            timeout_risk: if avg > 0.0 {
                (1.0 - (completed.len() as f64 / nodes.len() as f64)).min(1.0)
            } else {
                0.0
            },
        },
        estimated_completion,
    }
}

/// Classify a failed node into an alert. Prefers the shared
/// [`DiagnosticsEngine`] pattern table run over the persisted stderr/stdout
/// tails (issue #907 P1-3) — the engine knows 30+ concrete signatures, so
/// exit 1 no longer collapses every failure into "generic error". Falls
/// back to the classic exit-code signatures (OOM 137/9, segfault 139) when
/// no tail survived, and to the generic message when neither matches.
/// Returns `(prediction, suggestion, auto_fixable, level, pattern_id)`.
fn classify_failure(
    node: &NodeExecutionStatus,
) -> (String, String, bool, AlertLevel, &'static str) {
    let tail = [node.stderr_tail.as_deref(), node.stdout_tail.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<&str>>()
        .join("\n");

    if tail.trim().is_empty() {
        // No tail: fall through to the exit-code-only signatures below.
    } else if let Some(result) = DiagnosticsEngine::global()
        .analyze(&node.rule, &tail, node.exit_code)
        .into_iter()
        .find(|r| r.error_pattern.as_deref() != Some("unknown_error"))
    {
        let pattern = result
            .error_pattern
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let pattern_id: &'static str = match pattern.as_str() {
            "oom_killed" => "oom_killed",
            "segfault" => "segfault",
            "command_not_found" => "command_not_found",
            "timeout" => "timeout",
            "disk_full" => "disk_full",
            "permission_denied" => "permission_denied",
            "network_error" => "network_error",
            "conda_env_fail" => "conda_env_fail",
            "shell_syntax" => "shell_syntax",
            // The engine's full table is the identity, not just the 9 most
            // common signatures — an unmapped id is used verbatim so
            // alert ids stay specific and dedup consumers can tell causes
            // apart (issue #908 review). Runtime ids are bounded by the
            // static pattern table, so the &'static leak is fixed at size.
            other => Box::leak(other.to_string().into_boxed_str()),
        };
        let level = match result.category {
            // Resource exhaustion escalates the run regardless of whether the
            // evidence came from a tail or only the exit code — the no-tail
            // fallbacks below rate OOM/segfault Critical, so a tail-classified
            // match of the same failure must not downgrade it (#908 review).
            ErrorCategory::Resource => AlertLevel::Critical,
            ErrorCategory::Tool
                if matches!(node.exit_code, Some(139) | Some(11))
                    && result.error_pattern.as_deref() == Some("segfault") =>
            {
                AlertLevel::Critical
            }
            _ => AlertLevel::Alert,
        };
        let prediction = format!(
            "Rule '{}' failed: {} (exit {})",
            node.rule,
            result.likely_cause,
            node.exit_code.unwrap_or(-1)
        );
        let mut suggestion = result
            .suggestions
            .first()
            .cloned()
            .unwrap_or_else(|| "Review logs and adjust parameters.".into());
        if result.auto_fixable {
            suggestion.push_str(" (auto-fix available via Retry)");
        }
        return (
            prediction,
            suggestion,
            result.auto_fixable,
            level,
            pattern_id,
        );
    }

    // No tail (or no match): fall back to the exit-code-only signatures.
    match node.exit_code {
        Some(137) | Some(9) => (
            format!(
                "Rule '{}' killed by OOM (exit {})",
                node.rule,
                node.exit_code.unwrap()
            ),
            "Increase memory allocation and retry from checkpoint.".into(),
            true,
            AlertLevel::Critical,
            "oom_killed",
        ),
        Some(139) => (
            format!("Rule '{}' segfaulted (exit 139)", node.rule),
            "Possible memory corruption -- reduce threads or check input data integrity.".into(),
            false,
            AlertLevel::Critical,
            "segfault",
        ),
        _ => (
            format!(
                "Rule '{}' failed with exit code {}",
                node.rule,
                node.exit_code.unwrap_or(-1)
            ),
            "Review logs and adjust parameters.".into(),
            false,
            AlertLevel::Alert,
            "exit_code",
        ),
    }
}

/// Generate a fix suggestion for a failed rule.
pub fn suggest_fix(rule: &str, exit_code: Option<i32>, _log_excerpt: &str) -> Vec<String> {
    match exit_code {
        Some(137) | Some(9) => vec![
            format!("Increase memory allocation for '{rule}'"),
            "Add checkpoint/retry mechanism".into(),
            "Reduce input file size or split into chunks".into(),
        ],
        Some(139) => vec![
            format!("Reduce threads for '{rule}'"),
            "Verify input data integrity".into(),
            "Update tool to latest version".into(),
        ],
        Some(1) => vec![
            format!("Check '{rule}' for missing input files or incorrect paths"),
            "Verify environment/tool availability".into(),
        ],
        _ => vec![
            format!("Review logs for '{rule}' and adjust parameters"),
            "Try running manually to isolate the issue".into(),
        ],
    }
}

/// Check if a failed rule matches a known error pattern.
///
/// Delegates to the shared [`DiagnosticsEngine`] pattern table so the AI
/// monitor and the web diagnostics service agree on what counts as a known
/// error (issue #708) — the previous hand-rolled substring checks matched
/// near-miss words (`contains("oom")` matched "zoom"). Returns
/// `(auto_fixable, pattern_id)`.
///
/// Exit-code-only signatures still fire when no stderr text survived:
/// 137/9 is the OOM killer's SIGKILL, 139 is SIGSEGV.
pub fn is_known_error_pattern(exit_code: Option<i32>, log_excerpt: &str) -> (bool, String) {
    match exit_code {
        Some(137) | Some(9) => return (true, "oom_killed".into()),
        Some(139) => return (true, "segfault".into()),
        _ => {}
    }

    match DiagnosticsEngine::global()
        .analyze("monitor", log_excerpt, exit_code)
        .into_iter()
        .find(|r| r.error_pattern.as_deref() != Some("unknown_error"))
    {
        Some(result) => (
            result.auto_fixable,
            result.error_pattern.unwrap_or_else(|| "unknown".into()),
        ),
        None => (false, "unknown".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_normal_run() {
        let nodes = vec![
            NodeExecutionStatus {
                rule: "step1".into(),
                status: "success".into(),
                duration_ms: Some(10_000),
                exit_code: Some(0),
                started_at: None,
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeExecutionStatus {
                rule: "step2".into(),
                status: "running".into(),
                duration_ms: Some(5_000),
                exit_code: None,
                started_at: None,
                stderr_tail: None,
                stdout_tail: None,
            },
        ];
        let resources = ResourceUsage::default();
        let status = analyze_run_status(&nodes, &resources);
        assert_eq!(status.overall, "normal");
        assert!(status.alerts.is_empty());
    }

    #[test]
    fn test_analyze_oom_kill() {
        let nodes = vec![NodeExecutionStatus {
            rule: "star_align".into(),
            status: "failed".into(),
            duration_ms: Some(30_000),
            exit_code: Some(137),
            started_at: None,
            stderr_tail: None,
            stdout_tail: None,
        }];
        let resources = ResourceUsage {
            memory_pct: 95.0,
            ..Default::default()
        };
        let status = analyze_run_status(&nodes, &resources);
        assert!(
            status
                .alerts
                .iter()
                .any(|a| a.level == AlertLevel::Critical)
        );
        assert!(status.alerts.iter().any(|a| a.auto_fixable));
    }

    #[test]
    fn test_is_known_oom_pattern() {
        let (known, pattern) = is_known_error_pattern(Some(137), "");
        assert!(known);
        assert_eq!(pattern, "oom_killed");
    }

    #[test]
    fn test_tail_classification_uses_diagnostics_engine() {
        // Issue #907 P1-3: exit 127 used to collapse into "generic error" even
        // when the persisted stderr named the real cause. With a tail, the
        // classification comes from the shared DiagnosticsEngine table.
        let nodes = vec![NodeExecutionStatus {
            rule: "qc".into(),
            status: "failed".into(),
            duration_ms: Some(5_000),
            exit_code: Some(127),
            started_at: None,
            stderr_tail: Some("sh: fastqc: command not found".into()),
            stdout_tail: None,
        }];
        let status = analyze_run_status(&nodes, &ResourceUsage::default());
        let alert = status
            .alerts
            .iter()
            .find(|a| a.rule_name.as_deref() == Some("qc"))
            .expect("failed rule must raise an alert");
        assert_eq!(alert.alert_id, "failed:qc:command_not_found");
        assert!(alert.prediction.contains("not installed or not in PATH"));
        assert!(alert.auto_fixable);
    }

    #[test]
    fn test_tail_unbound_wildcard_alert_path_end_to_end() {
        // Issue #917 NIT-2: the pattern table has a unbound_wildcard entry,
        // but nothing proved the alert path (analyze_run_status ->
        // classify_failure -> DiagnosticsEngine) maps the short-form tail
        // P1-3 checkpoints into stderr_tail to the specific alert id. The
        // tail is the exact engine wording from the residual-wildcard gate
        // (executor/process.rs), persisted with exit code -1.
        let nodes = vec![NodeExecutionStatus {
            rule: "call_variants".into(),
            status: "failed".into(),
            duration_ms: Some(1_000),
            exit_code: Some(-1),
            started_at: None,
            stderr_tail: Some(
                "output pattern 'results/{sample}/out.bam' contains unbound wildcard {sample} for this instance — declare a value source (sample_pattern, [[values]], [[sample_groups]], input_groups) or remove it"
                    .into(),
            ),
            stdout_tail: None,
        }];
        let status = analyze_run_status(&nodes, &ResourceUsage::default());
        let alert = status
            .alerts
            .iter()
            .find(|a| a.rule_name.as_deref() == Some("call_variants"))
            .expect("failed rule must raise an alert");
        assert_eq!(alert.alert_id, "failed:call_variants:unbound_wildcard");
        assert!(!alert.auto_fixable);
        // The Config category must not escalate to Critical (only Resource
        // failures do) and must not collapse to the generic exit_code id.
        assert_ne!(alert.alert_id, "failed:call_variants:exit_code");
    }

    #[test]
    fn test_tail_classified_segfault_stays_critical() {
        // Issue #908 review: the no-tail exit-139 fallback is Critical, so a
        // tail-classified segfault must not downgrade to Alert.
        let nodes = vec![NodeExecutionStatus {
            rule: "align".into(),
            status: "failed".into(),
            duration_ms: Some(8_000),
            exit_code: Some(139),
            started_at: None,
            stderr_tail: Some("segmentation fault (core dumped)".into()),
            stdout_tail: None,
        }];
        let status = analyze_run_status(&nodes, &ResourceUsage::default());
        let alert = status
            .alerts
            .iter()
            .find(|a| a.rule_name.as_deref() == Some("align"))
            .expect("failed rule must raise an alert");
        assert_eq!(alert.alert_id, "failed:align:segfault");
        assert!(matches!(alert.level, AlertLevel::Critical));
    }

    #[test]
    fn test_unmapped_engine_pattern_keeps_specific_alert_id() {
        // Issue #908 review: ids outside the 10 static arms must keep the
        // engine's own identity instead of collapsing to "exit_code".
        let nodes = vec![NodeExecutionStatus {
            rule: "fetch".into(),
            status: "failed".into(),
            duration_ms: Some(2_000),
            exit_code: Some(1),
            started_at: None,
            stderr_tail: Some("curl: (6) could not resolve host: example.org".into()),
            stdout_tail: None,
        }];
        let status = analyze_run_status(&nodes, &ResourceUsage::default());
        let alert = status
            .alerts
            .iter()
            .find(|a| a.rule_name.as_deref() == Some("fetch"))
            .expect("failed rule must raise an alert");
        assert_ne!(alert.alert_id, "failed:fetch:exit_code");
    }

    #[test]
    fn test_eta_is_none_when_only_failures_remain() {
        // Issue #908 review: failed/skipped nodes are not pending work, so a
        // terminal-failed run must not report a permanently positive ETA.
        let nodes = vec![
            NodeExecutionStatus {
                rule: "a".into(),
                status: "success".into(),
                duration_ms: Some(10_000),
                exit_code: Some(0),
                started_at: None,
                stderr_tail: None,
                stdout_tail: None,
            },
            NodeExecutionStatus {
                rule: "b".into(),
                status: "failed".into(),
                duration_ms: Some(5_000),
                exit_code: Some(1),
                started_at: None,
                stderr_tail: None,
                stdout_tail: None,
            },
        ];
        let status = analyze_run_status(&nodes, &ResourceUsage::default());
        assert_eq!(status.estimated_completion, None);
    }

    #[test]
    fn test_alert_ids_are_stable_across_polls() {
        // Issue #907 P1-8: the ai-status endpoint is polled every 5s; alert
        // identities (not timestamps) must be stable so consumers can dedup.
        let nodes = vec![NodeExecutionStatus {
            rule: "star_align".into(),
            status: "failed".into(),
            duration_ms: Some(30_000),
            exit_code: Some(137),
            started_at: None,
            stderr_tail: None,
            stdout_tail: None,
        }];
        let mk = || analyze_run_status(&nodes, &ResourceUsage::default());
        let a = mk();
        let b = mk();
        let ids_a: Vec<&str> = a.alerts.iter().map(|x| x.alert_id.as_str()).collect();
        let ids_b: Vec<&str> = b.alerts.iter().map(|x| x.alert_id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
        assert!(ids_a.iter().any(|id| id.starts_with("failed:star_align:")));
    }

    #[test]
    fn test_is_known_log_pattern() {
        let (known, pattern) = is_known_error_pattern(None, "Out of memory: killed process");
        assert!(known);
        assert_eq!(pattern, "oom_killed");
    }

    #[test]
    fn test_zoom_does_not_match_oom() {
        // Regression for issue #709: contains("oom") matched "zoom".
        let (known, pattern) = is_known_error_pattern(None, "Zooming the canvas to level 5");
        assert!(!known);
        assert_eq!(pattern, "unknown");
    }

    #[test]
    fn test_shared_engine_reports_diagnostics_pattern_id() {
        // The monitor and the diagnostics engine now share one table, so
        // pattern ids and auto-fixability cannot drift apart (#708).
        let (known, pattern) =
            is_known_error_pattern(Some(127), "sh: 1: fastqc: command not found");
        assert!(known);
        assert_eq!(pattern, "command_not_found");
    }

    #[test]
    fn test_suggest_fix_oom() {
        let fixes = suggest_fix("star_align", Some(137), "");
        assert!(fixes[0].contains("memory"));
    }

    #[test]
    fn test_resource_usage_default() {
        let r = ResourceUsage::default();
        assert!(r.memory_mb > 0.0);
    }

    #[test]
    fn test_disk_warning() {
        let nodes = vec![];
        let resources = ResourceUsage {
            disk_pct: 90.0,
            ..Default::default()
        };
        let status = analyze_run_status(&nodes, &resources);
        assert!(status.alerts.iter().any(|a| a.level == AlertLevel::Warn));
    }
}
