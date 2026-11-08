//! MCP Streamable HTTP transport against a loopback JSON-RPC mock server.
//!
//! Official grounding:
//! - MCP 2025-06-18 Transports: every client message is a POST, `Accept`
//!   must list both `application/json` and `text/event-stream`, the server
//!   may answer with either an SSE stream or one JSON object, a session id
//!   returned at initialize must be sent on every subsequent request, and
//!   the negotiated version is replayed as `MCP-Protocol-Version`.
//!   <https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>
//! - JSON-RPC 2.0 request/response/notification framing.
//!   <https://www.jsonrpc.org/specification>
//!
//! The mock records the exact request headers and bodies; nothing here
//! touches the network beyond 127.0.0.1 or uses a real credential.

use oxo_flow_ai::error::AiError;
use oxo_flow_ai::mcp::{McpClient, McpHttpClient, McpToolBridge};
use oxo_flow_ai::tools::Tool;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

mod mcp_mock;
use mcp_mock::{MockMcp, Reply};

fn allowed_loopback(url: &str) -> McpHttpClient {
    McpHttpClient::new_with_allowlist(url, vec!["127.0.0.1".to_string()])
        .expect("loopback endpoint exempted explicitly")
}

fn initialize_reply(request: &mcp_mock::Received) -> Reply {
    let body = request.json();
    Reply::Json {
        body: json!({
            "jsonrpc": "2.0",
            "id": body["id"],
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": {"name": "mock", "version": "1.0"}
            }
        }),
        session: Some("sess-abc"),
    }
}

#[tokio::test]
async fn full_json_rpc_round_trip_over_http() {
    let server = MockMcp::spawn(|request| {
        let body = request.json();
        match body["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            // Exercise the SSE response framing while interleaving an
            // unrelated notification before the matching response, exactly
            // as the transport spec allows.
            "tools/list" => {
                let progress = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {"progress": 1}
                });
                let result = json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"tools": [
                        {
                            "name": "echo",
                            "description": "echo a message",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"msg": {"type": "string"}}
                            }
                        },
                        {
                            "name": "sum",
                            "description": "add two numbers",
                            "inputSchema": {"type": "object"}
                        }
                    ]}
                });
                Reply::Sse(format!(
                    "event: message\ndata: {progress}\n\ndata: {result}\n\n"
                ))
            }
            "tools/call" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"content": [{"type": "text", "text": "echo: hi"}]}
                }),
                session: None,
            },
            other => panic!("unexpected MCP method: {other}"),
        }
    });

    let client = allowed_loopback(&server.url);
    let tools = client.list_tools().await.expect("initialize + tools/list");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, vec!["echo", "sum"], "SSE-framed page must parse");

    let result = client
        .call_tool("echo", r#"{"msg":"hi"}"#)
        .await
        .expect("tools/call round trip");
    assert_eq!(result, "echo: hi");

    let requests = server.requests();
    assert_eq!(requests.len(), 4, "initialize, notification, list, call");
    for request in &requests {
        assert_eq!(request.method, "POST", "every MCP message is a POST");
        assert_eq!(
            request.header("accept"),
            Some("application/json, text/event-stream"),
            "the transport spec requires both content types"
        );
    }

    // initialize: proposed version + client identity; no session yet.
    let initialize = &requests[0];
    let body = initialize.json();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["method"], "initialize");
    assert_eq!(body["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(body["params"]["clientInfo"]["name"], "oxo-flow");
    assert!(
        initialize.header("mcp-session-id").is_none(),
        "no session exists before initialize returns one"
    );

    // notifications/initialized: no id, and it carries the session.
    let notification = &requests[1];
    let body = notification.json();
    assert_eq!(body["method"], "notifications/initialized");
    assert!(body.get("id").is_none(), "a notification has no id");
    assert_eq!(notification.header("mcp-session-id"), Some("sess-abc"));

    // tools/list and tools/call forward the session id and negotiated
    // protocol version, and use monotonically increasing JSON-RPC ids.
    let list = &requests[2];
    assert_eq!(list.json()["method"], "tools/list");
    assert_eq!(list.header("mcp-session-id"), Some("sess-abc"));
    assert_eq!(list.header("mcp-protocol-version"), Some("2025-06-18"));

    let call = &requests[3];
    let call_body = call.json();
    assert_eq!(call_body["method"], "tools/call");
    assert_eq!(
        call_body["params"]["name"], "echo",
        "the ORIGINAL server tool name goes back on the wire"
    );
    assert_eq!(call_body["params"]["arguments"]["msg"], "hi");
    assert_eq!(call.header("mcp-session-id"), Some("sess-abc"));
    assert_eq!(call.header("mcp-protocol-version"), Some("2025-06-18"));

    let ids: Vec<u64> = requests
        .iter()
        .filter_map(|request| request.json()["id"].as_u64())
        .collect();
    assert_eq!(ids, vec![1, 2, 3], "fresh monotonic ids per request");
}

#[tokio::test]
async fn bridge_discovery_keeps_server_names() {
    let server = MockMcp::spawn(
        |request| match request.json()["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {"tools": [{
                        "name": "echo",
                        "description": "echo",
                        "inputSchema": {"type": "object"}
                    }]}
                }),
                session: None,
            },
            "tools/call" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {"content": [{"type": "text", "text": "ok"}]}
                }),
                session: None,
            },
            other => panic!("unexpected MCP method: {other}"),
        },
    );

    let client = Arc::new(allowed_loopback(&server.url));
    let bridges = McpToolBridge::discover(client).await.expect("discover");
    assert_eq!(bridges.len(), 1);
    assert_eq!(bridges[0].execute("{}").await.expect("bridge call"), "ok");
    let name = bridges[0].def().name;
    assert!(
        name.starts_with("mcp_") && name.ends_with("_echo"),
        "registry name is the endpoint identity plus the server tool name: {name}"
    );
}

