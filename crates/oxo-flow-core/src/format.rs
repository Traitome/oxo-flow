//! .oxoflow file format specification, validation, formatting, and linting.
// Accesses deprecated `Rule::threads` / `Rule::memory` for linting and
// canonical formatting output.
#![allow(deprecated)]
//!
//! This module provides utilities for working with .oxoflow files beyond
//! basic TOML parsing — including deep validation, best-practice linting,
//! canonical formatting, and format version management.

use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;
use crate::rule::{EnvironmentSpec, Rule};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Matches a `wildcard.<key>` reference inside a `when` expression (the
/// per-instance pair/group binding vocabulary, including metadata keys).
static WHEN_WILDCARD_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"wildcard\.(\w+)").expect("valid when-wildcard regex"));

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
///
/// E005: references to undefined config variables across a rule's shell,
/// script, input/output paths, and `when` condition.
///
/// The single source of truth shared by `validate` (E005 diagnostics) and
/// `run`/`dry-run` (hard gate before execution, issue #142 H1): a typo'd
/// config key must never silently expand to the literal placeholder text
/// and exit 0 with wrong data. `when` conditions reference keys bare
/// (`config.enabled`), the other surfaces use the `{config.key}` brace form.
/// Best-effort recovery of the offending environment field's value for
/// the E016 message (shell_risk returns only the field name + character).
fn environment_field_value(env: &EnvironmentSpec, field: &str) -> String {
    let value = match field {
        "conda" => env.conda.as_deref(),
        "mamba" => env.mamba.as_deref(),
        "pixi" => env.pixi.as_deref(),
        "docker" => env.docker.as_deref(),
        "singularity" => env.singularity.as_deref(),
        "venv" => env.venv.as_deref(),
        "conda_prefix" => env.conda_prefix.as_deref(),
        "mamba_prefix" => env.mamba_prefix.as_deref(),
        "venv_requirements" => env.venv_requirements.as_deref(),
        _ => None,
    };
    value.unwrap_or_default().to_string()
}

