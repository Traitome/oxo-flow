//! Workflow configuration and `.oxoflow` file parsing.
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use crate::error::{OxoFlowError, Result};
use crate::rule::{EnvironmentSpec, Rule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Top-level workflow metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMeta {
    /// Workflow name.
    pub name: String,

    /// Semantic version.
    #[serde(default = "default_version")]
    pub version: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,

    /// Author name or organization.
    #[serde(default)]
    pub author: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Default settings applied to all rules unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default thread count.
    #[serde(default)]
    pub threads: Option<u32>,

    /// Default memory.
    #[serde(default)]
    pub memory: Option<String>,

    /// Default environment.
    #[serde(default)]
    pub environment: Option<EnvironmentSpec>,
}

/// Report configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReportConfig {
    /// Report template name.
    #[serde(default)]
    pub template: Option<String>,

    /// Output formats (html, pdf, json).
    #[serde(default)]
    pub format: Vec<String>,

    /// Report sections to include.
    #[serde(default)]
    pub sections: Vec<String>,
}

/// Complete workflow configuration parsed from an `.oxoflow` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// Workflow metadata.
    pub workflow: WorkflowMeta,

    /// Configuration variables (user-defined key-value pairs).
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,

    /// Default settings for all rules.
    #[serde(default)]
    pub defaults: Defaults,

    /// Report configuration.
    #[serde(default)]
    pub report: Option<ReportConfig>,

    /// List of rules (pipeline steps).
    #[serde(default, rename = "rules")]
    pub rules: Vec<Rule>,
}

impl WorkflowConfig {
    /// Parse a workflow configuration from a TOML string.
    pub fn parse(content: &str) -> Result<Self> {
        let config: WorkflowConfig = toml::from_str(content)?;
        config.validate()?;
        Ok(config)
    }

    /// Parse a workflow configuration from a `.oxoflow` file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let config: WorkflowConfig = toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the workflow configuration for internal consistency.
    pub fn validate(&self) -> Result<()> {
        // Check for duplicate rule names
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            if !seen.insert(&rule.name) {
                return Err(OxoFlowError::DuplicateRule {
                    name: rule.name.clone(),
                });
            }
        }

        // Ensure each rule has either shell or script
        for rule in &self.rules {
            if rule.shell.is_none() && rule.script.is_none() && !rule.output.is_empty() {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "rule '{}' has outputs but no shell command or script",
                        rule.name
                    ),
                });
            }
        }

        Ok(())
    }

    /// Get a rule by name.
    pub fn get_rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Get a config value by key.
    pub fn get_config_value(&self, key: &str) -> Option<&toml::Value> {
        self.config.get(key)
    }

    /// Returns the list of all rule names.
    pub fn rule_names(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_WORKFLOW: &str = r#"
        [workflow]
        name = "test-pipeline"
        version = "0.1.0"
    "#;

    const FULL_WORKFLOW: &str = r#"
        [workflow]
        name = "test-pipeline"
        version = "1.0.0"
        description = "A test pipeline"
        author = "Test"

        [config]
        reference = "/path/to/ref.fa"
        samples = "samples.csv"

        [defaults]
        threads = 4
        memory = "8G"

        [[rules]]
        name = "fastqc"
        input = ["{sample}_R1.fastq.gz"]
        output = ["qc/{sample}_fastqc.html"]
        threads = 2
        shell = "fastqc {input} -o qc/"

        [rules.environment]
        conda = "envs/qc.yaml"

        [[rules]]
        name = "align"
        input = ["{sample}_R1.fastq.gz"]
        output = ["{sample}.bam"]
        threads = 16
        memory = "32G"
        shell = "bwa mem {config.reference} {input} | samtools sort -o {output}"

        [rules.environment]
        docker = "biocontainers/bwa:0.7.17"
    "#;

    #[test]
    fn parse_minimal_workflow() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert_eq!(config.workflow.name, "test-pipeline");
        assert_eq!(config.workflow.version, "0.1.0");
        assert!(config.rules.is_empty());
    }

    #[test]
    fn parse_full_workflow() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert_eq!(config.workflow.name, "test-pipeline");
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].name, "fastqc");
        assert_eq!(config.rules[1].name, "align");
        assert_eq!(config.rules[0].environment.kind(), "conda");
        assert_eq!(config.rules[1].environment.kind(), "docker");
    }

    #[test]
    fn config_values() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert!(config.get_config_value("reference").is_some());
        assert!(config.get_config_value("nonexistent").is_none());
    }

    #[test]
    fn duplicate_rule_names() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"

            [[rules]]
            name = "step1"
            output = ["out2.txt"]
            shell = "echo world"
        "#;

        let result = WorkflowConfig::parse(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate rule name"));
    }

    #[test]
    fn rule_names_list() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        let names = config.rule_names();
        assert_eq!(names, vec!["fastqc", "align"]);
    }

    #[test]
    fn get_rule_by_name() {
        let config = WorkflowConfig::parse(FULL_WORKFLOW).unwrap();
        assert!(config.get_rule("fastqc").is_some());
        assert!(config.get_rule("nonexistent").is_none());
    }
}
