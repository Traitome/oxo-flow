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
        // #518: the server-asserted readOnlyHint is advisory, not
        // authorization — a hostile or compromised MCP server can claim it
        // for an effectful tool, and auto-executing on it contradicts the
        // stated trust boundary (prompt injection only, zero code
        // execution). MCP tool calls therefore always route through the
        // human approver; where none exists (non-interactive sessions) they
        // are refused.
        false
    }
}

// ── Streamable HTTP transport ──────────────────────────────────────────────

/// Latest MCP protocol version this client speaks (#948 P3-7a): proposed
/// at `initialize`. The server answers with the version it wants — one we
/// don't understand fails the session closed (spec: the client SHOULD
/// disconnect). Newer revisions can be appended as the transport needs
/// them; the request handling below is identical across all of them.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Versions this client can work with. The transport (JSON-RPC over POST,
/// JSON or SSE bodies, `mcp-session-id`) is identical across them.
const MCP_UNDERSTOOD_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Pure precedence core for the MCP bearer token (#948 P3-7d): the
/// per-server token wins over the generic one — a credential pinned to
/// this server must never be shadowed by the shared default. Blank or
/// whitespace values count as unset; the picked value is trimmed so
/// embedded/trailing whitespace cannot poison the `Authorization` header.
fn pick_bearer_token<'a>(generic: &'a str, specific: &'a str) -> Option<&'a str> {
    let specific = specific.trim();
    if !specific.is_empty() {
        return Some(specific);
    }
    let generic = generic.trim();
    if !generic.is_empty() {
        return Some(generic);
    }
    None
}

/// Resolve the MCP bearer token for a server (#948 P3-7d): the per-server
/// `OXO_FLOW_MCP_TOKEN_<SERVER>` first, then the generic
/// `OXO_FLOW_MCP_TOKEN`. Auth material lives in the environment, never in
/// a skill manifest — the manifest is attacker-influenced text (#518).
/// Returns `None` when nothing is set.
fn resolve_bearer_token(server_name: &str) -> Option<String> {
    let specific_var = token_env_var(server_name);
    let specific = std::env::var(&specific_var).unwrap_or_default();
    let generic = std::env::var("OXO_FLOW_MCP_TOKEN").unwrap_or_default();
    // Log the variable NAMES only — never the values (single log site for
    // the whole consult, per the re-review NIT: the constructor no longer
    // duplicates it).
    tracing::debug!(
        var = %specific_var,
        generic_var = "OXO_FLOW_MCP_TOKEN",
        "MCP auth: token variables consulted (names only, never values)"
    );
    pick_bearer_token(&generic, &specific).map(str::to_string)
}

/// Environment variable name for a server's bearer token: the server name
/// uppercased with non-alphanumeric characters folded to `_` (pure —
/// unit-testable without touching process env, which tests may not do).
fn token_env_var(server_name: &str) -> String {
    format!(
        "OXO_FLOW_MCP_TOKEN_{}",
        server_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
            .to_ascii_uppercase()
    )
}

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
    /// Protocol version the server negotiated at initialize (#948 P3-7a),
    /// attached as `MCP-Protocol-Version` on every subsequent request.
    protocol_version: std::sync::Mutex<Option<String>>,
    /// Monotonic JSON-RPC request-id source (#948 P3-7c): hardcoded ids
    /// collided with the pagination ids and defeated the replay guard.
    next_id: std::sync::atomic::AtomicU64,
    /// Optional bearer token sent as `Authorization: Bearer …` on every
    /// request (#948 P3-7d). Resolved from the per-server
    /// `<OXO_FLOW_MCP_TOKEN_<SERVER>>` first, then the generic
    /// `<OXO_FLOW_MCP_TOKEN>` (server name uppercased, non-word runs →
    /// `_`) so per-server credentials stay out of skill manifests.
    /// Never logged; empty = no header.
    bearer_token: Option<String>,
    http: reqwest::Client,
    server_name: String,
    /// Exemptions for per-hop SSRF screening of redirect targets (#642).
    allow: Vec<String>,
    /// Vetted address from the construction-time screen (#948 P2-1), with
    /// the same semantics as `ScreenedTarget.pin`: set whenever DNS was
    /// consulted, so requests go to exactly what was screened. Allowlisted
    /// hosts and IP literals skip DNS and need no pin.
    pin: Option<(String, std::net::SocketAddr)>,
}