#[tokio::test]
async fn ssrf_guard_rejects_private_literals_without_an_allowlist() {
    for url in [
        "mcp://127.0.0.1:8080",
        "mcp://10.0.0.5:8080",
        "mcp://169.254.169.254:80",
        "mcp://localhost:8080",
    ] {
        let error = McpHttpClient::new_with_allowlist(url, Vec::new())
            .err()
            .expect("internal address space must be refused");
        assert!(
            error.to_string().contains("refused")
                && error.to_string().contains("OXO_FLOW_AI_FETCH_ALLOW"),
            "the refusal names the allowlist escape hatch: {error}"
        );
    }
}

#[tokio::test]
async fn redirect_into_blocked_address_space_is_refused() {
    let server = MockMcp::spawn(|_| Reply::Redirect("http://10.0.0.5:9/mcp".to_string()));
    let client = allowed_loopback(&server.url);
    let error = client
        .list_tools()
        .await
        .expect_err("redirect must be re-screened");
    assert!(
        matches!(error, AiError::Config { .. }),
        "redirect target in private space must fail the SSRF screen: {error:?}"
    );
    assert!(error.to_string().contains("refused"), "{error}");
}

#[tokio::test]
async fn json_rpc_error_objects_are_surfaced() {
    let server = MockMcp::spawn(
        |request| match request.json()["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "error": {"code": -32601, "message": "method not found"}
                }),
                session: None,
            },
            other => panic!("unexpected MCP method: {other}"),
        },
    );
    let client = allowed_loopback(&server.url);
    let error = client.list_tools().await.expect_err("JSON-RPC error");
    assert!(
        matches!(error, AiError::Protocol { .. }),
        "JSON-RPC errors are protocol failures: {error:?}"
    );
    assert!(error.to_string().contains("method not found"), "{error}");
}

#[tokio::test]
async fn non_json_http_error_bodies_surface_as_protocol_errors() {
    let server = MockMcp::spawn(
        |request| match request.json()["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            _ => Reply::Raw {
                status: 500,
                content_type: "text/html",
                body: "<html>upstream proxy error</html>".to_string(),
            },
        },
    );
    let client = allowed_loopback(&server.url);
    let error = client.list_tools().await.expect_err("HTML body");
    assert!(
        matches!(error, AiError::Protocol { .. }),
        "unparseable body is reported as a protocol error: {error:?}"
    );
    assert!(error.to_string().contains("unparseable"), "{error}");
}

