use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
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

/// The wire contract of `POST /api/runs` (issue #324 F-4): a flat
/// `toml_content` plus flat options — the exact shape the frontend client
/// sends and the handler parses. Kept in one place so the OpenAPI spec,
/// the handler, and the TypeScript client can never drift again.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateRunRequest {
    /// Workflow definition in `.oxoflow` TOML format. Required.
    pub toml_content: String,
    #[serde(default)]
    pub max_jobs: Option<usize>,
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub keep_going: Option<bool>,
    #[serde(default)]
    pub cluster_id: Option<String>,
    #[serde(default)]
    pub pipeline_id: Option<String>,
    #[serde(default)]
    pub samples: Option<Vec<String>>,
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Secondary workflow files ([[include]] sources) keyed by their
    /// workflow-relative path, e.g. `{"envs/clinical/pixi.toml": "..."}`.
    /// Keys are validated as relative workspace paths (no absolute paths,
    /// no `..`, not `workflow.oxoflow` itself) and written beside
    /// `workflow.oxoflow` in the run directory, where the CLI's
    /// `resolve_includes` resolves them unchanged. `BTreeMap` keeps the
    /// persisted staging order-independent.
    #[serde(default)]
    pub include_files: Option<std::collections::BTreeMap<String, String>>,
    /// CLI config overrides passed through to the spawned run as
    /// `KEY=VALUE` argv entries (`--arg`/positional semantics live inside
    /// the CLI). Each entry must be a non-empty `KEY=VALUE` string; the
    /// CLI warns on unknown keys and hard-fails on invalid values for
    /// declared keys.
    #[serde(default)]
    pub config_overrides: Option<Vec<String>>,
    /// Proceed despite protected data tree drift (issue #935). The CLI's
    /// run gate refuses to execute when protected-tree files changed since
    /// the recorded baseline; this flag forwards `--allow-protected-drift`
    /// to override that refusal. Preview the drift first via
    /// `GET /api/runs/{id}/protected-drift` — the CLI still narrates the
    /// drift into the run log. Retry keeps the gate: only explicit create
    /// requests may set it.
    #[serde(default)]
    pub allow_protected_drift: Option<bool>,
}

/// Response of `GET /api/runs/{id}/protected-drift` — the read-only
/// protected-tree drift preview (issue #935 slice 3). Mirrors the CLI
/// dry-run's `protected_tree_drift` JSON object: one `describe()` line per
/// drifted file across all `[engine] protected_data_trees`, the number of
/// trees that would be baselined on first observation, and per-tree
/// warnings (unreadable tree, ignored empty path values).
///
/// The preview is read-only: unlike `oxo-flow run` (which persists new
/// baselines into the checkpoint) nothing is recorded here, so repeated
/// calls are stable and a `new_trees` count is reported as "would be
/// baselined" rather than adopted. An absent `drifted_files` list plus a
/// non-zero `new_trees` means the trees are clean but not yet baselined —
/// the next real run adopts them silently.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProtectedDriftResponse {
    /// One human-readable line per drifted file (removed / added / changed,
    /// with `(TRUNCATED)` and `sha256:` labels — `ManifestChange::describe`
    /// rendering, identical to the CLI dry-run and the run-gate refusal).
    pub drifted_files: Vec<String>,
    /// Trees under `[engine] protected_data_trees` that exist but have no
    /// baseline yet — the next real run adopts them; this preview never does.
    pub new_trees: usize,
    /// Per-tree warnings (unreadable tree, ignored empty path values).
    pub warnings: Vec<String>,
}

/// The wire contract of `POST /api/data/verify` (issue #935 P2 web slice):
/// a workspace-relative tree to audit plus the vendor md5 sidecar content
/// inline — the server stages it for the CLI subprocess, so the caller
/// never needs filesystem access to the delivery.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VerifyDataRequest {
    /// Directory to verify, relative to the acting user's workspace root
    /// (`workspace/users/<user>/...` — uploaded inputs and run workdirs
    /// both live there). Absolute paths and `..` are rejected.
    pub tree: String,
    /// Full md5 sidecar text in the standard `md5sum` format
    /// (`<32-hex md5> <path>`), as shipped by vendors.
    pub md5_content: String,
}