/// SSRF-screen an MCP endpoint host:port (same policy as `fetch_url`):
/// loopback/link-local/RFC1918/ULA and site-local names are refused unless
/// the exact host is exempted via `OXO_FLOW_AI_FETCH_ALLOW`. A host that
/// cannot resolve is rejected too — an unreachable server is unusable, and
/// failing closed keeps a later DNS change from smuggling the connection
/// into internal space.
///
/// Returns the vetted pin (same semantics as `ScreenedTarget.pin` in
/// `tools::builtin`): `Some((host, addr))` whenever DNS was consulted, so
/// every request is sent to exactly the address that was screened — a DNS
/// change between here and `rpc` cannot re-point the connection into
/// internal space. Allowlisted hosts and IP literals skip DNS and need no
/// pin (`None`).
fn screen_mcp_endpoint(
    base_url: &str,
    allow: &[String],
) -> Result<Option<(String, std::net::SocketAddr)>, AiError> {
    use std::net::ToSocketAddrs;

    use crate::tools::builtin::{BLOCKED_HOST_SUFFIXES, ip_is_forbidden};

    let parsed = reqwest::Url::parse(base_url).map_err(|e| AiError::Config {
        message: format!("invalid MCP endpoint '{base_url}': {e}"),
    })?;
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let blocked = |why: String| AiError::Config {
        message: format!(
            "MCP endpoint '{base_url}' refused: {why} (internal address space is blocked; OXO_FLOW_AI_FETCH_ALLOW exempts trusted hosts)"
        ),
    };
    if allow.iter().any(|a| a == &host) {
        return Ok(None);
    }
    if host == "localhost" || BLOCKED_HOST_SUFFIXES.iter().any(|sfx| host.ends_with(sfx)) {
        return Err(blocked(format!("host {host:?} is site-local")));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return if ip_is_forbidden(ip) {
            Err(blocked(format!(
                "host {ip} lies in forbidden address space"
            )))
        } else {
            // Literal IP: no name resolution happens, so there is nothing
            // to pin.
            Ok(None)
        };
    }
    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let addrs: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| blocked(format!("host {host:?} does not resolve: {e}")))?
        .collect();
    if let Some(offender) = addrs.iter().map(|a| a.ip()).find(|ip| ip_is_forbidden(*ip)) {
        return Err(blocked(format!(
            "host {host:?} resolves into forbidden address space ({offender})"
        )));
    }
    // Connect to the address that was screened — the first one suffices;
    // every entry was vetted above and multi-address failover would just
    // re-open name-resolution timing gaps we explicitly closed (#948 P2-1).
    // to_socket_addrs can legally yield an empty iterator without an error,
    // so fail closed like the fetch_url screen does.
    let Some(first) = addrs.first().copied() else {
        return Err(blocked(format!(
            "DNS resolution returned no addresses for {host:?}"
        )));
    };
    Ok(Some((host, first)))
}

/// Build an MCP HTTP client, optionally pinned to a vetted address.
///
/// Pinning overrides name resolution ONLY: SNI and certificate checks still
/// run against the hostname, so https stays verifiable (same pattern as
/// `fetch_client` in tools::builtin). Every client bounds its requests —
/// a hung MCP server must fail the tool call (fail-safe), never block the
/// AI command indefinitely — and follows NO redirects automatically;
/// `rpc` re-screens each hop manually (#642).
fn pinned_client(pin: Option<(&str, std::net::SocketAddr)>) -> Result<reqwest::Client, AiError> {
    let builder = reqwest::Client::builder();
    let builder = match pin {
        Some((host, addr)) => builder.resolve(host, addr),
        None => builder,
    };
    builder
        .connect_timeout(MCP_CONNECT_TIMEOUT)
        .timeout(MCP_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AiError::Config {
            message: format!("failed to build MCP HTTP client: {e}"),
        })
}

