use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Parsing,
    Validating,
    Preparing,
    Executing,
    Reporting,
}

impl std::fmt::Display for RunPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parsing => write!(f, "parsing"),
            Self::Validating => write!(f, "validating"),
            Self::Preparing => write!(f, "preparing"),
            Self::Executing => write!(f, "executing"),
            Self::Reporting => write!(f, "reporting"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Running,
    Success,
    Failed,
    Skipped,
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub pipeline_id: String,
    pub config: Option<RunConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub max_jobs: Option<usize>,
    pub dry_run: Option<bool>,
    pub keep_going: Option<bool>,
    pub resource_budget: Option<ResourceBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub max_memory: Option<String>,
    pub max_threads: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub status: String,
    pub estimated_resources: EstimatedResources,
    pub execution_plan: ExecutionPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatedResources {
    pub max_memory_mb: u64,
    pub max_threads: u32,
    pub estimated_duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub total_rules: usize,
    pub parallel_groups: Vec<Vec<String>>,
    pub execution_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStatusResponse {
    pub status: RunStatus,
    pub phase: String,
    pub nodes: Vec<NodeStatusItem>,
    pub timeline: Vec<TimelineEvent>,
    pub resources: ResourceSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatusItem {
    pub rule: String,
    pub status: NodeStatus,
    pub started_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub progress_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: String,
    pub event: String,
    pub node: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_pct: f64,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

impl Default for ResourceSnapshot {
    fn default() -> Self {
        Self {
            cpu_pct: 0.0,
            memory_mb: 0,
            disk_mb: 0,
        }
    }
}

// DAG status types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagStatusResponse {
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
    pub parallel_groups: Vec<Vec<String>>,
    pub critical_path: Vec<String>,
    pub metrics: DagMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub id: String,
    pub label: String,
    pub status: String,
    pub color: String,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEdge {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetrics {
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub failed_nodes: usize,
    pub running_nodes: usize,
    pub pending_nodes: usize,
    pub eta_ms: Option<u64>,
}

// Diagnostics types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsResponse {
    pub failed_nodes: Vec<FailedNode>,
    pub warnings: Vec<DiagnosticWarning>,
    pub resource_bottlenecks: Vec<ResourceBottleneck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedNode {
    pub rule: String,
    pub error_pattern: Option<String>,
    pub likely_cause: String,
    pub suggestions: Vec<String>,
    pub auto_fixable: bool,
    pub fix_action: Option<FixAction>,
    pub relevant_log_lines: Vec<String>,
}

/// Actionable fix that can be applied to resolve a diagnostic finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixAction {
    pub description: String,
    pub config_change: Option<ConfigChange>,
    pub command: Option<String>,
}

/// A specific configuration change suggested by the diagnostics engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChange {
    pub path: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticWarning {
    pub rule: String,
    pub pattern: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBottleneck {
    pub rule: String,
    pub metric: String,
    pub actual: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryRequest {
    pub from_rule: Option<String>,
    pub skip_succeeded: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryResponse {
    pub new_run_id: String,
    pub will_rerun: Vec<String>,
    pub will_skip: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_display() {
        assert_eq!(RunStatus::Queued.to_string(), "queued");
        assert_eq!(RunStatus::Running.to_string(), "running");
        assert_eq!(RunStatus::Completed.to_string(), "completed");
        assert_eq!(RunStatus::Failed.to_string(), "failed");
        assert_eq!(RunStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_run_phase_display() {
        assert_eq!(RunPhase::Parsing.to_string(), "parsing");
        assert_eq!(RunPhase::Validating.to_string(), "validating");
        assert_eq!(RunPhase::Preparing.to_string(), "preparing");
        assert_eq!(RunPhase::Executing.to_string(), "executing");
        assert_eq!(RunPhase::Reporting.to_string(), "reporting");
    }

    #[test]
    fn test_node_status_display() {
        assert_eq!(NodeStatus::Pending.to_string(), "pending");
        assert_eq!(NodeStatus::Running.to_string(), "running");
        assert_eq!(NodeStatus::Success.to_string(), "success");
        assert_eq!(NodeStatus::Failed.to_string(), "failed");
        assert_eq!(NodeStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn test_create_run_request_roundtrip() {
        let req = CreateRunRequest {
            pipeline_id: "p1".into(),
            config: Some(RunConfig {
                max_jobs: Some(4),
                dry_run: Some(true),
                keep_going: Some(false),
                resource_budget: Some(ResourceBudget {
                    max_memory: Some("8G".into()),
                    max_threads: Some(4),
                }),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: CreateRunRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pipeline_id, req.pipeline_id);
    }

    #[test]
    fn test_run_status_response_roundtrip() {
        let resp = RunStatusResponse {
            status: RunStatus::Running,
            phase: "executing".to_string(),
            nodes: vec![NodeStatusItem {
                rule: "rule1".into(),
                status: NodeStatus::Running,
                started_at: Some("2024-01-01T00:00:00Z".into()),
                duration_ms: Some(1000),
                exit_code: None,
                progress_pct: Some(50),
            }],
            timeline: vec![TimelineEvent {
                timestamp: "2024-01-01T00:00:00Z".into(),
                event: "started".into(),
                node: Some("rule1".into()),
                message: Some("rule started".into()),
            }],
            resources: ResourceSnapshot {
                cpu_pct: 45.5,
                memory_mb: 1024,
                disk_mb: 2048,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RunStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, RunStatus::Running);
    }

    #[test]
    fn test_diagnostics_response_roundtrip() {
        let resp = DiagnosticsResponse {
            failed_nodes: vec![FailedNode {
                rule: "rule1".into(),
                error_pattern: Some("OOM".into()),
                likely_cause: "out of memory".into(),
                suggestions: vec!["increase memory".into()],
                auto_fixable: true,
                fix_action: Some(FixAction {
                    description: "Increase memory".into(),
                    config_change: Some(ConfigChange {
                        path: "resources.memory".into(),
                        old_value: "16GB".into(),
                        new_value: "32GB".into(),
                    }),
                    command: None,
                }),
                relevant_log_lines: vec!["fatal error".into()],
            }],
            warnings: vec![DiagnosticWarning {
                rule: "rule2".into(),
                pattern: "slow".into(),
                suggestion: "optimize".into(),
            }],
            resource_bottlenecks: vec![ResourceBottleneck {
                rule: "rule3".into(),
                metric: "memory".into(),
                actual: 8000.0,
                limit: 4096.0,
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: DiagnosticsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.failed_nodes.len(), 1);
    }

    #[test]
    fn test_retry_response_roundtrip() {
        let resp = RetryResponse {
            new_run_id: "run2".into(),
            will_rerun: vec!["rule1".into()],
            will_skip: vec!["rule2".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RetryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.new_run_id, "run2");
    }
}
