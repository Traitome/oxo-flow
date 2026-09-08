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
use std::time::Duration;

use crate::error::AiError;
use crate::tools::Tool;
use crate::types::ToolDef;

/// Connect timeout for MCP HTTP endpoints.
const MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Total request timeout for MCP HTTP calls (a hung server fails the
/// call instead of blocking the AI command).
const MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on `tools/list` pages followed per server: a server that
/// loops on `nextCursor` must not spin the engine forever.
const MAX_TOOL_PAGES: usize = 50;

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
    /// Registry key — must equal `def().name` (`mcp_<server>_<tool>`):
    /// the agent invokes tools by their def name, and [`ToolRegistry`]
    /// keys on `Tool::name()`. Stored once so the two stay in sync.
    name: String,
}

impl McpToolBridge {
    /// Create bridges for all tools on an MCP server.
    pub async fn discover(client: Arc<dyn McpClient>) -> Result<Vec<McpToolBridge>, AiError> {
        let tools = client.list_tools().await?;
        Ok(tools
            .into_iter()
            .map(|tool_def| {
                // The registry/wire name is sanitized: OpenAI-compatible
                // providers reject the whole request when any tool name
                // breaks `^[a-zA-Z0-9_-]{1,64}$`, and MCP tool names come
                // from the server. The ORIGINAL name still goes back to the
                // server (`execute` uses `tool_def.name`).
                let name = crate::tools::sanitize_tool_name(&format!(
                    "mcp_{}_{}",
                    client.server_name(),
                    tool_def.name
                ));
                McpToolBridge {
                    client: Arc::clone(&client),
                    tool_def,
                    name,
                }
            })
            .collect())
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name.clone(),
            description: format!(
                "[MCP: {}] {}",
                self.client.server_name(),
                self.tool_def.description
            ),
            parameters: self.tool_def.input_schema.clone(),
        }
    }

    fn name(&self) -> &str {
        &self.name
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
        // Bound every request: a hung MCP server must fail the tool call
        // (fail-safe), never block the AI command indefinitely.
        let http = reqwest::Client::builder()
            .connect_timeout(MCP_CONNECT_TIMEOUT)
            .timeout(MCP_REQUEST_TIMEOUT)
            .build()
            .map_err(|e| AiError::Config {
                message: format!("failed to build MCP HTTP client: {e}"),
            })?;
        Ok(Self {
            base_url,
            session_id: std::sync::Mutex::new(None),
            http,
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
        parse_rpc_response(&text, id).map_err(|e| AiError::Protocol { message: e })
    }
}

/// Whether a JSON-RPC response carries the id of the request we sent.
///
/// JSON-RPC ids may be numbers or strings; the engine sends numbers, and a
/// server echoing `"3"` for `3` is still a match.
fn rpc_id_matches(value: &serde_json::Value, expected: u64) -> bool {
    match value.get("id") {
        Some(serde_json::Value::Number(n)) => n.as_u64() == Some(expected),
        Some(serde_json::Value::String(s)) => s == &expected.to_string(),
        _ => false,
    }
}

/// Parse a JSON-RPC response body — plain JSON or SSE-framed
/// (`data: {...}` lines) — and require its `id` to match `expected_id`.
///
/// Without the id check a stale, replayed or proxied body was accepted as
/// this call's answer (audit finding): the engine then acted on another
/// request's result.
fn parse_rpc_response(text: &str, expected_id: u64) -> Result<serde_json::Value, String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return extract_rpc_result(value, expected_id);
    }
    // SSE framing: prefer the event whose id matches (servers interleave
    // notifications and progress events around the answer), fall back to
    // the last event so an id-less error body still yields a diagnosis.
    let mut last: Option<&str> = None;
    let mut matched: Option<serde_json::Value> = None;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        last = Some(payload);
        if matched.is_none()
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(payload)
            && rpc_id_matches(&value, expected_id)
        {
            matched = Some(value);
        }
    }
    match matched {
        Some(value) => extract_rpc_result(value, expected_id),
        None => match last {
            Some(payload) => {
                let value: serde_json::Value = serde_json::from_str(payload)
                    .map_err(|e| format!("invalid SSE payload: {e}"))?;
                extract_rpc_result(value, expected_id)
            }
            // Char-boundary safe prefix: a plain byte slice at 200 panics when
            // the body is a multi-byte (e.g. localized) error page (audit finding).
            _ => Err(format!(
                "unparseable MCP response: {}",
                crate::types::truncate_utf8_from_start(text, 200)
            )),
        },
    }
}

