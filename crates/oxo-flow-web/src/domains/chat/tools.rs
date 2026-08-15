//! Tool registry for the web chat agent.
//!
//! Read-only by design: the embedded knowledge lookups ground tool selection
//! in the Bioconda/bioSkills/pipeline-graph databases. No filesystem or
//! database WRITE tools are registered — saving a generated workflow is the
//! user's explicit Accept click, never a model action.

use oxo_flow_ai::tools::ToolRegistry;
use oxo_flow_ai::tools::builtin::{FetchUrlTool, LookupPipelineTool, LookupSkillTool, LookupTool};

use oxo_flow_ai::error::AiError;
use oxo_flow_ai::tools::{Tool, ToolDef};

/// Read-only run status from the engine's checkpoint + runs table.
struct RunStatusTool {
    run_id: String,
    user_id: String,
    is_admin: bool,
}

#[async_trait::async_trait]
impl Tool for RunStatusTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "get_run_status".into(),
            description: "Get the status of a workflow run: overall status and per-rule completion from the engine checkpoint.".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
    async fn execute(&self, _arguments: &str) -> Result<String, AiError> {
        let Ok(pool) = crate::infra::db::sqlite::try_pool() else {
            return Err(AiError::ToolError {
                tool: "get_run_status".into(),
                message: "database unavailable".into(),
            });
        };
        let run: Option<crate::infra::db::models::RunRow> =
            sqlx::query_as("SELECT * FROM runs WHERE id = ?")
                .bind(&self.run_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AiError::ToolError {
                    tool: "get_run_status".into(),
                    message: e.to_string(),
                })?;
        let Some(run) = run else {
            return Err(AiError::ToolError {
                tool: "get_run_status".into(),
                message: format!("run {} not found", self.run_id),
            });
        };
        // Ownership gate: a chat tool must not leak another tenant's run
        // (the REST endpoints scope by owner; this tool path bypassed them).
        if run.user_id != self.user_id && !self.is_admin {
            return Err(AiError::ToolError {
                tool: "get_run_status".into(),
                message: format!("run {} not found", self.run_id),
            });
        }
        let nodes = crate::domains::execution::checkpoint_status::load_node_statuses(
            std::path::Path::new(run.workdir.as_deref().unwrap_or("")),
            run.status == "running",
        );
        Ok(serde_json::json!({
            "run_id": run.id,
            "status": run.status,
            "rules": nodes.iter().map(|n| serde_json::json!({
                "rule": n.rule,
                "status": n.status.to_string(),
                "duration_ms": n.duration_ms,
            })).collect::<Vec<_>>(),
        })
        .to_string())
    }
    fn is_read_only(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "get_run_status"
    }
}

/// Read-only tail of the run's execution log (workdir-scoped).
struct RunLogsTool {
    run_id: String,
    user_id: String,
    is_admin: bool,
}

#[async_trait::async_trait]
impl Tool for RunLogsTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "get_run_logs".into(),
            description: "Get the tail of a workflow run's execution log for error diagnosis."
                .into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
    async fn execute(&self, _arguments: &str) -> Result<String, AiError> {
        let Ok(pool) = crate::infra::db::sqlite::try_pool() else {
            return Err(AiError::ToolError {
                tool: "get_run_logs".into(),
                message: "database unavailable".into(),
            });
        };
        let run: Option<crate::infra::db::models::RunRow> =
            sqlx::query_as("SELECT * FROM runs WHERE id = ?")
                .bind(&self.run_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AiError::ToolError {
                    tool: "get_run_logs".into(),
                    message: e.to_string(),
                })?;
        let Some(run) = run else {
            return Err(AiError::ToolError {
                tool: "get_run_logs".into(),
                message: format!("run {} not found", self.run_id),
            });
        };
        // Ownership gate — same scope rule as get_run_status.
        if run.user_id != self.user_id && !self.is_admin {
            return Err(AiError::ToolError {
                tool: "get_run_logs".into(),
                message: format!("run {} not found", self.run_id),
            });
        }
        let log = run
            .workdir
            .as_deref()
            .and_then(|wd| std::fs::read_to_string(format!("{wd}/execution.log")).ok())
            .unwrap_or_default();
        // Bound the result: the last 200 lines carry the failure context.
        let tail: Vec<&str> = log.lines().rev().take(200).collect();
        Ok(tail.into_iter().rev().collect::<Vec<_>>().join(
            "
",
        ))
    }
    fn is_read_only(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "get_run_logs"
    }
}

/// Build the registry used by every chat request (cheap — in-memory).
/// When `run_id` is present, the read-only run-diagnosis tools join in —
/// scoped to the acting user so a chat cannot probe another tenant's runs.
pub fn build_chat_tool_registry(
    run_id: Option<&str>,
    user_id: &str,
    is_admin: bool,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(LookupTool::new()));
    registry.register(Box::new(LookupSkillTool::new()));
    registry.register(Box::new(LookupPipelineTool::new()));
    registry.register(Box::new(FetchUrlTool::new()));
    if let Some(run_id) = run_id {
        registry.register(Box::new(RunStatusTool {
            run_id: run_id.to_string(),
            user_id: user_id.to_string(),
            is_admin,
        }));
        registry.register(Box::new(RunLogsTool {
            run_id: run_id.to_string(),
            user_id: user_id.to_string(),
            is_admin,
        }));
    }
    registry
}
