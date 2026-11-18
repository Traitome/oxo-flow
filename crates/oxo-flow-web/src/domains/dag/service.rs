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

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagEditCommand {
    pub source: String,
    pub operation: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagEditResponse {
    pub success: bool,
    pub toml_content: String,
    pub validation_errors: Vec<String>,
}

/// Maximum number of undo transitions retained per (identity, pipeline)
/// history stack. Pushing an edit onto a full stack drops the oldest
/// transition, so undo can walk back at most this many edits.
pub const MAX_EDIT_HISTORY_DEPTH: usize = 50;

/// Maximum number of (identity, pipeline) history stacks kept process-wide.
/// Recording an edit for a new pair evicts the least-recently-used stack, so
/// a long-lived server serving many pipeline ids cannot grow without bound.
pub const MAX_EDIT_HISTORY_PIPELINES: usize = 256;

/// Outcome of an undo/redo request (see [`undo`] and [`redo`]).
///
/// The non-`Applied` variants are fail-closed: the caller is told why no
/// edit was produced instead of receiving a silently-empty result or a
/// guessed state (issue #1137).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryOutcome {
    /// A transition was applied; the value is the destination TOML state.
    Applied(String),
    /// No history is recorded for this (identity, pipeline) pair — the
    /// first edit has not happened yet, or the stack was lost (server
    /// restart, LRU eviction). The HTTP layer answers
    /// `409 EDIT_HISTORY_UNAVAILABLE`; an undo can never be guessed from
    /// the client's document alone.
    Unavailable,
    /// History exists, but the caller's `current` TOML is not the tip of
    /// the recorded history (stale or racing client). Nothing is applied;
    /// the HTTP layer answers `409 EDIT_HISTORY_STALE`.
    Stale,
    /// History exists but the requested direction has nothing left to apply
    /// (every recorded edit was already undone, or nothing was undone to
    /// redo). Both stacks empty means no history at all ([`Self::Unavailable`]).
    Exhausted,
}

/// Undo/redo history key: the owning identity *and* the pipeline id.
///
/// Two users editing pipelines that share an id own independent stacks, so
/// neither can observe or undo the other's edits (issue #1137).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HistoryKey {
    owner: String,
    pipeline_id: String,
}

/// One (owner, pipeline) stack. Each undo entry is a `(from, to)` TOML
/// transition, so redo can restore the post-edit state (issue #79 P1-09:
/// the old design stored only pre-edit snapshots, so redo could not
/// produce the state it claimed to restore — and never pushed back either).
#[derive(Debug, Default)]
struct HistoryEntry {
    undo: Vec<(String, String)>,
    redo: Vec<(String, String)>,
    /// Monotonic use counter driving LRU eviction.
    last_used: u64,
}

/// In-memory undo/redo history with documented bounds.
///
/// - **Scope**: one stack per (owning identity, pipeline id). The identity
///   is the `CurrentUser.id` the auth middleware resolved for the request
///   (`default` in personal mode).
/// - **Bounds**: [`MAX_EDIT_HISTORY_DEPTH`] transitions per stack (oldest
///   dropped), [`MAX_EDIT_HISTORY_PIPELINES`] stacks process-wide (LRU).
/// - **Lifetime**: RAM only. A restart empties every stack, and undo/redo
///   then fail closed with `EDIT_HISTORY_UNAVAILABLE` — the server never
///   guesses an undo target from the client's document (issue #1137).
#[derive(Debug)]
struct EditHistory {
    stacks: HashMap<HistoryKey, HistoryEntry>,
    clock: u64,
    max_depth: usize,
    max_pipelines: usize,
}

