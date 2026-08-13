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

/// Fetch content from a URL.
#[derive(Default)]
pub struct FetchUrlTool {
    client: reqwest::Client,
}

impl FetchUrlTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
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

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| AiError::ToolError {
                tool: "fetch_url".into(),
                message: format!("request failed: {e}"),
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

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
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

        Ok(crate::knowledge::bioconda::format_search_results(
            query, limit,
        ))
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

/// Query the embedded bioinformatics pipeline knowledge graph (78 skills,
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
            description: "Query the embedded bioinformatics pipeline knowledge graph (78 workflow skills, 470 data-flow transitions with data types and literature evidence). Use 'transitions' to see what feeds into/out of a step, or 'path' to find the pipeline between two steps. Use this to design correct multi-step workflow topologies (e.g. from alignment to variant calling to annotation).".into(),
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