/// One classified file from a verify-data audit — mirrors the CLI's
/// `--json` entries (`VerifyDataEntry`, oxo-flow-cli `verify_data.rs`).
/// `expected` is absent for extras, `actual` for missing files; `note`
/// carries free-form context (checksum read errors, the empty-sidecar
/// notice).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VerifyDataEntry {
    pub file: String,
    /// `matched` | `mismatched` | `missing` | `extra`
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Response of `POST /api/data/verify` — the parsed CLI `--json` document.
/// The audit operation itself always succeeds here (HTTP 200) even when it
/// finds drift: `mismatched`/`missing`/`extra` > 0 IS the finding, not an
/// error. HTTP errors are reserved for infrastructure failures (spawn,
/// parse of the CLI output) and request problems (bad tree).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VerifyDataResponse {
    /// Workspace-relative tree as requested (echoed for correlation).
    pub tree: String,
    pub matched: usize,
    pub mismatched: usize,
    pub missing: usize,
    /// Files present under the tree but not listed in the sidecar.
    pub extra: usize,
    pub entries: Vec<VerifyDataEntry>,
    /// Set only for the empty-sidecar case: nothing to verify, extras scan
    /// skipped (mirrors the CLI's `"note"` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RunConfig {
    pub max_jobs: Option<usize>,
    pub dry_run: Option<bool>,
    pub keep_going: Option<bool>,
    pub resource_budget: Option<ResourceBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResourceBudget {
    pub max_memory: Option<String>,
    pub max_threads: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub status: String,
    pub estimated_resources: EstimatedResources,
    pub execution_plan: ExecutionPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EstimatedResources {
    pub max_memory_mb: u64,
    pub max_threads: u32,
    pub estimated_duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExecutionPlan {
    pub total_rules: usize,
    pub parallel_groups: Vec<Vec<String>>,
    pub execution_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RunStatusResponse {
    pub status: RunStatus,
    pub phase: String,
    pub nodes: Vec<NodeStatusItem>,
    pub timeline: Vec<TimelineEvent>,
    pub resources: ResourceSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeStatusItem {
    pub rule: String,
    pub status: NodeStatus,
    pub started_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub progress_pct: Option<u8>,
    /// Bounded stderr tail of a failed rule, straight from the checkpoint's
    /// `rule_runs` record (issue #758) — lets the monitor and the AI
    /// assistant show WHY a rule failed without opening the raw log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    /// Bounded stdout tail of a failed rule (issue #765) — some tools print
    /// their root cause on stdout, which #691 persisted for exactly this
    /// reason; the web surfaces it alongside stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TimelineEvent {
    pub timestamp: String,
    pub event: String,
    pub node: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagStatusResponse {
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
    pub parallel_groups: Vec<Vec<String>>,
    pub critical_path: Vec<String>,
    pub metrics: DagMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagNode {
    pub id: String,
    pub label: String,
    pub status: String,
    pub color: String,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    /// Bounded stderr tail of a failed rule (issue #758) — the DAG canvas
    /// uses it to show WHY a node failed without opening the raw log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    /// Bounded stdout tail of a failed rule (issue #765) — some tools print
    /// their root cause on stdout; surfaced alongside stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagEdge {
    pub source: String,
    pub target: String,
}

// Run-tracking DAG metrics. Schema name is distinct from the workflow
// domain's `DagMetrics` (different field sets; both land in the OpenAPI
// components and would otherwise collide).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = RunDagMetrics)]
pub struct DagMetrics {
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub failed_nodes: usize,
    pub running_nodes: usize,
    pub pending_nodes: usize,
    /// when-gated-off / abort-cancelled nodes — counted separately so
    /// `completed + failed + running + pending + skipped == total` partitions
    /// exactly (issue #907 P1-2).
    pub skipped_nodes: usize,
    pub eta_ms: Option<u64>,
}

// Diagnostics types
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiagnosticsResponse {
    pub failed_nodes: Vec<FailedNode>,
    pub warnings: Vec<DiagnosticWarning>,
    pub resource_bottlenecks: Vec<ResourceBottleneck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FixAction {
    pub description: String,
    pub config_change: Option<ConfigChange>,
    pub command: Option<String>,
}

/// A specific configuration change suggested by the diagnostics engine.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigChange {
    pub path: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiagnosticWarning {
    pub rule: String,
    pub pattern: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResourceBottleneck {
    pub rule: String,
    pub metric: String,
    pub actual: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetryResponse {
    pub new_run_id: String,
    pub will_rerun: Vec<String>,
    pub will_skip: Vec<String>,
    /// Honesty note when the plan cannot describe the retry (issue #760):
    /// a previous run that failed before executing any rule yields an
    /// empty `will_rerun`/`will_skip` — the retry re-attempts the same
    /// pipeline and re-fails unless the configuration error is fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
        // The WIRE contract of POST /api/runs (issue #324 F-4): a flat
        // toml_content plus flat options — the shape the frontend client
        // and the handler actually speak. The type must parse exactly this
        // payload; a divergent typed contract is a decorated lie.
        let json = serde_json::json!({
            "toml_content": "[workflow]\nname = \"x\"",
            "max_jobs": 4,
            "dry_run": true,
            "keep_going": false,
            "cluster_id": null,
            "pipeline_id": "p1",
            "samples": ["S1"],
            "targets": [],
            "include_files": {"envs/clinical/pixi.toml": "[env]"},
            "config_overrides": ["min_quality=30"],
        });
        let back: CreateRunRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.toml_content, "[workflow]\nname = \"x\"");
        assert_eq!(back.pipeline_id.as_deref(), Some("p1"));
        assert_eq!(back.max_jobs, Some(4));
        assert_eq!(back.dry_run, Some(true));
        assert_eq!(back.samples.as_deref(), Some(&["S1".to_string()][..]));
        assert_eq!(
            back.include_files
                .as_ref()
                .and_then(|m| m.get("envs/clinical/pixi.toml")),
            Some(&"[env]".to_string())
        );
        assert_eq!(
            back.config_overrides.as_deref(),
            Some(&["min_quality=30".to_string()][..])
        );
    }

    #[test]
    fn test_create_run_request_defaults() {
        // Both new fields are optional with serde defaults — an older
        // client payload (no include_files / config_overrides) must keep
        // parsing so the API stays backward-compatible.
        let back: CreateRunRequest =
            serde_json::from_value(serde_json::json!({"toml_content": "[workflow]\nname = \"x\""}))
                .unwrap();
        assert!(back.include_files.is_none());
        assert!(back.config_overrides.is_none());
    }

    #[test]
    fn test_verify_data_request_roundtrip() {
        // The WIRE contract of POST /api/data/verify (issue #935 P2): a
        // workspace-relative tree plus the inline sidecar text.
        let back: VerifyDataRequest = serde_json::from_value(serde_json::json!({
            "tree": "inputs/delivery-2026-10",
            "md5_content": "d41d8cd98f00b204e9800998ecf8427e  raw/S1.fq\n"
        }))
        .unwrap();
        assert_eq!(back.tree, "inputs/delivery-2026-10");
        assert!(back.md5_content.contains("raw/S1.fq"));
    }

    #[test]
    fn test_verify_data_response_roundtrip() {
        // Must parse the CLI's `--json` document shape: statuses are
        // free-form strings here (the CLI owns the vocabulary), optional
        // fields skip when absent, `note` only appears when set.
        let cli_doc = serde_json::json!({
            "tree": "inputs/delivery-2026-10",
            "matched": 1,
            "mismatched": 1,
            "missing": 1,
            "extra": 1,
            "entries": [
                {"file": "raw/S1.fq", "status": "matched", "expected": "aa", "actual": "aa"},
                {"file": "raw/S2.fq", "status": "mismatched", "expected": "bb", "actual": "cc"},
                {"file": "raw/S3.fq", "status": "missing", "expected": "dd"},
                {"file": "raw/unlisted.txt", "status": "extra", "actual": "ee"}
            ]
        });
        let resp: VerifyDataResponse = serde_json::from_value(cli_doc).unwrap();
        assert_eq!(resp.matched, 1);
        assert_eq!(resp.mismatched, 1);
        assert_eq!(resp.missing, 1);
        assert_eq!(resp.extra, 1);
        assert_eq!(resp.entries.len(), 4);
        assert_eq!(resp.entries[0].status, "matched");
        assert!(resp.entries[2].actual.is_none());
        assert!(resp.entries[3].expected.is_none());
        assert!(resp.note.is_none());

        // Empty-sidecar document: counts zero, note present.
        let empty: VerifyDataResponse = serde_json::from_value(serde_json::json!({
            "tree": "inputs/delivery",
            "matched": 0, "mismatched": 0, "missing": 0, "extra": 0,
            "entries": [],
            "note": "The md5 sidecar lists no files."
        }))
        .unwrap();
        assert_eq!(
            empty.note.as_deref(),
            Some("The md5 sidecar lists no files.")
        );
        assert!(empty.entries.is_empty());
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
                stderr_tail: None,
                stdout_tail: None,
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
            note: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RetryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.new_run_id, "run2");
        assert!(back.note.is_none());
        // The note survives serialization when set.
        let noted = RetryResponse {
            note: Some("previous run failed before executing any rule".into()),
            ..resp
        };
        let json = serde_json::to_string(&noted).unwrap();
        let back: RetryResponse = serde_json::from_str(&json).unwrap();
        assert!(back.note.is_some());
    }
}
