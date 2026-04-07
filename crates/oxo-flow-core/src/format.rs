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
    /// Optional suggestion for how to fix the issue.
    #[serde(default)]
    pub suggestion: Option<String>,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.severity, self.message)?;
        if let Some(ref rule) = self.rule {
            write!(f, " (rule: {})", rule)?;
        }
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " — hint: {}", suggestion)?;
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
            suggestion: Some("add a non-empty name to the [workflow] section".to_string()),
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
                suggestion: None,
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
                    suggestion: Some(format!("add '{{{{{}}}}}' to the rule's input patterns", wc)),
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
                suggestion: Some(
                    "use a valid format like \"8G\", \"16384M\", or \"1T\"".to_string(),
                ),
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
                        suggestion: Some(format!("define '{}' in the [config] section", key)),
                    });
                }
            }
        }
    }

    // E007: depends_on references non-existent rules
    let rule_names: std::collections::HashSet<&str> =
        config.rules.iter().map(|r| r.name.as_str()).collect();
    for rule in &config.rules {
        for dep in &rule.depends_on {
            if !rule_names.contains(dep.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("depends_on references non-existent rule '{}'", dep),
                    rule: Some(rule.name.clone()),
                    code: "E007".to_string(),
                    suggestion: Some(format!(
                        "ensure rule '{}' is defined in the workflow or remove it from depends_on",
                        dep
                    )),
                });
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
                suggestion: Some("check for circular dependencies between rules".to_string()),
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
            suggestion: Some("add description = \"...\" to the [workflow] section".to_string()),
        });
    }

    // W002: Missing workflow author
    if config.workflow.author.is_none() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: "workflow has no author".to_string(),
            rule: None,
            code: "W002".to_string(),
            suggestion: Some("add author = \"...\" to the [workflow] section".to_string()),
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
                suggestion: Some("add description = \"...\" to this rule".to_string()),
            });
        }

        // W004: Missing log file for rules with shell commands
        if rule.shell.is_some() && rule.log.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule has a shell command but no log file specified".to_string(),
                rule: Some(rule.name.clone()),
                code: "W004".to_string(),
                suggestion: Some(format!("add log = \"logs/{}.log\"", rule.name)),
            });
        }

        // W005: High thread count without memory specification
        if rule.effective_threads() > 8 && rule.effective_memory().is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule uses >8 threads but has no memory specification".to_string(),
                rule: Some(rule.name.clone()),
                code: "W005".to_string(),
                suggestion: Some(
                    "add memory = \"32G\" or appropriate memory specification".to_string(),
                ),
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
                suggestion: Some(format!("rename to \"{}\"", rule.name.replace('-', "_"))),
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
                suggestion: Some("add target = true to this rule".to_string()),
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
                suggestion: Some(
                    "add an [environment] section with conda, docker, or another backend"
                        .to_string(),
                ),
            });
        }

        // W009: Very high thread count (>32) without memory specification
        if rule.effective_threads() > 32 && rule.effective_memory().is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule uses >32 threads but has no memory specification — high-thread jobs typically need significant memory".to_string(),
                rule: Some(rule.name.clone()),
                code: "W009".to_string(),
                suggestion: Some("add memory = \"64G\" or appropriate value for high-thread workloads".to_string()),
            });
        }

        // W010: Rule has checkpoint = true but no output files
        if rule.checkpoint && rule.output.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule has checkpoint = true but no output files".to_string(),
                rule: Some(rule.name.clone()),
                code: "W010".to_string(),
                suggestion: Some(
                    "add output files to the checkpoint rule, or remove checkpoint = true"
                        .to_string(),
                ),
            });
        }

        // W011: Rule uses shadow but has no inputs (shadow is unnecessary)
        if rule.shadow.is_some() && rule.input.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule uses shadow but has no inputs — shadow directory is unnecessary"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W011".to_string(),
                suggestion: Some("remove the shadow setting, or add input files".to_string()),
            });
        }

        // W012: Rule has retries but no retry_delay
        if rule.retries > 0 && rule.retry_delay.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "rule has retries but no retry_delay — retries will execute immediately"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W012".to_string(),
                suggestion: Some(
                    "add retry_delay = \"10s\" to add a backoff between retry attempts".to_string(),
                ),
            });
        }

        // W013: Rule has on_failure but no retries
        if rule.on_failure.is_some() && rule.retries == 0 {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message:
                    "rule has on_failure hook but no retries — on_failure runs on first failure"
                        .to_string(),
                rule: Some(rule.name.clone()),
                code: "W013".to_string(),
                suggestion: Some(
                    "consider adding retries = 1 or more before triggering on_failure".to_string(),
                ),
            });
        }

        // W014: depends_on references non-existent rule
        for dep in &rule.depends_on {
            if !config.rules.iter().any(|r| r.name == *dep) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("depends_on references unknown rule '{}'", dep),
                    rule: Some(rule.name.clone()),
                    code: "W014".to_string(),
                    suggestion: Some(format!("check that rule '{}' exists in the workflow", dep)),
                });
            }
        }

        // W015: Rule extends a non-existent base rule
        if let Some(ref base) = rule.extends
            && !config.rules.iter().any(|r| r.name == *base)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!("rule extends unknown base rule '{}'", base),
                rule: Some(rule.name.clone()),
                code: "W015".to_string(),
                suggestion: Some(format!("check that rule '{}' exists in the workflow", base)),
            });
        }

        // W016: Environment uses unpinned specification
        if let Some(ref conda_env) = rule.environment.conda
            && !conda_env.ends_with(".lock")
            && !conda_env.ends_with(".lock.yml")
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message:
                    "conda environment file is not a lockfile — builds may not be reproducible"
                        .to_string(),
                rule: Some(rule.name.clone()),
                code: "W016".to_string(),
                suggestion: Some(format!(
                    "generate a lockfile with 'conda-lock -f {}' for reproducible builds",
                    conda_env
                )),
            });
        }
        if let Some(ref pixi_env) = rule.environment.pixi
            && !pixi_env.ends_with(".lock")
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "pixi environment is not a lockfile — builds may not be reproducible"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W016".to_string(),
                suggestion: Some("use 'pixi.lock' for reproducible builds".to_string()),
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

