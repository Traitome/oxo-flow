//! .oxoflow file format specification, validation, formatting, and linting.
//!
//! This module provides utilities for working with .oxoflow files beyond
//! basic TOML parsing — including deep validation, best-practice linting,
//! canonical formatting, and format version management.

use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;
use serde::{Deserialize, Serialize};

/// Current .oxoflow format specification version.
pub const FORMAT_VERSION: &str = "1.0";

/// Severity level for validation and lint messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Informational note.
    Info,
    /// Suggestion for improvement.
    Warning,
    /// Must be fixed for the workflow to be valid.
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// A single diagnostic message from validation or linting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Severity of the issue.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Optional rule name this diagnostic relates to.
    pub rule: Option<String>,
    /// Diagnostic code for programmatic handling (e.g. "E001", "W001").
    pub code: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.severity, self.message)?;
        if let Some(ref rule) = self.rule {
            write!(f, " (rule: {})", rule)?;
        }
        Ok(())
    }
}

/// Result of validating a workflow file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the workflow is valid (no errors).
    pub valid: bool,
    /// List of diagnostic messages.
    pub diagnostics: Vec<Diagnostic>,
    /// Format version detected.
    pub format_version: String,
}

impl ValidationResult {
    /// Returns true if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Returns true if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning)
    }

    /// Returns only error diagnostics.
    pub fn errors(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }

    /// Returns only warning diagnostics.
    pub fn warnings(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect()
    }
}

/// Perform deep validation of a workflow configuration.
///
/// Goes beyond basic TOML parsing to check:
/// - Required fields and valid types
/// - Rule name conventions
/// - Input/output pattern validity
/// - DAG cycle detection
/// - Resource constraint consistency
/// - Environment specification validity
/// - Wildcard consistency between inputs and outputs
pub fn validate_format(config: &WorkflowConfig) -> ValidationResult {
    let mut diagnostics = Vec::new();

    // E001: Workflow name is required
    if config.workflow.name.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            message: "workflow name cannot be empty".to_string(),
            rule: None,
            code: "E001".to_string(),
        });
    }

    let config_ref_re = regex::Regex::new(r"\{config\.(\w+)\}").expect("valid regex");

    // Validate each rule
    for rule in &config.rules {
        // E002: Rule validation
        if let Err(msg) = rule.validate() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: msg,
                rule: Some(rule.name.clone()),
                code: "E002".to_string(),
            });
        }

        // E003: Wildcard consistency - output wildcards must appear in inputs
        let input_wildcards = crate::wildcard::extract_wildcards_from_patterns(&rule.input);
        let output_wildcards = crate::wildcard::extract_wildcards_from_patterns(&rule.output);
        for wc in &output_wildcards {
            if !input_wildcards.contains(wc) && !rule.input.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("wildcard '{{{}}}' appears in output but not in input", wc),
                    rule: Some(rule.name.clone()),
                    code: "E003".to_string(),
                });
            }
        }

        // E004: Memory format validation
        if let Some(ref mem) = rule.memory
            && crate::scheduler::parse_memory_mb(mem).is_none()
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("invalid memory specification: '{}'", mem),
                rule: Some(rule.name.clone()),
                code: "E004".to_string(),
            });
        }

        // E005: Shell command references existing config variables
        if let Some(ref shell) = rule.shell {
            for cap in config_ref_re.captures_iter(shell) {
                let key = &cap[1];
                if config.get_config_value(key).is_none() {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "shell command references undefined config variable '{}'",
                            key
                        ),
                        rule: Some(rule.name.clone()),
                        code: "E005".to_string(),
                    });
                }
            }
        }
    }

    // E006: DAG cycle detection
    match WorkflowDag::from_rules(&config.rules) {
        Ok(_) => {}
        Err(e) => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("DAG error: {}", e),
                rule: None,
                code: "E006".to_string(),
            });
        }
    }

    let valid = !diagnostics.iter().any(|d| d.severity == Severity::Error);

    ValidationResult {
        valid,
        diagnostics,
        format_version: FORMAT_VERSION.to_string(),
    }
}

