//! AI Runtime — wires all 5 layers together for command use.
//!
//! Central integration point connecting L1 (Provider), L2 (Knowledge/Config),
//! and L3 (Agent/Tools) for L4 (Commands). Ensures scope config resolution,
//! tool registration, and skill/MCP discovery happen once at initialization.

use std::path::Path;

use anyhow::Result;
use colored::Colorize as _;
use oxo_flow_ai::agent::orchestrator::Orchestrator;
use oxo_flow_ai::config::AiConfig;
use oxo_flow_ai::provider::{AiDegradedReason, AiProvider};
use oxo_flow_ai::tools::{ToolRegistry, builtin};

/// An AI scope resolved from the documented config chain: the provider
/// built from the resolved provider/model, plus the chain itself (skills,
/// retries, team profile, auto_fix).
pub struct ResolvedAiScope {
    pub provider: AiProvider,
    pub config: AiConfig,
}

impl ResolvedAiScope {
    /// Structured reason why an enabled scope still degraded to the no-op
    /// provider (RB-14 / AI-09). `None` when the provider is usable.
    pub fn degraded_reason(&self) -> Option<AiDegradedReason> {
        if self.provider.is_usable() {
            return None;
        }
        // `provider_explicit` is set by the tier that selected a provider;
        // the env tier is merged into the chain, so this covers both. When
        // no tier selected one, the reason helper inspects
        // `OXO_FLOW_AI_PROVIDER` (invalid value vs nothing configured).
        let selected = self
            .config
            .provider_explicit
            .then_some(self.config.provider);
        Some(oxo_flow_ai::provider::degraded_reason(selected))
    }
}

/// Resolve the AI config chain for a workflow/project and build the
/// provider FROM THE RESOLVED CONFIG (issue #1122 / RB-14).
///
/// Documented precedence (later overrides earlier): defaults → global
/// config file → environment → project `.oxo-flow/ai.toml` → workflow
/// `[ai]` → CLI overrides. `OXO_FLOW_AI_PROVIDER=disabled` remains a hard
/// pin. When no tier explicitly selects a provider, the provider still
/// falls through the canonical env/saved/auto-detection chain, so
/// zero-config setups keep working.
///
/// Cheap by design (no tools, MCP, or skill loading) so the analysis
/// surfaces can share the same resolution as [`AiRuntime`].
pub fn resolve_ai_scope(
    workflow_path: Option<&Path>,
    project_dir: Option<&Path>,
    cli_max_retries: Option<u32>,
) -> ResolvedAiScope {
    let global = AiConfig::from_global_file();
    let env_layer = AiConfig::from_env();
    let project_root = project_dir.or_else(|| workflow_path.and_then(|p| p.parent()));
    let project = project_root.and_then(AiConfig::from_project_file);
    let workflow = workflow_path.and_then(|p| {
        std::fs::read_to_string(p).ok().and_then(|c| {
            c.parse::<toml::Table>()
                .ok()
                .and_then(|t| AiConfig::from_workflow_toml(&t))
        })
    });
    let cli_overrides = cli_max_retries.map(|n| AiConfig {
        max_retries: n,
        ..Default::default()
    });

    let config = AiConfig::resolve_chain_with_env(
        global.as_ref(),
        Some(&env_layer),
        project.as_ref(),
        workflow.as_ref(),
        None,
        cli_overrides.as_ref(),
    );
    // The provider is built AFTER the chain resolves — the RB-14 defect was
    // building it from the environment before `[ai] provider/model` was
    // known.
    let provider =
        oxo_flow_ai::provider::create_provider_from_config_chain(&config, global.as_ref());
    ResolvedAiScope { provider, config }
}

/// Fully-configured AI runtime for a command execution.
pub struct AiRuntime {
    pub provider: AiProvider,
    pub config: AiConfig,
    pub tool_registry: ToolRegistry,
    pub orchestrator: Orchestrator,
    /// Prompt context from activated user-defined skills.
    pub skill_context: String,
}

