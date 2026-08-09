//! Context assembler — builds scenario-specific prompts from knowledge.

use crate::agent::AgentContext;
use crate::knowledge::builtin;

/// Assembled context for injection into an agent's system prompt.
pub struct AssembledContext {
    pub system_additions: Vec<String>,
    pub user_additions: Vec<String>,
}

impl AssembledContext {
    pub fn new() -> Self {
        Self {
            system_additions: Vec::new(),
            user_additions: Vec::new(),
        }
    }

    /// Merge all additions into a single string for system prompt injection.
    pub fn system_section(&self) -> String {
        self.system_additions.join("\n\n")
    }

    /// Merge all additions into a single string for user prompt injection.
    pub fn user_section(&self) -> String {
        self.user_additions.join("\n\n")
    }
}

impl Default for AssembledContext {
    fn default() -> Self {
        Self::new()
    }
}

// ── Scenario assemblers ────────────────────────────────────────────────────

/// Assemble context for the template generation scenario.
pub fn for_generate(ctx: &AgentContext) -> AssembledContext {
    let mut ac = AssembledContext::new();

    // Always include the tool reference table
    ac.system_additions.push(format!(
        "## Bioinformatics Tool Reference\n\n{}",
        builtin::format_tool_table()
    ));

    // Include best practices
    ac.system_additions.push(builtin::format_best_practices());

    // Include external sources in the user section
    for src in &ctx.external_sources {
        ac.user_additions.push(src.to_prompt_section());
    }

    ac
}

/// Assemble context for the dry-run check scenario.
pub fn for_check(ctx: &AgentContext) -> AssembledContext {
    let mut ac = AssembledContext::new();

    // Tool reference for matching allocations
    ac.system_additions.push(format!(
        "## Tool Reference (for resource audit)\n\n{}",
        builtin::format_tool_table()
    ));

    // Best practices as audit checklist
    ac.system_additions.push(format!(
        "## Audit Checklist\n\n{}",
        builtin::format_best_practices()
    ));

    // Workflow content for audit
    if let Some(ref content) = ctx.workflow_content {
        ac.user_additions
            .push(format!("## Workflow to Audit\n\n```toml\n{content}\n```"));
    }

    ac
}

/// Assemble context for the error diagnosis scenario.
pub fn for_diagnose(ctx: &AgentContext) -> AssembledContext {
    let mut ac = AssembledContext::new();

    // Error pattern table
    ac.system_additions.push(format!(
        "## Known Error Patterns\n\n{}",
        builtin::format_error_patterns()
    ));

    // Workflow context
    if let Some(ref content) = ctx.workflow_content {
        ac.user_additions.push(format!(
            "## Workflow Configuration\n\n```toml\n{content}\n```"
        ));
    }

    ac
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentContext, ExternalSource};
    use crate::session::AiSession;
    use crate::tools::ToolRegistry;

    fn test_ctx() -> AgentContext {
        AgentContext {
            intent: "RNA-seq".into(),
            command: "template".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            max_rounds: 3,
            tool_registry: ToolRegistry::new(),
            session: AiSession::new("test", "test", "noop", "none"),
        }
    }

    #[test]
    fn generate_context_includes_tool_table() {
        let ctx = test_ctx();
        let ac = for_generate(&ctx);
        let system = ac.system_section();
        assert!(system.contains("STAR"));
        assert!(system.contains("fastp"));
        assert!(system.contains("Best Practices"));
    }

    #[test]
    fn check_context_includes_best_practices() {
        let ctx = test_ctx();
        let ac = for_check(&ctx);
        let system = ac.system_section();
        assert!(system.contains("Audit Checklist"));
        assert!(system.contains("ERROR"));
    }

    #[test]
    fn diagnose_context_includes_error_patterns() {
        let ctx = test_ctx();
        let ac = for_diagnose(&ctx);
        let system = ac.system_section();
        assert!(system.contains("exit code 137"));
        assert!(system.contains("Error Patterns"));
    }

    #[test]
    fn generate_with_external_sources() {
        let mut ctx = test_ctx();
        ctx.external_sources.push(ExternalSource::Url {
            url: "https://example.com".into(),
            content: "Use fastp for QC".into(),
        });
        let ac = for_generate(&ctx);
        let user = ac.user_section();
        assert!(user.contains("example.com"));
        assert!(user.contains("fastp"));
    }
}