impl McpHttpClient {
    /// Build a client for an `mcp://` (or `http(s)://`) endpoint URL,
    /// SSRF-screened against the `OXO_FLOW_AI_FETCH_ALLOW` exemptions.
    pub fn new(url: &str) -> Result<Self, AiError> {
        Self::new_with_allowlist(url, crate::tools::builtin::fetch_allowlist())
    }

    /// Build a client with an explicit exemption list (test seam; the
    /// crate forbids unsafe code so tests cannot set process env).
    pub fn new_with_allowlist(url: &str, allow: Vec<String>) -> Result<Self, AiError> {
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
        // #518: a skill manifest is attacker-influenced text, so its
        // mcp:// target gets the same SSRF screen as fetch_url — internal
        // address space is unreachable unless explicitly allowlisted.
        // Fail before any model call or request. The returned pin travels
        // with the client (#948 P2-1): DNS-resolved endpoints connect to
        // the exact address that was screened.
        let pin = screen_mcp_endpoint(&base_url, &allow)?;
        // Identity used for the per-server token variable and error
        // messages: the URL minus scheme and trailing slash, with `:` and
        // `/` folded to `_`. The whole tail — host, port AND path — is
        // part of the identity (two MCP servers can share a host on
        // different paths), so `mcp://fs-gdrive:9000/v1` consults
        // `OXO_FLOW_MCP_TOKEN_FS_GDRIVE_9000_V1`.
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
        // (fail-safe), never block the AI command indefinitely. Redirects
        // are followed MANUALLY in `rpc` (policy none + re-screen per hop):
        // the default policy follows up to 10 hops blindly, letting a
        // hostile endpoint 302 the client into internal address space and
        // feed the fetched body back into the model transcript (#642 —
        // same chain #518 closed for fetch_url). A DNS-pinned endpoint
        // gets its pin baked into the shared client; redirect hops rebuild
        // short-lived clients in `rpc` for their own (re-screened) pins.
        let http = pinned_client(pin.as_ref().map(|(h, a)| (h.as_str(), *a)))?;
        let bearer_token = resolve_bearer_token(&server_name);
        Ok(Self {
            base_url,
            session_id: std::sync::Mutex::new(None),
            protocol_version: std::sync::Mutex::new(None),
            next_id: std::sync::atomic::AtomicU64::new(1),
            bearer_token,
            http,
            server_name,
            allow,
            pin,
        })
    }