impl EditHistory {
    fn new(max_depth: usize, max_pipelines: usize) -> Self {
        Self {
            stacks: HashMap::new(),
            clock: 0,
            // A zero cap would mean "record nothing"; clamp to one instead
            // so a misconfiguration cannot silently disable undo entirely.
            max_depth: max_depth.max(1),
            max_pipelines: max_pipelines.max(1),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Record a successful `(from, to)` edit, refreshing the stack's LRU
    /// position, dropping the oldest transition past the depth cap, and
    /// clearing redo (standard undo/redo semantics).
    fn record(&mut self, owner: &str, pipeline_id: &str, from: &str, to: &str) {
        let tick = self.tick();
        let key = HistoryKey {
            owner: owner.to_string(),
            pipeline_id: pipeline_id.to_string(),
        };
        let entry = self.stacks.entry(key.clone()).or_default();
        entry.undo.push((from.to_string(), to.to_string()));
        if entry.undo.len() > self.max_depth {
            entry.undo.remove(0);
        }
        entry.redo.clear();
        entry.last_used = tick;
        self.evict_lru(&key);
    }

    /// Drop least-recently-used stacks until the process-wide cap holds.
    /// The stack just touched is never the victim.
    fn evict_lru(&mut self, keep: &HistoryKey) {
        while self.stacks.len() > self.max_pipelines {
            let victim = self
                .stacks
                .iter()
                .filter(|(key, _)| *key != keep)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            match victim {
                Some(key) => {
                    self.stacks.remove(&key);
                }
                None => break,
            }
        }
    }

    fn undo(&mut self, owner: &str, pipeline_id: &str, current: &str) -> HistoryOutcome {
        let key = HistoryKey {
            owner: owner.to_string(),
            pipeline_id: pipeline_id.to_string(),
        };
        let tick = self.tick();
        let Some(entry) = self.stacks.get_mut(&key) else {
            return HistoryOutcome::Unavailable;
        };
        entry.last_used = tick;
        if entry.undo.is_empty() {
            return if entry.redo.is_empty() {
                HistoryOutcome::Unavailable
            } else {
                HistoryOutcome::Exhausted
            };
        }
        // The top transition's `to` must equal the client's current state;
        // checked before popping so a stale client changes nothing.
        let top_matches = entry.undo.last().is_some_and(|(_, to)| to == current);
        if !top_matches {
            return HistoryOutcome::Stale;
        }
        let (from, to) = entry.undo.pop().expect("checked non-empty");
        entry.redo.push((from.clone(), to));
        HistoryOutcome::Applied(from)
    }

    fn redo(&mut self, owner: &str, pipeline_id: &str, current: &str) -> HistoryOutcome {
        let key = HistoryKey {
            owner: owner.to_string(),
            pipeline_id: pipeline_id.to_string(),
        };
        let tick = self.tick();
        let Some(entry) = self.stacks.get_mut(&key) else {
            return HistoryOutcome::Unavailable;
        };
        entry.last_used = tick;
        if entry.redo.is_empty() {
            return if entry.undo.is_empty() {
                HistoryOutcome::Unavailable
            } else {
                HistoryOutcome::Exhausted
            };
        }
        let top_matches = entry.redo.last().is_some_and(|(from, _)| from == current);
        if !top_matches {
            return HistoryOutcome::Stale;
        }
        let (from, to) = entry.redo.pop().expect("checked non-empty");
        entry.undo.push((from, to.clone()));
        HistoryOutcome::Applied(to)
    }
}

static EDIT_HISTORY: std::sync::LazyLock<Mutex<EditHistory>> = std::sync::LazyLock::new(|| {
    Mutex::new(EditHistory::new(
        MAX_EDIT_HISTORY_DEPTH,
        MAX_EDIT_HISTORY_PIPELINES,
    ))
});

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

/// Rewrite every reference to rule `old` as `new` inside one rule table:
/// `depends_on` entries, `extends`, and `scatter.gather` (table or inline).
fn rename_references(rule: &mut Table, old: &str, new: &str) {
    if let Some(arr) = rule
        .get_mut("depends_on")
        .and_then(Item::as_value_mut)
        .and_then(Value::as_array_mut)
    {
        for v in arr.iter_mut() {
            if v.as_str() == Some(old) {
                let decor = v.decor().clone();
                *v = Value::from(new.to_string());
                *v.decor_mut() = decor;
            }
        }
    }
    if rule.get("extends").and_then(Item::as_str) == Some(old) {
        rule.insert("extends", Item::Value(Value::from(new.to_string())));
    }
    if let Some(scatter) = rule.get_mut("scatter").and_then(Item::as_table_like_mut)
        && scatter.get("gather").and_then(Item::as_str) == Some(old)
    {
        scatter.insert("gather", Item::Value(Value::from(new.to_string())));
    }
}

/// Reject a `connect` that would make the workflow cyclic. Uses the core
/// DAG builder (the same one `run` uses, with config expansion) so file-
/// inferred edges participate: `a → b` via files plus a drawn `b → a` is a
/// cycle even though neither `depends_on` alone is.
fn ensure_acyclic_after_connect(new_toml: &str, from: &str, to: &str) -> Result<(), String> {
    let Ok(config) = WorkflowConfig::parse(new_toml) else {
        // Parse failures are reported by the validation gate below.
        return Ok(());
    };
    match oxo_flow_core::dag::WorkflowDag::from_rules_with_config(
        &config.rules,
        &config.config_placeholder_values(),
    ) {
        Err(oxo_flow_core::OxoFlowError::CycleDetected { details }) => Err(format!(
            "Connecting '{from}' → '{to}' would create a cycle ({details})"
        )),
        _ => Ok(()),
    }
}

/// Parse TOML, apply edit on the document, validate.
///
/// `owner` is the acting user's canonical id (the same value the pipeline
/// handlers resolve); it scopes the recorded undo history so identities
/// never share a stack.
pub fn execute_edit(
    owner: &str,
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
            if from == to {
                // The core DAG builder drops self-edges on purpose (directory
                // write-back patterns), so cycle detection cannot see this —
                // guard at the edit boundary instead.
                return Err("Cannot connect a rule to itself".into());
            }
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
        "rename_rule" => {
            let name = command.payload["name"].as_str().ok_or("Missing name")?;
            let new_name = command.payload["new_name"]
                .as_str()
                .ok_or("Missing new_name")?;
            if new_name.trim().is_empty() {
                return Err("Rule name cannot be empty".into());
            }
            let rules = rules_array_of_tables_mut(&mut doc)?;
            let mut renamed = false;
            for rule in rules.iter_mut() {
                if rule_name(rule) == Some(name) {
                    if let Some(item) = rule.get_mut("name") {
                        // Keep the value decor so a trailing inline comment on
                        // the name line survives the rename.
                        let decor = item.as_value().map(|v| v.decor().clone());
                        let mut val = Value::from(new_name.to_string());
                        if let Some(d) = decor {
                            *val.decor_mut() = d;
                        }
                        *item = Item::Value(val);
                    }
                    renamed = true;
                }
            }
            if !renamed {
                return Err(format!("Rule '{name}' not found"));
            }
            let rules = rules_array_of_tables_mut(&mut doc)?;
            for rule in rules.iter_mut() {
                rename_references(rule, name, new_name);
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

    // A drawn edge must not close a cycle — including one formed with
    // file-inferred edges, which `depends_on` alone cannot see.
    if command.operation == "connect"
        && let (Some(from), Some(to)) = (
            command.payload["from"].as_str(),
            command.payload["to"].as_str(),
        )
    {
        ensure_acyclic_after_connect(&new_toml, from, to)?;
    }

    // Validate: the edited document must still parse as a WorkflowConfig
    // (validate_pipeline re-parses it) and pass the workflow lints.
    let validation = crate::domains::workflow::service::validate_pipeline(&new_toml, None)?;
    let errors: Vec<String> = validation
        .errors
        .iter()
        .map(|e| e.message.clone())
        .collect();

    // Validation failures roll back atomically: the caller's TOML is echoed
    // unchanged, so a client that adopts `toml_content` never desyncs from
    // a document the engine would reject (e.g. buildDag 400s on cycles).
    if !validation.valid {
        return Ok(DagEditResponse {
            success: false,
            toml_content: toml_content.to_string(),
            validation_errors: errors,
        });
    }

    // Record the (from, to) transition only now — rejected edits leave the
    // history untouched (atomic rollback).
    {
        let mut history = EDIT_HISTORY.lock().map_err(|_| "Lock poisoned")?;
        history.record(owner, pipeline_id, toml_content, &new_toml);
    }

    Ok(DagEditResponse {
        success: validation.valid,
        toml_content: new_toml,
        validation_errors: errors,
    })
}

/// Undo the latest edit recorded by `owner` for `pipeline_id`.
///
/// The (from, to) transition is verified against `current`: a stale or
/// racing client gets [`HistoryOutcome::Stale`] and the history is left
/// untouched. No recorded history at all (first undo before any edit, or
/// after a server restart / LRU eviction) gets
/// [`HistoryOutcome::Unavailable`] — fail closed, never a guessed edit.
pub fn undo(owner: &str, pipeline_id: &str, current: &str) -> Result<HistoryOutcome, String> {
    let mut history = EDIT_HISTORY.lock().map_err(|_| "Lock poisoned")?;
    Ok(history.undo(owner, pipeline_id, current))
}

/// Redo the latest edit `owner` undid for `pipeline_id`, given the client's
/// current TOML.
///
/// Pushes the transition back onto the undo stack (issue #79 P1-09: the old
/// redo popped without pushing back, so the next undo had nothing to undo).
/// Fail-closed semantics mirror [`undo`].
pub fn redo(owner: &str, pipeline_id: &str, current: &str) -> Result<HistoryOutcome, String> {
    let mut history = EDIT_HISTORY.lock().map_err(|_| "Lock poisoned")?;
    Ok(history.redo(owner, pipeline_id, current))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_OWNER: &str = "test-owner";

    const TEST_TOML: &str = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\n\n[[rules]]\nname = \"s2\"\nshell = \"echo s2\"\ndepends_on = [\"s1\"]\n";

    #[test]
    fn test_add_rule() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "s3", "shell": "echo s3"}),
        };
        let r = execute_edit(TEST_OWNER, TEST_TOML, "test-id", &cmd).unwrap();
        assert!(r.toml_content.contains("s3"), "should contain new rule");
    }

