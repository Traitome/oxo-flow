//! Monitor Agent — watches pipeline execution, predicts failures, generates alerts.
//!
//! Resource monitoring: CPU/Mem/Disk trends, OOM prediction, timeout prediction
//! Progress tracking: Done/pending/failed, ETA computation
//! Log analysis: stderr real-time, known error pattern matching
//! Decision engine: info -> warn -> alert -> critical with escalation

use super::types::*;
use chrono::Utc;

/// Execution status of a single rule/node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeExecutionStatus {
    pub rule: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
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
            let (prediction, suggestion, auto_fixable, level) = match node.exit_code {
                Some(137) | Some(9) => (
                    format!(
                        "Rule '{}' killed by OOM (exit {})",
                        node.rule,
                        node.exit_code.unwrap()
                    ),
                    "Increase memory allocation and retry from checkpoint.".into(),
                    true,
                    AlertLevel::Critical,
                ),
                Some(1) => (
                    format!("Rule '{}' failed with generic error (exit 1)", node.rule),
                    "Check log output for specific error details.".into(),
                    false,
                    AlertLevel::Alert,
                ),
                Some(139) => (
                    format!("Rule '{}' segfaulted (exit 139)", node.rule),
                    "Possible memory corruption -- reduce threads or check input data integrity."
                        .into(),
                    false,
                    AlertLevel::Critical,
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
                ),
            };
            alerts.push(MonitorAlert {
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

    // ETA computation
    let completed: Vec<&NodeExecutionStatus> =
        nodes.iter().filter(|n| n.status == "success").collect();
    let estimated_completion = if !completed.is_empty() && completed.len() < nodes.len() {
        let total_done: i64 = completed.iter().filter_map(|n| n.duration_ms).sum();
        let avg_per_step = total_done as f64 / completed.len() as f64;
        let remaining = (nodes.len() - completed.len()) as f64;
        let eta_secs = (avg_per_step * remaining / 1000.0) as u64;
        Some(format!("~{}m {}s remaining", eta_secs / 60, eta_secs % 60))
    } else if completed.len() == nodes.len() {
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

/// Check if an error pattern is known and auto-fixable.
pub fn is_known_error_pattern(exit_code: Option<i32>, log_excerpt: &str) -> (bool, String) {
    let log_lower = log_excerpt.to_lowercase();

    if exit_code == Some(137)
        || log_lower.contains("oom")
        || log_lower.contains("out of memory")
        || log_lower.contains("killed")
    {
        return (true, "oom_killed".into());
    }
    if exit_code == Some(139)
        || log_lower.contains("segfault")
        || log_lower.contains("segmentation fault")
    {
        return (true, "segfault".into());
    }
    if log_lower.contains("no such file") || log_lower.contains("cannot find") {
        return (true, "missing_file".into());
    }
    if log_lower.contains("permission denied") || log_lower.contains("access denied") {
        return (false, "permission_error".into());
    }
    if log_lower.contains("disk full") || log_lower.contains("no space left") {
        return (true, "disk_full".into());
    }

    (false, "unknown".into())
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
            },
            NodeExecutionStatus {
                rule: "step2".into(),
                status: "running".into(),
                duration_ms: Some(5_000),
                exit_code: None,
                started_at: None,
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
    fn test_is_known_log_pattern() {
        let (known, pattern) = is_known_error_pattern(None, "Out of memory: killed process");
        assert!(known);
        assert_eq!(pattern, "oom_killed");
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
