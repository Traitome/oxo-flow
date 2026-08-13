//! AI Runtime — wires all 5 layers together for command use.
//!
//! Central integration point connecting L1 (Provider), L2 (Knowledge/Config),
//! and L3 (Agent/Tools) for L4 (Commands). Ensures scope config resolution,
//! tool registration, and skill/MCP discovery happen once at initialization.

use std::path::Path;

use anyhow::Result;
use oxo_flow_ai::agent::orchestrator::Orchestrator;
use oxo_flow_ai::agent::{AgentContext, ExternalSource};
use oxo_flow_ai::config::AiConfig;
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::session::AiSession;
use oxo_flow_ai::tools::{ToolRegistry, builtin};

/// Fully-configured AI runtime for a command execution.
pub struct AiRuntime {
    pub provider: AiProvider,
    pub config: AiConfig,
    pub tool_registry: ToolRegistry,
    pub orchestrator: Orchestrator,
}

impl AiRuntime {
    /// Initialize AI runtime with full scope config resolution.
    ///
    /// Resolution chain: env vars → global config → project .oxo-flow/ai.toml →
    /// workflow [ai] section → CLI overrides.
    pub fn new(
        workflow_path: Option<&Path>,
        project_dir: Option<&Path>,
        cli_max_retries: Option<u32>,
    ) -> Result<Self> {
        let provider = super::ai_template::resolve_ai_provider()?;

        // Resolve config chain
        let global = load_global_config();
        let project = project_dir.and_then(AiConfig::from_project_file);
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

        let config = AiConfig::resolve_chain(
            global.as_ref(),
            project.as_ref(),
            workflow.as_ref(),
            None,
            cli_overrides.as_ref(),
        );

        // Register tools: builtins + discovered skills
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(builtin::ReadFileTool::new()));
        tool_registry.register(Box::new(builtin::FetchUrlTool::new()));
        tool_registry.register(Box::new(builtin::LookupTool::new()));
        tool_registry.register(Box::new(builtin::LookupSkillTool::new()));
        tool_registry.register(Box::new(builtin::LookupPipelineTool::new()));
        register_discovered_tools(&mut tool_registry);

        let orchestrator = Orchestrator::new(provider.clone(), config.max_retries);

        Ok(Self {
            provider,
            config,
            tool_registry,
            orchestrator,
        })
    }

    pub fn create_context(
        &self,
        command: &str,
        intent: &str,
        workflow_path: Option<&Path>,
        workflow_content: Option<&str>,
        external_sources: Vec<ExternalSource>,
    ) -> AgentContext {
        AgentContext {
            intent: intent.to_string(),
            command: command.to_string(),
            workflow_path: workflow_path.map(|p| p.to_path_buf()),
            workflow_content: workflow_content.map(|s| s.to_string()),
            external_sources,
            max_rounds: self.config.max_retries,
            tool_registry: self.tool_registry.clone(),
            session: AiSession::new(
                command,
                intent,
                self.provider.name(),
                &self.provider.model().unwrap_or_else(|| "default".into()),
            ),
        }
    }
}

fn load_global_config() -> Option<AiConfig> {
    let path = oxo_flow_ai::provider::ai_config_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let provider_str = json["provider"].as_str().unwrap_or("");
    let provider: oxo_flow_ai::provider::ProviderKind = provider_str.parse().ok()?;
    Some(AiConfig {
        enabled: true,
        provider,
        model: json["model"].as_str().map(String::from),
        api_url: json["api_url"].as_str().map(String::from),
        ..Default::default()
    })
}

fn register_discovered_tools(_registry: &mut ToolRegistry) {
    let skills = oxo_flow_ai::skill::discover_skills(None);
    for skill in &skills {
        if skill.skill_type == "tool" {
            tracing::info!("Discovered AI skill: {} v{}", skill.name, skill.version);
        }
    }
    if !skills.is_empty() {
        tracing::info!(
            "Registered {} builtin tools + {} discovered skills",
            2,
            skills.len()
        );
    }
}
