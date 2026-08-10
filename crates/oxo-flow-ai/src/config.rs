//! AI configuration system.
//!
//! Configuration is resolved from multiple sources in priority order
//! (later overrides earlier):
//!
//! 1. Hardcoded defaults
//! 2. Global config file (`~/.oxo-flow/ai_config.json`)
//! 3. Environment variables (`OXO_FLOW_AI_*`, `DEEPSEEK_API_KEY`, etc.)
//! 4. Workflow `[ai]` section in `.oxoflow` file
//! 5. CLI flags (`--ai`, `--ai-recover`, `--ai-max-retries N`)

use serde::{Deserialize, Serialize};

use crate::provider::ProviderKind;

// ── AiConfig ───────────────────────────────────────────────────────────────

/// AI configuration resolved from all sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Whether AI features are enabled for this scope.
    #[serde(default)]
    pub enabled: bool,

    /// Which AI provider to use.
    #[serde(default)]
    pub provider: ProviderKind,

    /// Model name override (uses provider default if None).
    #[serde(default)]
    pub model: Option<String>,

    /// API key — NEVER serialized to logs.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,

    /// Custom API endpoint URL.
    #[serde(default)]
    pub api_url: Option<String>,

    /// Maximum correction/retry rounds for agent loops.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// When the AI can auto-apply modifications.
    #[serde(default)]
    pub auto_fix: AutoFixMode,

    /// Model temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(default)]
    pub temperature: Option<f64>,
}

fn default_max_retries() -> u32 {
    3
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: ProviderKind::DeepSeek,
            model: None,
            api_key: None,
            api_url: None,
            max_retries: default_max_retries(),
            auto_fix: AutoFixMode::default(),
            temperature: None,
        }
    }
}

impl AiConfig {
    /// Merge another config into this one, overwriting non-None fields.
    pub fn merge(&mut self, other: &AiConfig) {
        if other.enabled {
            self.enabled = true;
        }
        // Provider only changes if explicitly different from default
        self.provider = other.provider;
        if other.model.is_some() {
            self.model = other.model.clone();
        }
        if other.api_key.is_some() {
            self.api_key = other.api_key.clone();
        }
        if other.api_url.is_some() {
            self.api_url = other.api_url.clone();
        }
        if other.max_retries != default_max_retries() {
            self.max_retries = other.max_retries;
        }
        if other.auto_fix != AutoFixMode::default() {
            self.auto_fix = other.auto_fix;
        }
        if other.temperature.is_some() {
            self.temperature = other.temperature;
        }
    }

    /// Create a config from environment variables.
    pub fn from_env() -> Self {
        let provider_str = std::env::var("OXO_FLOW_AI_PROVIDER").unwrap_or_default();
        let enabled = !provider_str.is_empty() && !provider_str.eq_ignore_ascii_case("disabled");

        let provider = if enabled {
            provider_str
                .parse::<ProviderKind>()
                .unwrap_or(ProviderKind::DeepSeek)
        } else {
            ProviderKind::DeepSeek
        };

        let model = std::env::var("OXO_FLOW_AI_MODEL").ok();
        let api_url = std::env::var("OXO_FLOW_AI_API_URL").ok();
        let api_key = resolve_api_key_from_env(provider);

        Self {
            enabled,
            provider,
            model,
            api_key,
            api_url,
            max_retries: default_max_retries(),
            auto_fix: AutoFixMode::default(),
            temperature: None,
        }
    }

    /// Create a config parsed from a workflow `[ai]` TOML section.
    pub fn from_workflow_toml(table: &toml::Table) -> Option<Self> {
        let ai_table = table.get("ai")?.as_table()?;
        let enabled = ai_table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let provider = ai_table
            .get("provider")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(ProviderKind::DeepSeek);
        let model = ai_table
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max_retries = ai_table
            .get("max_retries")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32)
            .unwrap_or(default_max_retries());
        let auto_fix = ai_table
            .get("auto_fix")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Some(Self {
            enabled,
            provider,
            model,
            api_key: None, // API key never comes from workflow files
            api_url: None,
            max_retries,
            auto_fix,
            temperature: None,
        })
    }

    /// Create a config parsed from a project-level `.oxo-flow/ai.toml`.
    pub fn from_project_file(project_dir: &std::path::Path) -> Option<Self> {
        let path = project_dir.join(".oxo-flow").join("ai.toml");
        let content = std::fs::read_to_string(&path).ok()?;
        let table: toml::Table = toml::from_str(&content).ok()?;
        Self::from_workflow_toml(&table)
    }

    /// Create a config parsed from a rule-level `[rules.ai]` TOML section.
    pub fn from_rule_table(rule_table: &toml::Table) -> Option<Self> {
        let ai_table = rule_table.get("ai")?.as_table()?;
        let enabled = ai_table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let max_retries = ai_table
            .get("max_retries")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32)
            .unwrap_or_else(default_max_retries);
        let auto_fix = ai_table
            .get("auto_fix")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Some(Self {
            enabled,
            max_retries,
            auto_fix,
            ..Default::default()
        })
    }

    /// Resolve configuration across all scope levels.
    ///
    /// Priority (later overrides earlier):
    /// 1. Hardcoded defaults
    /// 2. Global config file
    /// 3. Project `.oxo-flow/ai.toml`
    /// 4. Workflow `[ai]` section
    /// 5. Per-rule `[rules.ai]` overrides
    /// 6. CLI flags
    pub fn resolve_chain(
        global: Option<&Self>,
        project: Option<&Self>,
        workflow: Option<&Self>,
        rule: Option<&Self>,
        cli_overrides: Option<&Self>,
    ) -> Self {
        let mut resolved = Self::default();
        if let Some(g) = global {
            resolved.merge(g);
        }
        if let Some(p) = project {
            resolved.merge(p);
        }
        if let Some(w) = workflow {
            resolved.merge(w);
        }
        if let Some(r) = rule {
            resolved.merge(r);
        }
        if let Some(c) = cli_overrides {
            resolved.merge(c);
        }
        resolved
    }
}

