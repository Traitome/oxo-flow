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

// ── Tool-name sanitization ─────────────────────────────────────────────────

/// Longest tool name the OpenAI-compatible function-calling API accepts.
const MAX_TOOL_NAME_LEN: usize = 64;

/// Normalize a tool name to `^[a-zA-Z0-9_-]{1,64}$`.
///
/// OpenAI-compatible providers reject the WHOLE request when any tool name
/// breaks that pattern, so one badly named MCP tool (names come from remote
/// servers and are not under our control) failed every call. Sanitization
/// happens on the name the model sees and on every registry lookup, so a
/// renamed tool still resolves.
///
/// Long names are truncated with an FNV-1a suffix rather than sliced: a
/// plain truncation could map two distinct MCP tools onto one registry key.
pub fn sanitize_tool_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        return "tool".to_string();
    }
    if cleaned.len() <= MAX_TOOL_NAME_LEN {
        return cleaned;
    }
    // FNV-1a over the ORIGINAL name keeps the suffix stable for a given tool.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let suffix = format!("-{:08x}", hash as u32);
    let keep = MAX_TOOL_NAME_LEN - suffix.len();
    cleaned.truncate(keep);
    cleaned.push_str(&suffix);
    cleaned
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
    ///
    /// The key is [`sanitize_tool_name`]-normalized so it always matches
    /// the name the model is given (see [`Self::to_defs`]) and calls back.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = sanitize_tool_name(tool.name());
        self.tools.insert(name, Arc::from(tool));
    }

    /// Get all tool definitions for passing to the AI model.
    ///
    /// Names are normalized here too: providers validate every tool name
    /// against `^[a-zA-Z0-9_-]{1,64}$` and reject the whole request if any
    /// one breaks it.
    pub fn to_defs(&self) -> Vec<ToolDef> {
        self.tools
            .values()
            .map(|t| {
                let mut def = t.def();
                def.name = sanitize_tool_name(&def.name);
                def
            })
            .collect()
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .get(&sanitize_tool_name(name))
            .map(|t| t.as_ref())
    }

    /// Whether a tool is read-only (safe to auto-execute).
    /// Unknown tools are treated as NOT read-only — deny by default.
    pub fn is_read_only(&self, name: &str) -> bool {
        self.tools
            .get(&sanitize_tool_name(name))
            .map(|t| t.is_read_only())
            .unwrap_or(false)
    }

    /// Execute a tool by name with JSON-encoded arguments.
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, AiError> {
        let tool =
            self.tools
                .get(&sanitize_tool_name(name))
                .ok_or_else(|| AiError::ToolNotFound {
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
    fn sanitize_tool_name_normalizes_illegal_names() {
        assert_eq!(sanitize_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_tool_name("mcp-srv.tool:v2"), "mcp-srv_tool_v2");
        assert_eq!(sanitize_tool_name(""), "tool");
        // Long names are truncated but stay unique (hash suffix).
        let long_a = sanitize_tool_name(&"a".repeat(80));
        let long_b = sanitize_tool_name(&format!("{}b", "a".repeat(79)));
        assert_eq!(long_a.len(), MAX_TOOL_NAME_LEN);
        assert_eq!(long_b.len(), MAX_TOOL_NAME_LEN);
        assert_ne!(long_a, long_b, "truncation must not collide");
    }

    /// A tool whose declared name is illegal on the wire.
    struct IllegalNameTool;
    #[async_trait]
    impl Tool for IllegalNameTool {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "read.file (v2)".into(),
                description: "illegal".into(),
                parameters: serde_json::json!({}),
            }
        }
        async fn execute(&self, _args: &str) -> Result<String, AiError> {
            Ok("ran".into())
        }
        fn name(&self) -> &str {
            "read.file (v2)"
        }
    }

    #[tokio::test]
    async fn registry_normalizes_illegal_names_end_to_end() {
        // One wire-illegal name fails every provider request, so the name
        // the model sees, the registry key and the lookup all normalize.
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(IllegalNameTool));
        let defs = reg.to_defs();
        assert_eq!(defs[0].name, "read_file__v2_");
        assert!(reg.get("read.file (v2)").is_some());
        assert_eq!(
            reg.execute("read.file (v2)", "{}").await.unwrap(),
            "ran",
            "the model's call (normalized name) must resolve"
        );
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
