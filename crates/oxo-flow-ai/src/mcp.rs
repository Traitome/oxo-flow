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
//! - MCP→Tool bridge: complete.
//! - Transport: Streamable HTTP client (`McpHttpClient`) for
//!   `mcp://host:port[/path]` endpoints — JSON-RPC over POST with SSE
//!   response support. Stdio servers are NOT spawned (trust boundary:
//!   the engine never manages MCP server processes).

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
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    /// Optional tool annotations (readOnlyHint etc.).
    #[serde(default)]
    pub annotations: Option<McpAnnotations>,
}

/// Tool annotations from the MCP spec (informational hints).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAnnotations {
    #[serde(rename = "readOnlyHint", default)]
    pub read_only_hint: Option<bool>,
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
        // Conservative: only the server's explicit readOnlyHint marks a
        // tool safe to auto-execute. Unmarked MCP tools can do anything —
        // they require human approval per invocation.
        self.tool_def
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }
}

// ── Streamable HTTP transport ──────────────────────────────────────────────

/// MCP client over the Streamable HTTP transport: JSON-RPC over POST,
/// with optional SSE response framing and `mcp-session-id` headers.
///
/// URLs use the `mcp://host:port[/path]` form declared in skill
/// manifests (`requires = ["mcp://..."]`); `https://` URLs are also
/// accepted directly. The engine never spawns MCP server processes —
/// the server must already be reachable over HTTP.
pub struct McpHttpClient {
    base_url: String,
    session_id: std::sync::Mutex<Option<String>>,
    http: reqwest::Client,
    server_name: String,
}

impl McpHttpClient {
    /// Build a client for an `mcp://` (or `http(s)://`) endpoint URL.
    pub fn new(url: &str) -> Result<Self, AiError> {
        let base_url = if let Some(rest) = url.strip_prefix("mcp://") {
            format!("http://{rest}")
        } else if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            return Err(AiError::Config {
                message: format!(
                    "invalid MCP endpoint '{url}' — expected mcp://host:port[/path] or http(s)://..."
                ),
            });
        };
        let server_name = url
            .trim_start_matches("mcp://")
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .replace([':', '/'], "_")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>();
        Ok(Self {
            base_url,
            session_id: std::sync::Mutex::new(None),
            http: reqwest::Client::new(),
            server_name,
        })
    }

    /// Send one JSON-RPC request, returning the `result` value.
    async fn rpc(
        &self,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut req = self
            .http
            .post(&self.base_url)
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(sid) = self.session_id.lock().unwrap().clone() {
            req = req.header("mcp-session-id", sid);
        }
        let response = req.send().await.map_err(|e| AiError::Transport {
            message: format!("MCP request to {} failed: {e}", self.base_url),
        })?;
        if let Some(Ok(v)) = response.headers().get("mcp-session-id").map(|s| s.to_str()) {
            *self.session_id.lock().unwrap() = Some(v.to_string());
        }
        let text = response.text().await.map_err(|e| AiError::Transport {
            message: format!("MCP response read failed: {e}"),
        })?;
        parse_rpc_response(&text).map_err(|e| AiError::Protocol { message: e })
    }
}

/// Parse a JSON-RPC response body — plain JSON or SSE-framed
/// (`data: {...}` lines).
fn parse_rpc_response(text: &str) -> Result<serde_json::Value, String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return extract_rpc_result(value);
    }
    // SSE framing: take the last `data:` payload.
    let mut last = None;
    for line in text.lines() {
        if let Some(payload) = line.strip_prefix("data:") {
            last = Some(payload.trim());
        }
    }
    match last {
        Some(payload) => {
            let value: serde_json::Value =
                serde_json::from_str(payload).map_err(|e| format!("invalid SSE payload: {e}"))?;
            extract_rpc_result(value)
        }
        None => Err(format!(
            "unparseable MCP response: {}",
            &text[..text.len().min(200)]
        )),
    }
}