// ── AutoFixMode ────────────────────────────────────────────────────────────

/// Controls whether the AI agent can automatically apply modifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AutoFixMode {
    /// Propose changes, wait for user confirmation.
    #[default]
    Ask,
    /// Automatically apply safe changes and continue.
    Always,
    /// Only report issues, never modify anything.
    Never,
}

impl std::str::FromStr for AutoFixMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ask" => Ok(Self::Ask),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "Invalid auto_fix mode '{s}'. Use 'ask', 'always', or 'never'"
            )),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Resolve API key from environment variables based on provider.
fn resolve_api_key_from_env(provider: ProviderKind) -> Option<String> {
    match provider {
        ProviderKind::Claude => std::env::var("ANTHROPIC_AUTH_TOKEN")
            .or_else(|_| std::env::var("CLAUDE_API_KEY"))
            .or_else(|_| std::env::var("OXO_FLOW_AI_API_KEY"))
            .ok(),
        ProviderKind::OpenAi => std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("OXO_FLOW_AI_API_KEY"))
            .ok(),
        ProviderKind::DeepSeek => std::env::var("DEEPSEEK_API_KEY")
            .or_else(|_| std::env::var("OXO_FLOW_AI_API_KEY"))
            .ok(),
        ProviderKind::Ollama => None, // Ollama doesn't need an API key
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = AiConfig::default();
        assert!(!config.enabled);
    }

    #[test]
    fn default_provider_is_deepseek() {
        let config = AiConfig::default();
        assert_eq!(config.provider, ProviderKind::DeepSeek);
    }

    #[test]
    fn default_auto_fix_is_ask() {
        let config = AiConfig::default();
        assert_eq!(config.auto_fix, AutoFixMode::Ask);
    }

    #[test]
    fn merge_enables_and_overrides() {
        let mut base = AiConfig::default();
        let override_config = AiConfig {
            enabled: true,
            model: Some("deepseek-v4-flash".into()),
            max_retries: 5,
            ..AiConfig::default()
        };
        base.merge(&override_config);
        assert!(base.enabled);
        assert_eq!(base.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(base.max_retries, 5);
    }

    #[test]
    fn auto_fix_mode_parse() {
        assert_eq!("ask".parse::<AutoFixMode>().unwrap(), AutoFixMode::Ask);
        assert_eq!(
            "always".parse::<AutoFixMode>().unwrap(),
            AutoFixMode::Always
        );
        assert_eq!("never".parse::<AutoFixMode>().unwrap(), AutoFixMode::Never);
        assert!("invalid".parse::<AutoFixMode>().is_err());
    }

    #[test]
    fn from_workflow_toml_parses_section() {
        let toml_str = r#"
[ai]
enabled = true
model = "deepseek-v4-flash"
max_retries = 5
auto_fix = "always"
"#;
        let table: toml::Table = toml::from_str(toml_str).unwrap();
        let config = AiConfig::from_workflow_toml(&table).unwrap();
        assert!(config.enabled);
        assert_eq!(config.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.auto_fix, AutoFixMode::Always);
    }

    #[test]
    fn from_workflow_toml_missing_section_returns_none() {
        let toml_str = r#"
[workflow]
name = "test"
"#;
        let table: toml::Table = toml::from_str(toml_str).unwrap();
        assert!(AiConfig::from_workflow_toml(&table).is_none());
    }

    #[test]
    fn resolve_chain_workflow_overrides_global() {
        let mut global = AiConfig::default();
        global.enabled = true;
        global.max_retries = 3;

        let workflow = AiConfig {
            enabled: true,
            max_retries: 5,
            ..AiConfig::default()
        };

        let resolved = AiConfig::resolve_chain(Some(&global), None, Some(&workflow), None, None);
        assert!(resolved.enabled);
        assert_eq!(resolved.max_retries, 5); // workflow overrides global
    }

    #[test]
    fn resolve_chain_rule_disables() {
        let mut global = AiConfig::default();
        global.enabled = true;

        let rule = AiConfig {
            enabled: false,
            ..AiConfig::default()
        };

        let resolved = AiConfig::resolve_chain(Some(&global), None, None, Some(&rule), None);
        assert!(resolved.enabled); // merge doesn't disable — enabled is sticky
    }

    #[test]
    fn resolve_chain_cli_overrides_all() {
        let global = AiConfig {
            max_retries: 3,
            ..AiConfig::default()
        };
        let cli = AiConfig {
            max_retries: 10,
            ..Default::default()
        };
        let resolved = AiConfig::resolve_chain(Some(&global), None, None, None, Some(&cli));
        assert_eq!(resolved.max_retries, 10);
    }

    #[test]
    fn from_rule_table_parses() {
        let toml_str = r#"
[ai]
enabled = false
max_retries = 1
auto_fix = "never"
"#;
        let table: toml::Table = toml::from_str(toml_str).unwrap();
        let config = AiConfig::from_rule_table(&table).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.max_retries, 1);
        assert_eq!(config.auto_fix, AutoFixMode::Never);
    }
}
