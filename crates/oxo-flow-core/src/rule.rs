//! Rule definitions for oxo-flow workflows.
//!
//! A [`Rule`] describes a single step in a bioinformatics pipeline, including
//! its inputs, outputs, shell command, resource requirements, and execution
//! environment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Resource requirements for a rule execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resources {
    /// Number of CPU threads.
    #[serde(default = "default_threads")]
    pub threads: u32,

    /// Memory requirement (e.g., "8G", "16G").
    #[serde(default)]
    pub memory: Option<String>,

    /// GPU requirement (number of GPUs).
    #[serde(default)]
    pub gpu: Option<u32>,

    /// Disk space requirement (e.g., "100G").
    #[serde(default)]
    pub disk: Option<String>,

    /// Wall-time limit (e.g., "24h", "30m").
    #[serde(default)]
    pub time_limit: Option<String>,
}

fn default_threads() -> u32 {
    1
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            threads: 1,
            memory: None,
            gpu: None,
            disk: None,
            time_limit: None,
        }
    }
}

/// Environment specification for a rule.
///
/// Each rule can optionally declare the software environment it should run in.
/// Multiple environment types are supported.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EnvironmentSpec {
    /// Conda environment YAML file path.
    #[serde(default)]
    pub conda: Option<String>,

    /// Pixi environment specification.
    #[serde(default)]
    pub pixi: Option<String>,

    /// Docker image reference (e.g., "biocontainers/bwa:0.7.17").
    #[serde(default)]
    pub docker: Option<String>,

    /// Singularity/Apptainer image reference.
    #[serde(default)]
    pub singularity: Option<String>,

    /// Python venv path or requirements file.
    #[serde(default)]
    pub venv: Option<String>,
}

impl EnvironmentSpec {
    /// Returns `true` if no environment is specified.
    pub fn is_empty(&self) -> bool {
        self.conda.is_none()
            && self.pixi.is_none()
            && self.docker.is_none()
            && self.singularity.is_none()
            && self.venv.is_none()
    }

    /// Returns the primary environment kind as a string.
    pub fn kind(&self) -> &str {
        if self.conda.is_some() {
            "conda"
        } else if self.pixi.is_some() {
            "pixi"
        } else if self.docker.is_some() {
            "docker"
        } else if self.singularity.is_some() {
            "singularity"
        } else if self.venv.is_some() {
            "venv"
        } else {
            "system"
        }
    }
}

/// A single rule (step) in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Unique name for this rule.
    pub name: String,

    /// Input file patterns (may contain wildcards like `{sample}`).
    #[serde(default)]
    pub input: Vec<String>,

    /// Output file patterns (may contain wildcards).
    #[serde(default)]
    pub output: Vec<String>,

    /// Shell command template to execute.
    #[serde(default)]
    pub shell: Option<String>,

    /// Script file to execute instead of a shell command.
    #[serde(default)]
    pub script: Option<String>,

    /// Number of threads (shorthand for resources.threads).
    #[serde(default)]
    pub threads: Option<u32>,

    /// Memory requirement (shorthand for resources.memory).
    #[serde(default)]
    pub memory: Option<String>,

    /// Full resource specification.
    #[serde(default)]
    pub resources: Resources,

    /// Environment specification for this rule.
    #[serde(default)]
    pub environment: EnvironmentSpec,

    /// Log file path pattern.
    #[serde(default)]
    pub log: Option<String>,

    /// Benchmark file path pattern.
    #[serde(default)]
    pub benchmark: Option<String>,

    /// Rule parameters (arbitrary key-value pairs).
    #[serde(default)]
    pub params: HashMap<String, toml::Value>,

    /// Priority (higher = run first). Default is 0.
    #[serde(default)]
    pub priority: i32,

    /// Whether this is a target rule (should be built by default).
    #[serde(default)]
    pub target: bool,

    /// Group label for grouping jobs on cluster execution.
    #[serde(default)]
    pub group: Option<String>,

    /// Optional description of what this rule does.
    #[serde(default)]
    pub description: Option<String>,
}

impl Rule {
    /// Returns the effective thread count, preferring the shorthand `threads`
    /// field over `resources.threads`.
    pub fn effective_threads(&self) -> u32 {
        self.threads.unwrap_or(self.resources.threads)
    }

    /// Returns the effective memory requirement.
    pub fn effective_memory(&self) -> Option<&str> {
        self.memory.as_deref().or(self.resources.memory.as_deref())
    }