    #[test]
    fn test_update_params() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "update_params".into(),
            payload: serde_json::json!({"name": "s1", "threads": 8}),
        };
        let r = execute_edit(TEST_OWNER, TEST_TOML, "test-id", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, TEST_TOML, "test-id", &cmd).unwrap();
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
        let edited = execute_edit(TEST_OWNER, TEST_TOML, "ur-test", &cmd).unwrap();
        assert!(edited.toml_content.contains("ux"));

        // Undo with the edited state as current.
        let HistoryOutcome::Applied(undone) =
            undo(TEST_OWNER, "ur-test", &edited.toml_content).unwrap()
        else {
            panic!("expected an applied undo");
        };
        assert!(
            !undone.contains("ux"),
            "undo should restore the pre-edit state"
        );

        // Redo with the undone state as current — must restore the edit
        // (the old redo returned the wrong state and never pushed back).
        let HistoryOutcome::Applied(redone) = redo(TEST_OWNER, "ur-test", &undone).unwrap() else {
            panic!("expected an applied redo");
        };
        assert!(redone.contains("ux"), "redo must restore the edit");

        // And undo again — the redo must have pushed the pair back, so the
        // stack is not exhausted (issue #79 P1-09 "redo 不回填栈").
        let HistoryOutcome::Applied(undone_again) = undo(TEST_OWNER, "ur-test", &redone).unwrap()
        else {
            panic!("expected a second applied undo");
        };
        assert!(!undone_again.contains("ux"));
    }

    #[test]
    fn test_undo_rejects_stale_client_state() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "ux", "shell": "ux"}),
        };
        let edited = execute_edit(TEST_OWNER, TEST_TOML, "stale-test", &cmd).unwrap();
        // A client whose state no longer matches the top transition is
        // refused with `Stale` — never a corrupting rollback.
        assert_eq!(
            undo(TEST_OWNER, "stale-test", TEST_TOML).unwrap(),
            HistoryOutcome::Stale
        );
        // The refused undo consumed nothing: the matching state still works.
        assert!(matches!(
            undo(TEST_OWNER, "stale-test", &edited.toml_content).unwrap(),
            HistoryOutcome::Applied(_)
        ));
    }

    #[test]
    fn test_edit_then_validate() {
        let cmd = DagEditCommand {
            source: "dag".into(),
            operation: "add_rule".into(),
            payload: serde_json::json!({"name": "s3", "shell": "echo s3"}),
        };
        let r = execute_edit(TEST_OWNER, TEST_TOML, "v-test", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, TEST_TOML, "full-spec", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, TEST_TOML, "patch-1", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s1 = config.rules.iter().find(|r| r.name == "s1").unwrap();
        assert_eq!(s1.shell.as_deref(), Some("process {input} > {output}"));
        assert_eq!(s1.input, vec!["data/in.fastq".to_string()].into());
    }

    #[test]
    fn update_rule_can_edit_inputs_to_change_file_derived_edges() {
        // The `input`/`output` fields control file-derived edges; `update_rule`
        // patches them like any other field, so the edit API can rewire
        // implicit edges (the "file-based edges are immutable" caveat in the
        // docs predates the `update_rule` patch path).
        let toml = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\noutput = [\"a.txt\"]\n\n[[rules]]\nname = \"s2\"\nshell = \"cat {input}\"\ninput = [\"a.txt\"]\n";
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({
                "name": "s2",
                "patch": {"input": ["b.txt"]}
            }),
        };
        let r = execute_edit(TEST_OWNER, toml, "rewire-1", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s2 = config.rules.iter().find(|r| r.name == "s2").unwrap();
        assert_eq!(s2.input, vec!["b.txt".to_string()].into());
    }

    #[test]
    fn update_rule_null_patch_key_removes_field() {
        // The rule starts with the deprecated shorthand `threads = 4`; a
        // null patch must remove it, so the effective count falls back to
        // the `resources.threads` default (1). Asserting the effective
        // value makes the test meaningful — checking `threads.is_none()`
        // against a fixture that never set it passed vacuously.
        let toml = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo s1\"\nthreads = 4\n";
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "s1", "patch": {"threads": null, "shell": "x"}}),
        };
        let r = execute_edit(TEST_OWNER, toml, "patch-null", &cmd).unwrap();
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let s1 = config.rules.iter().find(|r| r.name == "s1").unwrap();
        assert_eq!(s1.shell.as_deref(), Some("x"));
        assert_eq!(
            s1.effective_threads(),
            1,
            "null patch key must remove the shorthand field"
        );
    }

    #[test]
    fn update_rule_rejects_unknown_rule() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "nope", "patch": {"shell": "x"}}),
        };
        assert!(execute_edit(TEST_OWNER, TEST_TOML, "patch-2", &cmd).is_err());
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
        let r = execute_edit(TEST_OWNER, TEST_TOML, "wf-patch", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-comments", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-format", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-remove", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, toml, "p2-connect", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-disconnect", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-add", &cmd).unwrap();
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
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-wf", &cmd).unwrap();
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
        let edited = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-undo", &cmd).unwrap();
        let HistoryOutcome::Applied(undone) =
            undo(TEST_OWNER, "p2-undo", &edited.toml_content).unwrap()
        else {
            panic!("expected an applied undo");
        };
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
        let err = execute_edit(TEST_OWNER, toml, "p2-no-rules", &cmd).unwrap_err();
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
        let err = execute_edit(TEST_OWNER, TEST_TOML, "p2-connect-missing", &cmd).unwrap_err();
        assert_eq!(err, "Target rule 'nope' not found");
    }

    #[test]
    fn connect_rejects_self_loop() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "connect".into(),
            payload: serde_json::json!({"from": "s1", "to": "s1"}),
        };
        let err = execute_edit(TEST_OWNER, TEST_TOML, "p2-self-loop", &cmd).unwrap_err();
        assert_eq!(err, "Cannot connect a rule to itself");
    }

    #[test]
    fn connect_rejects_declared_cycle_and_leaves_undo_stack_untouched() {
        // TEST_TOML already has s1 → s2; the reverse edge closes the cycle.
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "connect".into(),
            payload: serde_json::json!({"from": "s2", "to": "s1"}),
        };
        let err = execute_edit(TEST_OWNER, TEST_TOML, "p2-cycle", &cmd).unwrap_err();
        assert!(err.contains("cycle"), "unexpected error: {err}");
        assert_eq!(
            undo(TEST_OWNER, "p2-cycle", TEST_TOML).unwrap(),
            HistoryOutcome::Unavailable,
            "a rejected edit must not enter the undo stack"
        );
    }

    #[test]
    fn connect_rejects_cycle_formed_with_file_inferred_edges() {
        let toml = "[workflow]\n\
                    name = \"file-cycle\"\n\
\n\
                    [[rules]]\n\
                    name = \"produce\"\n\
                    output = [\"out.txt\"]\n\
                    shell = \"echo hi > {output[0]}\"\n\
\n\
                    [[rules]]\n\
                    name = \"consume\"\n\
                    input = [\"out.txt\"]\n\
                    output = [\"final.txt\"]\n\
                    shell = \"cp {input[0]} {output[0]}\"\n";
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "connect".into(),
            payload: serde_json::json!({"from": "consume", "to": "produce"}),
        };
        let err = execute_edit(TEST_OWNER, toml, "p2-file-cycle", &cmd).unwrap_err();
        assert!(
            err.contains("cycle"),
            "file-inferred edge + drawn edge must be caught: {err}"
        );
    }

    #[test]
    fn validation_failed_edit_returns_original_toml() {
        // E007: depends_on references an unknown rule → success:false and the
        // caller's content echoed back (no partial state to adopt).
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "s1", "patch": {"depends_on": ["ghost"]}}),
        };
        let r = execute_edit(TEST_OWNER, TEST_TOML, "p2-invalid", &cmd).unwrap();
        assert!(!r.success);
        assert!(!r.validation_errors.is_empty());
        assert_eq!(r.toml_content, TEST_TOML, "failed edit must roll back");
    }

    #[test]
    fn rename_rule_updates_references_and_keeps_comments() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "rename_rule".into(),
            payload: serde_json::json!({"name": "trim", "new_name": "trim_reads"}),
        };
        let r = execute_edit(TEST_OWNER, COMMENTED_TOML, "p2-rename", &cmd).unwrap();
        assert!(r.success, "{:?}", r.validation_errors);
        assert!(
            r.toml_content
                .contains("name = \"trim_reads\"  # the trimming rule")
        );
        assert!(!r.toml_content.contains("\"trim\""));
        let config = WorkflowConfig::parse(&r.toml_content).unwrap();
        let align = config.rules.iter().find(|r| r.name == "align").unwrap();
        assert_eq!(align.depends_on, vec!["trim_reads"]);
    }

    #[test]
    fn rename_rule_rejects_unknown_rule() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "rename_rule".into(),
            payload: serde_json::json!({"name": "nope", "new_name": "x"}),
        };
        let err = execute_edit(TEST_OWNER, TEST_TOML, "p2-rename-missing", &cmd).unwrap_err();
        assert_eq!(err, "Rule 'nope' not found");
    }

    #[test]
    fn rename_rule_rejects_duplicate_target_name() {
        let cmd = DagEditCommand {
            source: "dag_editor".into(),
            operation: "rename_rule".into(),
            payload: serde_json::json!({"name": "s1", "new_name": "s2"}),
        };
        let err = execute_edit(TEST_OWNER, TEST_TOML, "p2-rename-dup", &cmd).unwrap_err();
        assert!(
            err.contains("duplicate rule name"),
            "unexpected error: {err}"
        );
    }

    /// Edit command that changes rule `s1`'s shell to `shell`.
    fn set_s1_shell(shell: &str) -> DagEditCommand {
        DagEditCommand {
            source: "dag_editor".into(),
            operation: "update_rule".into(),
            payload: serde_json::json!({"name": "s1", "patch": {"shell": shell}}),
        }
    }

    #[test]
    fn edit_history_is_scoped_by_owner() {
        let pid = "owner-scoped-pipeline";
        let alice =
            execute_edit("alice-scope", TEST_TOML, pid, &set_s1_shell("echo alice")).unwrap();
        // Bob has no history for the same pipeline id...
        assert_eq!(
            undo("bob-scope", pid, &alice.toml_content).unwrap(),
            HistoryOutcome::Unavailable,
            "another identity must not see (or undo) Alice's edit"
        );
        // ...and Bob's own edit on the same id leaves Alice's stack intact.
        let bob = execute_edit(
            "bob-scope",
            &alice.toml_content,
            pid,
            &set_s1_shell("echo bob"),
        )
        .unwrap();
        let HistoryOutcome::Applied(alice_from) =
            undo("alice-scope", pid, &alice.toml_content).unwrap()
        else {
            panic!("Alice's own undo must still apply");
        };
        assert_eq!(alice_from, TEST_TOML);
        let HistoryOutcome::Applied(bob_from) = undo("bob-scope", pid, &bob.toml_content).unwrap()
        else {
            panic!("Bob's own undo must apply");
        };
        assert_eq!(bob_from, alice.toml_content);
        // Bob's single transition is now used up; it never became Alice's.
        assert_eq!(
            undo("bob-scope", pid, &alice.toml_content).unwrap(),
            HistoryOutcome::Exhausted
        );
    }

    #[test]
    fn edit_history_depth_cap_drops_the_oldest_transition() {
        let owner = "depth-cap-owner";
        let pid = "depth-cap-pipeline";
        let mut current = TEST_TOML.to_string();
        // One edit past the cap: the first transition must be dropped.
        for i in 0..=MAX_EDIT_HISTORY_DEPTH {
            let edited =
                execute_edit(owner, &current, pid, &set_s1_shell(&format!("echo s1-{i}"))).unwrap();
            assert!(edited.success, "{:?}", edited.validation_errors);
            current = edited.toml_content;
        }

        let mut applied = 0usize;
        loop {
            match undo(owner, pid, &current).unwrap() {
                HistoryOutcome::Applied(state) => {
                    applied += 1;
                    current = state;
                }
                other => {
                    assert_eq!(other, HistoryOutcome::Exhausted);
                    break;
                }
            }
        }
        assert_eq!(
            applied, MAX_EDIT_HISTORY_DEPTH,
            "undo must stop at the documented depth cap"
        );
        assert!(
            current.contains("echo s1-0"),
            "the oldest surviving transition leads back to edit 0: {current}"
        );
        assert_ne!(
            current, TEST_TOML,
            "the pre-edit original must have been dropped by the depth cap"
        );
    }

    #[test]
    fn edit_history_lru_evicts_the_oldest_pipeline() {
        let mut history = EditHistory::new(MAX_EDIT_HISTORY_DEPTH, 2);
        history.record("u", "p1", "a1", "b1");
        history.record("u", "p2", "a2", "b2");
        // Touch p1 so p2 becomes the least-recently-used stack.
        history.record("u", "p1", "b1", "c1");
        history.record("u", "p3", "a3", "b3");

        assert_eq!(history.stacks.len(), 2, "pipeline cap must hold");
        assert_eq!(
            history.undo("u", "p2", "b2"),
            HistoryOutcome::Unavailable,
            "p2 must have been evicted"
        );
        assert_eq!(
            history.undo("u", "p1", "c1"),
            HistoryOutcome::Applied("b1".into()),
            "the freshly used stack survives eviction"
        );

        // The LRU cap counts (identity, pipeline) pairs, not bare ids.
        history.record("v", "p1", "x", "y");
        assert_eq!(history.stacks.len(), 2);
        assert_eq!(
            history.undo("v", "p1", "y"),
            HistoryOutcome::Applied("x".into()),
            "a different owner's p1 is an independent stack"
        );
    }

    #[test]
    fn undo_and_redo_without_history_are_unavailable() {
        let owner = "restart-owner";
        let pid = "restart-pipeline";
        // An empty history is exactly the post-restart state.
        assert_eq!(
            undo(owner, pid, TEST_TOML).unwrap(),
            HistoryOutcome::Unavailable
        );
        assert_eq!(
            redo(owner, pid, TEST_TOML).unwrap(),
            HistoryOutcome::Unavailable
        );
        // A failed lookup must not create an empty stack (otherwise any
        // caller could plant unbounded keys without ever editing).
        assert!(
            !EDIT_HISTORY
                .lock()
                .unwrap()
                .stacks
                .contains_key(&HistoryKey {
                    owner: owner.into(),
                    pipeline_id: pid.into(),
                })
        );
    }

    #[test]
    fn exhausted_undo_is_distinguished_from_unavailable_history() {
        let owner = "exhausted-owner";
        let pid = "exhausted-pipeline";
        let edited = execute_edit(owner, TEST_TOML, pid, &set_s1_shell("echo once")).unwrap();
        assert!(matches!(
            undo(owner, pid, &edited.toml_content).unwrap(),
            HistoryOutcome::Applied(_)
        ));
        // History exists but is fully undone — not "lost".
        assert_eq!(
            undo(owner, pid, TEST_TOML).unwrap(),
            HistoryOutcome::Exhausted
        );
        // The undone transition is redoable, and then redo is exhausted.
        let HistoryOutcome::Applied(redone) = redo(owner, pid, TEST_TOML).unwrap() else {
            panic!("the undone transition must be redoable");
        };
        assert_eq!(redone, edited.toml_content);
        assert_eq!(
            redo(owner, pid, &redone).unwrap(),
            HistoryOutcome::Exhausted
        );
    }
}
