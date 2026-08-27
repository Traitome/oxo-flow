//! Built-in tools available to all AI agents.
//!
//! These tools are compiled into the binary and always available.
//! Additional tools can be registered by plugins or MCP servers.

use async_trait::async_trait;

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
/// hostile names pointing into internal space are caught pre-request.
async fn validate_public_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("unparseable URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "scheme {:?} not allowed (http/https only)",
            url.scheme()
        ));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if allowlist().contains(&host) {
        return Ok(url);
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
            Ok(url)
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
    Ok(url)
}

/// GET with manual redirects (max 5 hops), re-running the SSRF screen on
/// every Location target.
async fn validated_get(client: &reqwest::Client, raw: &str) -> Result<reqwest::Response, String> {
    let mut current = validate_public_url(raw).await?;
    for _hop in 0..=5 {
        let resp = client
            .get(current.clone())
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
                .join(loc)
                .map_err(|e| format!("bad redirect target: {e}"))?;
            current = validate_public_url(next.as_str()).await?;
            continue;
        }
        return Ok(resp);
    }
    Err("too many redirects (>5)".into())
}

/// Fetch content from a URL.
#[derive(Default)]
pub struct FetchUrlTool {
    client: reqwest::Client,
}

impl FetchUrlTool {
    pub fn new() -> Self {
        Self {
            // A browser-like User-Agent: some sources (Bioconductor, GitHub
            // raw, docs sites) 403 the default reqwest UA, which sent the
            // model into retry loops during pipeline generation (issue #79
            // P1-10). Timeouts bound hanging fetches inside the agent loop.
            client: reqwest::Client::builder()
                .user_agent(format!(
                    "oxo-flow/{} (+https://github.com/Traitome/oxo-flow)",
                    env!("CARGO_PKG_VERSION")
                ))
                .timeout(std::time::Duration::from_secs(15))
                // Redirects are followed manually (see `validated_get`) so
                // every hop re-runs the SSRF screen.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl Tool for FetchUrlTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "fetch_url".into(),
            description: "Fetch content from a URL. Use this to retrieve protocol documentation, tool references, or other web resources. Returns the text content of the page.".into(),
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

        let text = response.text().await.map_err(|e| AiError::ToolError {
            tool: "fetch_url".into(),
            message: format!("read response failed: {e}"),
        })?;

        Ok(text)
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
        let mut backed_up_to = None;
        if path.exists() {
            let backup = std::path::PathBuf::from(format!(
                "{}.bak.{}",
                path.display(),
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            ));
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
}

/// Look up tools in the embedded Bioconda CLI database (6103 tools).
///
/// Query by exact name, name prefix/substring, or summary keyword.
/// Returns real tool names, current Bioconda versions, descriptions,
/// and supported platforms.
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
            description: "Search the embedded Bioconda CLI database (6103 tools) for bioinformatics tools. \
                          Query by tool name, name fragment, or purpose keyword (e.g. 'star', 'align', 'variant calling'). \
                          Returns tool names, current Bioconda versions, descriptions, and platform support. \
                          Use this to pick the right tool and pin its current version instead of guessing.".into(),
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

/// Query the embedded bioinformatics pipeline knowledge graph (79 skills,
/// 470 literature-backed transitions). Understand what feeds into or out
/// of a workflow step, or find the pipeline path between two steps.
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
        ToolDef {
            name: "lookup_pipeline".into(),
            description: "Query the embedded bioinformatics pipeline knowledge graph (79 workflow skills, 469 data-flow transitions with data types and literature evidence). Use 'transitions' to see what feeds into/out of a step, or 'path' to find the pipeline between two steps. Use this to design correct multi-step workflow topologies (e.g. from alignment to variant calling to annotation).".into(),
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