#[tokio::test]
async fn connection_refused_is_a_transport_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let addr = listener.local_addr().expect("reserved addr");
    drop(listener);
    let client =
        McpHttpClient::new_with_allowlist(&format!("mcp://{addr}"), vec!["127.0.0.1".to_string()])
            .expect("allowlisted loopback");
    let error = client.list_tools().await.expect_err("connection refused");
    assert!(
        matches!(error, AiError::Transport { .. }),
        "a refused connection must be a transport error: {error:?}"
    );
}

#[tokio::test]
async fn tool_results_with_is_error_fail_the_call() {
    let server = MockMcp::spawn(
        |request| match request.json()["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {"tools": [{
                        "name": "fail",
                        "description": "always fails",
                        "inputSchema": {"type": "object"}
                    }]}
                }),
                session: None,
            },
            "tools/call" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {
                        "content": [{"type": "text", "text": "samtools: command not found"}],
                        "isError": true
                    }
                }),
                session: None,
            },
            other => panic!("unexpected MCP method: {other}"),
        },
    );
    let client = allowed_loopback(&server.url);
    let error = client
        .call_tool("fail", "{}")
        .await
        .expect_err("isError: true must not be returned as success");
    assert!(
        matches!(error, AiError::ToolError { .. }),
        "tool-execution failure is a ToolError: {error:?}"
    );
    assert!(error.to_string().contains("samtools"), "{error}");
}

/// Mint `sess-1`, `sess-2`, … on successive initialize requests.
fn mint_session(counter: &AtomicU64) -> &'static str {
    match counter.fetch_add(1, Ordering::SeqCst) {
        0 => "sess-1",
        _ => "sess-2",
    }
}

fn expired_reply() -> Reply {
    Reply::Raw {
        status: 404,
        content_type: "application/json",
        body: "{\"error\":\"session expired\"}".to_string(),
    }
}

#[tokio::test]
async fn session_expiry_during_tools_list_reinitializes_and_restarts_pagination() {
    // Transport spec: a 404 for a session-bearing request means the server
    // terminated the session; the client MUST start a new one with a fresh
    // initialize (without the dead session id).
    let sessions = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&sessions);
    let server = MockMcp::spawn(move |request| {
        let body = request.json();
        match body["method"].as_str().unwrap_or("") {
            "initialize" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"protocolVersion": "2025-06-18", "capabilities": {}}
                }),
                session: Some(mint_session(&counter)),
            },
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => {
                if request.header("mcp-session-id") == Some("sess-1") {
                    return expired_reply();
                }
                let cursor = body["params"]["cursor"].as_str().unwrap_or("");
                if cursor.is_empty() {
                    Reply::Json {
                        body: json!({
                            "jsonrpc": "2.0",
                            "id": body["id"],
                            "result": {
                                "tools": [{
                                    "name": "page1",
                                    "description": "first page",
                                    "inputSchema": {"type": "object"}
                                }],
                                "nextCursor": "page-2"
                            }
                        }),
                        session: None,
                    }
                } else {
                    Reply::Json {
                        body: json!({
                            "jsonrpc": "2.0",
                            "id": body["id"],
                            "result": {"tools": [{
                                "name": "page2",
                                "description": "second page",
                                "inputSchema": {"type": "object"}
                            }]}
                        }),
                        session: None,
                    }
                }
            }
            other => panic!("unexpected MCP method: {other}"),
        }
    });

    let client = allowed_loopback(&server.url);
    let tools = client
        .list_tools()
        .await
        .expect("session-expiry recovery must succeed");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["page1", "page2"],
        "pagination restarts from the first page after reinitializing"
    );

    let requests = server.requests();
    // initialize, notification, 404, initialize, notification, page1, page2.
    assert_eq!(requests.len(), 7, "{requests:#?}");
    let reinitialize = &requests[3];
    assert_eq!(reinitialize.json()["method"], "initialize");
    assert!(
        reinitialize.header("mcp-session-id").is_none(),
        "the dead session id must not be replayed on the fresh initialize"
    );
    assert!(
        reinitialize.header("mcp-protocol-version").is_none(),
        "the old negotiated version must be cleared with the session"
    );
    assert_eq!(
        requests[6].header("mcp-session-id"),
        Some("sess-2"),
        "the restarted session id rides the new page request"
    );
}

