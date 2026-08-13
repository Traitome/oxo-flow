//! Tool registry for the web chat agent.
//!
//! Read-only by design: the embedded knowledge lookups ground tool selection
//! in the Bioconda/bioSkills/pipeline-graph databases. No filesystem or
//! database WRITE tools are registered — saving a generated workflow is the
//! user's explicit Accept click, never a model action.

use oxo_flow_ai::tools::ToolRegistry;
use oxo_flow_ai::tools::builtin::{FetchUrlTool, LookupPipelineTool, LookupSkillTool, LookupTool};

/// Build the registry used by every chat request (cheap — in-memory).
pub fn build_chat_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(LookupTool::new()));
    registry.register(Box::new(LookupSkillTool::new()));
    registry.register(Box::new(LookupPipelineTool::new()));
    registry.register(Box::new(FetchUrlTool::new()));
    registry
}