/// Perform best-practice linting on a workflow configuration.
///
/// Checks for:
/// - Missing descriptions on rules
/// - Unused rules (no dependents and not a target)
/// - Naming convention violations
/// - Missing log files for complex rules
/// - Suboptimal resource allocations
pub fn lint_format(config: &WorkflowConfig) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // W001: Missing workflow description
    if config.workflow.description.is_none() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: "workflow has no description".to_string(),
            rule: None,
            code: "W001".to_string(),
        });
    }

    // W002: Missing workflow author
    if config.workflow.author.is_none() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: "workflow has no author".to_string(),
            rule: None,
            code: "W002".to_string(),
        });
    }

    // Build DAG for dependency analysis
    let dag = WorkflowDag::from_rules(&config.rules).ok();

    for rule in &config.rules {
        // W003: Missing rule description
        if rule.description.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule has no description".to_string(),
                rule: Some(rule.name.clone()),
                code: "W003".to_string(),
            });
        }

        // W004: Missing log file for rules with shell commands
        if rule.shell.is_some() && rule.log.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule has a shell command but no log file specified".to_string(),
                rule: Some(rule.name.clone()),
                code: "W004".to_string(),
            });
        }

        // W005: High thread count without memory specification
        if rule.effective_threads() > 8 && rule.effective_memory().is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule uses >8 threads but has no memory specification".to_string(),
                rule: Some(rule.name.clone()),
                code: "W005".to_string(),
            });
        }

        // W006: Naming convention (should use snake_case)
        if rule.name.contains('-') {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "consider using snake_case (underscores) instead of hyphens in rule names"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W006".to_string(),
            });
        }

        // W007: Leaf rule without target flag
        if let Some(ref dag) = dag
            && let Ok(dependents) = dag.dependents(&rule.name)
            && dependents.is_empty()
            && !rule.target
            && !rule.output.is_empty()
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "leaf rule (no dependents) could be marked as target = true".to_string(),
                rule: Some(rule.name.clone()),
                code: "W007".to_string(),
            });
        }

        // W008: No environment specified
        if rule.environment.is_empty() && rule.shell.is_some() {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "rule has no environment specification; will use system environment"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W008".to_string(),
            });
        }
    }

    diagnostics
}

/// Statistics about a workflow configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStats {
    /// Total number of rules.
    pub rule_count: usize,
    /// Number of rules with shell commands.
    pub shell_rules: usize,
    /// Number of rules with script files.
    pub script_rules: usize,
    /// Number of DAG edges (dependencies).
    pub dependency_count: usize,
    /// Number of parallel groups.
    pub parallel_groups: usize,
    /// Maximum depth of the DAG.
    pub max_depth: usize,
    /// Environment types used.
    pub environments: Vec<String>,
    /// Total declared threads across all rules.
    pub total_threads: u32,
    /// Number of unique wildcards.
    pub wildcard_count: usize,
    /// Wildcard names found.
    pub wildcard_names: Vec<String>,
}

/// Compute statistics for a workflow configuration.
pub fn workflow_stats(config: &WorkflowConfig) -> WorkflowStats {
    let mut environments = Vec::new();
    let mut total_threads: u32 = 0;
    let mut wildcard_names = Vec::new();
    let mut shell_rules = 0;
    let mut script_rules = 0;

    for rule in &config.rules {
        let kind = rule.environment.kind();
        if kind != "system" && !environments.contains(&kind.to_string()) {
            environments.push(kind.to_string());
        }
        total_threads = total_threads.saturating_add(rule.effective_threads());

        if rule.shell.is_some() {
            shell_rules += 1;
        }
        if rule.script.is_some() {
            script_rules += 1;
        }

        for wc in rule.wildcard_names() {
            if !wildcard_names.contains(&wc) {
                wildcard_names.push(wc);
            }
        }
    }

    let (dependency_count, parallel_groups, max_depth) =
        match WorkflowDag::from_rules(&config.rules) {
            Ok(dag) => {
                let groups = dag.parallel_groups().unwrap_or_default();
                (
                    dag.edge_count(),
                    groups.len(),
                    groups.len().saturating_sub(1),
                )
            }
            Err(_) => (0, 0, 0),
        };

    environments.sort();
    wildcard_names.sort();

    WorkflowStats {
        rule_count: config.rules.len(),
        shell_rules,
        script_rules,
        dependency_count,
        parallel_groups,
        max_depth,
        environments,
        total_threads,
        wildcard_count: wildcard_names.len(),
        wildcard_names,
    }
}

