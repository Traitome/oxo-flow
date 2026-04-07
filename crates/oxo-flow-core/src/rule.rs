//! Rule definitions for oxo-flow workflows.
//!
//! A [`Rule`] describes a single step in a bioinformatics pipeline, including
//! its inputs, outputs, shell command, resource requirements, and execution
//! environment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parse a duration string like "5s", "30s", "2m", "1h" into seconds.
///
/// Returns `None` if the format is invalid or would overflow.
#[must_use]
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(num) = s.strip_suffix('s').or_else(|| s.strip_suffix('S')) {
        return num.parse::<u64>().ok();
    }
    if let Some(num) = s.strip_suffix('m').or_else(|| s.strip_suffix('M')) {
        return num.parse::<u64>().ok().and_then(|v| v.checked_mul(60));
    }
    if let Some(num) = s.strip_suffix('h').or_else(|| s.strip_suffix('H')) {
        return num.parse::<u64>().ok().and_then(|v| v.checked_mul(3600));
    }
    None
}

/// GPU resource specification with detailed hardware requirements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GpuSpec {
    /// Number of GPUs required.
    #[serde(default)]
    pub count: u32,
    /// GPU model constraint (e.g., "A100", "V100", "RTX3090").
    #[serde(default)]
    pub model: Option<String>,
    /// Minimum GPU memory in GB (e.g., 40 for A100-40GB).
    #[serde(default)]
    pub memory_gb: Option<u32>,
    /// Minimum compute capability (e.g., "7.0" for V100).
    #[serde(default)]
    pub compute_capability: Option<String>,
}

/// Hints for resource estimation when exact requirements are unknown.
///
/// Allows workflow authors to provide scaling factors and size estimates
/// that the scheduler can use to dynamically allocate resources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ResourceHint {
    /// Estimated input size category: "small" (<1GB), "medium" (1-100GB), "large" (>100GB).
    #[serde(default)]
    pub input_size: Option<String>,
    /// Memory scaling factor relative to input size (e.g., 2.0 means 2x input size).
    #[serde(default)]
    pub memory_scale: Option<f64>,
    /// Estimated runtime category: "fast" (<10min), "medium" (10min-1h), "slow" (>1h).
    #[serde(default)]
    pub runtime: Option<String>,
    /// Whether this step is I/O bound (true) or CPU bound (false).
    #[serde(default)]
    pub io_bound: Option<bool>,
}

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

    /// Detailed GPU specification (alternative to simple `gpu` count).
    #[serde(default)]
    pub gpu_spec: Option<GpuSpec>,

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
            gpu_spec: None,
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

    /// HPC software modules to load (e.g., ["gcc/11.2", "cuda/11.7"]).
    #[serde(default)]
    pub modules: Vec<String>,
}

impl EnvironmentSpec {
    /// Returns `true` if no environment is specified.
    pub fn is_empty(&self) -> bool {
        self.conda.is_none()
            && self.pixi.is_none()
            && self.docker.is_none()
            && self.singularity.is_none()
            && self.venv.is_none()
            && self.modules.is_empty()
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
        } else if !self.modules.is_empty() {
            "modules"
        } else {
            "system"
        }
    }
}

/// Scatter configuration for fan-out parallel execution.
///
/// Distributes a rule across multiple values of a variable, executing
/// one instance per element. The gather step collects the outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScatterConfig {
    /// The variable to scatter over (e.g., "sample", "chromosome").
    pub variable: String,

    /// The values to scatter across.
    #[serde(default)]
    pub values: Vec<String>,

    /// Optional gather rule name that collects scattered outputs.
    #[serde(default)]
    pub gather: Option<String>,
}