    /// Reserve the next monotonically increasing request id. A fresh id per
    /// call keeps the replay guard (`extract_rpc_result` id check) sound:
    /// hardcoded ids collided with the `tools/list` pagination ids and let
    /// a stale body for a previous same-id request be accepted (#948 P3-7c).
    fn reserve_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
        // Manual redirect loop: every hop re-runs the SSRF screen before
        // the request is sent (#642), and the vetted pin is carried into
        // the next hop so it connects to exactly what was screened (#948
        // P2-1) — a DNS change mid-chain cannot re-point the connection.
        const MAX_REDIRECTS: usize = 5;
        // The endpoint's origin governs which credentials may cross a
        // redirect: `Authorization` is only sent to the origin the token
        // was minted for (RFC 6265bis §5.5 §7.1 spirit; the MCP spec does
        // not bless forwarding auth cross-origin), and the session id is
        // scoped to that origin's server too.
        let base_origin = origin_of(&self.base_url);
        let mut current_url = self.base_url.clone();
        let mut current_pin = self.pin.clone();
        for _hop in 0..=MAX_REDIRECTS {
            // A pinned hop builds its own short-lived client; this costs a
            // connect per fetch/hop but AI-driven MCP calls are rare and the
            // guarantee is worth it. The shared client serves pin-free
            // targets (allowlisted hosts and IP literals — same split as
            // `validated_get` in tools::builtin).
            let pinned;
            let send_client = match &current_pin {
                Some((host, addr)) => {
                    pinned = pinned_client(Some((host.as_str(), *addr)))?;
                    &pinned
                }
                None => &self.http,
            };
            let same_origin = origin_of(&current_url) == base_origin;
            let mut req = send_client
                .post(&current_url)
                .header("Accept", "application/json, text/event-stream")
                .json(&body);
            if same_origin {
                if let Some(sid) = self.session_id.lock().unwrap().clone() {
                    req = req.header("mcp-session-id", sid);
                }
                // Post-initialize requests announce the negotiated version
                // (#948 P3-7a) so version-strict servers can validate it.
                let pv = self.protocol_version.lock().unwrap().clone();
                if let Some(pv) = pv {
                    req = req.header("MCP-Protocol-Version", pv);
                }
                // Bearer auth (#948 P3-7d): token resolved from the
                // environment at construction — never logged, never stored
                // in manifests. Cross-origin redirect hops get nothing.
                if let Some(token) = &self.bearer_token {
                    req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
                }
            } else {
                tracing::info!(
                    "MCP redirect hop {current_url} crosses origins — auth, session and protocol headers omitted"
                );
            }
            let response = req.send().await.map_err(|e| AiError::Transport {
                message: format!("MCP request to {current_url} failed: {e}"),
            })?;

            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AiError::Transport {
                        message: format!(
                            "MCP endpoint {current_url} redirected without a Location header"
                        ),
                    })?;
                let next = reqwest::Url::parse(&current_url)
                    .and_then(|base| base.join(location))
                    .map_err(|e| AiError::Transport {
                        message: format!("MCP redirect Location '{location}' is invalid: {e}"),
                    })?;
                let next_url = next.to_string();
                current_pin = screen_mcp_endpoint(&next_url, &self.allow)?;
                tracing::info!("MCP endpoint {current_url} redirected to {next_url}");
                current_url = next_url;
                continue;
            }

            // Response-direction twin of the request-side gate: a session
            // id minted by a cross-origin hop's server must never be
            // adopted, or the next request to the base origin would hand
            // it a credential from a different origin.
            if same_origin
                && let Some(Ok(v)) = response.headers().get("mcp-session-id").map(|s| s.to_str())
            {
                *self.session_id.lock().unwrap() = Some(v.to_string());
            }
            let text = response.text().await.map_err(|e| AiError::Transport {
                message: format!("MCP response read failed: {e}"),
            })?;
            return parse_rpc_response(&text, id).map_err(|e| AiError::Protocol { message: e });
        }
        Err(AiError::Transport {
            message: format!("MCP endpoint {current_url} exceeded {MAX_REDIRECTS} redirects"),
        })
    }
}

