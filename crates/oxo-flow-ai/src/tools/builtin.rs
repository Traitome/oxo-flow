//! Built-in tools available to all AI agents.
//!
//! These tools are compiled into the binary and always available.
//! Additional tools can be registered by plugins or MCP servers.

use async_trait::async_trait;
use futures::StreamExt;

use super::{Tool, ToolDef};
use crate::error::AiError;

/// Read contents of a local file.
#[derive(Default)]
pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Read the contents of a local file. Use this to get information from user-provided reference files or existing workflow configurations.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    fn name(&self) -> &str {
        "read_file"
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "read_file".into(),
                message: format!("invalid arguments: {e}"),
            })?;

        let path = args["path"].as_str().ok_or_else(|| AiError::ToolError {
            tool: "read_file".into(),
            message: "missing 'path' argument".into(),
        })?;

        let content = std::fs::read_to_string(path).map_err(|e| AiError::ToolError {
            tool: "read_file".into(),
            message: format!("cannot read '{path}': {e}"),
        })?;

        Ok(content)
    }
}

/// Hostname suffixes treated as site-local and always blocked.
const BLOCKED_HOST_SUFFIXES: [&str; 3] = [".localhost", ".local", ".internal"];

/// Wall-clock bound for a single fetch hop, including TLS handshake.
const FETCH_TIMEOUT_SECS: u64 = 15;