/// A single rule (step) in a workflow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Conditional execution expression.
    ///
    /// The rule is only executed when this expression evaluates to true.
    /// Supports simple config-variable references (e.g., `config.enable_qc`)
    /// and file-existence checks.
    #[serde(default)]
    pub when: Option<String>,

    /// Scatter configuration for parallel execution across a variable.
    ///
    /// Fans out this rule into multiple parallel instances, one per element
    /// of the scatter variable.
    #[serde(default)]
    pub scatter: Option<ScatterConfig>,

    /// Temporary output files that should be cleaned up after downstream
    /// rules complete.
    #[serde(default)]
    pub temp_output: Vec<String>,

    /// Protected output files that should never be overwritten or deleted.
    #[serde(default)]
    pub protected_output: Vec<String>,

    /// Dynamic input function name for runtime input resolution.
    ///
    /// When set, inputs are resolved at execution time by calling this
    /// function with the current wildcard values.
    #[serde(default)]
    pub input_function: Option<String>,

    /// Number of times to automatically retry this rule on failure.
    #[serde(default)]
    pub retries: u32,

    /// Tags for categorization and filtering (e.g., ["qc", "alignment", "variant-calling"]).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Shadow directory mode for atomic rule execution.
    /// "minimal" copies only input files, "shallow" creates symlinks,
    /// "full" copies the entire working directory.
    #[serde(default)]
    pub shadow: Option<String>,

    /// Mark specific inputs as "ancient" - these inputs never trigger re-execution
    /// even if they are newer than outputs.
    #[serde(default)]
    pub ancient: Vec<String>,

    /// Whether this rule should always run locally (never submitted to cluster).
    #[serde(default)]
    pub localrule: bool,

    /// Environment variables to inject before running this rule.
    #[serde(default)]
    pub envvars: HashMap<String, String>,

    /// Whether this rule is a checkpoint that allows dynamic DAG modification.
    #[serde(default)]
    pub checkpoint: bool,

    /// Whether this rule is required (pipeline fails if this rule fails).
    #[serde(default)]
    pub required: bool,

    /// Explicit rule-level dependencies (rule names that must complete first).
    ///
    /// Unlike file-based dependency inference, `depends_on` allows declaring
    /// ordering constraints that are not mediated by input/output patterns.
    /// This is useful for setup/teardown steps, database migrations, or
    /// environment initialization that other rules depend on logically.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Delay between retry attempts (e.g., "5s", "30s", "2m").
    ///
    /// When `retries > 0`, this specifies the wait time before each retry.
    /// Supports seconds ("10s"), minutes ("2m"), and hours ("1h").
    #[serde(default)]
    pub retry_delay: Option<String>,

    /// Per-rule working directory override.
    ///
    /// If set, the rule executes in this directory instead of the workflow's
    /// global working directory. Relative paths are resolved against the
    /// workflow working directory.
    #[serde(default)]
    pub workdir: Option<String>,

    /// Shell command to execute on successful completion of this rule.
    ///
    /// Useful for notifications, cleanup, or triggering downstream processes.
    #[serde(default)]
    pub on_success: Option<String>,

    /// Shell command to execute when this rule fails (after all retries).
    ///
    /// Useful for cleanup, alerting, or fallback actions.
    #[serde(default)]
    pub on_failure: Option<String>,

    /// File format hints for inputs/outputs (e.g., "bam", "vcf", "h5ad", "fastq.gz").
    ///
    /// Helps the engine optimize I/O and select appropriate validation.
    #[serde(default)]
    pub format_hint: Vec<String>,

    /// Enable streaming/FIFO mode for this rule's inputs.
    ///
    /// When true, inputs may be provided via named pipes instead of regular files,
    /// enabling streaming data processing without intermediate disk I/O.
    #[serde(default)]
    pub pipe: bool,

    /// Checksum algorithm for output integrity verification ("md5", "sha256").
    ///
    /// When set, output file checksums are computed after execution and stored
    /// for later verification.
    #[serde(default)]
    pub checksum: Option<String>,

    /// Resource estimation hints for dynamic scheduling.
    #[serde(default)]
    pub resource_hint: Option<ResourceHint>,

    /// Arbitrary domain-specific metadata (e.g., assay type, organism, data type).
    ///
    /// This field allows workflow authors to attach structured metadata to rules
    /// for use by downstream tools, reports, or pipeline-specific logic.
    #[serde(default)]
    pub rule_metadata: HashMap<String, toml::Value>,

    /// Content-based cache key override.
    ///
    /// When set, the scheduler uses this key (along with input checksums) to
    /// determine if cached outputs can be reused, enabling content-addressed caching.
    #[serde(default)]
    pub cache_key: Option<String>,
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
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("rule name cannot be empty or whitespace-only".to_string());
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
        if let Some(ref mem) = self.memory {
            let mem_trimmed = mem.trim();
            if !mem_trimmed.is_empty() {
                // Must end with a valid unit suffix and have a numeric prefix
                let valid = mem_trimmed
                    .strip_suffix(['G', 'g', 'M', 'm', 'K', 'k', 'T', 't'])
                    .and_then(|num_part| num_part.parse::<f64>().ok())
                    .map(|v| v > 0.0)
                    .unwrap_or(false);
                if !valid {
                    return Err(format!(
                        "rule '{}' has invalid memory format '{}' (expected e.g. \"8G\", \"16384M\", \"1T\")",
                        self.name, mem
                    ));
                }
            }
        }
        // Validate retry_delay format if present
        if let Some(ref delay) = self.retry_delay
            && parse_duration_secs(delay).is_none()
        {
            return Err(format!(
                "rule '{}' has invalid retry_delay '{}' (expected e.g. \"5s\", \"30s\", \"2m\", \"1h\")",
                self.name, delay
            ));
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

/// Builder for constructing [`Rule`] instances safely with method chaining.
///
/// # Example
/// ```
/// # use oxo_flow_core::rule::RuleBuilder;
/// let rule = RuleBuilder::new("align")
///     .input(vec!["reads.fastq.gz".into()])
///     .output(vec!["aligned.bam".into()])
///     .shell("bwa mem ref.fa {input} > {output}")
///     .threads(16)
///     .memory("32G")
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct RuleBuilder {
    rule: Rule,
}

impl RuleBuilder {
    /// Create a new builder with the given rule name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            rule: Rule {
                name: name.into(),
                ..Default::default()
            },
        }
    }

    /// Set the input file patterns.
    #[must_use]
    pub fn input(mut self, input: Vec<String>) -> Self {
        self.rule.input = input;
        self
    }

    /// Set the output file patterns.
    #[must_use]
    pub fn output(mut self, output: Vec<String>) -> Self {
        self.rule.output = output;
        self
    }

    /// Set the shell command template.
    #[must_use]
    pub fn shell(mut self, shell: impl Into<String>) -> Self {
        self.rule.shell = Some(shell.into());
        self
    }

    /// Set the number of threads.
    #[must_use]
    pub fn threads(mut self, threads: u32) -> Self {
        self.rule.threads = Some(threads);
        self
    }

    /// Set the memory requirement (e.g., "8G", "16G").
    #[must_use]
    pub fn memory(mut self, memory: impl Into<String>) -> Self {
        self.rule.memory = Some(memory.into());
        self
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.rule.description = Some(description.into());
        self
    }

    /// Set priority (higher = run first).
    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.rule.priority = priority;
        self
    }

    /// Set the environment specification.
    #[must_use]
    pub fn environment(mut self, env: EnvironmentSpec) -> Self {
        self.rule.environment = env;
        self
    }

    /// Set the number of retries on failure.
    #[must_use]
    pub fn retries(mut self, retries: u32) -> Self {
        self.rule.retries = retries;
        self
    }

    /// Add tags for categorization.
    #[must_use]
    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.rule.tags = tags;
        self
    }

    /// Mark as a local-only rule (skip cluster submission).
    #[must_use]
    pub fn localrule(mut self, local: bool) -> Self {
        self.rule.localrule = local;
        self
    }

    /// Set the conditional execution expression.
    #[must_use]
    pub fn when(mut self, when: impl Into<String>) -> Self {
        self.rule.when = Some(when.into());
        self
    }

    /// Mark this rule as required (pipeline fails if this rule fails).
    #[must_use]
    pub fn required(mut self, required: bool) -> Self {
        self.rule.required = required;
        self
    }

    /// Set explicit rule-level dependencies.
    #[must_use]
    pub fn depends_on(mut self, deps: Vec<String>) -> Self {
        self.rule.depends_on = deps;
        self
    }

    /// Set the delay between retry attempts (e.g., "5s", "30s", "2m").
    #[must_use]
    pub fn retry_delay(mut self, delay: impl Into<String>) -> Self {
        self.rule.retry_delay = Some(delay.into());
        self
    }

    /// Set the per-rule working directory.
    #[must_use]
    pub fn workdir(mut self, workdir: impl Into<String>) -> Self {
        self.rule.workdir = Some(workdir.into());
        self
    }

    /// Set the on-success hook command.
    #[must_use]
    pub fn on_success(mut self, cmd: impl Into<String>) -> Self {
        self.rule.on_success = Some(cmd.into());
        self
    }

    /// Set the on-failure hook command.
    #[must_use]
    pub fn on_failure(mut self, cmd: impl Into<String>) -> Self {
        self.rule.on_failure = Some(cmd.into());
        self
    }

    /// Set file format hints.
    #[must_use]
    pub fn format_hint(mut self, hints: Vec<String>) -> Self {
        self.rule.format_hint = hints;
        self
    }

    /// Enable streaming/FIFO mode.
    #[must_use]
    pub fn pipe(mut self, pipe: bool) -> Self {
        self.rule.pipe = pipe;
        self
    }

    /// Set checksum algorithm for output verification.
    #[must_use]
    pub fn checksum(mut self, algorithm: impl Into<String>) -> Self {
        self.rule.checksum = Some(algorithm.into());
        self
    }

    /// Set resource estimation hints.
    #[must_use]
    pub fn resource_hint(mut self, hint: ResourceHint) -> Self {
        self.rule.resource_hint = Some(hint);
        self
    }

    /// Set domain-specific metadata.
    #[must_use]
    pub fn rule_metadata(mut self, metadata: HashMap<String, toml::Value>) -> Self {
        self.rule.rule_metadata = metadata;
        self
    }

    /// Set content-based cache key.
    #[must_use]
    pub fn cache_key(mut self, key: impl Into<String>) -> Self {
        self.rule.cache_key = Some(key.into());
        self
    }

    /// Set full resource specification.
    #[must_use]
    pub fn resources(mut self, resources: Resources) -> Self {
        self.rule.resources = resources;
        self
    }

    /// Set the script file path.
    #[must_use]
    pub fn script(mut self, script: impl Into<String>) -> Self {
        self.rule.script = Some(script.into());
        self
    }

    /// Set the log file path.
    #[must_use]
    pub fn log(mut self, log: impl Into<String>) -> Self {
        self.rule.log = Some(log.into());
        self
    }

    /// Set the benchmark file path.
    #[must_use]
    pub fn benchmark(mut self, benchmark: impl Into<String>) -> Self {
        self.rule.benchmark = Some(benchmark.into());
        self
    }

    /// Set rule parameters.
    #[must_use]
    pub fn params(mut self, params: HashMap<String, toml::Value>) -> Self {
        self.rule.params = params;
        self
    }

    /// Set target flag.
    #[must_use]
    pub fn target(mut self, target: bool) -> Self {
        self.rule.target = target;
        self
    }

    /// Set group label.
    #[must_use]
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.rule.group = Some(group.into());
        self
    }

    /// Set scatter configuration.
    #[must_use]
    pub fn scatter(mut self, scatter: ScatterConfig) -> Self {
        self.rule.scatter = Some(scatter);
        self
    }

    /// Set temporary output files.
    #[must_use]
    pub fn temp_output(mut self, temp: Vec<String>) -> Self {
        self.rule.temp_output = temp;
        self
    }

    /// Set protected output files.
    #[must_use]
    pub fn protected_output(mut self, protected: Vec<String>) -> Self {
        self.rule.protected_output = protected;
        self
    }

    /// Set dynamic input function.
    #[must_use]
    pub fn input_function(mut self, func: impl Into<String>) -> Self {
        self.rule.input_function = Some(func.into());
        self
    }

    /// Set shadow directory mode.
    #[must_use]
    pub fn shadow(mut self, shadow: impl Into<String>) -> Self {
        self.rule.shadow = Some(shadow.into());
        self
    }

    /// Set ancient inputs.
    #[must_use]
    pub fn ancient(mut self, ancient: Vec<String>) -> Self {
        self.rule.ancient = ancient;
        self
    }

    /// Set environment variables.
    #[must_use]
    pub fn envvars(mut self, envvars: HashMap<String, String>) -> Self {
        self.rule.envvars = envvars;
        self
    }

    /// Set checkpoint flag.
    #[must_use]
    pub fn checkpoint(mut self, checkpoint: bool) -> Self {
        self.rule.checkpoint = checkpoint;
        self
    }

    /// Build the [`Rule`], consuming the builder.
    #[must_use]
    pub fn build(self) -> Rule {
        self.rule
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        let err = rule.validate().unwrap_err();
        assert!(err.contains("empty or whitespace-only"));
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

    #[test]
    fn rule_when_conditional() {
        let toml_str = r#"
            name = "optional_step"
            output = ["opt.txt"]
            shell = "echo opt"
            when = "config.enable_qc"
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.when.as_deref(), Some("config.enable_qc"));
    }

    #[test]
    fn rule_scatter_config() {
        let toml_str = r#"
            name = "per_sample"
            input = ["{sample}.bam"]
            output = ["{sample}.vcf"]
            shell = "call {input} > {output}"

            [scatter]
            variable = "sample"
            values = ["S1", "S2", "S3"]
            gather = "merge_vcf"
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        let scatter = rule.scatter.as_ref().unwrap();
        assert_eq!(scatter.variable, "sample");
        assert_eq!(scatter.values, vec!["S1", "S2", "S3"]);
        assert_eq!(scatter.gather.as_deref(), Some("merge_vcf"));
    }

    #[test]
    fn rule_temp_and_protected_outputs() {
        let toml_str = r#"
            name = "align"
            input = ["reads.fq"]
            output = ["sorted.bam"]
            shell = "bwa mem ref reads.fq | samtools sort -o sorted.bam"
            temp_output = ["unsorted.bam"]
            protected_output = ["sorted.bam"]
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.temp_output, vec!["unsorted.bam"]);
        assert_eq!(rule.protected_output, vec!["sorted.bam"]);
    }

    #[test]
    fn rule_input_function() {
        let toml_str = r#"
            name = "dynamic"
            output = ["result.txt"]
            shell = "process {input} > {output}"
            input_function = "get_inputs"
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.input_function.as_deref(), Some("get_inputs"));
    }

    #[test]
    fn rule_retries() {
        let toml_str = r#"
            name = "flaky"
            output = ["out.txt"]
            shell = "maybe_fail > out.txt"
            retries = 3
        "#;

        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.retries, 3);
    }

    #[test]
    fn scatter_config_deserialization() {
        let toml_str = r#"
            variable = "chr"
            values = ["chr1", "chr2", "chr3"]
        "#;

        let scatter: ScatterConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(scatter.variable, "chr");
        assert_eq!(scatter.values.len(), 3);
        assert!(scatter.gather.is_none());
    }

    #[test]
    fn rule_default_new_fields() {
        let rule = Rule::default();
        assert!(rule.when.is_none());
        assert!(rule.scatter.is_none());
        assert!(rule.temp_output.is_empty());
        assert!(rule.protected_output.is_empty());
        assert!(rule.input_function.is_none());
        assert_eq!(rule.retries, 0);
        assert!(rule.tags.is_empty());
    }

    #[test]
    fn validate_whitespace_only_name() {
        let rule = Rule {
            name: "  ".to_string(),
            ..Default::default()
        };
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_valid_memory() {
        let rule = Rule {
            name: "test".to_string(),
            memory: Some("8G".to_string()),
            ..Default::default()
        };
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn validate_invalid_memory() {
        let rule = Rule {
            name: "test".to_string(),
            memory: Some("8X".to_string()),
            ..Default::default()
        };
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_invalid_memory_no_unit() {
        let rule = Rule {
            name: "test".to_string(),
            memory: Some("abc".to_string()),
            ..Default::default()
        };
        assert!(rule.validate().is_err());
    }

    #[test]
    fn rule_with_tags() {
        let rule = Rule {
            name: "align".to_string(),
            tags: vec!["alignment".to_string(), "mapping".to_string()],
            ..Default::default()
        };
        assert_eq!(rule.tags.len(), 2);
        assert!(rule.tags.contains(&"alignment".to_string()));
    }

    #[test]
    fn rule_shadow_field() {
        let toml_str = r#"
            name = "align"
            input = ["reads.fq"]
            output = ["sorted.bam"]
            shell = "bwa mem ref reads.fq > sorted.bam"
            shadow = "minimal"
        "#;
        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.shadow.as_deref(), Some("minimal"));
    }

    #[test]
    fn rule_shadow_default_none() {
        let rule = Rule::default();
        assert!(rule.shadow.is_none());
    }

    #[test]
    fn rule_ancient_field() {
        let toml_str = r#"
            name = "call"
            input = ["ref.fa", "reads.bam"]
            output = ["variants.vcf"]
            shell = "caller ref.fa reads.bam > variants.vcf"
            ancient = ["ref.fa"]
        "#;
        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.ancient, vec!["ref.fa"]);
    }

    #[test]
    fn rule_ancient_default_empty() {
        let rule = Rule::default();
        assert!(rule.ancient.is_empty());
    }

    #[test]
    fn rule_localrule_field() {
        let toml_str = r#"
            name = "setup"
            shell = "mkdir -p output"
            localrule = true
        "#;
        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert!(rule.localrule);
    }

    #[test]
    fn rule_localrule_default_false() {
        let rule = Rule::default();
        assert!(!rule.localrule);
    }

    #[test]
    fn rule_envvars_field() {
        let toml_str = r#"
            name = "step"
            shell = "echo $MY_VAR"

            [envvars]
            MY_VAR = "hello"
            PATH_EXTRA = "/usr/local/bin"
        "#;
        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert_eq!(rule.envvars.len(), 2);
        assert_eq!(rule.envvars.get("MY_VAR").unwrap(), "hello");
    }

    #[test]
    fn rule_envvars_default_empty() {
        let rule = Rule::default();
        assert!(rule.envvars.is_empty());
    }

    #[test]
    fn rule_checkpoint_field() {
        let toml_str = r#"
            name = "discover"
            output = ["samples.txt"]
            shell = "find . -name '*.fastq' > samples.txt"
            checkpoint = true
        "#;
        let rule: Rule = toml::from_str(toml_str).unwrap();
        assert!(rule.checkpoint);
    }

    #[test]
    fn rule_checkpoint_default_false() {
        let rule = Rule::default();
        assert!(!rule.checkpoint);
    }

    #[test]
    fn rule_builder_basic() {
        let rule = RuleBuilder::new("test_rule")
            .input(vec!["input.txt".to_string()])
            .output(vec!["output.txt".to_string()])
            .shell("cat input.txt > output.txt")
            .threads(4)
            .memory("8G")
            .build();
        assert_eq!(rule.name, "test_rule");
        assert_eq!(rule.input, vec!["input.txt"]);
        assert_eq!(rule.shell, Some("cat input.txt > output.txt".to_string()));
        assert_eq!(rule.effective_threads(), 4);
    }

    #[test]
    fn rule_builder_with_all_options() {
        let rule = RuleBuilder::new("complex")
            .input(vec!["a.txt".into()])
            .output(vec!["b.txt".into()])
            .shell("process a.txt b.txt")
            .priority(10)
            .retries(3)
            .tags(vec!["alignment".into()])
            .localrule(true)
            .description("A complex rule")
            .build();
        assert_eq!(rule.priority, 10);
        assert_eq!(rule.retries, 3);
        assert!(rule.localrule);
        assert_eq!(rule.tags, vec!["alignment"]);
        assert_eq!(rule.description, Some("A complex rule".to_string()));
    }

    // ---- Tests for new fields ------------------------------------------------

    #[test]
    fn depends_on_deserialization() {
        let toml = r#"
            name = "align"
            depends_on = ["setup_ref", "index"]
            shell = "bwa mem ref.fa input.fq"
        "#;
        let rule: Rule = toml::from_str(toml).unwrap();
        assert_eq!(rule.depends_on, vec!["setup_ref", "index"]);
    }

    #[test]
    fn depends_on_default_is_empty() {
        let rule = Rule::default();
        assert!(rule.depends_on.is_empty());
    }

    #[test]
    fn retry_delay_deserialization() {
        let toml = r#"
            name = "flaky_step"
            retries = 3
            retry_delay = "30s"
            shell = "curl http://example.com"
        "#;
        let rule: Rule = toml::from_str(toml).unwrap();
        assert_eq!(rule.retry_delay, Some("30s".to_string()));
    }

    #[test]
    fn retry_delay_default_is_none() {
        let rule = Rule::default();
        assert!(rule.retry_delay.is_none());
    }

    #[test]
    fn retry_delay_valid_formats() {
        for (input, expected) in [("5s", 5), ("30s", 30), ("2m", 120), ("1h", 3600)] {
            let rule = Rule {
                name: "test".to_string(),
                retry_delay: Some(input.to_string()),
                ..Default::default()
            };
            assert!(rule.validate().is_ok());
            assert_eq!(parse_duration_secs(input), Some(expected));
        }
    }

    #[test]
    fn retry_delay_invalid_format_rejected_by_validate() {
        let rule = Rule {
            name: "test".to_string(),
            retry_delay: Some("5x".to_string()),
            ..Default::default()
        };
        let err = rule.validate().unwrap_err();
        assert!(err.contains("invalid retry_delay"));
    }

    #[test]
    fn workdir_deserialization() {
        let toml = r#"
            name = "compile"
            workdir = "/data/scratch"
            shell = "make all"
        "#;
        let rule: Rule = toml::from_str(toml).unwrap();
        assert_eq!(rule.workdir, Some("/data/scratch".to_string()));
    }

    #[test]
    fn workdir_default_is_none() {
        let rule = Rule::default();
        assert!(rule.workdir.is_none());
    }

    #[test]
    fn on_success_deserialization() {
        let toml = r#"
            name = "qc"
            on_success = "echo QC passed"
            shell = "fastqc input.fq"
        "#;
        let rule: Rule = toml::from_str(toml).unwrap();
        assert_eq!(rule.on_success, Some("echo QC passed".to_string()));
    }

    #[test]
    fn on_success_default_is_none() {
        let rule = Rule::default();
        assert!(rule.on_success.is_none());
    }

    #[test]
    fn on_failure_deserialization() {
        let toml = r#"
            name = "align"
            on_failure = "notify admin"
            shell = "bwa mem ref.fa input.fq"
        "#;
        let rule: Rule = toml::from_str(toml).unwrap();
        assert_eq!(rule.on_failure, Some("notify admin".to_string()));
    }

    #[test]
    fn on_failure_default_is_none() {
        let rule = Rule::default();
        assert!(rule.on_failure.is_none());
    }

    // ---- parse_duration_secs tests -------------------------------------------

    #[test]
    fn parse_duration_secs_seconds() {
        assert_eq!(parse_duration_secs("5s"), Some(5));
        assert_eq!(parse_duration_secs("30S"), Some(30));
        assert_eq!(parse_duration_secs("0s"), Some(0));
    }

    #[test]
    fn parse_duration_secs_minutes() {
        assert_eq!(parse_duration_secs("2m"), Some(120));
        assert_eq!(parse_duration_secs("10M"), Some(600));
    }

    #[test]
    fn parse_duration_secs_hours() {
        assert_eq!(parse_duration_secs("1h"), Some(3600));
        assert_eq!(parse_duration_secs("2H"), Some(7200));
    }

    #[test]
    fn parse_duration_secs_invalid() {
        assert_eq!(parse_duration_secs("5x"), None);
        assert_eq!(parse_duration_secs("abc"), None);
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("s"), None);
    }

    // ---- RuleBuilder new-field tests -----------------------------------------

    #[test]
    fn rule_builder_depends_on() {
        let rule = RuleBuilder::new("align")
            .depends_on(vec!["setup".into(), "index".into()])
            .build();
        assert_eq!(rule.depends_on, vec!["setup", "index"]);
    }

    #[test]
    fn rule_builder_retry_delay() {
        let rule = RuleBuilder::new("flaky")
            .retries(3)
            .retry_delay("10s")
            .build();
        assert_eq!(rule.retry_delay, Some("10s".to_string()));
        assert_eq!(rule.retries, 3);
    }

    #[test]
    fn rule_builder_workdir() {
        let rule = RuleBuilder::new("compile").workdir("/data/scratch").build();
        assert_eq!(rule.workdir, Some("/data/scratch".to_string()));
    }

    #[test]
    fn rule_builder_on_success() {
        let rule = RuleBuilder::new("qc").on_success("echo done").build();
        assert_eq!(rule.on_success, Some("echo done".to_string()));
    }

    #[test]
    fn rule_builder_on_failure() {
        let rule = RuleBuilder::new("align").on_failure("notify admin").build();
        assert_eq!(rule.on_failure, Some("notify admin".to_string()));
    }

    #[test]
    fn rule_builder_all_new_fields() {
        let rule = RuleBuilder::new("full")
            .shell("do something")
            .depends_on(vec!["dep1".into()])
            .retry_delay("5s")
            .workdir("/work")
            .on_success("echo ok")
            .on_failure("echo fail")
            .build();
        assert_eq!(rule.depends_on, vec!["dep1"]);
        assert_eq!(rule.retry_delay, Some("5s".to_string()));
        assert_eq!(rule.workdir, Some("/work".to_string()));
        assert_eq!(rule.on_success, Some("echo ok".to_string()));
        assert_eq!(rule.on_failure, Some("echo fail".to_string()));
    }

    #[test]
    fn gpu_spec_default() {
        let spec = GpuSpec::default();
        assert_eq!(spec.count, 0);
        assert!(spec.model.is_none());
        assert!(spec.memory_gb.is_none());
        assert!(spec.compute_capability.is_none());
    }

    #[test]
    fn resource_hint_default() {
        let hint = ResourceHint::default();
        assert!(hint.input_size.is_none());
        assert!(hint.memory_scale.is_none());
        assert!(hint.io_bound.is_none());
    }

    #[test]
    fn rule_builder_with_new_fields() {
        let rule = RuleBuilder::new("test")
            .format_hint(vec!["bam".to_string(), "vcf".to_string()])
            .pipe(true)
            .checksum("sha256")
            .cache_key("v1-align")
            .build();
        assert_eq!(rule.format_hint, vec!["bam", "vcf"]);
        assert!(rule.pipe);
        assert_eq!(rule.checksum, Some("sha256".to_string()));
        assert_eq!(rule.cache_key, Some("v1-align".to_string()));
    }

    #[test]
    fn environment_spec_with_modules() {
        let env = EnvironmentSpec {
            modules: vec!["gcc/11.2".to_string(), "cuda/11.7".to_string()],
            ..Default::default()
        };
        assert!(!env.is_empty());
        assert_eq!(env.kind(), "modules");
    }

    #[test]
    fn rule_metadata_field() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "organism".to_string(),
            toml::Value::String("human".to_string()),
        );
        let rule = RuleBuilder::new("test").rule_metadata(metadata).build();
        assert!(rule.rule_metadata.contains_key("organism"));
    }
}