fn extract_rpc_result(value: serde_json::Value) -> Result<serde_json::Value, String> {
    if let Some(err) = value.get("error") {
        return Err(format!("MCP error: {err}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| format!("MCP response missing 'result': {value}"))
}

#[async_trait]
impl McpClient for McpHttpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDef>, AiError> {
        // initialize → notifications/initialized → tools/list
        self.rpc(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "oxo-flow", "version": env!("CARGO_PKG_VERSION")},
            }),
        )
        .await?;
        let _ = self
            .http
            .post(&self.base_url)
            .header("Accept", "application/json, text/event-stream")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }))
            .send()
            .await;

        let result = self.rpc(2, "tools/list", serde_json::json!({})).await?;
        let tools: Vec<McpToolDef> = serde_json::from_value(
            result
                .get("tools")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|e| AiError::Protocol {
            message: format!("invalid tools/list payload: {e}"),
        })?;
        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::Protocol {
                message: format!("invalid tool arguments: {e}"),
            })?;
        let result = self
            .rpc(
                3,
                "tools/call",
                serde_json::json!({ "name": name, "arguments": args }),
            )
            .await?;

        // Result shape: { content: [{type:"text",text:"..."}], ... }
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            let parts: Vec<String> = content
                .iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()).map(String::from))
                .collect();
            if !parts.is_empty() {
                return Ok(parts.join("\n"));
            }
        }
        if let Some(text) = result.get("structuredContent").and_then(|t| t.as_str()) {
            return Ok(text.to_string());
        }
        Ok(result.to_string())
    }

    fn server_name(&self) -> &str {
        &self.server_name
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
                annotations: None,
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

    /// A minimal single-threaded HTTP fixture serving JSON-RPC responses —
    /// exercises the full McpHttpClient flow (initialize → session header →
    /// tools/list → tools/call).
    #[tokio::test]
    async fn mcp_http_client_full_flow() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            // reqwest keeps connections alive, so several requests arrive on
            // ONE connection — serve them in a loop until the client goes
            // quiet (read timeout) or the connection closes.
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            loop {
                let mut buf = [0u8; 16384];
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                        let json: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or_default();
                        let method = json["method"].as_str().unwrap_or("");
                        let (result, session) = match method {
                            "initialize" => {
                                (serde_json::json!({"protocolVersion": "2024-11-05"}), true)
                            }
                            "tools/list" => (
                                serde_json::json!({"tools": [
                                    {"name": "echo", "description": "echo", "inputSchema": {"type": "object"},
                                     "annotations": {"readOnlyHint": true}},
                                    {"name": "write", "description": "write", "inputSchema": {"type": "object"}}
                                ]}),
                                true,
                            ),
                            "tools/call" => (
                                serde_json::json!({"content": [{"type": "text", "text": "tool-result"}]}),
                                true,
                            ),
                            _ => (serde_json::json!({}), false),
                        };
                        let session_header = if session {
                            "mcp-session-id: s-1\r\n"
                        } else {
                            ""
                        };
                        let response_body = serde_json::json!({
                            "jsonrpc": "2.0", "id": json["id"], "result": result,
                        })
                        .to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{}",
                            response_body.len(),
                            session_header,
                            response_body
                        );
                        if stream.write_all(response.as_bytes()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let client = McpHttpClient::new(&format!("mcp://{addr}")).unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);

        // readOnlyHint drives the bridge's read-only flag; unannotated
        // tools default to NOT read-only (conservative).
        let bridges = McpToolBridge::discover(std::sync::Arc::new(client))
            .await
            .unwrap();
        assert!(bridges[0].is_read_only());
        assert!(!bridges[1].is_read_only());

        let result = bridges[0].execute(r#"{"msg": "hi"}"#).await.unwrap();
        assert!(result.contains("tool-result"));

        drop(bridges);
        // The fixture thread is left to die with the test process — joining
        // would deadlock on its blocking accept() (reqwest may reuse
        // connections, so the request count is not fixed).
        let _ = server;
    }

    #[test]
    fn mcp_tool_def_unannotated_has_no_readonly_hint() {
        let def: McpToolDef = serde_json::from_value(serde_json::json!({
            "name": "w", "description": "d", "inputSchema": {}
        }))
        .unwrap();
        assert!(def.annotations.is_none());
    }

    #[test]
    fn mcp_http_client_rejects_bad_url() {
        assert!(McpHttpClient::new("stdio://local").is_err());
        assert!(McpHttpClient::new("mcp://localhost:8080").is_ok());
        assert!(McpHttpClient::new("https://example.com/mcp").is_ok());
    }
}