fn extract_rpc_result(
    value: serde_json::Value,
    expected_id: u64,
) -> Result<serde_json::Value, String> {
    // Id first: a body belonging to another request must never be accepted
    // as this one's result — nor reported as this one's error.
    if !rpc_id_matches(&value, expected_id) {
        return Err(format!(
            "MCP response id mismatch: expected {expected_id}, got {}",
            value
                .get("id")
                .map(|i| i.to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        ));
    }
    if let Some(err) = value.get("error") {
        return Err(format!("MCP error: {err}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| format!("MCP response missing 'result': {value}"))
}

/// Turn a `tools/call` result into the tool's text.
///
/// Result shape: `{ content: [{type:"text",text:"..."}], isError?: bool }`.
/// `isError: true` means the tool RAN and failed: returning its message as
/// `Ok` made the caller record `success: true` and let the agent treat a
/// failed call as a result (audit finding).
fn tool_result_text(name: &str, result: &serde_json::Value) -> Result<String, AiError> {
    let mut text = None;
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        let parts: Vec<String> = content
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()).map(String::from))
            .collect();
        if !parts.is_empty() {
            text = Some(parts.join("\n"));
        }
    }
    if text.is_none()
        && let Some(structured) = result.get("structuredContent").and_then(|t| t.as_str())
    {
        text = Some(structured.to_string());
    }
    let text = text.unwrap_or_else(|| result.to_string());
    if result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(AiError::ToolError {
            tool: name.to_string(),
            message: text,
        });
    }
    Ok(text)
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
        // Spec: once initialize returns an mcp-session-id, every
        // subsequent request — the initialized notification included —
        // must carry it.
        let mut note_req = self
            .http
            .post(&self.base_url)
            .header("Accept", "application/json, text/event-stream")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }));
        if let Some(sid) = self.session_id.lock().unwrap().clone() {
            note_req = note_req.header("mcp-session-id", sid);
        }
        let _ = note_req.send().await;

        // tools/list is paginated: the first page carries `nextCursor` when
        // more tools exist. Ignoring it silently exposed only the first page
        // (audit finding) — follow the cursor, bounded so a looping server
        // cannot spin forever.
        let mut tools: Vec<McpToolDef> = Vec::new();
        let mut cursor: Option<String> = None;
        for request_id in (2_u64..).take(MAX_TOOL_PAGES) {
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            let result = self.rpc(request_id, "tools/list", params).await?;
            let page: Vec<McpToolDef> = serde_json::from_value(
                result
                    .get("tools")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            )
            .map_err(|e| AiError::Protocol {
                message: format!("invalid tools/list payload: {e}"),
            })?;
            tools.extend(page);
            cursor = result
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            tracing::warn!(
                server = %self.server_name,
                tools = tools.len(),
                pages = MAX_TOOL_PAGES,
                "tools/list pagination stopped at the page cap — later pages are not exposed"
            );
        }
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
        tool_result_text(name, &result)
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

    /// A test MCP client that serves two tools from one server (the
    /// registry-collision regression case: `name()` must key on the def
    /// name, never the bare server name).
    #[derive(Clone)]
    struct TwoToolClient;

    #[async_trait]
    impl McpClient for TwoToolClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDef>, AiError> {
            Ok(vec![
                McpToolDef {
                    name: "read_tool".into(),
                    description: "read-only lookup".into(),
                    input_schema: serde_json::json!({}),
                    annotations: Some(McpAnnotations {
                        read_only_hint: Some(true),
                    }),
                },
                McpToolDef {
                    name: "write_tool".into(),
                    description: "mutating tool".into(),
                    input_schema: serde_json::json!({}),
                    annotations: None,
                },
            ])
        }

        async fn call_tool(&self, name: &str, args: &str) -> Result<String, AiError> {
            Ok(format!("{name}: {args}"))
        }

        fn server_name(&self) -> &str {
            "two-server"
        }
    }

    /// A client whose server reports a tool name that is illegal on the
    /// wire (OpenAI-compatible providers require `^[a-zA-Z0-9_-]{1,64}$`).
    #[derive(Clone)]
    struct IllegalNameClient;

    #[async_trait]
    impl McpClient for IllegalNameClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDef>, AiError> {
            Ok(vec![McpToolDef {
                name: "read.file:v2 (fast)".into(),
                description: "illegal wire name".into(),
                input_schema: serde_json::json!({}),
                annotations: None,
            }])
        }

        async fn call_tool(&self, name: &str, _args: &str) -> Result<String, AiError> {
            Ok(format!("called:{name}"))
        }

        fn server_name(&self) -> &str {
            "dodgy-server"
        }
    }

    #[tokio::test]
    async fn mcp_client_lists_tools() {
        let client = TestMcpClient;
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
    }

    #[test]
    fn tool_result_text_surfaces_is_error() {
        // `isError: true` means the tool ran and failed; returning its text
        // as Ok made the caller record success: true.
        let failed = serde_json::json!({
            "content": [{"type": "text", "text": "samtools: command not found"}],
            "isError": true,
        });
        let err = tool_result_text("run_shell", &failed).unwrap_err();
        assert!(
            err.to_string().contains("samtools: command not found"),
            "{err}"
        );
        assert!(matches!(err, AiError::ToolError { .. }), "{err:?}");

        let ok = serde_json::json!({
            "content": [{"type": "text", "text": "done"}],
        });
        assert_eq!(tool_result_text("run_shell", &ok).unwrap(), "done");
    }

    #[test]
    fn parse_rpc_response_rejects_id_mismatch() {
        // A stale/replayed body must never be accepted as this call's answer.
        let stale = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": {"tools": []}
        })
        .to_string();
        let err = parse_rpc_response(&stale, 2).unwrap_err();
        assert!(err.contains("id mismatch"), "{err}");

        // The matching id is accepted, numbers or strings.
        let good = serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"ok": true}});
        assert!(parse_rpc_response(&good.to_string(), 2).is_ok());
        let stringified = serde_json::json!({"jsonrpc": "2.0", "id": "2", "result": {"ok": true}});
        assert!(parse_rpc_response(&stringified.to_string(), 2).is_ok());
    }

    #[test]
    fn parse_rpc_response_picks_the_matching_sse_event() {
        // SSE bodies interleave notifications with the answer; the matching
        // id wins over "last event".
        let text = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[]}}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\n";
        let result = parse_rpc_response(text, 7).unwrap();
        assert_eq!(result, serde_json::json!({"tools": []}));
    }

    #[tokio::test]
    async fn mcp_bridge_sanitizes_wire_illegal_tool_names() {
        // One illegal name fails EVERY provider request, so the composed
        // name is normalized for the wire — while the original name still
        // goes back to the MCP server.
        let bridges = McpToolBridge::discover(Arc::new(IllegalNameClient))
            .await
            .unwrap();
        let name = bridges[0].name().to_string();
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "wire name must match ^[a-zA-Z0-9_-]{{1,64}}$: {name}"
        );
        assert!(name.len() <= 64, "{name}");
        assert_eq!(
            bridges[0].execute("{}").await.unwrap(),
            "called:read.file:v2 (fast)",
            "the server must still receive the original tool name"
        );
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

    /// Regression (issue #61): the agent invokes tools by the name exposed
    /// in the tool defs (`mcp_<server>_<tool>`), and the registry keys on
    /// `Tool::name()` — so bridges must register under their def name.
    /// Keying on the bare server name made model-invoked MCP tools
    /// unresolvable and collapsed all tools of one server into a single
    /// registry entry.
    #[tokio::test]
    async fn mcp_bridges_register_under_def_names() {
        let bridges = McpToolBridge::discover(Arc::new(TwoToolClient))
            .await
            .unwrap();
        assert_eq!(bridges.len(), 2);

        let mut registry = crate::tools::ToolRegistry::new();
        for bridge in bridges {
            registry.register(Box::new(bridge));
        }

        // No key collisions: every tool must survive registration.
        assert_eq!(registry.len(), 2);
        // Model-invoked names must resolve in the registry.
        let read_name = "mcp_two-server_read_tool";
        let write_name = "mcp_two-server_write_tool";
        assert!(registry.get(read_name).is_some());
        assert!(registry.get(write_name).is_some());
        // readOnlyHint=true must mark the tool read-only (auto-execute);
        // the unannotated one stays approval-gated.
        assert!(registry.is_read_only(read_name));
        assert!(!registry.is_read_only(write_name));
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

    /// Serve `respond` on a fresh local port: the closure receives each
    /// parsed JSON-RPC request and returns the response body to write.
    /// Returns the `mcp://` URL and a request counter.
    fn json_rpc_fixture(
        respond: impl Fn(&serde_json::Value) -> serde_json::Value + Send + 'static,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_for_server = calls.clone();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            loop {
                let mut buf = [0u8; 65536];
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                        let json: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or_default();
                        calls_for_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let response_body = respond(&json).to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );
                        if stream.write_all(response.as_bytes()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        (format!("mcp://{addr}"), calls)
    }

    #[tokio::test]
    async fn mcp_http_client_follows_tools_list_pagination() {
        // Ignoring `nextCursor` silently exposed only the first page.
        let (url, calls) = json_rpc_fixture(|req| {
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "tools/list" => {
                    let cursor = req["params"]["cursor"].as_str().unwrap_or("");
                    if cursor.is_empty() {
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                            "tools":[{"name":"page1","description":"p1","inputSchema":{"type":"object"}}],
                            "nextCursor":"page-2"}})
                    } else {
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                            "tools":[{"name":"page2","description":"p2","inputSchema":{"type":"object"}}]}})
                    }
                }
                _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}}),
            }
        });
        let client = McpHttpClient::new(&url).unwrap();
        let tools = client.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["page1", "page2"], "both pages must be exposed");
        assert!(
            calls.load(std::sync::atomic::Ordering::SeqCst) >= 4,
            "initialize + notification + two tools/list pages"
        );
    }

    #[tokio::test]
    async fn mcp_http_client_rejects_a_response_for_another_request() {
        let (url, _calls) = json_rpc_fixture(|req| {
            if req["method"].as_str() == Some("tools/list") {
                // A stale/replayed body: this id belongs to another request.
                serde_json::json!({"jsonrpc":"2.0","id":99,"result":{"tools":[]}})
            } else {
                serde_json::json!({"jsonrpc":"2.0","id":req["id"].clone(),"result":{}})
            }
        });
        let client = McpHttpClient::new(&url).unwrap();
        let err = client.list_tools().await.unwrap_err().to_string();
        assert!(err.contains("id mismatch"), "{err}");
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

    #[test]
    fn unparseable_body_with_multibyte_chars_does_not_panic() {
        // A proxy/HTML error page longer than 200 bytes with multi-byte
        // content: the old `&text[..text.len().min(200)]` panicked.
        let body = format!("<html><body>{}</body></html>", "错".repeat(100));
        assert!(body.len() > 200);
        let err = parse_rpc_response(&body, 1).unwrap_err();
        assert!(err.starts_with("unparseable MCP response:"));
    }
}