fn user_agent() -> String {
    format!(
        "oxo-flow/{} (+https://github.com/Traitome/oxo-flow)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Build a fetch client, optionally pinned to one screened address.
///
// A browser-like User-Agent: some sources (Bioconductor, GitHub raw, docs
// sites) 403 the default reqwest UA, which sent the model into retry loops
// during pipeline generation (issue #79 P1-10). Redirects are manual so
// every hop re-runs the SSRF screen.
fn fetch_client(pin: Option<(&str, std::net::SocketAddr)>) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none());
    // Pinning overrides name resolution ONLY: SNI and certificate checks
    // still run against the hostname, so https stays verifiable.
    let builder = match pin {
        Some((host, addr)) => builder.resolve(host, addr),
        None => builder,
    };
    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// A URL that passed the SSRF screen, plus optional request pinning.
///
/// `pin` is set whenever DNS was consulted: the screened address travels
/// with the target so the connection goes to exactly what was vetted —
/// closing the re-resolution window (DNS rebinding) between screening and
/// connecting. Allowlisted hosts and literal IPs skip DNS and need no pin.
struct ScreenedTarget {
    url: reqwest::Url,
    pin: Option<(String, std::net::SocketAddr)>,
}

fn parse_allowlist(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Comma-separated explicit exemptions (`OXO_FLOW_AI_FETCH_ALLOW`), applied
/// to hostnames/IP literals verbatim before validation.
fn allowlist() -> Vec<String> {
    parse_allowlist(&std::env::var("OXO_FLOW_AI_FETCH_ALLOW").unwrap_or_default())
}

/// True for addresses an outbound model-driven fetch must never reach:
/// loopback, unspecified, link-local, RFC1918 / ULA, and IPv4-mapped IPv6
/// forms of any of those (cloud metadata endpoints live in 169.254/16).
fn ip_is_forbidden(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr::{V4, V6};
    let v4 = match ip {
        V4(v4) => v4,
        V6(v6) => match v6.to_ipv4_mapped() {
            Some(m) => m,
            None => {
                return v6.is_loopback()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
                    || (v6.segments()[0] & 0xfe00) == 0xfc00; // fc00::/7
            }
        },
    };
    v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() || v4.is_private()
}

/// Parse and SSRF-screen a model-supplied URL. Resolves DNS when needed so
/// hostile names pointing into internal space are caught pre-request; the
/// screened resolution is pinned onto the returned target so the later
/// connect cannot be silently re-pointed at internal space.
async fn validate_public_url(raw: &str) -> Result<ScreenedTarget, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("unparseable URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "scheme {:?} not allowed (http/https only) — embedded knowledge lives behind lookup_skill, not fetch_url",
            url.scheme()
        ));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if allowlist().contains(&host) {
        return Ok(ScreenedTarget { url, pin: None });
    }
    if host == "localhost" || BLOCKED_HOST_SUFFIXES.iter().any(|sfx| host.ends_with(sfx)) {
        return Err(format!("host {host:?} is site-local and blocked"));
    }
    if let Ok(ip) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    {
        return if ip_is_forbidden(ip) {
            Err(format!("host {ip} lies in forbidden address space"))
        } else {
            Ok(ScreenedTarget { url, pin: None })
        };
    }
    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let hostport = format!("{host}:{port}");
    let addrs = tokio::task::spawn_blocking(move || {
        std::net::ToSocketAddrs::to_socket_addrs(&hostport)
            .map(|it| it.collect::<Vec<_>>())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("resolver join failed: {e}"))?
    .map_err(|e| format!("DNS resolution failed for {host}: {e}"))?;
    if addrs.is_empty() {
        return Err(format!("no addresses resolved for {host}"));
    }
    if let Some(bad) = addrs.iter().find(|a| ip_is_forbidden(a.ip())) {
        return Err(format!("host {host} resolves to forbidden address {bad}"));
    }
    // Connect to the address that was screened — the first one suffices;
    // every entry was vetted above and multi-address failover would just
    // re-open name-resolution timing gaps we explicitly closed.
    Ok(ScreenedTarget {
        url,
        pin: Some((host, addrs[0])),
    })
}

/// GET with manual redirects (max 5 hops), re-running the SSRF screen on
/// every Location target. DNS-resolved hops are sent through a pinned
/// client, so each request reaches the address that was just screened.
async fn validated_get(client: &reqwest::Client, raw: &str) -> Result<reqwest::Response, String> {
    let mut current = validate_public_url(raw).await?;
    for _hop in 0..=5 {
        // A pinned hop builds its own short-lived client; this costs a TLS
        // handshake per fetch/hop but model-driven fetches are rare and the
        // guarantee is worth it. The shared client serves pin-free targets.
        let pinned;
        let send_client = match &current.pin {
            Some((host, addr)) => {
                pinned = fetch_client(Some((host.as_str(), *addr)));
                &pinned
            }
            None => client,
        };
        let resp = send_client
            .get(current.url.clone())
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "redirect without Location header".to_string())?;
            let next = current
                .url
                .join(loc)
                .map_err(|e| format!("bad redirect target: {e}"))?;
            current = validate_public_url(next.as_str()).await?;
            continue;
        }
        return Ok(resp);
    }
    Err("too many redirects (>5)".into())
}

/// Maximum bytes of a fetched response body handed back to the model. The
/// 15 s request timeout bounds *time*, not size: a fast endpoint can stream
/// hundreds of megabytes in that window, and nothing near that fits the
/// model's context (or the tool-result budget) anyway.
const MAX_FETCH_BYTES: usize = 256 * 1024;

/// Accumulates a response body up to [`MAX_FETCH_BYTES`] and reports whether
/// the source had more. Reading happens chunk-at-a-time so an oversized (or
/// endless) response is abandoned once the cap is reached instead of being
/// buffered whole.
struct CappedBody {
    bytes: Vec<u8>,
    truncated: bool,
}

impl CappedBody {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
        }
    }

    /// Append one chunk; returns `false` once the cap is reached, telling
    /// the caller to stop reading.
    fn push(&mut self, chunk: &[u8]) -> bool {
        let room = MAX_FETCH_BYTES.saturating_sub(self.bytes.len());
        if chunk.len() > room {
            self.bytes.extend_from_slice(&chunk[..room]);
            self.truncated = true;
            return false;
        }
        self.bytes.extend_from_slice(chunk);
        self.bytes.len() < MAX_FETCH_BYTES
    }

    /// The body as text, with a marker appended when it was cut short.
    fn finish(self) -> String {
        let text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            format!("{text}\n[... response truncated at {MAX_FETCH_BYTES} bytes ...]")
        } else {
            text
        }
    }
}