impl AiRuntime {
    /// Initialize AI runtime with full scope config resolution.
    pub async fn new(
        workflow_path: Option<&Path>,
        project_dir: Option<&Path>,
        cli_max_retries: Option<u32>,
    ) -> Result<Self> {
        let scope = resolve_ai_scope(workflow_path, project_dir, cli_max_retries);
        if !scope.provider.is_usable() {
            let reason = scope
                .degraded_reason()
                .unwrap_or_else(|| oxo_flow_ai::provider::degraded_reason(None));
            return Err(anyhow::Error::new(reason));
        }
        let ResolvedAiScope { provider, config } = scope;

        // User-defined skills: discovery (read-only) + explicit activation.
        let project = project_dir.or_else(|| workflow_path.and_then(|p| p.parent()));

        // Register tools: builtins + discovered skills. read_file is scoped
        // to the project directory (#518): the auto-approved read must not
        // reach ~/.ssh, ~/.oxo-flow/ai_config.json, or any other path
        // outside the workspace. Without a project directory the tool
        // registers unscoped — which is deliberately NOT read-only, so a
        // session without a human approver never executes it.
        let mut tool_registry = ToolRegistry::new();
        match project {
            Some(dir) => tool_registry.register(Box::new(builtin::ReadFileTool::scoped(dir))),
            None => {
                tracing::warn!(
                    "no project directory — read_file registered unscoped and requires approval"
                );
                tool_registry.register(Box::new(builtin::ReadFileTool::new()));
            }
        }
        // Knowledge tools come from the single canonical registry
        // (oxo-flow-ai); CLI extras (read_file, MCP bridges) sit on top.
        tool_registry.merge(builtin::knowledge_tool_registry());

        let skill_context = activated_skill_context(project, &config);

        // Activated tool skills reference MCP servers via
        // `requires = ["mcp://host:port"]` — connect and register their
        // tools through the MCP bridge. A failing server warns and is
        // skipped; it never blocks the run.
        let discovered = oxo_flow_ai::skill::discover_skills(project);
        let mut mcp_tools = 0usize;
        for skill in discovered {
            if skill.skill_type != "tool" || !config.skills.iter().any(|name| name == &skill.name) {
                continue;
            }
            let Some(url) = skill
                .requires
                .as_ref()
                .and_then(|r| r.iter().find(|s| s.starts_with("mcp://")))
            else {
                tracing::warn!(
                    "tool skill '{}' has no mcp:// requirement — skipped",
                    skill.name
                );
                continue;
            };
            // McpHttpClient::new screens the endpoint with a blocking
            // getaddrinfo hop — run it on the blocking pool so skill
            // loading never stalls an async worker on DNS (#268-family
            // discipline; the redirect loop already screens asynchronously).
            let screen_url = url.clone();
            match tokio::task::spawn_blocking(move || {
                oxo_flow_ai::mcp::McpHttpClient::new(&screen_url)
            })
            .await
            {
                Ok(Ok(client)) => {
                    match oxo_flow_ai::mcp::McpToolBridge::discover(std::sync::Arc::new(client))
                        .await
                    {
                        Ok(bridges) => {
                            let count = bridges.len();
                            for bridge in bridges {
                                tool_registry.register(Box::new(bridge));
                            }
                            mcp_tools += count;
                            tracing::info!(
                                "skill '{}': registered {} from {url}",
                                skill.name,
                                oxo_flow_core::plural(count, "MCP tool", "MCP tools")
                            );
                        }
                        Err(e) => tracing::warn!(
                            "skill '{}': failed to discover MCP tools at {url}: {e}",
                            skill.name
                        ),
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("skill '{}': MCP endpoint {url} rejected: {e}", skill.name)
                }
                Err(e) => tracing::warn!("skill '{}': {e}", skill.name),
            }
        }
        if mcp_tools > 0 {
            eprintln!(
                "  {} {} from activated tool skills (non-read-only calls require approval)",
                "•".cyan(),
                oxo_flow_core::plural(mcp_tools, "MCP tool", "MCP tools")
            );
        }

        let orchestrator = Orchestrator::new(provider.clone(), config.max_retries);

        Ok(Self {
            provider,
            config,
            tool_registry,
            orchestrator,
            skill_context,
        })
    }
}

/// Discover user-defined skills (home + project level) and return the
/// assembled prompt context of those explicitly activated in the config
/// (`[ai] skills = [...]`). Discovery is read-only — a skill is never
/// activated unless its name appears in the config. This is the trust
/// boundary: prompt injection only, zero code execution.
pub fn activated_skill_context(project_dir: Option<&Path>, config: &AiConfig) -> String {
    let discovered = oxo_flow_ai::skill::discover_skills(project_dir);
    let mut registry = oxo_flow_ai::skill::SkillRegistry::new();
    for skill in discovered {
        if config.skills.iter().any(|name| name == &skill.name) {
            tracing::info!("Activated custom skill: {} v{}", skill.name, skill.version);
            registry.activate(skill);
        }
    }
    registry.prompt_context().to_string()
}

/// Interactive human approval for a non-read-only tool call, in the sync
/// form the shared
/// [`ToolApprover`](oxo_flow_ai::agent::ToolApprover) closure requires.
/// Prompts on stderr
/// and reads stdin; non-interactive sessions are denied (the safe
/// default — matching the trust boundary: AI never executes autonomously).
pub fn prompt_tool_approval_blocking(tool_name: &str, arguments: &str) -> bool {
    use std::io::{IsTerminal, Write as _};

    let can_prompt = std::io::stderr().is_terminal() && std::io::stdin().is_terminal();
    if !can_prompt {
        return false;
    }
    eprintln!();
    eprintln!("  {} tool '{}' is not read-only:", "⚠".yellow(), tool_name);
    eprintln!("  Arguments: {arguments}");
    eprint!("  Allow execution? [y/N] ");
    std::io::stderr().flush().ok();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let trimmed = input.trim();
        trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard: the CLI runtime must register at least the canonical
    /// embedded-knowledge tool set from oxo-flow-ai (single source of
    /// truth). CLI extras (scoped read_file, MCP bridges) come on top.
    #[test]
    fn cli_registry_covers_knowledge_tools() {
        let base = oxo_flow_ai::tools::builtin::knowledge_tool_registry();
        let names = base.names();
        let mut merged = ToolRegistry::new();
        merged.merge(base);
        for name in names {
            assert!(
                merged.get(&name).is_some(),
                "CLI tool registry lost knowledge tool '{name}' — re-sync with \
                 oxo_flow_ai::tools::builtin::knowledge_tool_registry"
            );
        }
    }
}
