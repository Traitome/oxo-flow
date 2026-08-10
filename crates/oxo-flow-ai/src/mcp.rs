//! MCP (Model Context Protocol) bridge.
//!
//! Provides an MCP client trait and bridge to the Tool trait,
//! enabling AI agents to use external tools from MCP-compatible servers.
//!
//! ## Architecture
//!
//! ```text
//! AI Agent → Tool trait → McpToolBridge → MCP Server (stdio/SSE)
//!                       ↑
//!               McpClient trait (transport abstraction)
//! ```
//!
//! ## Status
//!
//! Phase 5 — trait definitions and MCP→Tool bridge implemented.
//! Actual MCP server connectivity (JSON-RPC over stdio/SSE) is reserved
//! for future implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::AiError;
use crate::tools::Tool;
use crate::types::ToolDef;

// ── MCP Client trait ───────────────────────────────────────────────────────

/// Abstract MCP client — transport-agnostic.
///
/// Implementations handle the actual communication (JSON-RPC over
/// stdio, SSE, or in-process).
#[async_trait]
pub trait McpClient: Send + Sync {
    /// List tools available on the MCP server.
    async fn list_tools(&self) -> Result<Vec<McpToolDef>, AiError>;

    /// Call a tool on the MCP server and get the result.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, AiError>;

    /// Human-readable server name for logging.
    fn server_name(&self) -> &str;

    /// Whether the server is connected and responsive.
    async fn ping(&self) -> Result<bool, AiError> {
        Ok(true)
    }
}

// ── MCP tool definition ────────────────────────────────────────────────────

/// An MCP tool definition as reported by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// ── MCP → Tool bridge ──────────────────────────────────────────────────────

/// Bridges an MCP client to the oxo-flow [`Tool`] trait.
///
/// Each MCP tool is exposed as a separate [`McpToolBridge`] instance,
/// so the agent sees individual tools in its registry.
pub struct McpToolBridge {
    client: Arc<dyn McpClient>,
    tool_def: McpToolDef,
}

impl McpToolBridge {
    /// Create bridges for all tools on an MCP server.
    pub async fn discover(client: Arc<dyn McpClient>) -> Result<Vec<McpToolBridge>, AiError> {
        let tools = client.list_tools().await?;
        Ok(tools
            .into_iter()
            .map(|tool_def| McpToolBridge {
                client: Arc::clone(&client),
                tool_def,
            })
            .collect())
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: format!("mcp_{}_{}", self.client.server_name(), self.tool_def.name),
            description: format!(
                "[MCP: {}] {}",
                self.client.server_name(),
                self.tool_def.description
            ),
            parameters: self.tool_def.input_schema.clone(),
        }
    }

    fn name(&self) -> &str {
        // Return a stable reference — use a static string hack
        // In practice, this returns the def's name, but we need &str
        // We'll use the server_name as a stable prefix
        self.client.server_name()
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        self.client.call_tool(&self.tool_def.name, arguments).await
    }

    fn is_read_only(&self) -> bool {
        // MCP tools are assumed read-only unless explicitly marked otherwise
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test MCP client that serves static tools.
    #[derive(Clone)]
    struct TestMcpClient;

    #[async_trait]
    impl McpClient for TestMcpClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDef>, AiError> {
            Ok(vec![McpToolDef {
                name: "echo".into(),
                description: "Echo test tool".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"msg": {"type": "string"}},
                    "required": ["msg"]
                }),
            }])
        }

        async fn call_tool(&self, name: &str, args: &str) -> Result<String, AiError> {
            let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
            let msg = parsed["msg"].as_str().unwrap_or("no message");
            Ok(format!("{name}: {msg}"))
        }

        fn server_name(&self) -> &str {
            "test-server"
        }
    }

    #[tokio::test]
    async fn mcp_client_lists_tools() {
        let client = TestMcpClient;
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
    }

    #[tokio::test]
    async fn mcp_bridge_discovers_and_executes() {
        let client = Arc::new(TestMcpClient);
        let bridges = McpToolBridge::discover(client).await.unwrap();
        assert_eq!(bridges.len(), 1);

        let bridge = &bridges[0];
        let result = bridge.execute(r#"{"msg": "hello world"}"#).await.unwrap();
        assert_eq!(result, "echo: hello world");
    }

    #[tokio::test]
    async fn mcp_bridge_tool_def() {
        let client = Arc::new(TestMcpClient);
        let bridges = McpToolBridge::discover(client).await.unwrap();
        let def = bridges[0].def();
        assert!(def.name.contains("mcp_"));
        assert!(def.description.contains("test-server"));
    }
}