/// Verify that a TOML string conforms to the .oxoflow schema.
///
/// This is a lighter-weight check than full parsing — it verifies the
/// presence of required sections and correct types without constructing
/// a full WorkflowConfig.
pub fn verify_schema(toml_content: &str) -> ValidationResult {
    let mut diagnostics = Vec::new();

    let table: toml::Table = match toml::from_str(toml_content) {
        Ok(t) => t,
        Err(e) => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("invalid TOML syntax: {}", e),
                rule: None,
                code: "S001".to_string(),
                suggestion: None,
            });
            return ValidationResult {
                valid: false,
                diagnostics,
                format_version: FORMAT_VERSION.to_string(),
            };
        }
    };

    // S002: [workflow] section is required
    if !table.contains_key("workflow") {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            message: "[workflow] section is required".to_string(),
            rule: None,
            code: "S002".to_string(),
            suggestion: Some("add a [workflow] section with at least a name field".to_string()),
        });
    } else if let Some(wf) = table.get("workflow").and_then(|v| v.as_table()) {
        // S003: workflow.name is required
        if !wf.contains_key("name") {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: "workflow.name is required".to_string(),
                rule: None,
                code: "S003".to_string(),
                suggestion: Some(
                    "add name = \"my-workflow\" to the [workflow] section".to_string(),
                ),
            });
        }
    }

    // S004: rules must be an array of tables
    if let Some(rules) = table.get("rules") {
        if !rules.is_array() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: "[[rules]] must be an array of tables".to_string(),
                rule: None,
                code: "S004".to_string(),
                suggestion: Some("use [[rules]] syntax for rule definitions".to_string()),
            });
        } else if let Some(arr) = rules.as_array() {
            for (i, item) in arr.iter().enumerate() {
                if !item.is_table() {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!("rules[{}] must be a table", i),
                        rule: None,
                        code: "S004".to_string(),
                        suggestion: None,
                    });
                } else if let Some(t) = item.as_table()
                    && !t.contains_key("name")
                {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!("rules[{}].name is required", i),
                        rule: None,
                        code: "S005".to_string(),
                        suggestion: Some("add a name field to each [[rules]] entry".to_string()),
                    });
                }
            }
        }
    }

    // S006: unknown top-level keys
    let known_keys = [
        "workflow",
        "config",
        "defaults",
        "rules",
        "report",
        "include",
        "execution_group",
        "citation",
        "cluster",
        "resource_budget",
    ];
    for key in table.keys() {
        if !known_keys.contains(&key.as_str()) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!("unknown top-level section: '{}'", key),
                rule: None,
                code: "S006".to_string(),
                suggestion: Some(format!("remove or rename '{}' — it is not recognized", key)),
            });
        }
    }

    // S007: Warn if format_version is present but unrecognized
    if let Some(wf) = table.get("workflow").and_then(|v| v.as_table())
        && let Some(fmt_ver) = wf.get("format_version").and_then(|v| v.as_str())
        && !check_format_version(fmt_ver)
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: format!(
                "format_version '{}' is newer than supported version '{}'",
                fmt_ver, FORMAT_VERSION
            ),
            rule: None,
            code: "S007".to_string(),
            suggestion: Some(format!(
                "use format_version = \"{}\" or upgrade oxo-flow",
                FORMAT_VERSION
            )),
        });
    }

    let valid = !diagnostics.iter().any(|d| d.severity == Severity::Error);
    ValidationResult {
        valid,
        diagnostics,
        format_version: FORMAT_VERSION.to_string(),
    }
}

