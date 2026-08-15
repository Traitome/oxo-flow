//! DAG Edit types — edit operation model, validation responses.
//!
//! Defines the edit operations that can be applied to a pipeline DAG:
//! add/remove rules, connect/disconnect edges, update params, replace tools.
//! All edits are validated before application.

use serde::{Deserialize, Serialize};

/// Source of the edit operation for audit and conflict resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditSource {
    DagEditor,
    Chat,
    Proposal,
}

/// Types of edit operations on a pipeline DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditOperation {
    AddRule,
    RemoveRule,
    Connect,
    Disconnect,
    UpdateParams,
    ReplaceTool,
    Reorder,
}

impl EditOperation {
    pub const ALL: &'static [&'static str] = &[
        "add_rule",
        "remove_rule",
        "connect",
        "disconnect",
        "update_params",
        "replace_tool",
        "reorder",
    ];

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "add_rule" => Some(Self::AddRule),
            "remove_rule" => Some(Self::RemoveRule),
            "connect" => Some(Self::Connect),
            "disconnect" => Some(Self::Disconnect),
            "update_params" => Some(Self::UpdateParams),
            "replace_tool" => Some(Self::ReplaceTool),
            "reorder" => Some(Self::Reorder),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AddRule => "add_rule",
            Self::RemoveRule => "remove_rule",
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
            Self::UpdateParams => "update_params",
            Self::ReplaceTool => "replace_tool",
            Self::Reorder => "reorder",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_operation_roundtrip() {
        for op_name in EditOperation::ALL {
            let op = EditOperation::from_str_name(op_name).unwrap();
            assert_eq!(op.as_str(), *op_name);
        }
    }

    #[test]
    fn test_edit_operation_invalid() {
        assert!(EditOperation::from_str_name("invalid_op").is_none());
    }

    #[test]
    fn test_edit_source_serde() {
        let json = serde_json::json!("dag_editor");
        let source: EditSource = serde_json::from_value(json).unwrap();
        assert_eq!(source, EditSource::DagEditor);
    }
}