    /// Validate the rule for internal consistency.
    ///
    /// Checks that:
    /// - The rule name is not empty and contains only valid characters
    /// - At least shell or script is provided if outputs exist
    /// - Thread count is positive (if specified)
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.is_empty() {
            return Err("rule name cannot be empty".to_string());
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "rule name '{}' contains invalid characters (allowed: alphanumeric, _, -)",
                self.name
            ));
        }
        if !self.output.is_empty() && self.shell.is_none() && self.script.is_none() {
            return Err(format!(
                "rule '{}' has outputs but no shell command or script",
                self.name
            ));
        }
        if let Some(threads) = self.threads
            && threads == 0
        {
            return Err(format!("rule '{}' has zero threads", self.name));
        }
        Ok(())
    }

    /// Extracts wildcard names from input/output patterns.
    ///
    /// For example, `"{sample}_R{read}.fastq.gz"` yields `["sample", "read"]`.
    pub fn wildcard_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let re = regex::Regex::new(r"\{(\w+)\}").expect("valid regex");

        for pattern in self.input.iter().chain(self.output.iter()) {
            for cap in re.captures_iter(pattern) {
                let name = cap[1].to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resources() {
        let res = Resources::default();
        assert_eq!(res.threads, 1);
        assert!(res.memory.is_none());
    }

    #[test]
    fn environment_spec_empty() {
        let env = EnvironmentSpec::default();
        assert!(env.is_empty());
        assert_eq!(env.kind(), "system");
    }

    #[test]
    fn environment_spec_conda() {
        let env = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            ..Default::default()
        };
        assert!(!env.is_empty());
        assert_eq!(env.kind(), "conda");
    }

    #[test]
    fn rule_wildcard_extraction() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec!["{sample}_R{read}.fastq.gz".to_string()],
            output: vec!["{sample}.bam".to_string()],
            shell: None,
            script: None,
            threads: None,
            memory: None,
            resources: Resources::default(),
            environment: EnvironmentSpec::default(),
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
        };

        let names = rule.wildcard_names();
        assert!(names.contains(&"sample".to_string()));
        assert!(names.contains(&"read".to_string()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn effective_threads_shorthand() {
        let rule = Rule {
            name: "test".to_string(),
            input: vec![],
            output: vec![],
            shell: None,
            script: None,
            threads: Some(8),
            memory: None,
            resources: Resources {
                threads: 4,
                ..Default::default()
            },
            environment: EnvironmentSpec::default(),
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
        };

        assert_eq!(rule.effective_threads(), 8);
    }

    fn make_rule(name: &str) -> Rule {
        Rule {
            name: name.to_string(),
            input: vec![],
            output: vec![],
            shell: Some("echo hello".to_string()),
            script: None,
            threads: None,
            memory: None,
            resources: Resources::default(),
            environment: EnvironmentSpec::default(),
            log: None,
            benchmark: None,
            params: HashMap::new(),
            priority: 0,
            target: false,
            group: None,
            description: None,
        }
    }

    #[test]
    fn validate_valid_rule() {
        let rule = make_rule("good-rule_1");
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn validate_empty_name() {
        let rule = make_rule("");
        assert_eq!(rule.validate().unwrap_err(), "rule name cannot be empty");
    }

    #[test]
    fn validate_invalid_name_chars() {
        let rule = make_rule("bad name");
        let err = rule.validate().unwrap_err();
        assert!(err.contains("invalid characters"));
    }

    #[test]
    fn validate_zero_threads() {
        let mut rule = make_rule("test");
        rule.threads = Some(0);
        let err = rule.validate().unwrap_err();
        assert!(err.contains("zero threads"));
    }

    #[test]
    fn validate_outputs_without_command() {
        let mut rule = make_rule("test");
        rule.output = vec!["out.txt".to_string()];
        rule.shell = None;
        rule.script = None;
        let err = rule.validate().unwrap_err();
        assert!(err.contains("no shell command or script"));
    }

    #[test]
    fn rule_deserialize_from_toml() {
        let toml_str = r#"
            name = "fastqc"
            input = ["{sample}_R1.fastq.gz"]
            output = ["qc/{sample}_fastqc.html"]
            threads = 4
            shell = "fastqc {input} -o qc/"

            [environment]
            conda = "envs/qc.yaml"
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.name, "fastqc");
        assert_eq!(rule.effective_threads(), 4);
        assert_eq!(rule.environment.kind(), "conda");
    }
}