#[tokio::test]
async fn session_expiry_during_tools_call_reinitializes_and_retries_once() {
    let sessions = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&sessions);
    let server = MockMcp::spawn(move |request| {
        let body = request.json();
        match body["method"].as_str().unwrap_or("") {
            "initialize" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"protocolVersion": "2025-06-18", "capabilities": {}}
                }),
                session: Some(mint_session(&counter)),
            },
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"tools": [{
                        "name": "echo",
                        "description": "echo",
                        "inputSchema": {"type": "object"}
                    }]}
                }),
                session: None,
            },
            "tools/call" => {
                if request.header("mcp-session-id") == Some("sess-1") {
                    return expired_reply();
                }
                Reply::Json {
                    body: json!({
                        "jsonrpc": "2.0",
                        "id": body["id"],
                        "result": {"content": [{"type": "text", "text": "after-reinit"}]}
                    }),
                    session: None,
                }
            }
            other => panic!("unexpected MCP method: {other}"),
        }
    });

    let client = allowed_loopback(&server.url);
    client.list_tools().await.expect("discovery");
    let result = client
        .call_tool("echo", "{}")
        .await
        .expect("the call must be retried on a fresh session");
    assert_eq!(result, "after-reinit");

    let requests = server.requests();
    let calls: Vec<&mcp_mock::Received> = requests
        .iter()
        .filter(|request| request.json()["method"] == "tools/call")
        .collect();
    assert_eq!(calls.len(), 2, "one expired attempt + one retry");
    assert_eq!(calls[0].header("mcp-session-id"), Some("sess-1"));
    assert_eq!(calls[1].header("mcp-session-id"), Some("sess-2"));
}

#[tokio::test]
async fn a_server_that_always_expires_sessions_fails_instead_of_looping() {
    let sessions = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&sessions);
    let server = MockMcp::spawn(move |request| {
        let body = request.json();
        match body["method"].as_str().unwrap_or("") {
            "initialize" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"protocolVersion": "2025-06-18", "capabilities": {}}
                }),
                session: Some(mint_session(&counter)),
            },
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => expired_reply(),
            other => panic!("unexpected MCP method: {other}"),
        }
    });

    let client = allowed_loopback(&server.url);
    let error = client.list_tools().await.expect_err("permanent expiry");
    assert!(
        matches!(error, AiError::SessionExpired),
        "the bounded recovery must end in SessionExpired: {error:?}"
    );
    // initialize + notification + 404, twice; no retry loop.
    assert_eq!(server.count(), 6, "recovery is attempted exactly once");
}

#[tokio::test]
async fn structured_tool_results_are_returned_losslessly() {
    let server = MockMcp::spawn(
        |request| match request.json()["method"].as_str().unwrap_or("") {
            "initialize" => initialize_reply(request),
            "notifications/initialized" => Reply::Raw {
                status: 202,
                content_type: "text/plain",
                body: String::new(),
            },
            "tools/list" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {"tools": [{
                        "name": "stats",
                        "description": "structured stats",
                        "inputSchema": {"type": "object"}
                    }]}
                }),
                session: None,
            },
            "tools/call" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": request.json()["id"],
                    "result": {
                        "content": [],
                        "structuredContent": {"reads": 42, "unit": "bp"}
                    }
                }),
                session: None,
            },
            other => panic!("unexpected MCP method: {other}"),
        },
    );
    let client = allowed_loopback(&server.url);
    let text = client.call_tool("stats", "{}").await.expect("structured");
    let value: Value = serde_json::from_str(&text).expect("JSON object");
    assert_eq!(value["reads"], 42);
    assert_eq!(value["unit"], "bp");
}