/// Check format version compatibility.
pub fn check_format_version(version: &str) -> bool {
    version == FORMAT_VERSION || version.starts_with("1.")
}

/// Known bioinformatics file format extensions.
pub const KNOWN_BIO_FORMATS: &[&str] = &[
    ".bam",
    ".sam",
    ".cram",
    ".vcf",
    ".vcf.gz",
    ".bcf",
    ".fastq",
    ".fastq.gz",
    ".fq",
    ".fq.gz",
    ".bed",
    ".bed.gz",
    ".gff",
    ".gff3",
    ".gtf",
    ".fa",
    ".fasta",
    ".fa.gz",
    ".fasta.gz",
    ".bw",
    ".bigwig",
    ".wig",
    ".tsv",
    ".csv",
    ".h5",
    ".hdf5",
    ".maf",
    ".seg",
    ".bai",
    ".crai",
    ".tbi",
    ".idx",
];

/// Check if a file path has a known bioinformatics format extension.
#[must_use]
pub fn is_known_bio_format(path: &str) -> bool {
    let lower = path.to_lowercase();
    KNOWN_BIO_FORMATS.iter().any(|ext| lower.ends_with(ext))
}

/// Scan text for common secret patterns (API keys, passwords, tokens).
///
/// Returns a list of warnings for any potential secrets found.
#[must_use]
pub fn scan_for_secrets(text: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let secret_patterns = [
        ("AKIA", "Possible AWS Access Key"),
        ("sk-", "Possible Stripe/OpenAI secret key"),
        ("ghp_", "Possible GitHub personal access token"),
        ("glpat-", "Possible GitLab personal access token"),
        ("password", "Possible password in configuration"),
        ("secret", "Possible secret in configuration"),
        ("api_key", "Possible API key in configuration"),
        ("access_token", "Possible access token in configuration"),
        ("private_key", "Possible private key in configuration"),
    ];
    for (pattern, description) in &secret_patterns {
        if text.to_lowercase().contains(&pattern.to_lowercase()) {
            diagnostics.push(Diagnostic {
                code: "S008".to_string(),
                severity: Severity::Warning,
                message: format!("{}: found pattern matching '{}'", description, pattern),
                rule: None,
                suggestion: Some(
                    "Remove secrets from workflow files and use environment variables instead"
                        .to_string(),
                ),
            });
        }
    }
    diagnostics
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
        if let Some(ref when) = rule.when {
            output.push_str(&format!("when = \"{}\"\n", when));
        }
        if let Some(ref input_function) = rule.input_function {
            output.push_str(&format!("input_function = \"{}\"\n", input_function));
        }
        if rule.retries > 0 {
            output.push_str(&format!("retries = {}\n", rule.retries));
        }
        if let Some(ref retry_delay) = rule.retry_delay {
            output.push_str(&format!("retry_delay = \"{}\"\n", retry_delay));
        }
        if !rule.depends_on.is_empty() {
            output.push_str(&format!(
                "depends_on = {}\n",
                format_string_array(&rule.depends_on)
            ));
        }
        if let Some(ref workdir) = rule.workdir {
            output.push_str(&format!("workdir = \"{}\"\n", workdir));
        }
        if let Some(ref on_success) = rule.on_success {
            output.push_str(&format!("on_success = \"{}\"\n", on_success));
        }
        if let Some(ref on_failure) = rule.on_failure {
            output.push_str(&format!("on_failure = \"{}\"\n", on_failure));
        }
        if !rule.temp_output.is_empty() {
            output.push_str(&format!(
                "temp_output = {}\n",
                format_string_array(&rule.temp_output)
            ));
        }
        if !rule.protected_output.is_empty() {
            output.push_str(&format!(
                "protected_output = {}\n",
                format_string_array(&rule.protected_output)
            ));
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
        if let Some(ref scatter) = rule.scatter {
            output.push_str("\n[rules.scatter]\n");
            output.push_str(&format!("variable = \"{}\"\n", scatter.variable));
            if !scatter.values.is_empty() {
                output.push_str(&format!(
                    "values = {}\n",
                    format_string_array(&scatter.values)
                ));
            }
            if let Some(ref gather) = scatter.gather {
                output.push_str(&format!("gather = \"{}\"\n", gather));
            }
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

    // [[include]] sections
    for inc in &config.includes {
        output.push_str("\n[[include]]\n");
        output.push_str(&format!("path = \"{}\"\n", inc.path));
        if let Some(ref ns) = inc.namespace {
            output.push_str(&format!("namespace = \"{}\"\n", ns));
        }
    }

    // [[execution_group]] sections
    for group in &config.execution_groups {
        output.push_str("\n[[execution_group]]\n");
        output.push_str(&format!("name = \"{}\"\n", group.name));
        if !group.rules.is_empty() {
            output.push_str(&format!("rules = {}\n", format_string_array(&group.rules)));
        }
        let mode_str = match group.mode {
            crate::config::ExecutionMode::Sequential => "sequential",
            crate::config::ExecutionMode::Parallel => "parallel",
        };
        output.push_str(&format!("mode = \"{}\"\n", mode_str));
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

/// A single difference between two workflow configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDiff {
    /// Category of the change.
    pub category: String,
    /// Human-readable description of the difference.
    pub description: String,
}

/// Compare two workflow configurations and return a list of differences.
///
/// This is useful for reviewing changes between workflow versions or
/// comparing variants of a pipeline.
#[must_use]
pub fn diff_workflows(a: &WorkflowConfig, b: &WorkflowConfig) -> Vec<WorkflowDiff> {
    let mut diffs = Vec::new();

    // Compare metadata
    if a.workflow.name != b.workflow.name {
        diffs.push(WorkflowDiff {
            category: "workflow".to_string(),
            description: format!(
                "name changed: \"{}\" → \"{}\"",
                a.workflow.name, b.workflow.name
            ),
        });
    }
    if a.workflow.version != b.workflow.version {
        diffs.push(WorkflowDiff {
            category: "workflow".to_string(),
            description: format!(
                "version changed: \"{}\" → \"{}\"",
                a.workflow.version, b.workflow.version
            ),
        });
    }
    if a.workflow.description != b.workflow.description {
        diffs.push(WorkflowDiff {
            category: "workflow".to_string(),
            description: format!(
                "description changed: {:?} → {:?}",
                a.workflow.description, b.workflow.description
            ),
        });
    }

    // Compare rules
    let a_names: std::collections::HashSet<&str> =
        a.rules.iter().map(|r| r.name.as_str()).collect();
    let b_names: std::collections::HashSet<&str> =
        b.rules.iter().map(|r| r.name.as_str()).collect();

    for name in a_names.difference(&b_names) {
        diffs.push(WorkflowDiff {
            category: "rules".to_string(),
            description: format!("rule removed: \"{}\"", name),
        });
    }
    for name in b_names.difference(&a_names) {
        diffs.push(WorkflowDiff {
            category: "rules".to_string(),
            description: format!("rule added: \"{}\"", name),
        });
    }

    // Compare common rules
    for a_rule in &a.rules {
        if let Some(b_rule) = b.rules.iter().find(|r| r.name == a_rule.name) {
            let rule_name = &a_rule.name;
            if a_rule.input != b_rule.input {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!("rule \"{}\": input changed", rule_name),
                });
            }
            if a_rule.output != b_rule.output {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!("rule \"{}\": output changed", rule_name),
                });
            }
            if a_rule.shell != b_rule.shell {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!("rule \"{}\": shell command changed", rule_name),
                });
            }
            if a_rule.effective_threads() != b_rule.effective_threads() {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!(
                        "rule \"{}\": threads changed: {} → {}",
                        rule_name,
                        a_rule.effective_threads(),
                        b_rule.effective_threads()
                    ),
                });
            }
            if a_rule.effective_memory() != b_rule.effective_memory() {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!(
                        "rule \"{}\": memory changed: {:?} → {:?}",
                        rule_name,
                        a_rule.effective_memory(),
                        b_rule.effective_memory()
                    ),
                });
            }
            if a_rule.environment != b_rule.environment {
                diffs.push(WorkflowDiff {
                    category: "rules".to_string(),
                    description: format!("rule \"{}\": environment changed", rule_name),
                });
            }
        }
    }

    // Compare config variables
    for (key, val) in &a.config {
        match b.config.get(key) {
            None => {
                diffs.push(WorkflowDiff {
                    category: "config".to_string(),
                    description: format!("config variable removed: \"{}\"", key),
                });
            }
            Some(bval) if val != bval => {
                diffs.push(WorkflowDiff {
                    category: "config".to_string(),
                    description: format!("config variable changed: \"{}\"", key),
                });
            }
            _ => {}
        }
    }
    for key in b.config.keys() {
        if !a.config.contains_key(key) {
            diffs.push(WorkflowDiff {
                category: "config".to_string(),
                description: format!("config variable added: \"{}\"", key),
            });
        }
    }

    diffs
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
            suggestion: None,
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

    // -- verify_schema tests -------------------------------------------------

    #[test]
    fn verify_schema_valid() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
        "#;
        let result = verify_schema(toml);
        assert!(result.valid);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn verify_schema_invalid_toml() {
        let result = verify_schema("this is not valid toml {{{");
        assert!(!result.valid);
        assert!(result.diagnostics.iter().any(|d| d.code == "S001"));
    }

    #[test]
    fn verify_schema_missing_workflow() {
        let toml = r#"
            [[rules]]
            name = "step1"
        "#;
        let result = verify_schema(toml);
        assert!(!result.valid);
        assert!(result.diagnostics.iter().any(|d| d.code == "S002"));
    }

    #[test]
    fn verify_schema_missing_workflow_name() {
        let toml = r#"
            [workflow]
            version = "1.0"
        "#;
        let result = verify_schema(toml);
        assert!(!result.valid);
        assert!(result.diagnostics.iter().any(|d| d.code == "S003"));
    }

    #[test]
    fn verify_schema_rule_missing_name() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            shell = "echo hi"
        "#;
        let result = verify_schema(toml);
        assert!(!result.valid);
        assert!(result.diagnostics.iter().any(|d| d.code == "S005"));
    }

    #[test]
    fn verify_schema_unknown_top_level_key() {
        let toml = r#"
            [workflow]
            name = "test"

            [custom_section]
            key = "value"
        "#;
        let result = verify_schema(toml);
        assert!(result.valid); // warnings don't make it invalid
        assert!(result.diagnostics.iter().any(|d| d.code == "S006"));
    }

    #[test]
    fn verify_schema_format_version_set() {
        let toml = r#"
            [workflow]
            name = "test"
        "#;
        let result = verify_schema(toml);
        assert_eq!(result.format_version, FORMAT_VERSION);
    }

    #[test]
    fn verify_schema_known_keys_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            ref = "/ref.fa"

            [defaults]
            threads = 4

            [report]
            template = "default"
        "#;
        let result = verify_schema(toml);
        assert!(result.valid);
        assert!(!result.diagnostics.iter().any(|d| d.code == "S006"));
    }

    // -- check_format_version tests ------------------------------------------

    #[test]
    fn check_format_version_exact_match() {
        assert!(check_format_version("1.0"));
    }

    #[test]
    fn check_format_version_compatible() {
        assert!(check_format_version("1.1"));
        assert!(check_format_version("1.99"));
    }

    #[test]
    fn check_format_version_incompatible() {
        assert!(!check_format_version("2.0"));
        assert!(!check_format_version("0.9"));
    }

    // -- format_workflow new fields roundtrip tests ---------------------------

    #[test]
    fn format_roundtrip_with_when() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "conditional"
            output = ["out.txt"]
            shell = "echo hi"
            when = "config.enabled"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("when = \"config.enabled\""));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.rules[0].when.as_deref(), Some("config.enabled"));
    }

    #[test]
    fn format_roundtrip_with_retries() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "flaky"
            output = ["out.txt"]
            shell = "echo hi"
            retries = 3
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("retries = 3"));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.rules[0].retries, 3);
    }

    #[test]
    fn format_roundtrip_with_temp_and_protected_output() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["sorted.bam"]
            shell = "sort input > sorted.bam"
            temp_output = ["unsorted.bam"]
            protected_output = ["sorted.bam"]
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("temp_output"));
        assert!(formatted.contains("protected_output"));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.rules[0].temp_output, vec!["unsorted.bam"]);
        assert_eq!(reparsed.rules[0].protected_output, vec!["sorted.bam"]);
    }

    #[test]
    fn format_roundtrip_with_input_function() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "dynamic"
            output = ["out.txt"]
            shell = "process"
            input_function = "get_inputs"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("input_function = \"get_inputs\""));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(
            reparsed.rules[0].input_function.as_deref(),
            Some("get_inputs")
        );
    }

    #[test]
    fn format_roundtrip_with_scatter() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "per_sample"
            input = ["{sample}.bam"]
            output = ["{sample}.vcf"]
            shell = "call {input}"

            [rules.scatter]
            variable = "sample"
            values = ["S1", "S2"]
            gather = "merge"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("[rules.scatter]"));
        assert!(formatted.contains("variable = \"sample\""));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        let scatter = reparsed.rules[0].scatter.as_ref().unwrap();
        assert_eq!(scatter.variable, "sample");
        assert_eq!(scatter.values, vec!["S1", "S2"]);
        assert_eq!(scatter.gather.as_deref(), Some("merge"));
    }

    #[test]
    fn format_roundtrip_with_includes() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[include]]
            path = "common/qc.oxoflow"
            namespace = "qc"

            [[include]]
            path = "align.oxoflow"

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("[[include]]"));
        assert!(formatted.contains("path = \"common/qc.oxoflow\""));
        assert!(formatted.contains("namespace = \"qc\""));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.includes.len(), 2);
        assert_eq!(reparsed.includes[0].namespace.as_deref(), Some("qc"));
    }

    #[test]
    fn format_roundtrip_with_execution_groups() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[execution_group]]
            name = "prep"
            rules = ["step1"]
            mode = "sequential"

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("[[execution_group]]"));
        assert!(formatted.contains("name = \"prep\""));
        assert!(formatted.contains("mode = \"sequential\""));
        let reparsed = WorkflowConfig::parse(&formatted).unwrap();
        assert_eq!(reparsed.execution_groups.len(), 1);
        assert_eq!(
            reparsed.execution_groups[0].mode,
            crate::config::ExecutionMode::Sequential
        );
    }

    #[test]
    fn format_retries_zero_not_emitted() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(!formatted.contains("retries"));
    }

    // -- lint checks for new features ----------------------------------------

    #[test]
    fn lint_when_conditional_rule() {
        let toml = r#"
            [workflow]
            name = "test"
            description = "desc"
            author = "me"

            [[rules]]
            name = "step1"
            description = "conditional step"
            output = ["out.txt"]
            shell = "echo hi"
            when = "config.enabled"
            log = "step1.log"

            [rules.environment]
            conda = "env.yaml"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.valid);
    }

    #[test]
    fn lint_scatter_rule() {
        let toml = r#"
            [workflow]
            name = "test"
            description = "desc"
            author = "me"

            [[rules]]
            name = "per_sample"
            description = "scatter step"
            input = ["{sample}.bam"]
            output = ["{sample}.vcf"]
            shell = "call {input}"
            log = "per_sample.log"

            [rules.scatter]
            variable = "sample"
            values = ["S1", "S2"]

            [rules.environment]
            conda = "env.yaml"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.valid);
    }

    #[test]
    fn verify_schema_unknown_format_version() {
        let toml = r#"
            [workflow]
            name = "test"
            format_version = "99.0"
            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let result = verify_schema(toml);
        assert!(result.diagnostics.iter().any(|d| d.code == "S007"));
    }

    #[test]
    fn diagnostic_with_suggestion() {
        let d = Diagnostic {
            severity: Severity::Warning,
            message: "missing description".to_string(),
            rule: Some("test".to_string()),
            code: "W003".to_string(),
            suggestion: Some("add description field".to_string()),
        };
        let display = d.to_string();
        assert!(display.contains("hint: add description field"));
    }

    #[test]
    fn lint_very_high_threads_no_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            threads = 64
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W009"));
    }

    #[test]
    fn lint_checkpoint_no_outputs() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "discover"
            shell = "find . -name '*.fastq'"
            checkpoint = true
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W010"));
    }

    #[test]
    fn lint_checkpoint_with_outputs_no_w010() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "discover"
            output = ["samples.txt"]
            shell = "find . -name '*.fastq' > samples.txt"
            checkpoint = true
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(!diagnostics.iter().any(|d| d.code == "W010"));
    }

    #[test]
    fn lint_shadow_no_inputs() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "generate"
            output = ["out.txt"]
            shell = "echo hello > out.txt"
            shadow = "minimal"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W011"));
    }

    #[test]
    fn lint_shadow_with_inputs_no_w011() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "process"
            input = ["in.txt"]
            output = ["out.txt"]
            shell = "cat in.txt > out.txt"
            shadow = "minimal"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(!diagnostics.iter().any(|d| d.code == "W011"));
    }

    #[test]
    fn known_bio_formats() {
        assert!(is_known_bio_format("sample.bam"));
        assert!(is_known_bio_format("variants.vcf.gz"));
        assert!(is_known_bio_format("reads.fastq.gz"));
        assert!(!is_known_bio_format("readme.txt"));
        assert!(!is_known_bio_format("config.toml"));
    }

    #[test]
    fn secret_scanning_detects_aws_key() {
        let diags = scan_for_secrets("aws_access_key = AKIAIOSFODNN7EXAMPLE");
        assert!(!diags.is_empty());
        assert!(diags.iter().any(|d| d.message.contains("AWS")));
    }

    #[test]
    fn secret_scanning_clean_config() {
        let diags = scan_for_secrets("reference = /data/hg38.fa\nthreads = 8");
        assert!(diags.is_empty());
    }

    // ---- E007: depends_on references non-existent rule ----------------------

    #[test]
    fn validate_e007_depends_on_nonexistent_rule() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            depends_on = ["nonexistent"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E007"));
    }

    #[test]
    fn validate_e007_depends_on_valid_rule_no_error() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "setup"
            shell = "echo setup"

            [[rules]]
            name = "step1"
            depends_on = ["setup"]
            shell = "echo step1"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.errors().iter().any(|d| d.code == "E007"));
    }

    // ---- W012: retries without retry_delay ----------------------------------

    #[test]
    fn lint_w012_retries_without_retry_delay() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "flaky"
            retries = 3
            output = ["out.txt"]
            shell = "curl http://example.com > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W012"));
    }

    #[test]
    fn lint_w012_retries_with_retry_delay_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "flaky"
            retries = 3
            retry_delay = "10s"
            output = ["out.txt"]
            shell = "curl http://example.com > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(!diagnostics.iter().any(|d| d.code == "W012"));
    }

    // ---- W013: on_failure without retries -----------------------------------

    #[test]
    fn lint_w013_on_failure_without_retries() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            on_failure = "echo failed"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W013"));
    }

    #[test]
    fn lint_w013_on_failure_with_retries_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            retries = 2
            on_failure = "echo failed"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(!diagnostics.iter().any(|d| d.code == "W013"));
    }

    // ---- W014: depends_on references unknown rule ---------------------------

    #[test]
    fn lint_w014_depends_on_unknown_rule() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            depends_on = ["ghost"]
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(diagnostics.iter().any(|d| d.code == "W014"));
    }

    #[test]
    fn lint_w014_depends_on_known_rule_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "setup"
            shell = "echo setup"

            [[rules]]
            name = "step1"
            depends_on = ["setup"]
            output = ["out.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        assert!(!diagnostics.iter().any(|d| d.code == "W014"));
    }

    // ---- diff_workflows tests -----------------------------------------------

    #[test]
    fn diff_identical_workflows() {
        let config = WorkflowConfig::parse(sample_workflow()).unwrap();
        let diffs = diff_workflows(&config, &config);
        assert!(diffs.is_empty());
    }

    #[test]
    fn diff_added_rule() {
        let toml_a = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            shell = "echo hello"

            [[rules]]
            name = "step2"
            shell = "echo world"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(diffs.iter().any(|d| d.description.contains("rule added")));
    }

    #[test]
    fn diff_removed_rule() {
        let toml_a = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            shell = "echo hello"

            [[rules]]
            name = "step2"
            shell = "echo world"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(diffs.iter().any(|d| d.description.contains("rule removed")));
    }

    #[test]
    fn diff_changed_field() {
        let toml_a = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"
            threads = 4
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"
            threads = 16
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(
            diffs
                .iter()
                .any(|d| d.description.contains("threads changed"))
        );
    }

    // ---- format_workflow new-fields tests -----------------------------------

    #[test]
    fn format_workflow_includes_depends_on() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "setup"
            shell = "echo setup"

            [[rules]]
            name = "step1"
            depends_on = ["setup"]
            shell = "echo step1"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("depends_on = [\"setup\"]"));
    }

    #[test]
    fn format_workflow_includes_retry_delay() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            retries = 3
            retry_delay = "30s"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("retry_delay = \"30s\""));
        assert!(formatted.contains("retries = 3"));
    }

    #[test]
    fn format_workflow_includes_workdir() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            workdir = "/data/scratch"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("workdir = \"/data/scratch\""));
    }

    #[test]
    fn format_workflow_includes_on_success() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            on_success = "echo done"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("on_success = \"echo done\""));
    }

    #[test]
    fn lint_warns_unlocked_environment() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            description = "test"
            author = "author"

            [[rules]]
            name = "align"
            description = "desc"
            shell = "echo hi"
            output = ["out.txt"]
            log = "log.txt"

            [rules.environment]
            conda = "envs/align.yaml"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config);
        let w016 = diagnostics.iter().find(|d| d.code == "W016");
        assert!(w016.is_some(), "should warn about unlocked conda env");
        assert!(w016.unwrap().message.contains("lockfile"));
    }

    #[test]
    fn format_workflow_includes_on_failure() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            on_failure = "notify admin"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let formatted = format_workflow(&config);
        assert!(formatted.contains("on_failure = \"notify admin\""));
    }
}