/// Fetch content from a URL.
#[derive(Default)]
pub struct FetchUrlTool {
    client: reqwest::Client,
}

impl FetchUrlTool {
    pub fn new() -> Self {
        Self {
            // Shared client for pin-free targets; DNS-resolved hops build a
            // short-lived pinned client per request instead (see `validated_get`).
            client: fetch_client(None),
        }
    }
}

#[async_trait]
impl Tool for FetchUrlTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "fetch_url".into(),
            description: "Fetch content from a public http or https URL. Use this to retrieve protocol documentation, tool references, or other web resources; returns the text content of the page. Only http/https URLs are allowed — never invent other schemes. For embedded knowledge (skills, tool docs) use lookup_skill instead of this tool.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn name(&self) -> &str {
        "fetch_url"
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "fetch_url".into(),
                message: format!("invalid arguments: {e}"),
            })?;

        let url = args["url"].as_str().ok_or_else(|| AiError::ToolError {
            tool: "fetch_url".into(),
            message: "missing 'url' argument".into(),
        })?;

        let response =
            validated_get(&self.client, url)
                .await
                .map_err(|reason| AiError::ToolError {
                    tool: "fetch_url".into(),
                    message: format!("blocked: {reason}"),
                })?;

        let mut body = CappedBody::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AiError::ToolError {
                tool: "fetch_url".into(),
                message: format!("read response failed: {e}"),
            })?;
            if !body.push(&chunk) {
                break;
            }
        }

        Ok(body.finish())
    }
}

