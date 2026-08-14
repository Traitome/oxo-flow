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
/// Undo/redo history per draft id: each entry is a (from, to) TOML
/// transition, so redo can actually restore the post-edit state (issue #79
/// P1-09: the old design stored only pre-edit snapshots, so redo could not
/// produce the state it claimed to restore — and never pushed back either).
static EDIT_STACKS: std::sync::LazyLock<
    Mutex<HashMap<String, (Vec<(String, String)>, Vec<(String, String)>)>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn stack_id(pipeline_id: &str) -> String {
    pipeline_id.to_string()
}

/// Convert a serde_json::Value into a toml::Value (all JSON types map to the
/// TOML equivalents; null is filtered by callers building tables).
fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::Integer(0)
            }
        }
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Array(a) => toml::Value::Array(a.iter().map(json_to_toml).collect()),
        serde_json::Value::Object(o) => toml::Value::Table(
            o.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), json_to_toml(v)))
                .collect(),
        ),
        serde_json::Value::Null => toml::Value::String(String::new()),
    }
}

/// Replace the in-memory config with one parsed from `doc` (TOML-value-level
/// edits round-trip through core parsing so validation stays the gate).
fn reparse(doc: &toml::Value) -> Result<WorkflowConfig, String> {
    let patched = toml::to_string(doc).map_err(|e| format!("TOML: {e}"))?;
    WorkflowConfig::parse(&patched).map_err(|e| format!("Parse: {e}"))
}

