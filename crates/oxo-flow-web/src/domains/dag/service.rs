//! DAG Edit service — command queue with undo/redo and DAG validation.
//!
//! Edits mutate a `toml_edit::DocumentMut` parsed from the raw TOML so the
//! author's comments and formatting survive every canvas/inspector edit
//! (issue #82 P2-3). Validation still runs through
//! `oxo_flow_core::config::WorkflowConfig::parse` (via `validate_pipeline`)
//! — the document is never re-serialized through the canonical
//! `format_workflow`.

use oxo_flow_core::WorkflowConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

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

/// Convert a serde_json::Value into a toml_edit Value (all JSON types map to
/// the TOML equivalents; JSON `null` maps to an empty string, mirroring the
/// legacy `json_to_toml` fallback — callers building tables filter nulls).
fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) => Value::from(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(f) = n.as_f64() {
                Value::from(f)
            } else {
                Value::from(0i64)
            }
        }
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for e in a {
                arr.push(json_to_value(e));
            }
            Value::Array(arr)
        }
        serde_json::Value::Object(o) => {
            let mut it = toml_edit::InlineTable::new();
            for (k, v) in o {
                if !v.is_null() {
                    it.insert(k, json_to_value(v));
                }
            }
            Value::InlineTable(it)
        }
        serde_json::Value::Null => Value::from(""),
    }
}

/// Apply a JSON object patch to a toml_edit table: a `null` patch value
/// removes the key (TOML cannot express null), anything else replaces it.
fn apply_patch(table: &mut Table, patch: &serde_json::Map<String, serde_json::Value>) {
    for (k, v) in patch {
        if v.is_null() {
            table.remove(k);
        } else {
            table.insert(k, Item::Value(json_to_value(v)));
        }
    }
}

/// Patch the workflow root table: `null` removes the key; an object patch on
/// a key that is already a table (`[workflow]`, `[config]`, …) is merged into
/// it so the section's comments survive (issue #82 P2-3); everything else is
/// replaced wholesale, exactly as before.
fn apply_root_patch(root: &mut Table, patch: &serde_json::Map<String, serde_json::Value>) {
    for (k, v) in patch {
        if v.is_null() {
            root.remove(k);
            continue;
        }
        let Some(obj) = v.as_object() else {
            root.insert(k, Item::Value(json_to_value(v)));
            continue;
        };
        match root.get_mut(k) {
            Some(Item::Table(t)) => apply_patch(t, obj),
            Some(item) if item.is_inline_table() => {
                let it = item.as_inline_table_mut().expect("checked above");
                for (ik, iv) in obj {
                    if iv.is_null() {
                        it.remove(ik);
                    } else {
                        it.insert(ik, json_to_value(iv));
                    }
                }
            }
            _ => {
                root.insert(k, Item::Value(json_to_value(v)));
            }
        }
    }
}

/// Borrow the workflow's `[[rules]]` array-of-tables. The legacy inline form
/// (`rules = [ { … }, … ]` — valid TOML, accepted by core) is rewritten to
/// `[[rules]]` in place so every mutation below is uniform.
fn rules_array_of_tables_mut(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables, String> {
    let root = doc.as_table_mut();
    let is_inline_array = root
        .get("rules")
        .is_some_and(|item| item.as_value().is_some_and(Value::is_array));
    if is_inline_array {
        let Some(Item::Value(Value::Array(arr))) = root.remove("rules") else {
            return Err("workflow has no [[rules]] array".into());
        };
        let mut aot = ArrayOfTables::new();
        for val in arr.iter() {
            let table = match val {
                Value::InlineTable(it) => {
                    let mut t = Table::new();
                    for (k, v) in it.iter() {
                        t.insert(k, Item::Value(v.clone()));
                    }
                    t
                }
                _ => return Err("workflow has no [[rules]] array".into()),
            };
            aot.push(table);
        }
        root.insert("rules", Item::ArrayOfTables(aot));
    }
    if root.get("rules").is_none() {
        return Err("workflow has no [[rules]] array".into());
    }
    root.get_mut("rules")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| "workflow has no [[rules]] array".into())
}

/// The rule's `name` key, when present as a string.
fn rule_name(rule: &Table) -> Option<&str> {
    rule.get("name").and_then(Item::as_str)
}