/// Origin of an endpoint URL: `scheme://host[:port]`, with the default
/// port omitted (`https` → 443, `http` → 80) so redirects that only
/// re-spell the default port stay same-origin. The path is not part of an
/// origin. Used to decide which redirect hops may carry credentials.
fn origin_of(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    let scheme = u.scheme();
    let port = match u.port_or_known_default() {
        Some(80) if scheme == "http" => None,
        Some(443) if scheme == "https" => None,
        p => p,
    };
    let host = u.host_str()?;
    Some(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
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
        && let Some(structured) = result.get("structuredContent")
    {
        // Spec: structuredContent is a JSON object, not a string — the old
        // `as_str()` peek made the branch dead and every structured result
        // degraded to the whole-envelope `to_string()` (#948 P3-7b).
        // Serialize it losslessly. Note: `serde_json::Value::to_string`
        // emits keys sorted (no preserve_order here), so the output is
        // deterministic even when the server varies key order.
        if !structured.is_null() {
            text = Some(structured.to_string());
        }
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
        //
        // Protocol negotiation (#948 P3-7a): propose the newest version we
        // speak; the server answers with the one it wants. A version we
        // don't understand fails the session closed — silently continuing
        // let two sides talk past each other with different framing
        // assumptions (the old client hardcoded "2024-11-05" and never
        // looked at the answer).
        let init_id = self.reserve_id();
        let init = self
            .rpc(
                init_id,
                "initialize",
                serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "oxo-flow", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        let server_version = init
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !MCP_UNDERSTOOD_VERSIONS.contains(&server_version.as_str()) {
            return Err(AiError::Protocol {
                message: format!(
                    "MCP server '{}' negotiated protocol version {server_version:?}, but this client speaks only {}",
                    self.server_name,
                    MCP_UNDERSTOOD_VERSIONS.join(", ")
                ),
            });
        }
        tracing::debug!(
            server = %self.server_name,
            version = %server_version,
            "MCP protocol version negotiated"
        );
        *self.protocol_version.lock().unwrap() = Some(server_version);
        // Spec: once initialize returns an mcp-session-id, every
        // subsequent request — the initialized notification included —
        // must carry it. This is a fire-and-forget notification (no id,
        // response discarded), but it still rides the same header set as
        // any other request to this origin: session id, negotiated
        // protocol version and bearer token. (The version is stored just
        // above, so it is already available here.)
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
        if let Some(pv) = self.protocol_version.lock().unwrap().clone() {
            note_req = note_req.header("MCP-Protocol-Version", pv);
        }
        if let Some(token) = &self.bearer_token {
            note_req = note_req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let _ = note_req.send().await;

        // tools/list is paginated: the first page carries `nextCursor` when
        // more tools exist. Ignoring it silently exposed only the first page
        // (audit finding) — follow the cursor, bounded so a looping server
        // cannot spin forever.
        let mut tools: Vec<McpToolDef> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let request_id = self.reserve_id();
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
            pages += 1;
            cursor = result
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .map(String::from);
            if cursor.is_none() {
                break;
            }
            // Bounded so a looping server cannot spin forever (and, since
            // #948 P3-7c, ids stay monotonic across pages — no collision
            // with a later call's id).
            if pages >= MAX_TOOL_PAGES {
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
        // Monotonic id (#948 P3-7c): the old hardcoded id 3 collided with
        // the tools/list pagination range, so after a paginated discover
        // every call reused an id and the replay guard could not tell a
        // fresh answer from a stale same-id body.
        let request_id = self.reserve_id();
        let result = self
            .rpc(
                request_id,
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

    /// Build a client as `new` does, but with an explicit allowlist — the
    /// crate forbids unsafe code, so tests cannot mutate the process env.
    fn client_with_allow(url: &str, allow: &[&str]) -> Result<McpHttpClient, AiError> {
        McpHttpClient::new_with_allowlist(url, allow.iter().map(|s| s.to_string()).collect())
    }

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
    fn tool_result_text_structured_content_object_is_serialized() {
        // Spec: structuredContent is a JSON object. The old `.as_str()` peek
        // made the branch dead, so a structured-only result degraded to the
        // whole-envelope dump including isError/content (#948 P3-7b).
        let result = serde_json::json!({
            "content": [],
            "structuredContent": {"result": 42, "unit": "bp"},
        });
        let text = tool_result_text("q", &result).unwrap();
        assert_eq!(text, r#"{"result":42,"unit":"bp"}"#);

        // Non-object structured values (legacy servers) are serialized too —
        // anything beats the envelope dump.
        let scalar = serde_json::json!({"structuredContent": 7});
        assert_eq!(tool_result_text("q", &scalar).unwrap(), "7");

        // content text still wins over structuredContent.
        let both = serde_json::json!({
            "content": [{"type": "text", "text": "plain"}],
            "structuredContent": {"hidden": true},
        });
        assert_eq!(tool_result_text("q", &both).unwrap(), "plain");
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
        // readOnlyHint is advisory (#518): NO MCP tool is read-only —
        // every call routes through the human approver regardless of the
        // server-asserted hint.
        assert!(!registry.is_read_only(read_name));
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

        // Loopback fixtures are exempted explicitly — exactly what a
        // deployment with a trusted internal MCP server must do (#518).
        let client = client_with_allow(&format!("mcp://{addr}"), &["127.0.0.1"]).unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);

        // readOnlyHint is advisory only (#518): even a hinted read-only MCP
        // tool routes through the human approver.
        let bridges = McpToolBridge::discover(std::sync::Arc::new(client))
            .await
            .unwrap();
        assert!(!bridges[0].is_read_only());
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
                // initialize with a version the client understands —
                // negotiation must succeed against an old-style server.
                "initialize" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2024-11-05"}}),
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
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
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
            match req["method"].as_str().unwrap_or("") {
                "initialize" => {
                    serde_json::json!({"jsonrpc":"2.0","id":req["id"].clone(),"result":{
                    "protocolVersion":"2024-11-05"}})
                }
                "tools/list" =>
                // A stale/replayed body: this id belongs to another request.
                {
                    serde_json::json!({"jsonrpc":"2.0","id":99,"result":{"tools":[]}})
                }
                _ => serde_json::json!({"jsonrpc":"2.0","id":req["id"].clone(),"result":{}}),
            }
        });
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
        let err = client.list_tools().await.unwrap_err().to_string();
        assert!(err.contains("id mismatch"), "{err}");
    }

    #[tokio::test]
    async fn mcp_http_client_ids_stay_monotonic_across_paginated_discovery_and_call() {
        // #948 P3-7c: the old client hardcoded tools/call id 3, colliding
        // with the pagination id range — after a paginated discover, the
        // replay guard could not distinguish a fresh answer from a stale
        // same-id body. Record every id the server sees; they must be
        // unique per request and a tools/call after pagination must use a
        // brand-new id.
        let seen_ids = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let seen_for_server = seen_ids.clone();
        let (url, _calls) = json_rpc_fixture(move |req| {
            let id = req["id"].as_u64().unwrap_or(0);
            if req["method"].as_str() != Some("notifications/initialized") {
                seen_for_server.lock().unwrap().push(id);
            }
            match req["method"].as_str().unwrap_or("") {
                "initialize" => serde_json::json!({
                    "jsonrpc":"2.0","id":id,
                    "result":{"protocolVersion":"2025-06-18"}
                }),
                "tools/list" => {
                    let cursor = req["params"]["cursor"].as_str().unwrap_or("");
                    if cursor.is_empty() {
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                            "tools":[{"name":"p1","description":"p1","inputSchema":{"type":"object"}}],
                            "nextCursor":"page-2"}})
                    } else {
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                            "tools":[{"name":"p2","description":"p2","inputSchema":{"type":"object"}}]}})
                    }
                }
                "tools/call" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "content":[{"type":"text","text":"call-ok"}]}}),
                _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}}),
            }
        });
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        let result = client
            .call_tool("p1", r#"{"x":1}"#)
            .await
            .expect("tools/call with a fresh id must succeed");
        assert_eq!(
            result, "call-ok",
            "the call's own text result must come back"
        );

        let ids = seen_ids.lock().unwrap().clone();
        // initialize + 2 pagination pages + tools/call.
        assert_eq!(ids, vec![1, 2, 3, 4], "ids must be exactly 1..=4: {ids:?}");
    }

    #[tokio::test]
    async fn mcp_http_client_fails_closed_on_unsupported_protocol_version() {
        // #948 P3-7a: the old client hardcoded its version and never read
        // the server's answer — a server negotiating a future protocol
        // would be talked to with mismatched framing assumptions.
        let (url, _calls) = json_rpc_fixture(|req| {
            let id = req["id"].clone();
            if req["method"].as_str() == Some("initialize") {
                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2999-01-01"}})
            } else {
                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}})
            }
        });
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
        let err = client.list_tools().await.unwrap_err().to_string();
        assert!(
            err.contains("protocol version") && err.contains("2999-01-01"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn mcp_http_client_announces_negotiated_version_after_initialize() {
        // #948 P3-7a: initialize persists the server's chosen version for
        // the MCP-Protocol-Version header path.
        let (url, _calls) = json_rpc_fixture(move |req| {
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2025-03-26"}}),
                _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}}),
            }
        });
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
        client.list_tools().await.unwrap();
        // The fixture thread can't see headers, so assert on client state:
        // the negotiated version must be stored for the header path.
        assert_eq!(
            client.protocol_version.lock().unwrap().as_deref(),
            Some("2025-03-26")
        );
    }

    /// Raw-socket JSON-RPC fixture: like `json_rpc_fixture`, but every
    /// request's header block is recorded so tests can assert on
    /// `MCP-Protocol-Version` / `Authorization` / `mcp-session-id`
    /// emission (#948 P3-7 review finding: state-only assertions never
    /// proved the headers hit the wire).
    fn raw_header_fixture(
        respond: impl Fn(&serde_json::Value) -> serde_json::Value + Send + 'static,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let headers: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let headers_for_server = headers.clone();
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
                        // Capture every header line of this request
                        // (everything between the request line and the
                        // blank line), lowercased for stable matching.
                        let head = req.split("\r\n\r\n").next().unwrap_or("");
                        let lines: Vec<String> = head
                            .lines()
                            .skip(1)
                            .filter_map(|l| {
                                l.split_once(':').map(|(k, v)| {
                                    format!("{}:{}", k.trim().to_ascii_lowercase(), v.trim())
                                })
                            })
                            .collect();
                        headers_for_server.lock().unwrap().extend(lines);
                        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                        let json: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or_default();
                        let response_body = respond(&json).to_string();
                        // Issue a session id like a real MCP server, so
                        // capture tests can assert it is forwarded.
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nmcp-session-id: sess-1\r\nContent-Length: {}\r\n\r\n{}",
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
        (format!("mcp://{addr}"), headers)
    }

    #[tokio::test]
    async fn mcp_http_client_post_initialize_requests_carry_protocol_and_auth_headers() {
        // #948 P3-7 review finding: the negotiated `MCP-Protocol-Version`
        // header (and the bearer token, when configured) must actually hit
        // the wire on requests after initialize — client state alone proves
        // nothing. Also covers the `notifications/initialized` hop: it
        // rides the same header set.
        let (url, headers) = raw_header_fixture(|req| {
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2025-03-26"}}),
                _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}}),
            }
        });
        let client = client_with_allow(&url, &["127.0.0.1"]).unwrap();
        client.list_tools().await.unwrap();
        let hdrs = headers.lock().unwrap().clone();
        // Exactly one MCP-Protocol-Version header, on the post-initialize
        // requests (tools/list and the notification), with the negotiated
        // version.
        let pv: Vec<&String> = hdrs
            .iter()
            .filter(|h| h.starts_with("mcp-protocol-version:"))
            .collect();
        assert_eq!(
            pv.len(),
            2,
            "protocol version on tools/list + notification, exactly once each: {hdrs:?}"
        );
        assert!(
            pv.iter().all(|h| h.ends_with("2025-03-26")),
            "negotiated version must be announced: {pv:?}"
        );
        // No Authorization header — but only meaningful when no token is
        // configured for this server; an exported OXO_FLOW_MCP_TOKEN in the
        // developer's shell would legitimately add one (re-review NIT-3).
        if client.bearer_token.is_none() {
            assert!(
                !hdrs.iter().any(|h| h.starts_with("authorization:")),
                "bearer token must be absent when none is configured: {hdrs:?}"
            );
        }
        // The session id, once issued, rides the post-initialize requests.
        assert!(
            hdrs.iter().any(|h| h.starts_with("mcp-session-id:")),
            "session id must be forwarded after initialize: {hdrs:?}"
        );
    }

    /// Second raw-socket fixture for cross-origin tests: serves a 302 to
    /// `redirect_to` on the first request, then answers normally.
    fn redirect_then_answer_fixture(
        redirect_to: &str,
        respond: impl Fn(&serde_json::Value) -> serde_json::Value + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let redirect_to = redirect_to.to_string();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let mut redirected = false;
            loop {
                let mut buf = [0u8; 65536];
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let response = if !redirected {
                            redirected = true;
                            "HTTP/1.1 302 Found\r\nLocation: REDIRECT_TARGET\r\nContent-Length: 0\r\n\r\n"
                                .replace("REDIRECT_TARGET", &redirect_to)
                        } else {
                            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
                            let json: serde_json::Value =
                                serde_json::from_str(body).unwrap_or_default();
                            let response_body = respond(&json).to_string();
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                response_body.len(),
                                response_body
                            )
                        };
                        if stream.write_all(response.as_bytes()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        format!("mcp://{addr}")
    }

    #[tokio::test]
    async fn mcp_http_client_redirect_hop_carries_no_auth_session_or_protocol_headers() {
        // #948 P3-7 review MAJOR-2: a cross-origin redirect hop must go out
        // without `Authorization`, `mcp-session-id` or `MCP-Protocol-Version`
        // — and its own session id must not be adopted for the base origin.
        let (target_url, target_headers) = raw_header_fixture(|req| {
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2025-03-26"}}),
                _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}}),
            }
        });
        // The fixture hands back `mcp://`; the redirect Location must be
        // the http form — the constructor rewrites the endpoint scheme
        // once, but `rpc` follows Location values verbatim and reqwest
        // cannot issue requests with an `mcp://` scheme.
        let target_http = target_url.replacen("mcp://", "http://", 1);
        let origin_url = redirect_then_answer_fixture(&target_http, |req| {
            let id = req["id"].clone();
            serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})
        });
        // The base origin 302s to the second fixture (different port =
        // different origin). The client must follow it, but stripped.
        let client = client_with_allow(&origin_url, &["127.0.0.1"]).unwrap();
        client.list_tools().await.unwrap();
        let hdrs = target_headers.lock().unwrap().clone();
        assert!(
            !hdrs.iter().any(|h| h.starts_with("authorization:")),
            "cross-origin hop must carry no Authorization header: {hdrs:?}"
        );
        assert!(
            !hdrs.iter().any(|h| h.starts_with("mcp-protocol-version:")),
            "cross-origin hop must carry no MCP-Protocol-Version header: {hdrs:?}"
        );
        assert!(
            !hdrs.iter().any(|h| h.starts_with("mcp-session-id:")),
            "cross-origin hop must carry no session id (and none from the origin may leak to it): {hdrs:?}"
        );
    }

    #[test]
    fn pick_bearer_token_prefers_the_per_server_value() {
        // #948 P3-7 review MAJOR-1: the docs promise per-server wins; the
        // resolver must implement that order, and blank values count as
        // unset.
        assert_eq!(pick_bearer_token("generic", "specific"), Some("specific"));
        assert_eq!(pick_bearer_token("generic", "  "), Some("generic"));
        assert_eq!(pick_bearer_token("generic", ""), Some("generic"));
        assert_eq!(pick_bearer_token("  ", "specific"), Some("specific"));
        assert_eq!(pick_bearer_token("  ", ""), None);
        // Whitespace is trimmed off the winner.
        assert_eq!(pick_bearer_token("  tok  ", ""), Some("tok"));
    }

    #[test]
    fn origin_of_normalizes_default_ports_and_ignores_paths() {
        assert_eq!(
            origin_of("http://example.com/mcp").as_deref(),
            Some("http://example.com")
        );
        assert_eq!(
            origin_of("http://example.com:80/mcp").as_deref(),
            Some("http://example.com")
        );
        assert_eq!(
            origin_of("https://example.com:443/a").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://example.com:8080/x").as_deref(),
            Some("http://example.com:8080")
        );
        assert_eq!(
            origin_of("https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(origin_of("not a url"), None);
    }

    #[test]
    fn token_env_var_folds_server_names() {
        // #948 P3-7d: per-server token vars fold the name to upper snake.
        assert_eq!(token_env_var("fs"), "OXO_FLOW_MCP_TOKEN_FS");
        assert_eq!(token_env_var("fs-gdrive"), "OXO_FLOW_MCP_TOKEN_FS_GDRIVE");
        assert_eq!(
            token_env_var("genomics.tools"),
            "OXO_FLOW_MCP_TOKEN_GENOMICS_TOOLS"
        );
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
        // Must be an IP literal: a DNS name would force a live resolve in
        // the screen, and CI sandboxes / fake-IP resolvers (Clash) return
        // addresses inside forbidden space for arbitrary hosts (#948).
        assert!(McpHttpClient::new("https://93.184.216.34/mcp").is_ok());
    }

    #[test]
    fn mcp_endpoints_are_ssrf_screened() {
        // #518: internal address space and site-local names are refused;
        // the allowlist exempts explicitly trusted hosts.
        assert!(McpHttpClient::new("mcp://localhost:8080").is_err());
        assert!(McpHttpClient::new("mcp://127.0.0.1:8080").is_err());
        assert!(McpHttpClient::new("mcp://10.0.0.5:8080").is_err());
        assert!(McpHttpClient::new("mcp://169.254.169.254:80").is_err());
        assert!(McpHttpClient::new("mcp://myserver.local:9000").is_err());
        let allow = vec!["10.0.0.5".to_string(), "127.0.0.1".to_string()];
        assert!(McpHttpClient::new_with_allowlist("mcp://127.0.0.1:8080", allow.clone()).is_ok());
        assert!(McpHttpClient::new_with_allowlist("mcp://10.0.0.5:8080", allow).is_ok());
        assert!(McpHttpClient::new("mcp://localhost:8080").is_err());
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