/// Parse TOML, apply edit, format back, validate.
pub fn execute_edit(
    toml_content: &str,
    pipeline_id: &str,
    command: &DagEditCommand,
) -> Result<DagEditResponse, String> {
    let mut config = WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;

    // Apply operation
    match command.operation.as_str() {
        "add_rule" => {
            if let Some(rule_val) = command.payload.get("rule") {
                // Full rule table: append as a new [[rules]] entry and let
                // core parsing validate the field set.
                let mut doc: toml::Value =
                    toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
                let rules = doc
                    .get_mut("rules")
                    .and_then(|v| v.as_array_mut())
                    .ok_or("workflow has no [[rules]] array")?;
                rules.push(json_to_toml(rule_val));
                config = reparse(&doc)?;
            } else {
                // Legacy shape: name + shell only.
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
        "update_rule" | "update_params" => {
            let name = command.payload["name"]
                .as_str()
                .ok_or("Missing rule name")?;
            // `update_rule` takes a TOML-table patch; the legacy
            // `update_params` shape ({threads, shell}) maps onto the same
            // mechanism when no patch is present.
            let patch = match command.payload.get("patch") {
                Some(p) => p.clone(),
                None if command.operation == "update_params" => {
                    let mut legacy = serde_json::Map::new();
                    if let Some(v) = command.payload.get("threads") {
                        legacy.insert("threads".into(), v.clone());
                    }
                    if let Some(v) = command.payload.get("shell") {
                        legacy.insert("shell".into(), v.clone());
                    }
                    serde_json::Value::Object(legacy)
                }
                None => return Err("update_rule requires a 'patch' table".into()),
            };
            let mut doc: toml::Value =
                toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
            let rules = doc
                .get_mut("rules")
                .and_then(|v| v.as_array_mut())
                .ok_or("workflow has no [[rules]] array")?;
            let rule = rules
                .iter_mut()
                .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(name))
                .ok_or_else(|| format!("Rule '{name}' not found"))?;
            let table = rule.as_table_mut().ok_or("rule entry is not a table")?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            for (k, v) in patch_table {
                // TOML cannot express null: `null` in a patch means "remove
                // this key" (e.g. dropping an environment or retries).
                if v.is_null() {
                    table.remove(k);
                } else {
                    table.insert(k.clone(), json_to_toml(v));
                }
            }
            config = reparse(&doc)?;
        }
        "update_workflow" => {
            let patch = command
                .payload
                .get("patch")
                .ok_or("update_workflow requires a 'patch' table")?;
            let mut doc: toml::Value =
                toml::from_str(toml_content).map_err(|e| format!("TOML: {e}"))?;
            let table = doc.as_table_mut().ok_or("workflow TOML is not a table")?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            for (k, v) in patch_table {
                if v.is_null() {
                    table.remove(k);
                } else {
                    table.insert(k.clone(), json_to_toml(v));
                }
            }
            config = reparse(&doc)?;
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

    // Record the (from, to) transition only now — rejected edits leave the
    // history untouched (atomic rollback).
    {
        let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
        let entry = stacks.entry(stack_id(pipeline_id)).or_default();
        entry.0.push((toml_content.to_string(), new_toml.clone()));
        if entry.0.len() > 50 {
            entry.0.remove(0);
        }
        entry.1.clear();
    }

    Ok(DagEditResponse {
        success: validation.valid,
        toml_content: new_toml,
        validation_errors: errors,
    })
}

/// Undo the latest edit, given the client's current TOML.
///
/// The (from, to) transition is verified against `current` — a stale or
/// racing client simply gets `None` ("nothing to undo") instead of
/// corrupting its state.
pub fn undo(pipeline_id: &str, current: &str) -> Result<Option<String>, String> {
    let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
    let entry = stacks.entry(stack_id(pipeline_id)).or_default();
    // The top transition's `to` must equal the client's current state.
    // (Checked before popping — the borrow of `last()` cannot overlap the
    // mutable `pop()`.)
    let top_matches = entry.0.last().is_some_and(|(_, to)| to.as_str() == current);
    if top_matches && let Some((from, to)) = entry.0.pop() {
        entry.1.push((from.clone(), to));
        Ok(Some(from))
    } else {
        Ok(None)
    }
}

/// Redo the latest undone edit, given the client's current TOML.
///
/// Pushes the transition back onto the undo stack (issue #79 P1-09: the old
/// redo popped without pushing back, so the next undo had nothing to undo).
pub fn redo(pipeline_id: &str, current: &str) -> Result<Option<String>, String> {
    let mut stacks = EDIT_STACKS.lock().map_err(|_| "Lock poisoned")?;
    let entry = stacks.entry(stack_id(pipeline_id)).or_default();
    let top_matches = entry
        .1
        .last()
        .is_some_and(|(from, _)| from.as_str() == current);
    if top_matches && let Some((from, to)) = entry.1.pop() {
        entry.0.push((from, to.clone()));
        Ok(Some(to))
    } else {
        Ok(None)
    }
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
    fn test_undo_redo_roundtrip() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "ux", "shell": "ux"}),
        };
        let edited = execute_edit(TEST_TOML, "ur-test", &cmd).unwrap();
        assert!(edited.toml_content.contains("ux"));

        // Undo with the edited state as current.
        let undone = undo("ur-test", &edited.toml_content).unwrap().unwrap();
        assert!(
            !undone.contains("ux"),
            "undo should restore the pre-edit state"
        );

        // Redo with the undone state as current — must restore the edit
        // (the old redo returned the wrong state and never pushed back).
        let redone = redo("ur-test", &undone).unwrap().unwrap();
        assert!(redone.contains("ux"), "redo must restore the edit");

        // And undo again — the redo must have pushed the pair back, so the
        // stack is not exhausted (issue #79 P1-09 "redo 不回填栈").
        let undone_again = undo("ur-test", &redone).unwrap().unwrap();
        assert!(!undone_again.contains("ux"));
    }

    #[test]
    fn test_undo_rejects_stale_client_state() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "ux", "shell": "ux"}),
        };
        let edited = execute_edit(TEST_TOML, "stale-test", &cmd).unwrap();
        // A client whose state no longer matches the top transition gets
        // None instead of a corrupting rollback.
        assert!(undo("stale-test", TEST_TOML).unwrap().is_none());
        assert!(undo("stale-test", &edited.toml_content).unwrap().is_some());
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

    #[test]
    fn add_rule_with_full_spec() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({
                "rule": {
                    "name": "fastp_trim",
                    "description": "Trim adapters",
                    "input": ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"],
                    "output": ["trimmed/{sample}_R1.fastq.gz", "trimmed/{sample}_R2.fastq.gz"],
                    "shell": "fastp --in1 {input[0]} --in2 {input[1]} --out1 {output[0]} --out2 {output[1]}",
                    "environment": {"conda": "envs/fastp.yaml"},
                    "resources": {"threads": 8, "memory": "16G"},
                    "retries": 2
                }
            }),
        };
        let r = execute_edit(TEST_TOML, "full-spec", &cmd).unwrap();
        assert!(r.success, "{:?}", r.validation_errors);
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let rule = config
            .rules
            .iter()
            .find(|r| r.name == "fastp_trim")
            .unwrap();
        assert_eq!(
            rule.input,
            vec![
                "raw/{sample}_R1.fastq.gz".to_string(),
                "raw/{sample}_R2.fastq.gz".to_string()
            ]
            .into()
        );
        assert_eq!(rule.retries, 2);
        assert_eq!(rule.resources.threads, 8);
    }

    #[test]
    fn update_rule_patches_fields_without_touching_others() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({
                "name": "s1",
                "patch": {
                    "input": ["data/in.fastq"],
                    "output": ["res/out.txt"],
                    "shell": "process {input} > {output}"
                }
            }),
        };
        let r = execute_edit(TEST_TOML, "patch-1", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s1 = config.rules.iter().find(|r| r.name == "s1").unwrap();
        assert_eq!(s1.shell.as_deref(), Some("process {input} > {output}"));
        assert_eq!(s1.input, vec!["data/in.fastq".to_string()].into());
    }

    #[test]
    fn update_rule_null_patch_key_removes_field() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "s1", "patch": {"threads": null, "shell": "x"}}),
        };
        let r = execute_edit(TEST_TOML, "patch-null", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s1 = config.rules.iter().find(|r| r.name == "s1").unwrap();
        assert_eq!(s1.shell.as_deref(), Some("x"));
        assert!(s1.threads.is_none(), "null patch key must remove the field");
    }

    #[test]
    fn update_rule_rejects_unknown_rule() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "nope", "patch": {"shell": "x"}}),
        };
        assert!(execute_edit(TEST_TOML, "patch-2", &cmd).is_err());
    }

    #[test]
    fn update_workflow_replaces_sections() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_workflow".into(),
            payload: serde_json::json!({
                "patch": {"workflow": {"name": "renamed", "version": "2.0.0", "description": "d"}}
            }),
        };
        let r = execute_edit(TEST_TOML, "wf-patch", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        assert_eq!(config.workflow.name, "renamed");
        assert_eq!(config.workflow.version, "2.0.0");
    }
}
