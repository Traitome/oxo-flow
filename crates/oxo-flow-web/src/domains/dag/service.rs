//! DAG Edit service — command queue with undo/redo and DAG validation.
//!
//! Uses oxo_flow_core::WorkflowConfig for parsing and
//! oxo_flow_core::format::format_workflow for TOML serialization.

use oxo_flow_core::{Rule, WorkflowConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEditCommand {
    pub source: String,
    pub operation: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEditResponse {
    pub success: bool,
    pub toml_content: String,
    pub validation_errors: Vec<String>,
}

#[allow(clippy::type_complexity)]
static EDIT_STACKS: std::sync::LazyLock<Mutex<HashMap<String, (Vec<String>, Vec<String>)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn stack_id(pipeline_id: &str) -> String {
    pipeline_id.to_string()
}

/// Parse TOML, apply edit, format back, validate.
pub fn execute_edit(
    toml_content: &str,
    pipeline_id: &str,
    command: &DagEditCommand,
) -> Result<DagEditResponse, String> {
    let mut config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;

    // Save undo state
    {
        let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
        let entry = stacks.entry(stack_id(pipeline_id)).or_default();
        entry.0.push(toml_content.to_string());
        if entry.0.len() > 50 {
            entry.0.remove(0);
        }
        entry.1.clear();
    }

    // Apply operation
    match command.operation.as_str() {
        "add_rule" => {
            let name = command.payload["name"]
                .as_str()
                .unwrap_or("new_rule")
                .to_string();
            let shell_val = command.payload["shell"]
                .as_str()
                .unwrap_or("echo 'new step'")
                .to_string();
            config.rules.push(Rule {
                name,
                shell: Some(shell_val),
                ..Default::default()
            });
        }
        "remove_rule" => {
            let name = command.payload["name"]
                .as_str()
                .ok_or("Missing rule name")?;
            config.rules.retain(|r| r.name != name);
            for rule in &mut config.rules {
                rule.depends_on.retain(|d| d != name);
            }
        }
        "connect" => {
            let from = command.payload["from"].as_str().ok_or("Missing from")?;
            let to = command.payload["to"].as_str().ok_or("Missing to")?;
            if let Some(rule) = config.rules.iter_mut().find(|r| r.name == to) {
                if !rule.depends_on.contains(&from.to_string()) {
                    rule.depends_on.push(from.to_string());
                }
            } else {
                return Err(format!("Target rule '{to}' not found"));
            }
        }
        "disconnect" => {
            let from = command.payload["from"].as_str().ok_or("Missing from")?;
            let to = command.payload["to"].as_str().ok_or("Missing to")?;
            if let Some(rule) = config.rules.iter_mut().find(|r| r.name == to) {
                rule.depends_on.retain(|d| d != from);
            } else {
                return Err(format!("Target rule '{to}' not found"));
            }
        }
        "update_params" => {
            let name = command.payload["name"]
                .as_str()
                .ok_or("Missing rule name")?;
            if let Some(rule) = config.rules.iter_mut().find(|r| r.name == name) {
                if let Some(val) = command.payload["threads"].as_u64() {
                    rule.threads = Some(val as u32);
                }
                if let Some(val) = command.payload["shell"].as_str() {
                    rule.shell = Some(val.to_string());
                }
            } else {
                return Err(format!("Rule '{name}' not found"));
            }
        }
        _ => return Err(format!("Unknown operation: {}", command.operation)),
    }

    // Format back to TOML via core
    let new_toml = oxo_flow_core::format::format_workflow(&config);

    // Validate
    let validation = crate::domains::workflow::service::validate_pipeline(&new_toml)?;
    let errors: Vec<String> = validation
        .errors
        .iter()
        .map(|e| e.message.clone())
        .collect();

    Ok(DagEditResponse {
        success: validation.valid,
        toml_content: new_toml,
        validation_errors: errors,
    })
}

pub fn undo(pipeline_id: &str) -> Result<Option<String>, String> {
    let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
    let entry = stacks.entry(stack_id(pipeline_id)).or_default();
    if let Some(prev) = entry.0.pop() {
        entry.1.push(prev.clone());
        Ok(Some(prev))
    } else {
        Ok(None)
    }
}

pub fn redo(pipeline_id: &str) -> Result<Option<String>, String> {
    let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
    let entry = stacks.entry(stack_id(pipeline_id)).or_default();
    Ok(entry.1.pop())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOML: &str = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\n\n[[rules]]\nname = \"s2\"\nshell = \"echo s2\"\ndepends_on = [\"s1\"]\n";

    #[test]
    fn test_add_rule() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "s3", "shell": "echo s3"}),
        };
        let r = execute_edit(TEST_TOML, "test-id", &cmd).unwrap();
        assert!(r.toml_content.contains("s3"), "should contain new rule");
    }

    #[test]
    fn test_update_params() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "update_params".into(),
            payload: serde_json::json!({"name": "s1", "threads": 8}),
        };
        let r = execute_edit(TEST_TOML, "test-id", &cmd).unwrap();
        assert!(
            r.toml_content.contains("threads = 8"),
            "should update threads"
        );
    }

    #[test]
    fn test_disconnect() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "disconnect".into(),
            payload: serde_json::json!({"from": "s1", "to": "s2"}),
        };
        let r = execute_edit(TEST_TOML, "test-id", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s2 = config.rules.iter().find(|r| r.name == "s2").unwrap();
        assert!(s2.depends_on.is_empty(), "dep should be removed");
    }

    #[test]
    fn test_undo_redo() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "ux", "shell": "ux"}),
        };
        execute_edit(TEST_TOML, "ur-test", &cmd).unwrap();
        let undone = undo("ur-test").unwrap();
        assert!(undone.is_some());
        assert!(
            !undone.unwrap().contains("ux"),
            "undo should restore original"
        );
    }

    #[test]
    fn test_edit_then_validate() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "s3", "shell": "echo s3"}),
        };
        let r = execute_edit(TEST_TOML, "v-test", &cmd).unwrap();
        assert!(r.success);
        assert!(r.success, "edit should succeed");
    }
}