/// Format a workflow configuration into canonical .oxoflow TOML string.
///
/// Produces a consistently formatted output suitable for version control.
pub fn format_workflow(config: &WorkflowConfig) -> String {
    let mut output = String::new();

    // [workflow] section
    output.push_str("[workflow]\n");
    output.push_str(&format!("name = \"{}\"\n", config.workflow.name));
    output.push_str(&format!("version = \"{}\"\n", config.workflow.version));
    if let Some(ref desc) = config.workflow.description {
        output.push_str(&format!("description = \"{}\"\n", desc));
    }
    if let Some(ref author) = config.workflow.author {
        output.push_str(&format!("author = \"{}\"\n", author));
    }

    // [config] section
    if !config.config.is_empty() {
        output.push_str("\n[config]\n");
        let mut keys: Vec<&String> = config.config.keys().collect();
        keys.sort();
        for key in keys {
            let value = &config.config[key];
            output.push_str(&format!("{} = {}\n", key, format_toml_value(value)));
        }
    }

    // [defaults] section
    let has_defaults = config.defaults.threads.is_some()
        || config.defaults.memory.is_some()
        || config.defaults.environment.is_some();
    if has_defaults {
        output.push_str("\n[defaults]\n");
        if let Some(threads) = config.defaults.threads {
            output.push_str(&format!("threads = {}\n", threads));
        }
        if let Some(ref memory) = config.defaults.memory {
            output.push_str(&format!("memory = \"{}\"\n", memory));
        }
    }

    // [[rules]] sections
    for rule in &config.rules {
        output.push_str("\n[[rules]]\n");
        output.push_str(&format!("name = \"{}\"\n", rule.name));
        if let Some(ref desc) = rule.description {
            output.push_str(&format!("description = \"{}\"\n", desc));
        }
        if !rule.input.is_empty() {
            output.push_str(&format!("input = {}\n", format_string_array(&rule.input)));
        }
        if !rule.output.is_empty() {
            output.push_str(&format!("output = {}\n", format_string_array(&rule.output)));
        }
        if let Some(threads) = rule.threads {
            output.push_str(&format!("threads = {}\n", threads));
        }
        if let Some(ref memory) = rule.memory {
            output.push_str(&format!("memory = \"{}\"\n", memory));
        }
        if rule.priority != 0 {
            output.push_str(&format!("priority = {}\n", rule.priority));
        }
        if rule.target {
            output.push_str("target = true\n");
        }
        if let Some(ref group) = rule.group {
            output.push_str(&format!("group = \"{}\"\n", group));
        }
        if let Some(ref log) = rule.log {
            output.push_str(&format!("log = \"{}\"\n", log));
        }
        if let Some(ref benchmark) = rule.benchmark {
            output.push_str(&format!("benchmark = \"{}\"\n", benchmark));
        }
        if let Some(ref shell) = rule.shell {
            if shell.contains('\n') {
                output.push_str(&format!("shell = \"\"\"\n{}\n\"\"\"\n", shell));
            } else {
                output.push_str(&format!("shell = \"{}\"\n", shell));
            }
        }
        if let Some(ref script) = rule.script {
            output.push_str(&format!("script = \"{}\"\n", script));
        }
        if !rule.environment.is_empty() {
            output.push_str("\n[rules.environment]\n");
            if let Some(ref conda) = rule.environment.conda {
                output.push_str(&format!("conda = \"{}\"\n", conda));
            }
            if let Some(ref pixi) = rule.environment.pixi {
                output.push_str(&format!("pixi = \"{}\"\n", pixi));
            }
            if let Some(ref docker) = rule.environment.docker {
                output.push_str(&format!("docker = \"{}\"\n", docker));
            }
            if let Some(ref singularity) = rule.environment.singularity {
                output.push_str(&format!("singularity = \"{}\"\n", singularity));
            }
            if let Some(ref venv) = rule.environment.venv {
                output.push_str(&format!("venv = \"{}\"\n", venv));
            }
        }
    }

    // [report] section
    if let Some(ref report) = config.report {
        output.push_str("\n[report]\n");
        if let Some(ref template) = report.template {
            output.push_str(&format!("template = \"{}\"\n", template));
        }
        if !report.format.is_empty() {
            output.push_str(&format!(
                "format = {}\n",
                format_string_array(&report.format)
            ));
        }
        if !report.sections.is_empty() {
            output.push_str(&format!(
                "sections = {}\n",
                format_string_array(&report.sections)
            ));
        }
    }

    output
}

