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

/// Include directive for modular workflow composition.
///
/// Allows importing rules from another `.oxoflow` file into the
/// current workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IncludeDirective {
    /// Path to the included `.oxoflow` file.
    pub path: String,

    /// Optional namespace prefix for included rule names.
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Execution mode for an execution group.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    /// Rules in the group execute one after another.
    Sequential,
    /// Rules in the group execute concurrently.
    #[default]
    Parallel,
}

/// Execution group for explicit rule ordering.
///
/// Groups a set of rules under a named block with a specified execution
/// mode (sequential or parallel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionGroup {
    /// Group name.
    pub name: String,

    /// Rules in this group (by name).
    #[serde(default)]
    pub rules: Vec<String>,

    /// Execution mode.
    #[serde(default)]
    pub mode: ExecutionMode,
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

    /// Include directives for importing rules from other workflow files.
    #[serde(default, rename = "include")]
    pub includes: Vec<IncludeDirective>,

    /// Explicit execution groups for sequential/parallel rule ordering.
    #[serde(default, rename = "execution_group")]
    pub execution_groups: Vec<ExecutionGroup>,
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

        self.validate_execution_groups()?;

        Ok(())
    }

    /// Resolve include directives by loading and merging rules from included files.
    /// Rules from included files are optionally prefixed with the namespace.
    pub fn resolve_includes(&mut self, base_dir: &Path) -> Result<()> {
        let includes = std::mem::take(&mut self.includes);
        for inc in &includes {
            let inc_path = base_dir.join(&inc.path);
            let content = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                path: inc_path.clone(),
                message: format!("failed to read include '{}': {}", inc.path, e),
            })?;
            let inc_config: WorkflowConfig =
                toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: e.to_string(),
                })?;
            for mut rule in inc_config.rules {
                if let Some(ref ns) = inc.namespace {
                    rule.name = format!("{}::{}", ns, rule.name);
                }
                if !self.rules.iter().any(|r| r.name == rule.name) {
                    self.rules.push(rule);
                }
            }
        }
        self.includes = includes;
        Ok(())
    }

    /// Validate that all execution group references point to existing rules.
    pub fn validate_execution_groups(&self) -> Result<()> {
        let rule_names: std::collections::HashSet<&str> =
            self.rules.iter().map(|r| r.name.as_str()).collect();
        for group in &self.execution_groups {
            for rule_ref in &group.rules {
                if !rule_names.contains(rule_ref.as_str()) {
                    return Err(OxoFlowError::Config {
                        message: format!(
                            "execution group '{}' references unknown rule '{}'",
                            group.name, rule_ref
                        ),
                    });
                }
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

    /// Apply global defaults to all rules that don't have explicit overrides.
    pub fn apply_defaults(&mut self) {
        for rule in &mut self.rules {
            // Apply default threads if rule doesn't specify one
            if rule.threads.is_none() {
                rule.threads = self.defaults.threads;
            }
            // Apply default memory if rule doesn't specify one
            if rule.memory.is_none()
                && let Some(ref mem) = self.defaults.memory
            {
                rule.memory = Some(mem.clone());
            }
            // Apply default environment if rule doesn't specify one
            if rule.environment.is_empty()
                && let Some(ref env) = self.defaults.environment
            {
                rule.environment = env.clone();
            }
        }
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

    #[test]
    fn apply_defaults_propagates() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 8
            memory = "16G"

            [defaults.environment]
            conda = "envs/default.yaml"

            [[rules]]
            name = "step1"
            shell = "echo hello"

            [[rules]]
            name = "step2"
            threads = 2
            memory = "4G"
            shell = "echo world"

            [rules.environment]
            docker = "ubuntu:latest"
        "#;

        let mut config = WorkflowConfig::parse(toml_str).unwrap();
        config.apply_defaults();

        // step1 should get defaults
        let step1 = config.get_rule("step1").unwrap();
        assert_eq!(step1.threads, Some(8));
        assert_eq!(step1.memory.as_deref(), Some("16G"));
        assert_eq!(step1.environment.kind(), "conda");

        // step2 already has overrides, should keep them
        let step2 = config.get_rule("step2").unwrap();
        assert_eq!(step2.threads, Some(2));
        assert_eq!(step2.memory.as_deref(), Some("4G"));
        assert_eq!(step2.environment.kind(), "docker");
    }

    #[test]
    fn parse_include_directives() {
        let toml_str = r#"
            [workflow]
            name = "modular"

            [[include]]
            path = "common/qc.oxoflow"
            namespace = "qc"

            [[include]]
            path = "align.oxoflow"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.includes.len(), 2);
        assert_eq!(config.includes[0].path, "common/qc.oxoflow");
        assert_eq!(config.includes[0].namespace.as_deref(), Some("qc"));
        assert_eq!(config.includes[1].path, "align.oxoflow");
        assert!(config.includes[1].namespace.is_none());
    }

    #[test]
    fn parse_execution_groups() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[execution_group]]
            name = "preprocessing"
            rules = ["fastp", "fastqc"]
            mode = "parallel"

            [[execution_group]]
            name = "alignment"
            rules = ["bwa", "sort", "index"]
            mode = "sequential"

            [[rules]]
            name = "fastp"
            shell = "fastp"

            [[rules]]
            name = "fastqc"
            shell = "fastqc"

            [[rules]]
            name = "bwa"
            shell = "bwa"

            [[rules]]
            name = "sort"
            shell = "sort"

            [[rules]]
            name = "index"
            shell = "index"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.execution_groups.len(), 2);
        assert_eq!(config.execution_groups[0].name, "preprocessing");
        assert_eq!(config.execution_groups[0].mode, ExecutionMode::Parallel);
        assert_eq!(config.execution_groups[0].rules.len(), 2);
        assert_eq!(config.execution_groups[1].name, "alignment");
        assert_eq!(config.execution_groups[1].mode, ExecutionMode::Sequential);
        assert_eq!(config.execution_groups[1].rules.len(), 3);
    }

    #[test]
    fn include_directive_deserialization() {
        let toml_str = r#"
            path = "sub/workflow.oxoflow"
            namespace = "sub"
        "#;

        let inc: IncludeDirective = toml::from_str(toml_str).unwrap();
        assert_eq!(inc.path, "sub/workflow.oxoflow");
        assert_eq!(inc.namespace.as_deref(), Some("sub"));
    }

    #[test]
    fn execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Parallel);
    }

    #[test]
    fn workflow_with_advanced_rule_features() {
        let toml_str = r#"
            [workflow]
            name = "advanced"

            [[rules]]
            name = "scattered_call"
            input = ["{sample}.bam"]
            output = ["{sample}.vcf"]
            shell = "call {input} > {output}"
            when = "config.run_calling"
            retries = 2
            temp_output = ["{sample}.tmp"]
            protected_output = ["{sample}.vcf"]

            [rules.scatter]
            variable = "sample"
            values = ["S1", "S2"]
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        let rule = &config.rules[0];
        assert_eq!(rule.when.as_deref(), Some("config.run_calling"));
        assert_eq!(rule.retries, 2);
        assert_eq!(rule.temp_output, vec!["{sample}.tmp"]);
        assert_eq!(rule.protected_output, vec!["{sample}.vcf"]);
        let scatter = rule.scatter.as_ref().unwrap();
        assert_eq!(scatter.variable, "sample");
        assert_eq!(scatter.values, vec!["S1", "S2"]);
    }

    #[test]
    fn resolve_includes_with_namespace() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "qc_step"
            shell = "fastqc"

            [[rules]]
            name = "trim_step"
            shell = "fastp"
        "#;
        let inc_path = dir.path().join("qc.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "qc.oxoflow"
            namespace = "qc"

            [[rules]]
            name = "align"
            shell = "bwa"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 3);
        assert_eq!(config.rules[0].name, "align");
        assert_eq!(config.rules[1].name, "qc::qc_step");
        assert_eq!(config.rules[2].name, "qc::trim_step");
    }

    #[test]
    fn resolve_includes_without_namespace() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "helper"
            shell = "echo help"
        "#;
        let inc_path = dir.path().join("helper.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "helper.oxoflow"

            [[rules]]
            name = "main_step"
            shell = "echo main"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[1].name, "helper");
    }

    #[test]
    fn resolve_includes_skips_duplicate_rules() {
        let dir = tempfile::tempdir().unwrap();

        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "shared_step"
            shell = "echo included"
        "#;
        let inc_path = dir.path().join("inc.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "inc.oxoflow"

            [[rules]]
            name = "shared_step"
            shell = "echo main"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        // Should NOT add duplicate
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].shell.as_deref(), Some("echo main"));
    }

    #[test]
    fn resolve_includes_missing_file() {
        let dir = tempfile::tempdir().unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "nonexistent.oxoflow"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        let result = config.resolve_includes(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn validate_execution_groups_valid() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[execution_group]]
            name = "prep"
            rules = ["step1"]

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert!(config.validate_execution_groups().is_ok());
    }

    #[test]
    fn validate_execution_groups_unknown_rule() {
        let toml_str = r#"
            [workflow]
            name = "grouped"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;

        let mut config = WorkflowConfig::parse(toml_str).unwrap();
        config.execution_groups.push(ExecutionGroup {
            name: "bad_group".to_string(),
            rules: vec!["nonexistent".to_string()],
            mode: ExecutionMode::Parallel,
        });

        let result = config.validate_execution_groups();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("bad_group"));
    }

    #[test]
    fn validate_rejects_bad_execution_groups() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [[execution_group]]
            name = "group1"
            rules = ["missing_rule"]

            [[rules]]
            name = "real_rule"
            shell = "echo hi"
        "#;

        let result = WorkflowConfig::parse(toml_str);
        assert!(result.is_err());
    }
}
