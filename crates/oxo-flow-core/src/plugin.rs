//! Plugin system traits for oxo-flow.
//!
//! Defines extension points for custom rule types, environment backends,
//! executors, and report renderers. Plugins implement these traits and
//! are dynamically loaded at runtime via libloading.

use crate::error::Result;
use crate::rule::Rule;
use std::collections::HashMap;

/// Version of the plugin API. Incremented on breaking changes.
pub const PLUGIN_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Rule Plugin
// ---------------------------------------------------------------------------

/// Trait for custom rule types (e.g., R functions, Python scripts).
///
/// Implementations define how to build shell commands, validate
/// configuration, and declare extra TOML fields for a rule type.
pub trait RulePlugin: Send + Sync {
    /// Unique type identifier (e.g., "r-function", "python-script").
    fn rule_type(&self) -> &str;

    /// Build the shell command to execute this rule.
    fn build_command(&self, rule: &Rule, values: &HashMap<String, String>) -> Result<String>;

    /// Validate the rule configuration for this type.
    fn validate(&self, rule: &Rule) -> Result<()>;

    /// Return extra TOML fields this rule type accepts (name, description).
    fn extra_fields(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Executor Plugin
// ---------------------------------------------------------------------------

/// Handle to a submitted job on an external executor.
#[derive(Debug, Clone)]
pub struct JobHandle {
    pub id: String,
    pub backend: String,
}

/// Trait for custom executors (e.g., AWS Batch, Kubernetes, GCP).
pub trait ExecutorPlugin: Send + Sync {
    /// Backend name (e.g., "aws-batch", "k8s").
    fn backend_name(&self) -> &str;

    /// Submit a rule for execution and return a job handle.
    fn submit(&self, rule: &Rule, workdir: &std::path::Path) -> Result<JobHandle>;

    /// Check the status of a submitted job.
    fn status(&self, handle: &JobHandle) -> Result<crate::executor::JobStatus>;

    /// Cancel a running job.
    fn cancel(&self, handle: &JobHandle) -> Result<()>;

    /// Retrieve logs for a completed job.
    fn logs(&self, handle: &JobHandle) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Report Plugin
// ---------------------------------------------------------------------------

/// Trait for custom report renderers (e.g., native PDF, DOCX, interactive HTML).
pub trait ReportPlugin: Send + Sync {
    /// Renderer name (e.g., "native-pdf", "docx", "interactive-html").
    fn renderer_name(&self) -> &str;

    /// Output format / file extension (e.g., "pdf", "docx").
    fn output_format(&self) -> &str;

    /// Render a report and return the output bytes.
    fn render(&self, report: &crate::report::Report) -> Result<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// Plugin Registry
// ---------------------------------------------------------------------------

/// Registry of loaded plugins.
#[derive(Default)]
pub struct PluginRegistry {
    pub rules: Vec<Box<dyn RulePlugin>>,
    pub executors: Vec<Box<dyn ExecutorPlugin>>,
    pub reports: Vec<Box<dyn ReportPlugin>>,
}

impl PluginRegistry {
    pub fn register_rule(&mut self, plugin: Box<dyn RulePlugin>) {
        self.rules.push(plugin);
    }

    pub fn register_executor(&mut self, plugin: Box<dyn ExecutorPlugin>) {
        self.executors.push(plugin);
    }

    pub fn register_report(&mut self, plugin: Box<dyn ReportPlugin>) {
        self.reports.push(plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub rule plugin for testing.
    struct StubRulePlugin;
    impl RulePlugin for StubRulePlugin {
        fn rule_type(&self) -> &str {
            "stub"
        }
        fn build_command(&self, _rule: &Rule, _values: &HashMap<String, String>) -> Result<String> {
            Ok("echo stub".into())
        }
        fn validate(&self, _rule: &Rule) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_accepts_plugins() {
        let mut registry = PluginRegistry::default();
        registry.register_rule(Box::new(StubRulePlugin));
        assert_eq!(registry.rules.len(), 1);
        assert_eq!(registry.rules[0].rule_type(), "stub");
    }

    #[test]
    fn plugin_api_version_is_stable() {
        assert_eq!(PLUGIN_API_VERSION, 1);
    }
}
