//! AI plugin system — extensible traits for custom AI capabilities.
//!
//! Three plugin types are supported, extending the existing oxo-flow
//! plugin architecture with AI-specific capabilities.
//!
//! ## Plugin Types
//!
//! - **AiToolPlugin**: Custom tools the AI agent can invoke during generation/analysis
//! - **AiKnowledgePlugin**: Custom knowledge sources for domain-specific context
//! - **AiValidatorPlugin**: Custom validation rules for AI-generated content

use async_trait::async_trait;

use crate::error::AiError;
use crate::types::ToolDef;

// ── AiToolPlugin ────────────────────────────────────────────────────────────

/// A custom tool the AI agent can call.
///
/// Unlike built-in tools (read_file, fetch_url, etc.) which are compiled into
/// the binary, tool plugins can be registered dynamically from configuration files.
///
/// # Example
///
/// ```rust,ignore
/// struct DatabaseQueryTool;
/// #[async_trait]
/// impl AiToolPlugin for DatabaseQueryTool {
///     fn tool_name(&self) -> &str { "db_query" }
///     fn tool_def(&self) -> ToolDef { /* ... */ }
///     async fn execute(&self, args: &str) -> Result<String, AiError> { /* ... */ }
/// }
/// ```
#[async_trait]
pub trait AiToolPlugin: Send + Sync {
    /// Unique tool name — must match the name in `.plugin.toml`.
    fn tool_name(&self) -> &str;

    /// Tool definition for the AI model's function-calling API.
    fn tool_def(&self) -> ToolDef;

    /// Execute the tool with JSON-encoded arguments.
    async fn execute(&self, arguments: &str) -> Result<String, AiError>;

    /// Whether the tool is safe to auto-execute without confirmation.
    fn is_read_only(&self) -> bool {
        true
    }
}

// ── AiKnowledgePlugin ───────────────────────────────────────────────────────

/// A custom knowledge source for domain-specific context.
///
/// Knowledge plugins provide contextual information to the AI agent, such as
/// institutional best practices, proprietary tool references, or regulatory
/// requirements.
///
/// # Example
///
/// ```rust,ignore
/// struct FdaComplianceKnowledge;
/// impl AiKnowledgePlugin for FdaComplianceKnowledge {
///     fn knowledge_name(&self) -> &str { "fda-compliance" }
///     fn domain(&self) -> &str { "clinical-genomics" }
///     fn provide_context(&self, topic: &str) -> Option<String> {
///         Some("FDA requires...".into())
///     }
/// }
/// ```
pub trait AiKnowledgePlugin: Send + Sync {
    /// Unique knowledge source name.
    fn knowledge_name(&self) -> &str;

    /// Domain this knowledge applies to (e.g., "clinical-genomics", "single-cell").
    fn domain(&self) -> &str;

    /// Provide context relevant to the given topic.
    /// Returns `None` if the topic is outside this plugin's domain.
    fn provide_context(&self, topic: &str) -> Option<String>;

    /// Provide a system prompt addition that's always injected when this plugin is active.
    fn system_prompt_addition(&self) -> Option<String> {
        None
    }
}

// ── AiValidatorPlugin ───────────────────────────────────────────────────────

/// A custom validation rule for AI-generated workflow content.
///
/// Validator plugins check AI output against domain-specific rules that go
/// beyond basic schema validation.
///
/// # Example
///
/// ```rust,ignore
/// struct FdaComplianceValidator;
/// impl AiValidatorPlugin for FdaComplianceValidator {
///     fn validator_name(&self) -> &str { "fda-compliance" }
///     fn validate(&self, toml: &str) -> Vec<ValidationIssue> {
///         vec![]
///     }
/// }
/// ```
pub trait AiValidatorPlugin: Send + Sync {
    /// Unique validator name.
    fn validator_name(&self) -> &str;

    /// Validate a workflow TOML and return any issues found.
    fn validate(&self, toml_content: &str) -> Vec<ValidationIssue>;
}

/// A validation issue found by an AI validator plugin.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub rule: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueSeverity {
    Error,
    Warning,
    Info,
}

// ── Plugin discovery (reserved interface) ──────────────────────────────────

/// Registry of loaded AI plugins.
#[derive(Default)]
pub struct AiPluginRegistry {
    pub tools: Vec<Box<dyn AiToolPlugin>>,
    pub knowledge: Vec<Box<dyn AiKnowledgePlugin>>,
    pub validators: Vec<Box<dyn AiValidatorPlugin>>,
}

impl AiPluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_knowledge(&mut self, plugin: Box<dyn AiKnowledgePlugin>) {
        self.knowledge.push(plugin);
    }

    pub fn register_validator(&mut self, plugin: Box<dyn AiValidatorPlugin>) {
        self.validators.push(plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestKnowledgePlugin;
    impl AiKnowledgePlugin for TestKnowledgePlugin {
        fn knowledge_name(&self) -> &str {
            "test-kb"
        }
        fn domain(&self) -> &str {
            "testing"
        }
        fn provide_context(&self, topic: &str) -> Option<String> {
            if topic.contains("test") {
                Some("Test context".into())
            } else {
                None
            }
        }
    }

    struct TestValidatorPlugin;
    impl AiValidatorPlugin for TestValidatorPlugin {
        fn validator_name(&self) -> &str {
            "test-validator"
        }
        fn validate(&self, toml: &str) -> Vec<ValidationIssue> {
            if toml.is_empty() {
                vec![ValidationIssue {
                    severity: IssueSeverity::Error,
                    rule: None,
                    message: "Empty TOML".into(),
                    suggestion: Some("Add content".into()),
                }]
            } else {
                vec![]
            }
        }
    }

    #[test]
    fn knowledge_plugin_provides_context() {
        let plugin = TestKnowledgePlugin;
        assert_eq!(plugin.knowledge_name(), "test-kb");
        assert!(plugin.provide_context("test something").is_some());
        assert!(plugin.provide_context("other").is_none());
    }

    #[test]
    fn validator_plugin_finds_issues() {
        let plugin = TestValidatorPlugin;
        let issues = plugin.validate("");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, IssueSeverity::Error);
    }

    #[test]
    fn validator_plugin_passes_valid_content() {
        let plugin = TestValidatorPlugin;
        let issues = plugin.validate("[workflow]\nname = \"test\"");
        assert!(issues.is_empty());
    }

    #[test]
    fn plugin_registry_registration() {
        let mut reg = AiPluginRegistry::new();
        reg.register_knowledge(Box::new(TestKnowledgePlugin));
        reg.register_validator(Box::new(TestValidatorPlugin));
        assert_eq!(reg.knowledge.len(), 1);
        assert_eq!(reg.validators.len(), 1);
    }
}