fn format_string_array(arr: &[String]) -> String {
    let items: Vec<String> = arr.iter().map(|s| format!("\"{}\"", s)).collect();
    format!("[{}]", items.join(", "))
}

fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_toml_value).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{} = {}", k, format_toml_value(v)))
                .collect();
            format!("{{ {} }}", items.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_workflow() -> &'static str {
        r#"
            [workflow]
            name = "test-pipeline"
            version = "1.0.0"
            description = "A test pipeline"
            author = "Test Author"

            [config]
            reference = "/path/to/ref.fa"

            [defaults]
            threads = 4
            memory = "8G"

            [[rules]]
            name = "step1"
            description = "First step"
            input = ["raw/{sample}.fastq.gz"]
            output = ["trimmed/{sample}.fastq.gz"]
            threads = 8
            memory = "16G"
            shell = "fastp -i {input} -o {output}"

            [rules.environment]
            conda = "envs/fastp.yaml"

            [[rules]]
            name = "step2"
            description = "Second step"
            input = ["trimmed/{sample}.fastq.gz"]
            output = ["aligned/{sample}.bam"]
            threads = 16
            memory = "32G"
            shell = "bwa mem -t {threads} {config.reference} {input} | samtools sort -o {output}"

            [rules.environment]
            docker = "biocontainers/bwa:0.7.17"
        "#
    }

    #[test]
    fn validate_valid_workflow() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let result = validate_format(&config);
        assert!(result.valid);
        assert!(!result.has_errors());
    }

    #[test]
    fn validate_empty_workflow_name() {
        let toml = r#"
            [workflow]
            name = ""
            version = "1.0.0"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E001"));
    }

    #[test]
    fn validate_invalid_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            memory = "invalid"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E004"));
    }

    #[test]
    fn validate_undefined_config_ref() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo {config.nonexistent}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E005"));
    }

    #[test]
    fn validate_wildcard_consistency() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["{sample}.fastq"]
            output = ["{sample}_{unknown}.bam"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E003"));
    }

    #[test]
    fn lint_missing_descriptions() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W001"));
        assert!(diagnostics.iter().any(|d| d.code == "W003"));
    }

    #[test]
    fn lint_high_threads_no_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            threads = 16
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W005"));
    }

    #[test]
    fn lint_missing_log() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W004"));
    }

    #[test]
    fn workflow_stats_basic() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let stats = workflow_stats(&config);
        assert_eq!(stats.rule_count, 2);
        assert_eq!(stats.shell_rules, 2);
        assert_eq!(stats.dependency_count, 1);
        assert!(stats.environments.contains(&"conda".to_string()));
        assert!(stats.environments.contains(&"docker".to_string()));
        assert!(stats.wildcard_names.contains(&"sample".to_string()));
    }

    #[test]
    fn format_roundtrip() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let formatted = format_workflow(&config);
        // The formatted output should be valid TOML that can be re-parsed
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.workflow.name, config.workflow.name);
        assert_eq!(reparsed.rules.len(), config.rules.len());
    }

    #[test]
    fn format_version_is_set() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let result = validate_format(&config);
        assert_eq!(result.format_version, FORMAT_VERSION);
    }

    #[test]
    fn validation_result_methods() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let result = validate_format(&config);
        assert!(!result.has_errors());
        assert!(result.errors().is_empty());
    }

    #[test]
    fn diagnostic_display() {
        let d = Diagnostic {
            severity: Severity::Error,
            message: "test error".to_string(),
            rule: Some("step1".to_string()),
            code: "E001".to_string(),
        };
        let s = format!("{}", d);
        assert!(s.contains("E001"));
        assert!(s.contains("error"));
        assert!(s.contains("step1"));
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }
}
