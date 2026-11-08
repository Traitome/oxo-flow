//! Bearer-token propagation for the MCP HTTP client, end to end.
//!
//! The operator-facing contract (`docs/guide/src/reference/custom-skills.md`):
//! `OXO_FLOW_MCP_TOKEN` is sent to every MCP endpoint, a per-server
//! `OXO_FLOW_MCP_TOKEN_<SERVER>` wins when both are set, and `<SERVER>` is the
//! endpoint identity (everything after the scheme) with non-alphanumeric
//! characters folded to `_`. The mock server observes the real
//! `Authorization` header on every POST.
//!
//! This file is deliberately its own test binary with a single test: it
//! mutates process-global environment variables, so nothing else may read
//! them concurrently.

use oxo_flow_ai::mcp::{McpClient, McpHttpClient};
use serde_json::json;

mod mcp_mock;
use mcp_mock::{MockMcp, Reply};

const GENERIC_TOKEN: &str = "generic-mock-token";
const PER_SERVER_TOKEN: &str = "per-server-mock-token";

/// The documented per-server variable name for an endpoint URL.
fn documented_token_var(url: &str) -> String {
    let suffix: String = url
        .trim_start_matches("mcp://")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("OXO_FLOW_MCP_TOKEN_{suffix}")
}

fn build_client(url: &str) -> McpHttpClient {
    McpHttpClient::new_with_allowlist(url, vec!["127.0.0.1".to_string()])
        .expect("loopback endpoint exempted explicitly")
}

fn authorization_headers(requests: &[mcp_mock::Received]) -> Vec<Option<String>> {
    requests
        .iter()
        .map(|request| request.header("authorization").map(str::to_string))
        .collect()
}

#[tokio::test]
async fn bearer_token_env_is_propagated_with_per_server_precedence() {
    let server = MockMcp::spawn(|request| {
        let body = request.json();
        match body["method"].as_str().unwrap_or("") {
            "initialize" => Reply::Json {
                body: json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {"protocolVersion": "2025-06-18", "capabilities": {}}
                }),
                session: Some("sess-bearer"),
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
            other => panic!("unexpected MCP method: {other}"),
        }
    });
    let per_server_var = documented_token_var(&server.url);

    // SAFETY: this test binary contains a single test, so no other thread
    // reads the process environment while it is mutated. The variables are
    // removed again at the end.
    unsafe {
        std::env::set_var("OXO_FLOW_MCP_TOKEN", GENERIC_TOKEN);
        std::env::remove_var(&per_server_var);
    }

    // Phase 1: only the generic token is configured.
    let generic_client = build_client(&server.url);
    generic_client
        .list_tools()
        .await
        .expect("list with generic token");
    let generic_requests = server.requests();
    assert!(!generic_requests.is_empty());
    let expected_generic = format!("Bearer {GENERIC_TOKEN}");
    for header in authorization_headers(&generic_requests) {
        assert_eq!(
            header.as_deref(),
            Some(expected_generic.as_str()),
            "every request carries the generic bearer token"
        );
    }

    // Phase 2: the per-server variable wins over the generic one.
    let offset = server.count();
    // SAFETY: same single-threaded mutation as above.
    unsafe { std::env::set_var(&per_server_var, PER_SERVER_TOKEN) };
    let specific_client = build_client(&server.url);
    specific_client
        .list_tools()
        .await
        .expect("list with per-server token");
    let specific_requests = server.requests_from(offset);
    assert!(!specific_requests.is_empty());
    let expected_specific = format!("Bearer {PER_SERVER_TOKEN}");
    for header in authorization_headers(&specific_requests) {
        assert_eq!(
            header.as_deref(),
            Some(expected_specific.as_str()),
            "the per-server token must shadow the generic one"
        );
    }

    // Phase 3: with nothing configured, no Authorization header is sent.
    let offset = server.count();
    // SAFETY: same single-threaded mutation as above.
    unsafe {
        std::env::remove_var("OXO_FLOW_MCP_TOKEN");
        std::env::remove_var(&per_server_var);
    }
    let anonymous_client = build_client(&server.url);
    anonymous_client
        .list_tools()
        .await
        .expect("list anonymously");
    for header in authorization_headers(&server.requests_from(offset)) {
        assert_eq!(
            header, None,
            "no token configured means no Authorization header"
        );
    }
}