/// Write content to a file (always archives before writing).
///
/// This is the only non-read-only builtin tool. It always creates a
/// backup before overwriting.
#[derive(Default)]
pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description: "Write content to a file. Always creates a backup first. Use this to save generated workflow files or apply modifications.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to write the file to"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn name(&self) -> &str {
        "write_file"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "write_file".into(),
                message: format!("invalid arguments: {e}"),
            })?;

        let path =
            std::path::Path::new(args["path"].as_str().ok_or_else(|| AiError::ToolError {
                tool: "write_file".into(),
                message: "missing 'path' argument".into(),
            })?);

        let content = args["content"].as_str().ok_or_else(|| AiError::ToolError {
            tool: "write_file".into(),
            message: "missing 'content' argument".into(),
        })?;

        // Archive the previous contents so an agent's overwrite is always
        // recoverable — this is what the tool description promises.
        //
        // The stamp carries nanoseconds and the loop below breaks any
        // residual tie: a second-granular name let two writes to the same
        // path within one second overwrite the first backup, silently
        // destroying the earlier version.
        let mut backed_up_to = None;
        if path.exists() {
            let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.9f").to_string();
            let mut backup = std::path::PathBuf::from(format!("{}.bak.{stamp}", path.display()));
            let mut collision = 0u32;
            while backup.exists() {
                collision += 1;
                backup =
                    std::path::PathBuf::from(format!("{}.bak.{stamp}.{collision}", path.display()));
            }
            match std::fs::copy(path, &backup) {
                Ok(_) => backed_up_to = Some(backup),
                Err(e) => {
                    return Err(AiError::ToolError {
                        tool: "write_file".into(),
                        message: format!(
                            "refusing to overwrite {}: cannot create backup ({}). \
                             Back up or remove the file manually first.",
                            path.display(),
                            e
                        ),
                    });
                }
            }
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AiError::ToolError {
                tool: "write_file".into(),
                message: format!("cannot create parent dir: {e}"),
            })?;
        }

        std::fs::write(path, content).map_err(|e| AiError::ToolError {
            tool: "write_file".into(),
            message: format!("cannot write file: {e}"),
        })?;

        match backed_up_to {
            Some(backup) => Ok(format!(
                "Successfully wrote {} bytes to {} (previous contents archived to {})",
                content.len(),
                path.display(),
                backup.display()
            )),
            None => Ok(format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                path.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_tool_has_correct_def() {
        let tool = ReadFileTool::new();
        let def = tool.def();
        assert_eq!(def.name, "read_file");
        assert!(def.description.contains("Read"));
        assert!(
            def.parameters["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("path"))
        );
    }

    #[test]
    fn fetch_url_tool_has_correct_def() {
        let tool = FetchUrlTool::new();
        assert_eq!(tool.name(), "fetch_url");
        // Models invent schemes when the constraint is unstated: a run
        // burned two rounds calling this tool with `skill://...` because
        // the description never said http/https-only and never said that
        // embedded knowledge comes from lookup_skill instead.
        let desc = tool.def().description;
        assert!(desc.contains("http"), "description must name the scheme constraint: {desc}");
        assert!(
            desc.to_lowercase().contains("lookup_skill"),
            "description must point at lookup_skill for embedded knowledge: {desc}"
        );
    }

    #[test]
    fn write_file_tool_is_not_read_only() {
        let tool = WriteFileTool::new();
        assert!(!tool.is_read_only());
    }

    #[test]
    fn read_file_tool_is_read_only() {
        let tool = ReadFileTool::new();
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn read_file_tool_reads_content() {
        let tool = ReadFileTool::new();
        // Read Cargo.toml of this crate
        let result = tool.execute(r#"{"path": "Cargo.toml"}"#).await.unwrap();
        assert!(result.contains("oxo-flow-ai"));
    }

    #[tokio::test]
    async fn read_file_tool_errors_on_missing_file() {
        let tool = ReadFileTool::new();
        let result = tool.execute(r#"{"path": "/nonexistent/file.txt"}"#).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_file_tool_writes_and_reads() {
        let tool = WriteFileTool::new();
        let tmp = std::env::temp_dir().join("oxo-flow-ai-test-write.txt");
        let _ = std::fs::remove_file(&tmp);

        let result = tool
            .execute(&format!(
                r#"{{"path": "{}", "content": "hello world"}}"#,
                tmp.display()
            ))
            .await
            .unwrap();
        assert!(result.contains("Successfully wrote"));

        let content = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(content, "hello world");
        std::fs::remove_file(&tmp).ok();
    }

    #[tokio::test]
    async fn write_file_tool_keeps_every_backup_within_one_second() {
        // Two overwrites inside the same second must BOTH be recoverable —
        // the old second-granular backup name overwrote the first backup.
        let tool = WriteFileTool::new();
        let dir = std::env::temp_dir().join("oxo-flow-ai-backup-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("main.oxoflow");

        for content in ["version one", "version two", "version three"] {
            tool.execute(&format!(
                r#"{{"path": "{}", "content": "{content}"}}"#,
                target.display()
            ))
            .await
            .unwrap();
        }

        let mut backups: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".bak."))
            .collect();
        backups.sort();
        assert_eq!(
            backups.len(),
            2,
            "each overwrite of an existing file must leave its own backup: {backups:?}"
        );
        let contents: Vec<String> = backups
            .iter()
            .map(|n| std::fs::read_to_string(dir.join(n)).unwrap())
            .collect();
        assert!(contents.contains(&"version one".to_string()));
        assert!(contents.contains(&"version two".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Look up tools in the embedded Bioconda CLI database.
///
/// Query by exact name, name prefix/substring, or summary keyword.
/// Returns real tool names, current Bioconda versions, descriptions,
/// and supported platforms. The advertised record count is derived from
/// the embedded data (never hardcoded — it drifted twice already).
#[derive(Default)]
pub struct LookupTool;

impl LookupTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LookupTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "lookup_tool".into(),
            description: format!(
                "Search the embedded Bioconda CLI database ({} tools) for bioinformatics tools. \
                 Query by tool name, name fragment, or purpose keyword (e.g. 'star', 'align', 'variant calling'). \
                 Returns tool names, current Bioconda versions, descriptions, and platform support. \
                 Use this to pick the right tool and pin its current version instead of guessing.",
                crate::knowledge::bioconda::tool_count()
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Tool name or purpose keyword to search for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default 10)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn name(&self) -> &str {
        "lookup_tool"
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "lookup_tool".into(),
                message: format!("invalid arguments: {e}"),
            })?;

        let query = args["query"].as_str().ok_or_else(|| AiError::ToolError {
            tool: "lookup_tool".into(),
            message: "missing 'query' argument".into(),
        })?;
        let limit = args["limit"].as_u64().unwrap_or(10).min(20) as usize;

        let mut results = crate::knowledge::bioconda::format_search_results(query, limit);
        // When Bioconda has no match, extend the answer with the merged
        // registry (nf-core modules, commercial tools, bio.tools overlay).
        if results.starts_with("No Bioconda") {
            let registry = crate::knowledge::registry::format_registry_results(query, limit);
            if !registry.is_empty() {
                results.push('\n');
                results.push_str(&registry);
            }
        }
        // Freshness note (data date + record count) so the agent can weigh
        // how current the embedded database is.
        let freshness = crate::knowledge::meta::embedded_meta().and_then(|m| {
            crate::knowledge::meta::freshness_line_for(m, "bioconda_tools", chrono::Utc::now())
        });
        Ok(match freshness {
            Some(line) => format!("{results}\n{line}"),
            None => results,
        })
    }
}

/// Look up embedded bioinformatics skills (562 Agent Skills from bioSkills).
///
/// Query by domain, tool name, or task keyword. Returns skill descriptions
/// with primary tools and procedure previews — curated agent expertise
/// for specific bioinformatics tasks.
#[derive(Default)]
pub struct LookupSkillTool;

impl LookupSkillTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LookupSkillTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "lookup_skill".into(),
            description: "Search the embedded bioinformatics skills library (562 Agent Skills curated from the bioSkills project) by domain, tool, or task keyword (e.g. 'rna-seq', 'variant-calling', 'samtools'). Returns domain expertise: correct commands, parameters, caveats, and procedure guidance for specific bioinformatics tasks. Use this before designing workflow rules for a domain you are less certain about.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Domain, tool, or task keyword to search for"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn name(&self) -> &str {
        "lookup_skill"
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "lookup_skill".into(),
                message: format!("invalid arguments: {e}"),
            })?;

        let query = args["query"].as_str().ok_or_else(|| AiError::ToolError {
            tool: "lookup_skill".into(),
            message: "missing 'query' argument".into(),
        })?;
        let limit = args["limit"].as_u64().unwrap_or(5).min(15) as usize;

        Ok(crate::knowledge::skills::format_skills(query, limit))
    }
}

/// Query the embedded bioinformatics pipeline knowledge graph. Understand
/// what feeds into or out of a workflow step, or find the pipeline path
/// between two steps. The advertised counts are derived from the embedded
/// data (never hardcoded — they drifted already).
#[derive(Default)]
pub struct LookupPipelineTool;

impl LookupPipelineTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LookupPipelineTool {
    fn def(&self) -> ToolDef {
        let (skills, transitions) = crate::knowledge::pipeline_graph::graph_stats();
        ToolDef {
            name: "lookup_pipeline".into(),
            description: format!(
                "Query the embedded bioinformatics pipeline knowledge graph ({skills} workflow skills, \
                 {transitions} data-flow transitions with data types and literature evidence). Use 'transitions' \
                 to see what feeds into/out of a step, or 'path' to find the pipeline between two steps. Use this \
                 to design correct multi-step workflow topologies (e.g. from alignment to variant calling to annotation)."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["transitions", "path", "stats"],
                        "description": "Query type"
                    },
                    "skill": {
                        "type": "string",
                        "description": "Skill ID or name (for transitions)"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["upstream", "downstream", "both"],
                        "description": "Transition direction (default: both)"
                    },
                    "from": {
                        "type": "string",
                        "description": "Starting skill (for path)"
                    },
                    "to": {
                        "type": "string",
                        "description": "Target skill (for path)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn name(&self) -> &str {
        "lookup_pipeline"
    }

    async fn execute(&self, arguments: &str) -> Result<String, AiError> {
        use crate::knowledge::pipeline_graph;
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| AiError::ToolError {
                tool: "lookup_pipeline".into(),
                message: format!("invalid arguments: {e}"),
            })?;
        let action = args["action"].as_str().unwrap_or("stats");
        let out = match action {
            "stats" => {
                let (n, e) = pipeline_graph::graph_stats();
                format!(
                    "Pipeline knowledge graph: {n} workflow skills, {e} literature-backed transitions."
                )
            }
            "transitions" => {
                let skill = args["skill"].as_str().ok_or_else(|| AiError::ToolError {
                    tool: "lookup_pipeline".into(),
                    message: "missing 'skill' argument".into(),
                })?;
                let direction = args["direction"].as_str().unwrap_or("both");
                pipeline_graph::format_transitions(skill, direction)
            }
            "path" => {
                let from = args["from"].as_str().ok_or_else(|| AiError::ToolError {
                    tool: "lookup_pipeline".into(),
                    message: "missing 'from' argument".into(),
                })?;
                let to = args["to"].as_str().ok_or_else(|| AiError::ToolError {
                    tool: "lookup_pipeline".into(),
                    message: "missing 'to' argument".into(),
                })?;
                pipeline_graph::format_path(from, to)
            }
            other => format!("Unknown action '{other}'. Use transitions, path, or stats."),
        };
        Ok(out)
    }
}

#[tokio::test]
async fn lookup_tool_handles_missing_query() {
    let tool = LookupTool::new();
    let result = tool.execute(r#"{}"#).await;
    assert!(result.is_err(), "missing query should error");
}

#[tokio::test]
async fn lookup_tool_unknown_query() {
    let tool = LookupTool::new();
    let result = tool
        .execute(r#"{"query": "zzzznonexistenttool"}"#)
        .await
        .unwrap();
    assert!(result.contains("No Bioconda"), "should report no matches");
}

#[tokio::test]
async fn lookup_skill_handles_missing_query() {
    let tool = LookupSkillTool::new();
    let result = tool.execute(r#"{}"#).await;
    assert!(result.is_err());
}

/// When the build embeds knowledge_meta.json (issue #153), lookup_tool
/// responses carry a freshness note: data date + record count.
#[tokio::test]
#[cfg(knowledge_meta_embedded)]
async fn lookup_tool_appends_freshness_line() {
    let tool = LookupTool::new();
    let result = tool.execute(r#"{"query": "bwa"}"#).await.unwrap();
    assert!(
        result.contains("Data: bioconda_tools generated"),
        "freshness line missing: {result}"
    );
    assert!(result.contains("records"), "record count missing: {result}");
}

#[tokio::test]
async fn lookup_pipeline_stats() {
    let tool = LookupPipelineTool::new();
    let result = tool.execute(r#"{"action": "stats"}"#).await.unwrap();
    assert!(result.contains("workflow skills"));
}

#[tokio::test]
async fn lookup_pipeline_path() {
    let tool = LookupPipelineTool::new();
    let result = tool
        .execute(r#"{"action": "path", "from": "wgs-alignment", "to": "variant-calling"}"#)
        .await
        .unwrap();
    assert!(
        result.contains("wgs-alignment"),
        "path should include start"
    );
}

#[tokio::test]
async fn lookup_pipeline_bad_action() {
    let tool = LookupPipelineTool::new();
    let result = tool.execute(r#"{"action": "bogus"}"#).await.unwrap();
    assert!(result.contains("Unknown action"));
}

/// Read-only registry of the embedded knowledge tools (bioconda lookup,
/// bioSkills, pipeline graph, SSRF-screened fetch). This is the tool
/// surface every embedded generation/check agent runs with; surfaces add
/// their own extras on top (CLI: file access + MCP, web chat: run
/// diagnostics). Read-only by construction, so it is safe to hand to an
/// agent without an approver.
pub fn knowledge_tool_registry() -> crate::tools::ToolRegistry {
    let mut registry = crate::tools::ToolRegistry::new();
    registry.register(Box::new(LookupTool::new()));
    registry.register(Box::new(LookupSkillTool::new()));
    registry.register(Box::new(LookupPipelineTool::new()));
    registry.register(Box::new(FetchUrlTool::new()));
    registry
}

#[cfg(test)]
mod knowledge_registry_tests {
    use super::*;

    #[test]
    fn registry_is_read_only_and_complete() {
        let registry = knowledge_tool_registry();
        assert_eq!(registry.len(), 4);
        for name in [
            "lookup_tool",
            "lookup_skill",
            "lookup_pipeline",
            "fetch_url",
        ] {
            assert!(registry.get(name).is_some(), "missing {name}");
            assert!(registry.is_read_only(name), "{name} must be read-only");
        }
    }
}

#[cfg(test)]
mod fetch_url_ssrf_tests {
    use super::*;

    #[test]
    fn forbidden_ip_classification() {
        for bad in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.4.4",
            "192.168.1.9",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::5",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(
                ip_is_forbidden(bad.parse().unwrap()),
                "{bad} must be forbidden"
            );
        }
        for ok in ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "172.32.0.1"] {
            assert!(
                !ip_is_forbidden(ok.parse().unwrap()),
                "{ok} must be allowed"
            );
        }
    }

    #[tokio::test]
    async fn literal_private_ip_urls_rejected_without_dns() {
        for url in [
            "http://127.0.0.1:3000/api/system",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:10.0.0.1]/x",
            "file:///etc/passwd",
            "http://catalog.internal/x",
        ] {
            assert!(
                validate_public_url(url).await.is_err(),
                "{url} must be rejected"
            );
        }
    }

    #[test]
    fn allowlist_parsing_normalizes_entries() {
        assert_eq!(
            parse_allowlist("Metadata.internal, example.com ,,"),
            vec!["metadata.internal".to_string(), "example.com".to_string()]
        );
        assert!(parse_allowlist("").is_empty());
    }

    #[test]
    fn capped_body_stops_at_cap_and_marks_truncation() {
        // The 15 s timeout bounds time, not size: without a byte cap a fast
        // endpoint can hand the model hundreds of megabytes.
        let mut body = CappedBody::new();
        assert!(body.push(b"hello "));
        assert!(body.push(b"world"));
        let oversized = vec![b'x'; MAX_FETCH_BYTES + 1024];
        assert!(!body.push(&oversized), "the cap must stop the read");
        let text = body.finish();
        let marker = format!("\n[... response truncated at {MAX_FETCH_BYTES} bytes ...]");
        assert_eq!(text.len(), MAX_FETCH_BYTES + marker.len());
        assert!(text.starts_with("hello world"));
        assert!(text.ends_with(&marker));

        // A body under the cap is returned verbatim, with no marker.
        let mut body = CappedBody::new();
        assert!(body.push(b"all of it"));
        assert_eq!(body.finish(), "all of it");
    }

    #[test]
    fn non_http_schemes_rejected() {
        // Synchronous part of validation surfaced without touching network.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = rt
            .block_on(validate_public_url("ftp://example.com/x"))
            .err()
            .unwrap();
        assert!(err.contains("scheme"), "got: {err}");
    }
}
