//! Tool system for AI agents.
//!
//! Tools are the only way agents interact with the outside world.
//! Each tool has a JSON Schema definition (for the AI's tool_call API)
//! and an async execute method.

pub mod builtin;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::AiError;
pub use crate::types::ToolDef;

// ── Tool trait ─────────────────────────────────────────────────────────────

/// A tool that an AI agent can invoke.
///
/// Tools are registered in a [`ToolRegistry`] and exposed to the AI model
/// via their [`ToolDef`] (name + description + JSON Schema parameters).
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's API definition — name, description, parameter schema.
    fn def(&self) -> ToolDef;

    /// Execute the tool with JSON-encoded arguments.
    /// Returns the tool's result as a string (often JSON, but can be plain text).
    async fn execute(&self, arguments: &str) -> Result<String, AiError>;

    /// Whether this tool is read-only (safe to auto-execute).
    fn is_read_only(&self) -> bool {
        true
    }

    /// Human-readable tool name (must match def().name).
    fn name(&self) -> &str;
}

// ── Tool registry ──────────────────────────────────────────────────────────

/// Registry of tools available to an AI agent.
///
/// # Example
///
/// ```rust,ignore
/// let mut registry = ToolRegistry::new();
/// registry.register(Box::new(ReadFileTool::new()));
/// let defs = registry.to_defs(); // Pass to AI model
/// let result = registry.execute("read_file", r#"{"path": "/tmp/test.txt"}"#).await?;
/// ```
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::from(tool));
    }

    /// Get all tool definitions for passing to the AI model.
    pub fn to_defs(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.def()).collect()
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with JSON-encoded arguments.
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, AiError> {
        let tool = self.tools.get(name).ok_or_else(|| AiError::ToolNotFound {
            tool: name.to_string(),
        })?;
        tool.execute(arguments).await
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "echo".into(),
                description: "Echo back the input".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"message": {"type": "string"}},
                    "required": ["message"]
                }),
            }
        }
        async fn execute(&self, args: &str) -> Result<String, AiError> {
            let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
            let msg = parsed["message"].as_str().unwrap_or("no message");
            Ok(msg.to_string())
        }
        fn name(&self) -> &str {
            "echo"
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        assert_eq!(reg.len(), 1);
        assert!(reg.get("echo").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn registry_to_defs() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        let defs = reg.to_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
    }

    #[tokio::test]
    async fn registry_execute_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        let result = reg
            .execute("echo", r#"{"message": "hello"}"#)
            .await
            .unwrap();
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn registry_execute_unknown_tool_errors() {
        let reg = ToolRegistry::new();
        let result = reg.execute("nope", "{}").await;
        assert!(result.is_err());
    }
}