pub fn undefined_config_refs(rule: &Rule, config: &WorkflowConfig) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let config_ref_re = regex::Regex::new(r"\{config\.(\w+)\}").expect("valid regex");
    let when_ref_re = regex::Regex::new(r"config\.(\w+)").expect("valid regex");

    let check = |field: &str, text: &str, diagnostics: &mut Vec<Diagnostic>| {
        for cap in config_ref_re.captures_iter(text) {
            let key = &cap[1];
            if config.get_config_value(key).is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("{field} references undefined config variable '{key}'"),
                    rule: Some(rule.name.clone()),
                    code: "E005".to_string(),
                    suggestion: Some(format!("define '{key}' in the [config] section")),
                });
            }
        }
    };

    if let Some(ref shell) = rule.shell {
        check("shell command", shell, &mut diagnostics);
    }
    if let Some(ref script) = rule.script {
        check("script path", script, &mut diagnostics);
    }
    for output in &rule.output {
        check("output path", output, &mut diagnostics);
    }
    for input in &rule.input {
        check("input path", input, &mut diagnostics);
    }
    if let Some(ref when) = rule.when {
        for cap in when_ref_re.captures_iter(when) {
            let key = &cap[1];
            if config.get_config_value(key).is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("when condition references undefined config variable '{key}'"),
                    rule: Some(rule.name.clone()),
                    code: "E005".to_string(),
                    suggestion: Some(format!("define '{key}' in the [config] section")),
                });
            }
        }
    }
    diagnostics
}

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

    // Validate each rule
    for rule in &config.rules {
        // E002: Rule validation
        if let Err(e) = rule.validate() {
            let (msg, sugg) = match e {
                crate::error::OxoFlowError::Validation {
                    message,
                    suggestion,
                    ..
                } => (message, suggestion),
                _ => (e.to_string(), None),
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: msg,
                rule: Some(rule.name.clone()),
                code: "E002".to_string(),
                suggestion: sugg,
            });
        }

        // E003: Wildcard consistency - output wildcards must appear in inputs
        // Exception: scatter rules use scatter variables, not input wildcards
        // Exception: pairs rules use {pair_id}, {experiment}, {control} wildcards
        // Exception: sample_groups rules use {group}, {sample} wildcards
        // Exception: input_groups rules bind the pattern wildcards AND the
        // `group_by` key — `group_by = "meta.<column>"` binds the column
        // name like a scatter variable (issue #227 items 3-4).
        let mut input_wildcards: Vec<String> =
            crate::wildcard::extract_wildcards_from_patterns(&rule.input.to_vec());
        for ig in &rule.input_groups {
            for wc in crate::wildcard::extract_wildcards(&ig.pattern) {
                if !input_wildcards.contains(&wc) {
                    input_wildcards.push(wc);
                }
            }
            if let Some(column) = ig.group_by.strip_prefix("meta.")
                && !column.is_empty()
                && !input_wildcards.contains(&column.to_string())
            {
                input_wildcards.push(column.to_string());
            }
        }
        let output_wildcards =
            crate::wildcard::extract_wildcards_from_patterns(&rule.output.to_vec());

        // Get scatter variable if present - scatter variables are exempt from E003
        let scatter_var = rule.scatter.as_ref().map(|s| s.variable.as_str());

        // Collect pair wildcards when pairs are defined
        let pair_wildcards: Vec<&str> = if config.pairs.is_empty() {
            Vec::new()
        } else {
            vec!["pair_id", "experiment", "control"]
        };

        // Collect sample group wildcards when sample_groups are defined
        let group_wildcards: Vec<&str> = if config.sample_groups.is_empty() {
            Vec::new()
        } else {
            vec!["group", "sample"]
        };

        // [[values]] tables declare parameter wildcards — these fan rules
        // out from the table, so they legitimately appear in outputs and
        // shells WITHOUT any input counterpart (the documented
        // "for each assembler" pattern).
        let value_wildcards: Vec<&str> = config.values.iter().map(|v| v.name.as_str()).collect();

        for wc in &output_wildcards {
            // Skip validation for scatter variable wildcards
            if scatter_var == Some(wc.as_str()) {
                continue;
            }
            // Skip validation for pair wildcards when pairs are defined
            if pair_wildcards.contains(&wc.as_str()) {
                continue;
            }
            // Skip validation for sample group wildcards when sample_groups are defined
            if group_wildcards.contains(&wc.as_str()) {
                continue;
            }
            // Skip validation for [[values]]-declared parameter wildcards
            if value_wildcards.contains(&wc.as_str()) {
                continue;
            }
            // Skip validation for transform split variables (prefixed with _)
            if wc.starts_with('_') {
                continue;
            }
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

        // E004: Memory format validation (check both rule.memory and rule.resources.memory)
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
        // Also check resources.memory
        if let Some(ref mem) = rule.resources.memory
            && crate::scheduler::parse_memory_mb(mem).is_none()
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("invalid memory specification in resources: '{}'", mem),
                rule: Some(rule.name.clone()),
                code: "E004".to_string(),
                suggestion: Some(
                    "use a valid format like \"8G\", \"16384M\", or \"1T\"".to_string(),
                ),
            });
        }

        // E005: Undefined config variable references — shared with the
        // run/dry-run pre-execution gate (issue #142 H1), so validate and
        // run cannot drift on what counts as defined.
        diagnostics.extend(undefined_config_refs(rule, config));

        // E016: shell-unsafe characters in the environment spec. Every
        // environment field is interpolated into a rendered shell line, so
        // a metacharacter in e.g. `environment.docker` would execute on
        // the host. Path fields keep `~`/`$VAR`/`{config.*}` semantics and
        // only reject the hard injection set — see
        // `EnvironmentSpec::shell_risk` for the two-tier rules.
        if let Some((field, ch)) = rule.environment.shell_risk() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!(
                    "environment.{field} contains shell-unsafe character {ch:?}: '{}'",
                    environment_field_value(&rule.environment, field),
                ),
                rule: Some(rule.name.clone()),
                code: "E016".to_string(),
                suggestion: Some(format!(
                    "remove shell metacharacters from environment.{field} — image refs and \
                     module names may only contain [A-Za-z0-9._:/@-]"
                )),
            });
        }
    }

    // E013: checkpoint rule without a re-entry manifest (issue #78 P3).
    // A warning, not an error: `checkpoint = true` predates the manifest
    // field (v0.11), so erroring here would break every pre-v0.12 workflow
    // at upgrade. The run itself fails loudly for the missing manifest, so
    // nothing executes silently.
    for rule in &config.rules {
        if rule.checkpoint && rule.checkpoint_manifest.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "checkpoint rule must declare checkpoint_manifest — the TOML file it writes at runtime to declare new re-entry values".to_string(),
                rule: Some(rule.name.clone()),
                code: "E013".to_string(),
                suggestion: Some(
                    "add `checkpoint_manifest = \"discover.toml\"` (the rule must write [reentry] sample = [...] to that file), or remove checkpoint = true".to_string(),
                ),
            });
        }
    }

    // E014: checkpoint rule parameterized by sample/group/pair wildcards
    // (bounded re-entry: checkpoint rules never re-expand themselves)
    for rule in &config.rules {
        let text = format!(
            "{} {} {}",
            rule.shell.as_deref().unwrap_or(""),
            rule.input.to_vec().join(" "),
            rule.output.to_vec().join(" ")
        );
        let sample_wildcard = text.contains("{sample}") || text.contains("{group}");
        // Pair-driven re-entry (issue #80 item 3) is bounded the same way:
        // a checkpoint rule parameterized by pair values would re-expand
        // itself when new pairs arrive.
        let pair_wildcard = text.contains("{pair_id}")
            || text.contains("{experiment}")
            || text.contains("{tumor}")
            || text.contains("{control}")
            || text.contains("{normal}")
            || text.contains("{experiment_type}")
            || text.contains("{tumor_type}");
        if rule.checkpoint && (sample_wildcard || pair_wildcard) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: "checkpoint rule cannot be parameterized by {sample}/{group}/{pair_id} — re-entry re-expansion is bounded to non-checkpoint rules".to_string(),
                rule: Some(rule.name.clone()),
                code: "E014".to_string(),
                suggestion: Some(
                    "move the wildcard into a downstream rule; the checkpoint rule itself runs once per round-0 plan".to_string(),
                ),
            });
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

    // E008: extends references non-existent rule
    for rule in &config.rules {
        if let Some(ref base) = rule.extends
            && !rule_names.contains(base.as_str())
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("extends references non-existent rule '{}'", base),
                rule: Some(rule.name.clone()),
                code: "E008".to_string(),
                suggestion: Some(format!("ensure rule '{}' is defined in the workflow", base)),
            });
        }
    }

    // E010: Check for undefined env_group references
    for rule in &config.rules {
        if let Some(ref group_name) = rule.env_group
            && !config.env_groups.contains_key(group_name)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!(
                    "Rule '{}' references undefined env_group '{}'",
                    rule.name, group_name
                ),
                rule: Some(rule.name.clone()),
                code: "E010".to_string(),
                suggestion: Some(format!(
                    "Define [env_groups.{}] or remove env_group from rule",
                    group_name
                )),
            });
        }
    }

    // E011 + W023: Shell command security checks (dangerous patterns detected at lint time)
    {
        static WARNING_PATTERNS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
            let patterns: &[(&str, &str)] = &[
                (r"\$\([^)]*\)", "command substitution via $()"),
                (r"`[^`]*`", "command substitution via backticks"),
                (
                    r"rm\s+-rf\s",
                    "recursive force removal (may be legitimate in bioinformatics)",
                ),
                (r"chmod\s+777\b", "world-writable permission change"),
                (r"\beval\s+", "eval usage"),
                (
                    r"(?:wget|curl).*?(?:\||&&).*?(?:sh|bash)",
                    "remote fetch with shell execution",
                ),
            ];
            patterns
                .iter()
                .filter_map(|(re, desc)| Regex::new(re).ok().map(|r| (r, *desc)))
                .collect()
        });

        for rule in &config.rules {
            // Collect all shell commands to check
            let mut commands: Vec<(&str, &str)> = Vec::new(); // (command, source_label)
            if let Some(ref shell) = rule.shell {
                commands.push((shell, "shell"));
            }
            if let Some(ref pre) = rule.pre_exec {
                commands.push((pre, "pre_exec"));
            }
            if let Some(ref on_ok) = rule.on_success {
                commands.push((on_ok, "on_success"));
            }
            if let Some(ref on_fail) = rule.on_failure {
                commands.push((on_fail, "on_failure"));
            }

            for (cmd, source) in &commands {
                // Blocking patterns → E011 Error
                if let Some((category, description)) = shell_blocking_pattern(cmd) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "{} command in rule '{}' matches dangerous pattern [{}]: {}",
                            source, rule.name, category, description
                        ),
                        rule: Some(rule.name.clone()),
                        code: "E011".to_string(),
                        suggestion: Some(
                            "remove dangerous shell constructs or use a script file instead"
                                .to_string(),
                        ),
                    });
                }

                // Warning patterns → W023 Warning
                for (re, description) in WARNING_PATTERNS.iter() {
                    if re.is_match(cmd) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "{} command in rule '{}' contains {}",
                                source, rule.name, description
                            ),
                            rule: Some(rule.name.clone()),
                            code: "W023".to_string(),
                            suggestion: Some(
                                "common in bioinformatics scripts; verify this is intentional"
                                    .to_string(),
                            ),
                        });
                    }
                }
            }
        }
    }

    // E009 + W020: Path safety + input existence validation
    for rule in &config.rules {
        // Check for path traversal (..) in inputs
        for input in &rule.input {
            // W020: Warn about non-existent concrete input files (non-wildcard, non-generated)
            // E010: Error only for absolute paths that don't exist (unambiguous failure)
            let is_dir_pattern = matches!(&rule.input, crate::rule::FilePatterns::Dir { .. });
            let is_concrete = !input.contains('{')
                && !input.contains('*')
                && !input.contains('?')
                && !input.contains("..")
                && !is_dir_pattern;
            if is_concrete && !std::path::Path::new(input).exists() {
                // Check if this file would be generated by an upstream rule
                let generated_by_upstream = config
                    .rules
                    .iter()
                    .any(|r| r.output.to_vec().iter().any(|o| o == input));
                if !generated_by_upstream {
                    let is_absolute = input.starts_with('/');
                    // Optional rules treat missing inputs as warnings, not errors
                    let severity = if rule.optional.is_optional() {
                        Severity::Warning
                    } else if is_absolute {
                        Severity::Error
                    } else {
                        Severity::Warning
                    };
                    diagnostics.push(Diagnostic {
                        severity,
                        message: format!("input file '{}' does not exist", input),
                        rule: Some(rule.name.clone()),
                        code: if severity == Severity::Error {
                            "E010".to_string()
                        } else {
                            "W020".to_string()
                        },
                        suggestion: Some(
                            if rule.optional.is_optional() {
                                "this rule is optional — missing input will cause it to be skipped at execution time"
                            } else if is_absolute {
                                "verify the absolute file path; this file must exist before workflow execution"
                            } else {
                                "verify the file path relative to the workflow directory or ensure it is generated upstream"
                            }
                            .to_string(),
                        ),
                    });
                }
            }
            if input.contains("..") {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "input path '{}' contains '..' which may escape the working directory",
                        input
                    ),
                    rule: Some(rule.name.clone()),
                    code: "E009".to_string(),
                    suggestion: Some(
                        "avoid using '..' in input paths; define the external directory in [config] and reference it as {{config.name}}"
                            .to_string(),
                    ),
                });
            }
            // Check for absolute paths
            if input.starts_with('/') {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("input path '{}' is an absolute path", input),
                    rule: Some(rule.name.clone()),
                    code: "W017".to_string(),
                    suggestion: Some(
                        "use relative paths within the workflow directory".to_string(),
                    ),
                });
            }
            // Check for home directory references
            if input.starts_with('~') {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("input path '{}' references home directory", input),
                    rule: Some(rule.name.clone()),
                    code: "W018".to_string(),
                    suggestion: Some(
                        "use relative paths within the workflow directory".to_string(),
                    ),
                });
            }
        }

        // Check for path traversal (..) in outputs
        for output in &rule.output {
            if output.contains("..") {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "output path '{}' contains '..' which may escape the working directory",
                        output
                    ),
                    rule: Some(rule.name.clone()),
                    code: "E009".to_string(),
                    suggestion: Some(
                        "avoid using '..' in output paths; keep all outputs within the workflow directory"
                            .to_string(),
                    ),
                });
            }
            // Check for absolute paths
            if output.starts_with('/') {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("output path '{}' is an absolute path", output),
                    rule: Some(rule.name.clone()),
                    code: "W017".to_string(),
                    suggestion: Some(
                        "use relative paths within the workflow directory".to_string(),
                    ),
                });
            }
            // Check for home directory references
            if output.starts_with('~') {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("output path '{}' references home directory", output),
                    rule: Some(rule.name.clone()),
                    code: "W018".to_string(),
                    suggestion: Some(
                        "use relative paths within the workflow directory".to_string(),
                    ),
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

/// Blocking shell patterns shared by lint (E011) and the dry-run preview
/// (issue #142 LOW) — a blocked command must never be previewed as "would
/// execute".
static BLOCKING_PATTERNS: LazyLock<Vec<(Regex, &str, &str)>> = LazyLock::new(|| {
    let patterns: &[(&str, &str, &str)] = &[
        (
            r"rm\s+-rf\s+(?:--\S+\s+)*[/~]",
            "RECURSIVE_DELETION",
            "dangerous recursive deletion of root/home",
        ),
        (
            r"rm\s+-r\s+(?:--\S+\s+)*/",
            "RECURSIVE_DELETION",
            "recursive deletion of root without force flag",
        ),
        (
            r"mkfs\.?\w*",
            "FILESYSTEM_DESTRUCTION",
            "filesystem creation command",
        ),
        (r"mkswap", "FILESYSTEM_DESTRUCTION", "swap creation command"),
        (
            r"dd\s+if=.*of=/dev/sd",
            "FILESYSTEM_DESTRUCTION",
            "dd write to block device",
        ),
        (
            r"dd\s+if=/dev/(?:zero|random|urandom)",
            "DATA_DESTRUCTION",
            "data destruction via dd from /dev",
        ),
        (
            r"chmod\s+.*777\s+/",
            "PERMISSION_ESCALATION",
            "world-writable root permission",
        ),
        (
            r"chmod\s+-R\s+777",
            "PERMISSION_ESCALATION",
            "recursive world-writable permission",
        ),
        (
            r">\s*/dev/sd[a-z]",
            "BLOCK_DEVICE_WRITE",
            "redirect to block device",
        ),
        (
            r">>\s*/dev/sd[a-z]",
            "BLOCK_DEVICE_WRITE",
            "append to block device",
        ),
        (
            r"(?:wget|curl).*\|\s*(?:sh|bash|dash)",
            "REMOTE_EXECUTION",
            "remote script piped to shell",
        ),
        (r"\(\)\s*\{.*:.*\|.*&.*\}", "FORK_BOMB", "fork bomb pattern"),
        (r":\(\)\s*\{", "FORK_BOMB", "fork bomb variant"),
    ];
    patterns
        .iter()
        .filter_map(|(re, name, desc)| Regex::new(re).ok().map(|r| (r, *name, *desc)))
        .collect()
});

/// Whether `shell` matches a blocking (E011) pattern, returning the
/// category and description of the first match.
pub fn shell_blocking_pattern(shell: &str) -> Option<(&'static str, &'static str)> {
    BLOCKING_PATTERNS
        .iter()
        .find_map(|(re, category, description)| {
            re.is_match(shell).then_some((*category, *description))
        })
}

/// Perform best-practice linting on a workflow configuration.
///
/// Checks for:
/// - Missing descriptions on rules
/// - Unused rules (no dependents and not a target)
/// - Naming convention violations
/// - Missing log files for complex rules
/// - Suboptimal resource allocations
pub fn lint_format(
    config: &WorkflowConfig,
    script_base: Option<&std::path::Path>,
) -> Vec<Diagnostic> {
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

    // Wildcard sources the engine can actually expand (W024): the same
    // trigger sets expand_wildcards fans out on (config.rs) plus group/pair
    // metadata keys and [[values]] tables — anything else referenced as
    // `{name}` stays LITERAL at run time.
    let mut declared_wildcards: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    if config.workflow.sample_pattern.is_some()
        || !config.sample_groups.is_empty()
        || !config.pairs.is_empty()
    {
        declared_wildcards.insert("sample".to_string());
    }
    if !config.sample_groups.is_empty() {
        declared_wildcards.insert("group".to_string());
        for group in &config.sample_groups {
            for key in group.metadata.keys() {
                declared_wildcards.insert(key.clone());
            }
        }
    }
    if !config.pairs.is_empty() {
        for wc in [
            "pair_id",
            "experiment",
            "control",
            "tumor",
            "normal",
            "experiment_type",
            "tumor_type",
        ] {
            declared_wildcards.insert(wc.to_string());
        }
        for pair in &config.pairs {
            for key in pair.metadata.keys() {
                declared_wildcards.insert(key.clone());
            }
        }
    }
    for table in &config.values {
        declared_wildcards.insert(table.name.clone());
    }
    // Engine placeholders substituted at execution time (process.rs) —
    // always available, never literal.
    const ENGINE_PLACEHOLDERS: [&str; 7] = [
        "input",
        "output",
        "log",
        "threads",
        "memory",
        "effective_threads",
        "effective_memory_mb",
    ];

    for rule in &config.rules {
        // W003: Missing rule description
        if rule.description.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule has no description".to_string(),
                rule: Some(rule.name.clone()),
                code: "W003".to_string(),
                suggestion: Some(format!(
                    "add description = \"Brief one-line description of what {} does\" to this rule",
                    rule.name
                )),
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

        // W025: Deprecated rule-level threads/memory keys (issue #142 M12).
        // `threads`/`memory` at rule level were superseded by
        // `resources.threads`/`resources.memory` in v0.4 and are never
        // flagged — a deprecated key that silently passes lint invites
        // new workflows to keep using it.
        if rule.threads.is_some() || rule.memory.is_some() {
            diagnostics.push(Diagnostic {
                severity: Severity::Info,
                message: "rule uses deprecated rule-level threads/memory — move them under \
                          [rules.resources]"
                    .to_string(),
                rule: Some(rule.name.clone()),
                code: "W025".to_string(),
                suggestion: Some(
                    "replace `threads = N` / `memory = \"8G\"` with \
                     `resources.threads = N` / `resources.memory = \"8G\"` under this rule"
                        .to_string(),
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

        // W019: Rule executes a command but declares no outputs. The engine's
        // correctness machinery is output-driven — freshness checks, dataflow
        // edges, failure invalidation (issue #118) — so an empty output list
        // means downstream rules can only order against this rule via an
        // explicit depends_on, and the produced files are invisible to the
        // planner (live: auto-sra's dumps declared output = [] while writing
        // the FASTQs merges consume — the missing edges fed a priority
        // starvation incident).
        //
        // Skipped when the rule has dependents — they already order against
        // it via depends_on (the only possible edge for an output-less
        // rule), so the old suggestion ("add depends_on to every consumer")
        // could never silence the warning — and when `when = "false"`: the
        // rule can never execute, so its missing outputs are moot (mirrors
        // the engine's evaluate_condition literal handling). Transform rules
        // are included — `transform.map` executes even though the rule
        // declares no outputs of its own.
        let executes = rule.shell.is_some() || rule.script.is_some() || rule.transform.is_some();
        let can_never_run = matches!(rule.when.as_deref().map(str::trim), Some("false"));
        let has_dependents = dag
            .as_ref()
            .and_then(|d| d.dependents(&rule.name).ok())
            .is_some_and(|deps| !deps.is_empty());
        if rule.output.is_empty() && executes && !can_never_run && !has_dependents {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: "rule executes a command but declares no outputs".to_string(),
                rule: Some(rule.name.clone()),
                code: "W019".to_string(),
                suggestion: Some(
                    "declare output = [...] naming the files this rule produces, or add a depends_on entry for this rule to each rule that consumes its files".to_string(),
                ),
            });
        }

        // W021: a script rule consumes another rule's declared output
        // without any ordering edge. Scripts are opaque to the DAG builder
        // — their file references form no edges — so the producer can race
        // or starve the consumer (live: auto-sra's script rules polled
        // 00_/01_/02_ directories for 90 minutes before the FIFO fair-
        // dispatch exposed the missing depends_on). The check reads each
        // script's CONTENT (relative to the workflow file) and matches the
        // literal prefix of other rules' output patterns (up to the first
        // wildcard; fully literal paths match whole). Excluded when the
        // ordering already exists (depends_on or an inferred DAG edge).
        // Without a script base dir (e.g. bare-config callers) the content
        // scan is skipped — the script path itself is still matched.
        {
            let dag = crate::dag::WorkflowDag::from_rules(&config.rules).ok();
            let has_edge = |rule: &crate::rule::Rule, producer: &str| {
                rule.depends_on.iter().any(|d| d == producer)
                    || dag.as_ref().is_some_and(|d| {
                        d.dependencies(&rule.name)
                            .map(|deps| deps.iter().any(|d| d == producer))
                            .unwrap_or(false)
                    })
            };
            for rule in &config.rules {
                let Some(script_path) = rule.script.as_deref() else {
                    continue;
                };
                let mut scanned = script_path.to_string();
                if let Some(base) = script_base
                    && let Ok(content) = std::fs::read_to_string(base.join(script_path))
                {
                    scanned.push('\n');
                    scanned.push_str(&content);
                }
                for producer in &config.rules {
                    if producer.name == rule.name || has_edge(rule, &producer.name) {
                        continue;
                    }
                    for output in &producer.output {
                        // The literal prefix up to the first wildcard; a
                        // fully literal path matches itself. Prefixes under
                        // 4 chars are too generic to flag.
                        let prefix: &str = output
                            .split(['{', '*', '['])
                            .next()
                            .unwrap_or(output)
                            .trim_end_matches('/');
                        if prefix.len() < 4 || !scanned.contains(prefix) {
                            continue;
                        }
                        diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "script of rule '{}' references output path '{}' of rule '{}' without an ordering edge",
                                rule.name, output, producer.name
                            ),
                            rule: Some(rule.name.clone()),
                            code: "W021".to_string(),
                            suggestion: Some(format!(
                                "add depends_on = [\"{}\"] to '{}' — script references form no DAG edges",
                                producer.name, rule.name
                            )),
                        });
                    }
                }
            }
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

        // W020-W022: Hook command safety checks
        let dangerous_patterns = [
            ("$(", "command substitution"),
            ("`", "backtick substitution"),
            (";", "unconditional chaining"),
            ("\n", "newline injection"),
            ("rm -rf /", "dangerous deletion"),
        ];

        // W020: pre_exec safety
        if let Some(ref hook_cmd) = rule.pre_exec {
            for (pattern, desc) in &dangerous_patterns {
                if hook_cmd.contains(pattern) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("pre_exec contains {} pattern", desc),
                        rule: Some(rule.name.clone()),
                        code: "W020".to_string(),
                        suggestion: Some(
                            "remove dangerous shell constructs from pre_exec hook".to_string(),
                        ),
                    });
                    break;
                }
            }
        }

        // W021: on_success safety
        if let Some(ref hook_cmd) = rule.on_success {
            for (pattern, desc) in &dangerous_patterns {
                if hook_cmd.contains(pattern) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("on_success contains {} pattern", desc),
                        rule: Some(rule.name.clone()),
                        code: "W021".to_string(),
                        suggestion: Some(
                            "remove dangerous shell constructs from on_success hook".to_string(),
                        ),
                    });
                    break;
                }
            }
        }

        // W022: on_failure safety
        if let Some(ref hook_cmd) = rule.on_failure {
            for (pattern, desc) in &dangerous_patterns {
                if hook_cmd.contains(pattern) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("on_failure contains {} pattern", desc),
                        rule: Some(rule.name.clone()),
                        code: "W022".to_string(),
                        suggestion: Some(
                            "remove dangerous shell constructs from on_failure hook".to_string(),
                        ),
                    });
                    break;
                }
            }
        }

        // W027: `when` references a wildcard key no instance can ever bind
        // (issue #85 live incident: snparcher's
        // `when = "wildcard.input_type == 'srr'"` fired for a fastq cohort
        // whose group metadata declared no `input_type` — the unbound
        // comparison used to evaluate TRUE, running `download_sra` against
        // a literal `{accession}`). Unbound comparisons now evaluate
        // false, so a key outside the bindable vocabulary (standard
        // pair/group keys, declared metadata, [[values]] names) makes the
        // rule never run — worth a warning, never an error: metadata can
        // arrive at run time via a pairs/sample_groups file.
        if let Some(ref when) = rule.when
            && WHEN_WILDCARD_REF_RE.is_match(when)
        {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for cap in WHEN_WILDCARD_REF_RE.captures_iter(when) {
                let key = cap[1].to_string();
                if declared_wildcards.contains(&key) || !seen.insert(key.clone()) {
                    continue;
                }
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "when references 'wildcard.{key}' but no [[pairs]]/[[sample_groups]]/[[values]] can bind it — the condition evaluates false and the rule never runs"
                    ),
                    rule: Some(rule.name.clone()),
                    code: "W027".to_string(),
                    suggestion: Some(
                        "declare the key in a [[pairs]]/[[sample_groups]] metadata table or a [[values]] table, or check the spelling".to_string(),
                    ),
                });
            }
        }

        // W024: expandable wildcard with no declared source (issue #142 H3).
        // A `{sample}`/`{group}`/pair wildcard the engine can never expand —
        // no sample_pattern, [[sample_groups]], [[pairs]], [[values]], or
        // metadata declares it — stays LITERAL at run time: the shell
        // receives the raw text and may write a file literally named
        // `out_{sample}.txt`, silently producing wrong data with exit 0.
        // MEDIUM severity: a warning, not an error — `--samples <name>`
        // can declare samples at run time, and transform split variables
        // (`_`-prefixed) are engine-managed.
        if !can_never_run {
            let text = format!(
                "{} {} {} {}",
                rule.shell.as_deref().unwrap_or(""),
                rule.script.as_deref().unwrap_or(""),
                rule.input.to_vec().join(" "),
                rule.output.to_vec().join(" ")
            );
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for wc in crate::wildcard::extract_wildcards(&text) {
                let declared_here = rule.scatter.as_ref().is_some_and(|s| s.variable == wc)
                    || ENGINE_PLACEHOLDERS.contains(&wc.as_str())
                    || declared_wildcards.contains(&wc)
                    || wc.starts_with('_');
                if declared_here || !seen.insert(wc.clone()) {
                    continue;
                }
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "wildcard '{{{wc}}}' has no declared source — it will stay literal at run time and files may be written literally named after the placeholder"
                    ),
                    rule: Some(rule.name.clone()),
                    code: "W024".to_string(),
                    suggestion: Some(
                        "declare a source for the wildcard (sample_pattern in [config], [[sample_groups]], [[pairs]], or a [[values]] table), or remove the placeholder".to_string(),
                    ),
                });
            }
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
        "pairs",
        "sample_groups",
        "plugins",
        "env_groups",
        "resource_groups",
        "reference_db",
        "wildcard_constraints",
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
    // Use serde serialization for complete roundtrip fidelity.
    // This preserves ALL sections (pairs, sample_groups, includes, env_groups,
    // resource_budget, plugins, citation, etc.) and correctly escapes
    // strings containing special characters (quotes, newlines).
    toml::to_string_pretty(config).unwrap_or_else(|e| {
        format!(
            "# Serialization error: {}\n# Falling back to inline format\n{}",
            e,
            toml::to_string(config).unwrap_or_default()
        )
    })
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

    // Compare defaults (threads/memory/environment) — silently ignoring
    // these changes hides meaningful workflow modifications.
    if a.defaults.threads != b.defaults.threads {
        diffs.push(WorkflowDiff {
            category: "defaults".to_string(),
            description: format!(
                "defaults threads changed: {:?} → {:?}",
                a.defaults.threads, b.defaults.threads
            ),
        });
    }
    if a.defaults.memory != b.defaults.memory {
        diffs.push(WorkflowDiff {
            category: "defaults".to_string(),
            description: format!(
                "defaults memory changed: {:?} → {:?}",
                a.defaults.memory, b.defaults.memory
            ),
        });
    }
    if a.defaults.environment != b.defaults.environment {
        diffs.push(WorkflowDiff {
            category: "defaults".to_string(),
            description: "defaults environment changed".to_string(),
        });
    }

    // Compare pairs by keyed membership — a swapped experiment/control or a
    // replaced pair with the same count is still a meaningful change, which
    // a count-only comparison would hide.
    fn keyed_pairs(wf: &WorkflowConfig) -> Vec<(String, String, Option<String>, Option<String>)> {
        let mut out: Vec<_> = wf
            .pairs
            .iter()
            .map(|p| {
                (
                    p.pair_id.clone(),
                    p.experiment.clone(),
                    p.control.clone(),
                    p.experiment_type.clone(),
                )
            })
            .collect();
        out.sort();
        out
    }
    let a_pairs = keyed_pairs(a);
    let b_pairs = keyed_pairs(b);
    if a_pairs != b_pairs {
        diffs.push(WorkflowDiff {
            category: "pairs".to_string(),
            description: format!("pairs changed: {:?} → {:?}", a_pairs, b_pairs),
        });
    }

    // Sample groups compared by full (sorted) membership, not just counts.
    fn keyed_groups(wf: &WorkflowConfig) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<_> = wf
            .sample_groups
            .iter()
            .map(|g| {
                let mut samples = g.samples.clone();
                samples.sort();
                (g.name.clone(), samples)
            })
            .collect();
        out.sort();
        out
    }
    let a_groups = keyed_groups(a);
    let b_groups = keyed_groups(b);
    if a_groups != b_groups {
        diffs.push(WorkflowDiff {
            category: "sample_groups".to_string(),
            description: format!("sample groups changed: {:?} → {:?}", a_groups, b_groups),
        });
    }

    diffs
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_blocking_pattern_detects_e011_only() {
        assert!(shell_blocking_pattern("rm -rf /").is_some());
        assert!(shell_blocking_pattern("rm -rf ~").is_some());
        assert!(shell_blocking_pattern("rm -rf safe_dir").is_none());
        assert!(shell_blocking_pattern("echo hello").is_none());
    }

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
    fn validate_rejects_injection_in_docker_spec() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "r1"
            output = ["o.txt"]
            shell = "echo hi > {output[0]}"

            [rules.environment]
            docker = "alpine ; touch HOSTPWN ; echo x"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(
            !result.valid,
            "E016 must reject shell metacharacters in environment.docker"
        );
        assert!(result.errors().iter().any(|d| d.code == "E016"));
    }

    #[test]
    fn validate_checkpoint_without_manifest_is_error() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "discover"
            output = ["d.done"]
            shell = "true"
            checkpoint = true
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        // Warning, not error: pre-v0.12 workflows carry checkpoint=true
        // without a manifest and must keep validating through the upgrade.
        assert!(result.warnings().iter().any(|d| d.code == "E013"));
        assert!(!result.errors().iter().any(|d| d.code == "E013"));
    }

    #[test]
    fn validate_checkpoint_with_sample_wildcard_is_error() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "discover"
            output = ["d.done"]
            shell = "touch d_{sample}.done"
            checkpoint = true
            checkpoint_manifest = "d.toml"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.errors().iter().any(|d| d.code == "E014"));
    }

    #[test]
    fn validate_checkpoint_with_manifest_passes() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "discover"
            output = ["d.done"]
            shell = "true"
            checkpoint = true
            checkpoint_manifest = "d.toml"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(
            result
                .errors()
                .iter()
                .all(|d| d.code != "E013" && d.code != "E014")
        );
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
    fn validate_wildcard_consistency_exempts_values_tables() {
        // The documented [[values]] fan-out pattern: the parameter
        // wildcard appears in outputs and shells WITHOUT an input
        // counterpart — it must not trigger E003.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "assemble"
            input = ["reads/{sample}.fq"]
            output = ["assemblies/{assembler}/{sample}/contigs.fa"]
            shell = "{assembler} -o {output} {input}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(
            result.errors().iter().all(|d| d.code != "E003"),
            "values-declared wildcards must be E003-exempt: {:?}",
            result.errors()
        );
    }

    #[test]
    fn validate_wildcard_consistency_exempts_input_groups() {
        // input_groups rules (issue #227 items 3-4): the pattern wildcards
        // and the group key are bound at fan-out, so outputs may reference
        // them without an `input` counterpart — group_by = "meta.antibody"
        // binds the column name like a scatter variable.
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "consensus"
            input_groups = [
                { pattern = "peaks/{sample}_peaks.broadPeak", group_by = "meta.antibody" }
            ]
            output = ["consensus/{antibody}/{antibody}.peaks.bed"]
            shell = "echo {input} > {output}"

            [[rules]]
            name = "consensus_pattern_key"
            input_groups = [
                { pattern = "consensus/{antibody}/{antibody}.peaks.bed", group_by = "{antibody}" }
            ]
            output = ["counts/{antibody}.txt"]
            shell = "wc -l {input[0]} > {output[0]}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(
            result.errors().iter().all(|d| d.code != "E003"),
            "input_groups-bound wildcards must be E003-exempt: {:?}",
            result.errors()
        );
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
        assert!(diagnostics.iter().any(|d| d.code == "W005"));
    }

    #[test]
    fn lint_undeclared_sample_wildcard_fires_w024() {
        // Issue #142 H3: `{sample}` with no sample_pattern / [[pairs]] /
        // [[sample_groups]] stays literal at run time — the lint must say
        // so instead of passing silently.
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "gen"
            input = ["in_{sample}.txt"]
            output = ["out_{sample}.txt"]
            shell = "cp {input[0]} {output[0]}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        let w024: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.code == "W024").collect();
        assert_eq!(
            w024.len(),
            1,
            "one W024 for {{sample}}, got: {diagnostics:?}"
        );
        assert!(w024[0].message.contains("sample"));
        assert_eq!(w024[0].severity, Severity::Warning);
        assert_eq!(w024[0].rule.as_deref(), Some("gen"));
        assert!(w024[0].suggestion.is_some());
    }

    #[test]
    fn lint_when_unbound_wildcard_key_fires_w027() {
        // Issue #85: a `when` referencing `wildcard.<key>` that no pair/
        // group metadata or [[values]] table can bind now evaluates false
        // (the rule never runs) — the lint must say so instead of the
        // incident repeating silently.
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "download_sra"
            output = ["raw/{sample}.fq"]
            when = "wildcard.input_type == 'srr'"
            shell = "echo hi"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        let w027: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.code == "W027").collect();
        assert_eq!(
            w027.len(),
            1,
            "one W027 for unbound 'input_type', got: {diagnostics:?}"
        );
        assert!(w027[0].message.contains("input_type"));
        assert_eq!(w027[0].severity, Severity::Warning);
        assert_eq!(w027[0].rule.as_deref(), Some("download_sra"));
    }

    #[test]
    fn lint_when_bindable_wildcard_keys_are_silent() {
        // Standard pair/group keys, declared metadata keys, and [[values]]
        // names are all bindable per instance — no W027.
        let toml = r#"
            [workflow]
            name = "test"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]
            metadata = { input_type = "srr" }

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"
            metadata = { source = "sra" }

            [[values]]
            name = "assembler"
            values = ["spades"]

            [[rules]]
            name = "sra_download"
            input = ["raw/{sample}.fq"]
            output = ["out/{sample}.fq"]
            when = "wildcard.input_type == 'srr'"
            shell = "echo {input}"

            [[rules]]
            name = "pair_step"
            input = ["reads/{pair_id}.fq"]
            output = ["out/{pair_id}.bam"]
            when = "wildcard.control != '' && wildcard.source == 'sra'"
            shell = "echo {input}"

            [[rules]]
            name = "value_step"
            input = ["x_{assembler}.txt"]
            output = ["y_{assembler}.txt"]
            when = "wildcard.assembler == 'spades'"
            shell = "echo {input}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        let w027: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.code == "W027").collect();
        assert_eq!(
            w027.len(),
            0,
            "declared metadata and values keys must not fire W027: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_declared_sample_sources_are_silent() {
        // sample_pattern, sample groups, pairs, values tables, and engine
        // placeholders all keep the wildcard expandable — no W024.
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            sample_pattern = "data/{sample}.txt"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]
            metadata = { batch = "A" }

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"
            metadata = { lane = "L1" }

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "gen"
            input = ["data/{sample}_{batch}.txt"]
            output = ["out_{sample}.txt"]
            shell = "echo {threads} {input} {output} {assembler} {experiment} {pair_id} {lane} > {log}"

            [[rules]]
            name = "split_gen"
            input = ["x_{assembler}.txt"]
            output = ["y_{assembler}.txt"]
            scatter = { variable = "assembler", values = ["spades", "megahit"] }
            shell = "echo {assembler}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W024"),
            "no W024 expected, got: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_underscore_split_variable_is_not_literal() {
        // Transform split variables (`_`-prefixed) are engine-managed —
        // never flagged as undeclared.
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "map"
            output = ["chunk_{_part}.txt"]
            shell = "echo {_part}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(!diagnostics.iter().any(|d| d.code == "W024"));
    }

    #[test]
    fn undefined_config_refs_covers_all_surfaces() {
        // The shared E005 gate (issue #142 H1): shell, script, input,
        // output, and when must all flag an unknown `{config.*}` key.
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            good = "yes"

            [[rules]]
            name = "gen"
            input = ["{config.ineed_input}.txt"]
            output = ["{config.ineed_output}.txt"]
            script = "scripts/{config.ineed_script}.sh"
            shell = "echo {config.ineed_shell}"
            when = "config.ineed_when"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = config.rules[0].clone();
        let diags = undefined_config_refs(&rule, &config);
        let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, vec!["E005"; 5], "one E005 per surface: {diags:?}");
        let joined = diags
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        for key in [
            "ineed_shell",
            "ineed_script",
            "ineed_input",
            "ineed_output",
            "ineed_when",
        ] {
            assert!(joined.contains(key), "missing {key}: {joined}");
        }
        for d in &diags {
            assert_eq!(d.rule.as_deref(), Some("gen"));
            assert!(d.suggestion.is_some());
        }
        // Defined keys never flag.
        let defined_toml = r#"
            [workflow]
            name = "test"

            [config]
            good = "yes"

            [[rules]]
            name = "gen"
            input = ["{config.good}.txt"]
            output = ["{config.good}_out.txt"]
            shell = "echo {config.good}"
            when = "config.good"
        "#;
        let defined = WorkflowConfig::parse(defined_toml).unwrap();
        assert!(undefined_config_refs(&defined.rules[0], &defined).is_empty());
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
        let diagnostics = lint_format(&config, None);
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
        // The `when` key must be defined: an undefined one silently
        // disables the rule (evaluate_condition → false) — E005, the same
        // gate the run pre-execution check enforces (issue #142 H1).
        let toml = r#"
            [workflow]
            name = "test"
            description = "desc"
            author = "me"

            [config]
            enabled = "true"

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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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

    // ---- E008: extends references non-existent rule --------------------------

    #[test]
    fn validate_e008_extends_nonexistent_rule() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            extends = "nonexistent"
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E008"));
    }

    #[test]
    fn validate_e008_extends_valid_rule_no_error() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "base_rule"
            shell = "echo base"

            [[rules]]
            name = "step1"
            extends = "base_rule"
            shell = "echo step1"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.errors().iter().any(|d| d.code == "E008"));
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
        let diagnostics = lint_format(&config, None);
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
    fn diff_defaults_change_detected() {
        let toml_a = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 2

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 4

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(
            diffs
                .iter()
                .any(|d| d.category == "defaults" && d.description.contains("threads")),
            "defaults threads change must be detected: {diffs:?}"
        );
    }

    #[test]
    fn diff_pairs_count_change_detected() {
        let toml_a = r#"
            [workflow]
            name = "test"

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[pairs]]
            pair_id = "P2"
            experiment = "E2"
            control = "C2"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(
            diffs.iter().any(|d| d.category == "pairs"),
            "pairs count change must be detected: {diffs:?}"
        );
    }

    #[test]
    fn diff_pair_membership_swap_detected() {
        let toml_a = r#"
            [workflow]
            name = "test"

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"

            [[pairs]]
            pair_id = "P1"
            experiment = "C1"
            control = "E1"

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(
            diffs.iter().any(|d| d.category == "pairs"),
            "swapping experiment/control within a pair must be detected: {diffs:?}"
        );
    }

    #[test]
    fn diff_sample_group_membership_change_detected() {
        let toml_a = r#"
            [workflow]
            name = "test"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2", "S3"]

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let toml_b = r#"
            [workflow]
            name = "test"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2", "S4"]

            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let a = WorkflowConfig::parse(toml_a).unwrap();
        let b = WorkflowConfig::parse(toml_b).unwrap();
        let diffs = diff_workflows(&a, &b);
        assert!(
            diffs.iter().any(|d| d.category == "sample_groups"),
            "replacing one sample with another (same count) must be detected: {diffs:?}"
        );
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
        let diagnostics = lint_format(&config, None);
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

    // ---- E009: Path traversal detection ----------------------------------------

    #[test]
    fn validate_e009_path_traversal_in_output() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["../../../etc/passwd"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E009"));
    }

    #[test]
    fn validate_e009_path_traversal_in_input() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["../../../data/secret.txt"]
            output = ["out.txt"]
            shell = "cat {input} > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E009"));
    }

    #[test]
    fn validate_e009_no_error_for_safe_paths() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["data/input.txt"]
            output = ["results/output.txt"]
            shell = "cat {input} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.errors().iter().any(|d| d.code == "E009"));
    }

    // ---- E010: Undefined env_group references -----------------------------------

    #[test]
    fn validate_e010_undefined_env_group() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["in.txt"]
            output = ["out.txt"]
            shell = "cat {input} > {output}"
            env_group = "undefined_group"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.valid);
        assert!(result.errors().iter().any(|d| d.code == "E010"));
    }

    #[test]
    fn validate_e010_defined_env_group_passes() {
        let toml = r#"
            [workflow]
            name = "test"

            [env_groups.qc]
            conda = "envs/qc.yaml"

            [[rules]]
            name = "step1"
            input = ["in.txt"]
            output = ["out.txt"]
            shell = "cat {input} > {output}"
            env_group = "qc"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.errors().iter().any(|d| d.code == "E010"));
    }

    // ---- W017: Absolute path warning -------------------------------------------

    #[test]
    fn validate_w017_absolute_path_in_output() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["/etc/output.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.diagnostics.iter().any(|d| d.code == "W017"));
    }

    #[test]
    fn validate_w017_absolute_path_in_input() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["/data/reference.fa"]
            output = ["out.txt"]
            shell = "cat {input} > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.diagnostics.iter().any(|d| d.code == "W017"));
    }

    #[test]
    fn validate_w017_no_warning_for_relative_paths() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["data/input.txt"]
            output = ["results/output.txt"]
            shell = "cat {input} > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.diagnostics.iter().any(|d| d.code == "W017"));
    }

    // ---- W018: Home directory warning ------------------------------------------

    #[test]
    fn validate_w018_home_directory_in_output() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["~/output.txt"]
            shell = "echo hello"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.diagnostics.iter().any(|d| d.code == "W018"));
    }

    #[test]
    fn validate_w018_home_directory_in_input() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["~/.ssh/id_rsa"]
            output = ["out.txt"]
            shell = "cat {input} > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(result.diagnostics.iter().any(|d| d.code == "W018"));
    }

    #[test]
    fn validate_w018_no_warning_for_relative_paths() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            input = ["data/input.txt"]
            output = ["results/output.txt"]
            shell = "cat {input} > out.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let result = validate_format(&config);
        assert!(!result.diagnostics.iter().any(|d| d.code == "W018"));
    }

    // ---- Secret scanning patterns ----------------------------------------------

    #[test]
    fn secret_scanning_detects_stripe_key() {
        let diags = scan_for_secrets("api_key = sk-test123456789");
        assert!(diags.iter().any(|d| d.message.contains("Stripe")));
    }

    #[test]
    fn secret_scanning_detects_github_token() {
        let diags = scan_for_secrets("token = ghp_1234567890abcdef");
        assert!(diags.iter().any(|d| d.message.contains("GitHub")));
    }

    #[test]
    fn secret_scanning_detects_gitlab_token() {
        let diags = scan_for_secrets("token = glpat-1234567890abcdef");
        assert!(diags.iter().any(|d| d.message.contains("GitLab")));
    }

    #[test]
    fn secret_scanning_detects_password_pattern() {
        let diags = scan_for_secrets("password = my_secret_password");
        assert!(diags.iter().any(|d| d.message.contains("password")));
    }

    #[test]
    fn secret_scanning_detects_api_key_pattern() {
        let diags = scan_for_secrets("api_key = AKIAIOSFODNN7EXAMPLE");
        assert!(diags.iter().any(|d| d.message.contains("API key")));
    }

    // ---- W020-W022: Hook command safety -----------------------------------------

    #[test]
    fn lint_w020_pre_exec_dangerous_pattern() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            pre_exec = "echo $(whoami)"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(diagnostics.iter().any(|d| d.code == "W020"));
    }

    #[test]
    fn lint_w020_pre_exec_safe_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            pre_exec = "mkdir -p output"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(!diagnostics.iter().any(|d| d.code == "W021"));
    }

    #[test]
    fn lint_w021_on_success_dangerous_pattern() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            on_success = "ls; rm -rf /"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(diagnostics.iter().any(|d| d.code == "W021"));
    }

    #[test]
    fn lint_w021_on_success_safe_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            on_success = "echo completed"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(!diagnostics.iter().any(|d| d.code == "W021"));
    }

    #[test]
    fn lint_w022_on_failure_dangerous_pattern() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            retries = 1
            on_failure = "curl http://x.com`whoami`"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(diagnostics.iter().any(|d| d.code == "W022"));
    }

    #[test]
    fn lint_w022_on_failure_safe_no_warning() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            retries = 1
            on_failure = "notify admin"
            output = ["out.txt"]
            shell = "process data"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(!diagnostics.iter().any(|d| d.code == "W022"));
    }

    #[test]
    fn lint_executes_without_declared_outputs() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "produce"
            shell = "echo data > sra/x.fastq"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            diagnostics.iter().any(|d| d.code == "W019"),
            "a rule executing a command with no declared outputs must warn: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_w021_script_content_referencing_output_without_edge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(
            dir.path().join("scripts/merge.py"),
            "for f in 00_fastq/*.fastq.gz:\n    merge(f)\n",
        )
        .unwrap();
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "dump"
            output = ["00_fastq/{sample}.fastq.gz"]
            shell = "echo hi > 00_fastq/x.fastq.gz"

            [[rules]]
            name = "merge"
            script = "scripts/merge.py"
            output = ["merged.fastq.gz"]
        "#;
        let config: WorkflowConfig = toml::from_str(toml).unwrap();
        let diagnostics = lint_format(&config, Some(dir.path()));
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "W021" && d.rule.as_deref() == Some("merge")),
            "script content referencing dump's output dir must warn: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_no_w021_when_ordering_edge_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(
            dir.path().join("scripts/merge.py"),
            "for f in 00_fastq/*.fastq.gz:\n    merge(f)\n",
        )
        .unwrap();
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "dump"
            output = ["00_fastq/{sample}.fastq.gz"]
            shell = "echo hi > 00_fastq/x.fastq.gz"

            [[rules]]
            name = "merge"
            depends_on = ["dump"]
            script = "scripts/merge.py"
            output = ["merged.fastq.gz"]
        "#;
        let config: WorkflowConfig = toml::from_str(toml).unwrap();
        let diagnostics = lint_format(&config, Some(dir.path()));
        assert!(
            !diagnostics.iter().any(|d| d.code == "W021"),
            "an existing depends_on edge must silence W020: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_no_w019_for_declared_outputs() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "produce"
            output = ["x.fastq"]
            shell = "echo data > x.fastq"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(!diagnostics.iter().any(|d| d.code == "W019"));
    }

    #[test]
    fn lint_no_w019_for_rule_with_dependents() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "produce"
            shell = "echo data > sra/x.fastq"

            [[rules]]
            name = "consume"
            input = ["sra/x.fastq"]
            output = ["out.txt"]
            shell = "cat sra/x.fastq > out.txt"
            depends_on = ["produce"]
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W019"),
            "an output-less rule with dependents already orders against them via \
             depends_on — W019's suggestion could never silence the warning: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_no_w019_when_condition_is_false() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "never_runs"
            when = "false"
            shell = "echo data > sra/x.fastq"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W019"),
            "a rule with when = \"false\" can never execute — missing outputs are moot: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_w019_for_transform_map_without_outputs() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "split_qc"

            [rules.transform]
            split = { by = "chr", values = ["1", "2"] }
            map = "echo processing {chr}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            diagnostics.iter().any(|d| d.code == "W019"),
            "a transform rule executes `map` but declares no outputs — it must warn: {diagnostics:?}"
        );
    }

    #[test]
    fn lint_w019_suggestion_matches_new_semantics() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "produce"
            shell = "echo data > sra/x.fastq"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        let w019 = diagnostics
            .iter()
            .find(|d| d.code == "W019")
            .expect("shell rule without outputs and without dependents must warn");
        let suggestion = w019.suggestion.as_deref().unwrap_or_default();
        assert!(
            suggestion.contains("declare output = [...]"),
            "the suggestion must lead with declaring outputs, got: {suggestion}"
        );
        assert!(
            !suggestion.contains("every consumer"),
            "the old suggestion ('add depends_on to every consumer') could not silence \
             the warning once dependents skip it: {suggestion}"
        );
    }

    // ---- cache_key is now consulted (issue #194 §2.3) — the former W026 ----
    // lint was removed when content-addressed reuse became real; a rule
    // declaring `cache_key` must stay lint-silent.

    #[test]
    fn lint_cache_key_is_consulted_and_silent() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi > out.txt"
            cache_key = "my-content-key"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W026"),
            "W026 was removed with the cache_key implementation: {diagnostics:?}"
        );
    }

    // ---- W025: Deprecated rule-level threads/memory (issue #142 M12) ----

    #[test]
    fn lint_w025_flags_deprecated_rule_level_threads_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi > out.txt"
            threads = 4
            memory = "8G"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        let w025 = diagnostics
            .iter()
            .find(|d| d.code == "W025")
            .expect("rule-level threads/memory must be flagged (issue #142 M12)");
        assert_eq!(w025.rule.as_deref(), Some("step1"));
        let suggestion = w025.suggestion.as_deref().unwrap_or_default();
        assert!(
            suggestion.contains("resources"),
            "the suggestion must point at [rules.resources], got: {suggestion}"
        );
    }

    #[test]
    fn lint_w025_silent_for_resources_block() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi > out.txt"

            [rules.resources]
            threads = 4
            memory = "8G"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let diagnostics = lint_format(&config, None);
        assert!(
            !diagnostics.iter().any(|d| d.code == "W025"),
            "resources-block keys must not be flagged"
        );
    }
}