/// Remove every occurrence of `target` from the rule's `depends_on` array and
/// drop the key entirely once it empties — mirrors core's
/// `skip_serializing_if = "Vec::is_empty"` so the TOML stays clean.
fn strip_dependency(rule: &mut Table, target: &str) {
    let mut emptied = false;
    if let Some(arr) = rule
        .get_mut("depends_on")
        .and_then(Item::as_value_mut)
        .and_then(Value::as_array_mut)
    {
        let hits: Vec<usize> = arr
            .iter()
            .enumerate()
            .filter(|(_, v)| v.as_str() == Some(target))
            .map(|(i, _)| i)
            .collect();
        for i in hits.into_iter().rev() {
            arr.remove(i);
        }
        emptied = arr.is_empty();
    }
    if emptied {
        rule.remove("depends_on");
    }
}

/// Parse TOML, apply edit on the document, validate.
pub fn execute_edit(
    toml_content: &str,
    pipeline_id: &str,
    command: &DagEditCommand,
) -> Result<DagEditResponse, String> {
    // Gate: the workflow must parse before any mutation — same gate and
    // error message as before.
    WorkflowConfig::parse(toml_content).map_err(|e| format!("Parse: {e}"))?;
    let mut doc: DocumentMut = toml_content.parse().map_err(|e| format!("TOML: {e}"))?;

    // Apply operation
    match command.operation.as_str() {
        "add_rule" => {
            if let Some(rule_val) = command.payload.get("rule") {
                // Full rule table: append as a new [[rules]] entry and let
                // core parsing validate the field set.
                let rules = rules_array_of_tables_mut(&mut doc)?;
                let mut table = Table::new();
                if let serde_json::Value::Object(map) = rule_val {
                    for (k, v) in map {
                        if !v.is_null() {
                            table.insert(k, Item::Value(json_to_value(v)));
                        }
                    }
                }
                rules.push(table);
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
                // Works on documents without a rules array yet — the old
                // struct-level path re-emitted [[rules]] via format_workflow.
                if doc.as_table().get("rules").is_none() {
                    doc.as_table_mut()
                        .insert("rules", Item::ArrayOfTables(ArrayOfTables::new()));
                }
                let rules = rules_array_of_tables_mut(&mut doc)?;
                let mut table = Table::new();
                table.insert("name", Item::Value(Value::from(name)));
                table.insert("shell", Item::Value(Value::from(shell_val)));
                rules.push(table);
            }
        }
        "remove_rule" => {
            let name = command.payload["name"]
                .as_str()
                .ok_or("Missing rule name")?;
            // A workflow without a rules array has nothing to remove — the
            // old struct-level retain was a no-op there.
            if doc.as_table().get("rules").is_some() {
                let rules = rules_array_of_tables_mut(&mut doc)?;
                let mut idx = 0;
                while idx < rules.len() {
                    if rules
                        .iter()
                        .nth(idx)
                        .is_some_and(|rule| rule_name(rule) == Some(name))
                    {
                        rules.remove(idx);
                    } else {
                        idx += 1;
                    }
                }
                // Strip the removed rule out of every remaining rule's
                // depends_on.
                for rule in rules.iter_mut() {
                    strip_dependency(rule, name);
                }
            }
        }
        "connect" => {
            let from = command.payload["from"].as_str().ok_or("Missing from")?;
            let to = command.payload["to"].as_str().ok_or("Missing to")?;
            let rules = rules_array_of_tables_mut(&mut doc)?;
            let target = rules
                .iter_mut()
                .find(|rule| rule_name(rule) == Some(to))
                .ok_or_else(|| format!("Target rule '{to}' not found"))?;
            if target.get("depends_on").is_some() {
                let arr = target
                    .get_mut("depends_on")
                    .and_then(Item::as_value_mut)
                    .and_then(Value::as_array_mut)
                    .ok_or("depends_on must be an array")?;
                if !arr.iter().any(|v| v.as_str() == Some(from)) {
                    arr.push(Value::from(from.to_string()));
                }
            } else {
                let mut arr = toml_edit::Array::new();
                arr.push(Value::from(from.to_string()));
                target.insert("depends_on", Item::Value(Value::Array(arr)));
            }
        }
        "disconnect" => {
            let from = command.payload["from"].as_str().ok_or("Missing from")?;
            let to = command.payload["to"].as_str().ok_or("Missing to")?;
            let rules = rules_array_of_tables_mut(&mut doc)?;
            let target = rules
                .iter_mut()
                .find(|rule| rule_name(rule) == Some(to))
                .ok_or_else(|| format!("Target rule '{to}' not found"))?;
            strip_dependency(target, from);
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
            let rules = rules_array_of_tables_mut(&mut doc)?;
            let rule = rules
                .iter_mut()
                .find(|rule| rule_name(rule) == Some(name))
                .ok_or_else(|| format!("Rule '{name}' not found"))?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            apply_patch(rule, patch_table);
        }
        "update_workflow" => {
            let patch = command
                .payload
                .get("patch")
                .ok_or("update_workflow requires a 'patch' table")?;
            let patch_table = patch.as_object().ok_or("patch must be a table")?;
            apply_root_patch(doc.as_table_mut(), patch_table);
        }
        _ => return Err(format!("Unknown operation: {}", command.operation)),
    }

    // Serialize the mutated document — toml_edit keeps every comment and
    // formatting choice of the original (issue #82 P2-3). No canonical
    // re-serialization through format_workflow.
    let new_toml = doc.to_string();

    // Validate: the edited document must still parse as a WorkflowConfig
    // (validate_pipeline re-parses it) and pass the workflow lints.
    let validation = crate::domains::workflow::service::validate_pipeline(&new_toml, None)?;
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

    // -----------------------------------------------------------------------
    // Comment/formatting preservation (issue #82 P2-3)
    // -----------------------------------------------------------------------

    const COMMENTED_TOML: &str = "\
# ============================================================
# demo pipeline — hand-written, comments must survive edits
# ============================================================

[workflow]
name = \"test\"

# first step: trim adapters
[[rules]]
name = \"trim\"  # the trimming rule
shell = \"fastp --in1 {input} --out1 {output}\"
input = [\"raw/R1.fastq.gz\"]
output = [\"trimmed/R1.fastq.gz\"]

# second step: align against the reference
[[rules]]
name = \"align\"
shell = \"bwa mem ref.fa {input} > {output}\"
depends_on = [\"trim\"]

# third step: index the alignments
[[rules]]
name = \"index\"
shell = \"samtools index {input}\"
depends_on = [\"align\"]
";

    #[test]
    fn update_rule_preserves_comments_and_blank_lines_around_edited_rule() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({
                "name": "trim",
                "patch": {"shell": "fastp --in1 {input} --out1 {output} --trim_poly_g"}
            }),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-comments", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(r.success, "{:?}", r.validation_errors);
        // The patch applied.
        assert!(out.contains("--trim_poly_g"));
        // Header, section headers, blank lines and inline comments around the
        // edited rule survive.
        assert!(out.contains("# ============================================================"));
        assert!(out.contains("# first step: trim adapters"));
        assert!(out.contains("# the trimming rule"));
        assert!(out.contains("# second step: align against the reference"));
        assert!(out.contains("# third step: index the alignments"));
        assert!(out.contains("depends_on = [\"align\"]"));
        assert!(out.contains("\n\n# first step"));
    }

    #[test]
    fn untouched_rules_keep_exact_formatting() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "trim", "patch": {"shell": "fastp --trim_poly_g"}}),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-format", &cmd).unwrap();
        let out = &r.toml_content;
        // Only the patched value line may differ — everything else, including
        // the untouched rules' blocks, must be byte-identical.
        let expected = COMMENTED_TOML.replacen(
            "shell = \"fastp --in1 {input} --out1 {output}\"",
            "shell = \"fastp --trim_poly_g\"",
            1,
        );
        assert_eq!(out, &expected);
    }

    #[test]
    fn remove_rule_strips_dependencies_and_preserves_other_comments() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "remove_rule".into(),
            payload: serde_json::json!({"name": "trim"}),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-remove", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(r.success, "{:?}", r.validation_errors);
        assert!(!out.contains("name = \"trim\""));
        // The removed rule's depends_on reference is gone from align, but the
        // align block and its section header survive untouched.
        assert!(!out.contains("depends_on = [\"trim\"]"));
        assert!(out.contains("# second step: align against the reference"));
        assert!(out.contains("shell = \"bwa mem ref.fa {input} > {output}\""));
        assert!(out.contains("# third step: index the alignments"));
        let config = WorkflowConfig::parse(out).unwrap();
        let align = config.rules.iter().find(|r| r.name == "align").unwrap();
        assert!(align.depends_on.is_empty());
        let index = config.rules.iter().find(|r| r.name == "index").unwrap();
        assert_eq!(index.depends_on, vec!["align"]);
    }

    #[test]
    fn connect_creates_depends_on_when_absent() {
        let toml = "\
# pipeline
[workflow]
name = \"test\"

[[rules]]
name = \"s1\"
shell = \"echo s1\"

# the rule being connected
[[rules]]
name = \"s2\"
shell = \"echo s2\"
";
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "connect".into(),
            payload: serde_json::json!({"from": "s1", "to": "s2"}),
        };
        let r = execute_edit(toml, "p2-connect", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(out.contains("# the rule being connected"));
        let config = WorkflowConfig::parse(out).unwrap();
        let s2 = config.rules.iter().find(|r| r.name == "s2").unwrap();
        assert_eq!(s2.depends_on, vec!["s1"]);
    }

    #[test]
    fn disconnect_drops_empty_depends_on_key() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "disconnect".into(),
            payload: serde_json::json!({"from": "align", "to": "index"}),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-disconnect", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(r.success, "{:?}", r.validation_errors);
        // The emptied key disappears entirely from index — keeps the TOML
        // clean (core skips serializing empty depends_on); align's own
        // depends_on is untouched.
        assert_eq!(
            out.matches("depends_on").count(),
            1,
            "only align's depends_on should remain"
        );
        assert!(out.contains("depends_on = [\"trim\"]"));
        assert!(out.contains("# third step: index the alignments"));
        let config = WorkflowConfig::parse(out).unwrap();
        let index = config.rules.iter().find(|r| r.name == "index").unwrap();
        assert!(index.depends_on.is_empty());
    }

    #[test]
    fn add_rule_appends_after_existing_rules_keeping_comments() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({
                "rule": {
                    "name": "qc",
                    "shell": "multiqc .",
                    "depends_on": ["align", "index"]
                }
            }),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-add", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(r.success, "{:?}", r.validation_errors);
        // Existing comments survive and the new rule lands at the end.
        assert!(out.contains("# first step: trim adapters"));
        assert!(out.contains("# third step: index the alignments"));
        let pos_rules = out.find("[[rules]]").unwrap();
        let pos_qc = out.find("name = \"qc\"").unwrap();
        assert!(pos_qc > pos_rules, "new rule must be appended");
        let config = WorkflowConfig::parse(out).unwrap();
        let qc = config.rules.iter().find(|r| r.name == "qc").unwrap();
        assert_eq!(qc.depends_on, vec!["align", "index"]);
    }

    #[test]
    fn update_workflow_keeps_section_header_comments() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_workflow".into(),
            payload: serde_json::json!({"patch": {"workflow": {"name": "renamed"}}}),
        };
        let r = execute_edit(COMMENTED_TOML, "p2-wf", &cmd).unwrap();
        let out = &r.toml_content;
        assert!(r.success, "{:?}", r.validation_errors);
        assert!(out.contains("# demo pipeline — hand-written, comments must survive edits"));
        assert!(out.contains("# ============================================================"));
        assert!(out.contains("# first step: trim adapters"));
        let config = WorkflowConfig::parse(out).unwrap();
        assert_eq!(config.workflow.name, "renamed");
    }

    #[test]
    fn undo_returns_previous_toml_verbatim() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "trim", "patch": {"shell": "fastp --trim_poly_g"}}),
        };
        let edited = execute_edit(COMMENTED_TOML, "p2-undo", &cmd).unwrap();
        let undone = undo("p2-undo", &edited.toml_content).unwrap().unwrap();
        assert_eq!(
            undone, COMMENTED_TOML,
            "undo must return the exact original"
        );
    }

    #[test]
    fn update_rule_without_rules_array_errors() {
        let toml = "[workflow]\nname = \"test\"\n";
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "s1", "patch": {"shell": "x"}}),
        };
        let err = execute_edit(toml, "p2-no-rules", &cmd).unwrap_err();
        assert!(
            err.contains("workflow has no [[rules]] array"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn connect_targets_unknown_rule_with_not_found_error() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "connect".into(),
            payload: serde_json::json!({"from": "s1", "to": "nope"}),
        };
        let err = execute_edit(TEST_TOML, "p2-connect-missing", &cmd).unwrap_err();
        assert_eq!(err, "Target rule 'nope' not found");
    }
}
