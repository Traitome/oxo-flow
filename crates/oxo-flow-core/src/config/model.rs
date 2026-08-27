//! Workflow-config data model: types, state machine, shared helpers.
//! Workflow configuration and `.oxoflow` file parsing.
// Accesses deprecated `Rule::threads` / `Rule::memory` shorthand fields to
// apply defaults and expand rules.  Will be removed once the shorthand
// fields are retired.
#![allow(deprecated)]
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

pub use crate::clinical::*;
use crate::error::{OxoFlowError, Result};
use crate::rule::{EnvironmentSpec, FilePatterns, Rule};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
/// [`crate::wildcard::expand_values_namespace`].
pub(crate) static VALUES_NS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{values\.(\w+)\}").expect("valid values-namespace regex"));

/// Matches a `wildcard.<key>` reference inside a `when` expression (the
/// per-instance pair/group binding vocabulary, including metadata keys).
pub(crate) static WHEN_WILDCARD_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"wildcard\.(\w+)").expect("valid when-wildcard regex"));

pub(crate) fn is_defaults_empty(d: &Defaults) -> bool {
    d.threads.is_none() && d.memory.is_none() && d.environment.is_none()
}

/// Maximum depth for nested include directives to prevent infinite recursion.
pub(crate) const MAX_INCLUDE_DEPTH: usize = 16;

/// Strongly-typed rule name for compile-time safety.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleName(pub String);

impl std::fmt::Display for RuleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for RuleName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for RuleName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Strongly-typed wildcard pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WildcardPattern(pub String);

impl std::fmt::Display for WildcardPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for WildcardPattern {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WildcardPattern {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// How `profiles/<name>.toml` values merge into the workflow config.
///
/// Declared as `[workflow] profile_mode = "fill" | "override"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    /// Profile values only fill in keys the workflow does not set —
    /// existing workflow values always win. Default, keeps pre-1.x
    /// behavior.
    #[default]
    Fill,
    /// Profile values replace workflow values. Nested tables deep-merge
    /// recursively (keys from both sides survive); scalars and arrays
    /// replace the workflow value wholesale. Enables "cluster vs local"
    /// profile variants of the same workflow.
    Override,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Author name or organization.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Format specification version (e.g., "1.0").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,

    /// Format specification version for compatibility checking.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_version: Option<String>,

    /// Genome build (e.g., "GRCh38", "hg38", "GRCh37").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genome_build: Option<String>,

    /// Custom interpreter mappings for script file extensions.
    ///
    /// Overrides default auto-detection for specified extensions.
    /// Example: `interpreter_map = { ".m" = "octave", ".sas" = "sas" }`.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub interpreter_map: HashMap<String, String>,

    /// Path to an external file containing experiment-control pairs.
    ///
    /// Supports TSV, CSV, and JSON formats. Useful for large cohort studies
    /// with hundreds or thousands of sample pairs.
    ///
    /// # File format
    ///
    /// **TSV/CSV** (tab or comma separated):
    /// ```text
    /// pair_id    experiment    control    experiment_type
    /// CASE_001    EXP_01    CTRL_01    lung_adenocarcinoma
    /// CASE_002    EXP_02    CTRL_02    colorectal
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
    ///   {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
    /// ]
    /// ```
    ///
    /// Inline `[[pairs]]` and `pairs_file` can be used together; entries from
    /// both sources are merged.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_file: Option<String>,

    /// Path to an external file containing sample groups.
    ///
    /// Supports TSV, CSV, and JSON formats.
    ///
    /// # File format
    ///
    /// **TSV/CSV**:
    /// ```text
    /// name    samples
    /// control    CTRL_001,CTRL_002,CTRL_003
    /// case    S001,S002,S003
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"name": "control", "samples": ["CTRL_001", "CTRL_002"]},
    ///   {"name": "case", "samples": ["S001", "S002"]}
    /// ]
    /// ```
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_groups_file: Option<String>,

    /// Wildcard pattern for auto-discovering experiment-control pairs.
    ///
    /// The pattern must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards.
    /// oxo-flow scans matching files and extracts pair definitions from paths.
    ///
    /// # Example
    ///
    /// ```toml
    /// [workflow]
    /// pairs_pattern = "aligned/{pair_id}/{experiment}_vs_{control}.bam"
    /// ```
    ///
    /// For file `aligned/CASE_001/EXP_01_vs_CTRL_01.bam`, creates pair:
    /// - pair_id = CASE_001, experiment = EXP_01, control = CTRL_01
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_pattern: Option<String>,

    /// Wildcard pattern for auto-discovering samples from the filesystem.
    ///
    /// When specified, oxo-flow scans for files matching this pattern and extracts
    /// `{sample}` values automatically, eliminating the need for explicit
    /// `[[sample_groups]]` declaration.
    ///
    /// The pattern must contain the `{sample}` wildcard.
    ///
    /// # Example
    ///
    /// ```toml
    /// [workflow]
    /// sample_pattern = "raw/{sample}_R1.fastq.gz"
    /// ```
    ///
    /// For files `raw/Pt01_R1.fastq.gz`, `raw/Pt02_R1.fastq.gz`, creates sample
    /// group `auto-discovered` with samples ["Pt01", "Pt02"].
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_pattern: Option<String>,

    /// How `profiles/<name>.toml` values merge into this workflow:
    /// `"fill"` (default) fills in only unset keys, `"override"` lets the
    /// profile replace workflow values (deep merge for nested tables,
    /// scalar/array replacement).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_mode: Option<ProfileMode>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Default settings applied to all rules unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Defaults {
    /// Default thread count.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,

    /// Default memory.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,

    /// Default environment.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentSpec>,

    /// Shell prelude prepended to every rule shell command (and hooks),
    /// on its own line — e.g. `"set -euo pipefail"` for fail-fast shells
    /// (issue #92). Opt-in: empty by default, so existing workflows keep
    /// their exact command text.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_prelude: Option<String>,
}

/// Prepend a shell prelude to a command on its own line (issue #92).
///
/// Returns the command unchanged when no prelude is configured. The single
/// point all execution paths route through — local rules, hooks, reference
/// builds, and the cluster plan — so prelude semantics cannot drift.
pub fn prepend_shell_prelude(cmd: &str, prelude: Option<&str>) -> String {
    match prelude {
        Some(p) if !p.trim().is_empty() => format!("{p}\n{cmd}"),
        _ => cmd.to_string(),
    }
}

impl Defaults {
    /// Prepend the configured shell prelude to a command on its own line.
    ///
    /// Returns the command unchanged when no prelude is configured. Applied
    /// at command-build time — inside environment wrappers — so the local,
    /// container, and reference-build paths share one semantics (issue #92).
    pub fn apply_shell_prelude(&self, cmd: &str) -> String {
        prepend_shell_prelude(cmd, self.shell_prelude.as_deref())
    }
}

/// Report configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReportConfig {
    /// Report template name.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Output formats (html, pdf, json).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub format: Vec<String>,

    /// Report sections to include.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Module name for partial runs (`run --module <name>`, issue #112
    /// elasticity). Defaults to the included file's stem (`rules/20_germline.oxoflow`
    /// → `20_germline`), so existing composed workflows are addressable
    /// without changes.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Git repository the included path lives in (issue #112): `path` is
    /// resolved inside a checkout pinned at `ref`. Supports any git URL —
    /// https, ssh, file:// — with the China-mirror fallback for github.com.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,

    /// The git ref (tag/branch/commit) pinning the module version. Required
    /// when `repo` is set — versioned modules are the point.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,

    /// Interface contract (issue #112 module slice): file patterns the HOST
    /// must wire into the module. Validation fails when a concrete (no
    /// `{`-wildcard) declared input is not produced by any rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,

    /// Interface contract: files the module exposes to the host. Validation
    /// fails when a declared output is not produced by a module rule, and
    /// warns when a host rule reads a module-internal file that is NOT
    /// declared here (encapsulation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,

    /// Interface contract: defaults for config keys the module reads —
    /// filled into the host config profile-style (host values win).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, toml::Value>,
}

/// A resolved include contract, namespaced rule provenance attached.
#[derive(Debug, Clone, Default)]
pub struct ResolvedIncludeContract {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
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

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Sequential => write!(f, "sequential"),
            ExecutionMode::Parallel => write!(f, "parallel"),
        }
    }
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,

    /// Execution mode.
    #[serde(default)]
    pub mode: ExecutionMode,
}

/// Citation information for workflow reproducibility and publication.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CitationInfo {
    /// DOI reference for this workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    /// URL to the workflow repository or publication.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Authors of this workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Associated publication title.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Cluster execution profile for HPC deployment.
///
/// Declared as `[cluster]` in a workflow or in `profiles/<NAME>.toml`. Its
/// presence is what makes `run --profile <NAME>` submit to a scheduler
/// instead of executing locally (issue #74); a profile with only `[config]`
/// keeps the local path.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterProfile {
    /// Backend type (slurm, pbs, sge, lsf).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Default partition/queue.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
    /// Default account for billing.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Default wall-time limit (`24h`, `2d`, or `24:00:00`). A rule's own
    /// `time_limit` wins over this.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walltime: Option<String>,
    /// Jobs in flight at once (pending + running); submissions top up to
    /// this cap as slots free.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_submitted: Option<usize>,
    /// Maximum scheduler array size (SLURM `MaxArraySize`, commonly 1001):
    /// larger scatter groups are chunked into several arrays (issue #74
    /// phase 3).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_array_size: Option<usize>,
    /// Delay between scheduler polls, as a duration string (`30s`, `2m`).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval: Option<String>,
    /// Additional arguments passed to the scheduler.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

impl ClusterProfile {
    /// Poll interval in seconds, or `None` when unset/unparseable — the
    /// caller supplies the driver default rather than guessing here.
    pub fn poll_interval_secs(&self) -> Option<u64> {
        self.poll_interval
            .as_deref()
            .and_then(crate::rule::parse_duration_secs)
    }

    /// Fold `other` into `self` for a profile merge. `override_mode`
    /// mirrors [`ProfileMode`]: fill only sets fields this profile leaves
    /// empty, override replaces any field `other` actually sets.
    pub(crate) fn merge_from(&mut self, other: &ClusterProfile, override_mode: bool) {
        macro_rules! merge_opt {
            ($($field:ident),+ $(,)?) => {$(
                if other.$field.is_some() && (override_mode || self.$field.is_none()) {
                    self.$field = other.$field.clone();
                }
            )+};
        }
        merge_opt!(
            backend,
            partition,
            account,
            walltime,
            max_submitted,
            max_array_size,
            poll_interval
        );
        // Arrays replace wholesale in override mode, matching how
        // `deep_merge_value` treats arrays elsewhere in the profile merge.
        if !other.extra_args.is_empty() && (override_mode || self.extra_args.is_empty()) {
            self.extra_args = other.extra_args.clone();
        }
    }
}

/// A declared configuration parameter with optional metadata.
///
/// Declared in the `[config]` block of a `.oxoflow` file.  Every config key
/// becomes a CLI `--key` flag.  The declarative inline-table form adds
/// validation, help text, and type constraints:
///
/// ```toml
/// [config]
/// reference = "/data/hg38.fa"                 # bare string = default value
/// database  = { required = true, help = "BLAST database" }
/// threshold = { default = "1e-5" }
/// mode      = { default = "dna", choices = ["dna", "rna"] }
/// ```
///
/// Referenced in shells / inputs / outputs / `when` as `{config.key}`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigDef {
    /// Default value when not provided via CLI or profile.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// Whether this parameter must be set (via CLI, profile, or explicit config).
    #[serde(default)]
    pub required: bool,

    /// Human-readable help text shown in `--help`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,

    /// Mask the value as `****` in logs, `--help`, and error output.
    #[serde(default)]
    pub sensitive: bool,

    /// Expected value type for validation.
    /// One of: `"string"`, `"int"`, `"float"`, `"bool"`, `"path"`.
    #[serde(default, rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,

    /// Allowed values (requires `type = "string"`).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,

    /// Numeric range `"min..max"` (requires `type = "int"` or `"float"`).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,

    /// Path must exist on disk (requires `type = "path"`).
    #[serde(default)]
    pub must_exist: bool,
}

/// A declared reference artifact — a pre-built index or data file.
///
/// References are declared in `[[references]]` blocks. The engine checks if the
/// output exists before execution, and auto-builds it using the declared build
/// command if missing. Built references are tracked in the checkpoint state so
/// they are not rebuilt on resume.
///
/// `build` accepts either a handwritten shell command or the name of a
/// built-in builder template from [`crate::references`]:
///
/// ```toml
/// [[references]]
/// name = "genome"
/// source = "refs/genome.fa"
/// output = "refs/genome.fa.fai"
/// build = "samtools_faidx"   # expands to a canonical `samtools faidx` command
/// threads = 2
/// ```
///
/// A `build` value that is a single bare identifier (no spaces, slashes, or
/// shell syntax) is treated as a template name; unknown names are rejected
/// during validation. Handwritten shell commands pass through unchanged.
///
/// Naming standard: name the primary reference `genome` (or `transcriptome`),
/// and derived indexes `genome_faidx`, `genome_bwa_index`, `genome_star_index`,
/// … Each reference's `name` becomes a keyed config value — `config.<name>`
/// is injected as the reference's `output` path — so rules reference the
/// artifact via `{config.genome}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceDef {
    /// Unique name for this reference — used for checkpoint tracking, and
    /// injected into `[config]` as `config.<name>` = `output` so rules can
    /// reference the artifact by name.
    pub name: String,

    /// Path to the source file (e.g., genome.fa) — used for freshness checks,
    /// and as `{input}` when `build` names a builder template.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Path to the output artifact (index file or directory). Must be a path
    /// the build command actually creates — the engine skips the build when
    /// this path exists and rebuilds when it is missing.
    pub output: String,

    /// Shell command to build this reference from source, or the name of a
    /// built-in builder template (see [`crate::references`]).
    pub build: String,

    /// CPU threads for the build command.
    #[serde(default)]
    pub threads: Option<u32>,

    /// Memory limit for the build command.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,

    /// Environment that provides the build tool (e.g. `conda = "envs/bowtie2.yaml"`).
    /// Without it the build runs in the bare system shell — reference builds
    /// that need workflow tools (bowtie2-build, STAR genomeGenerate, …)
    /// must declare one, exactly like `[rules.environment]`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::rule::EnvironmentSpec>,

    /// Human-readable description.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Resource budget constraints for the entire workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceBudget {
    /// Maximum total CPU threads across all running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_threads: Option<u32>,
    /// Maximum total memory across all running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory: Option<String>,
    /// Maximum total running jobs.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<usize>,
}

/// Reference database configuration for tracking versions and provenance.
///
/// Bioinformatics workflows often depend on reference databases (genome builds,
/// annotation databases, variant databases). This section tracks versions for
/// reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceDatabase {
    /// Database name (e.g., "GRCh38", "dbSNP", "ClinVar", "COSMIC").
    pub name: String,
    /// Version string (e.g., "p14", "b156", "v99").
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// URL or path to the database.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Checksum of the database file for integrity verification.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Date when this database version was downloaded/accessed.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessed_date: Option<String>,
}

impl std::fmt::Display for ReferenceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(ref v) = self.version {
            write!(f, " v{v}")?;
        }
        Ok(())
    }
}

/// An experiment-control sample pair for comparative analysis workflows.
///
/// Each pair defines `{pair_id}`, `{experiment}`, and `{control}` wildcard
/// values.
///
/// Backward compatibility:
/// - `{tumor}` aliases `{experiment}`
/// - `{normal}` aliases `{control}`
///
/// Rules containing any of these wildcards in their `input`, `output`, or
/// `shell` fields are expanded once per pair.
///
/// # Example `.oxoflow` usage
///
/// ```toml
/// [[pairs]]
/// pair_id = "CASE_001"
/// experiment = "SAMPLE_EXP_01"
/// control    = "SAMPLE_CTRL_01"
/// experiment_type = "condition_a"
///
/// [[pairs]]
/// pair_id = "CASE_002"
/// experiment = "SAMPLE_EXP_02"
/// control    = "SAMPLE_CTRL_02"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentControlPair {
    /// Unique identifier for this pair (available as `{pair_id}`).
    pub pair_id: String,

    /// Experiment sample identifier (available as `{experiment}`).
    ///
    /// Backward-compatible TOML alias: `tumor`.
    #[serde(alias = "tumor")]
    pub experiment: String,

    /// Control sample identifier (available as `{control}`).
    ///
    /// When `None` (e.g., tumor-only CNV detection), `{control}` expands to
    /// an empty string. Rules should handle this via shell conditionals or
    /// declarative `[config]` entries.
    ///
    /// Backward-compatible TOML alias: `normal`.
    #[serde(default)]
    #[serde(alias = "normal")]
    pub control: Option<String>,

    /// Optional experiment type / cohort label (available as `{experiment_type}`).
    ///
    /// Backward-compatible TOML alias: `tumor_type`.
    #[serde(default)]
    #[serde(alias = "tumor_type")]
    pub experiment_type: Option<String>,

    /// Arbitrary key-value metadata; each key is available as a wildcard.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Backward-compatible alias; prefer [`ExperimentControlPair`].
pub type TumorNormalPair = ExperimentControlPair;

impl ExperimentControlPair {
    /// Load pairs from a TSV, CSV, or JSON file.
    ///
    /// # File format
    ///
    /// **TSV** (tab-separated, header required):
    /// ```text
    /// pair_id    experiment    control    experiment_type
    /// CASE_001    EXP_01    CTRL_01    lung_adenocarcinoma
    /// CASE_002    EXP_02    CTRL_02    colorectal
    /// ```
    ///
    /// **CSV** (comma-separated):
    /// ```text
    /// pair_id,experiment,control,experiment_type
    /// CASE_001,EXP_01,CTRL_01,lung_adenocarcinoma
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"pair_id": "CASE_001", "experiment": "EXP_01", "control": "CTRL_01"},
    ///   {"pair_id": "CASE_002", "experiment": "EXP_02", "control": "CTRL_02"}
    /// ]
    /// ```
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        let metadata = std::fs::metadata(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read metadata for pairs file: {}", e),
        })?;

        // 50MB limit to prevent OOM on accidental binary file input
        if metadata.len() > 50 * 1024 * 1024 {
            return Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "pairs file is too large ({} bytes). Maximum allowed size is 50MB.",
                    metadata.len()
                ),
            });
        }

        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read pairs file: {}", e),
        })?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match extension {
            "json" => Self::parse_json(&content, path),
            "csv" => Self::parse_csv(&content, path),
            "tsv" | "txt" | "" => Self::parse_tsv(&content, path),
            _ => Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("unsupported pairs file format: {}", extension),
            }),
        }
    }

    /// Discover pairs from a wildcard pattern by scanning the filesystem.
    ///
    /// The pattern must contain `{pair_id}`, `{experiment}`, and `{control}` wildcards.
    /// oxo-flow scans matching files and extracts wildcard values from paths.
    ///
    /// # Example patterns
    ///
    /// - `aligned/{pair_id}/{experiment}_vs_{control}.bam`
    /// - `results/{pair_id}/mutect2_{experiment}_{control}.vcf.gz`
    ///
    /// For file `aligned/CASE_001/EXP_01_vs_CTRL_01.bam`, extracts:
    /// - pair_id = CASE_001
    /// - experiment = EXP_01
    /// - control = CTRL_01
    pub fn discover_from_pattern(pattern: &str, base_dir: &Path) -> Result<Vec<Self>> {
        use crate::wildcard::{extract_wildcards, pattern_to_regex};

        // Validate pattern contains required wildcards
        if !pattern.contains("{pair_id}")
            || !pattern.contains("{experiment}")
            || !pattern.contains("{control}")
        {
            return Err(OxoFlowError::Config {
                message: format!(
                    "pairs_pattern must contain {{pair_id}}, {{experiment}}, and {{control}}: {}",
                    pattern
                ),
            });
        }

        // Get wildcard names from pattern
        let wildcard_names = extract_wildcards(pattern);

        // Build regex from pattern for matching
        let re = pattern_to_regex(pattern)?;

        // Convert pattern to glob pattern for filesystem scanning
        let glob_pattern = pattern
            .replace("{pair_id}", "*")
            .replace("{experiment}", "*")
            .replace("{control}", "*")
            .replace("{experiment_type}", "*");

        let full_glob = if glob_pattern.starts_with('/') {
            glob_pattern
        } else {
            base_dir.join(&glob_pattern).to_string_lossy().to_string()
        };

        // Scan filesystem for matching files
        let mut pairs: Vec<Self> = Vec::new();
        let mut seen_pair_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entry in glob::glob(&full_glob).map_err(|e| OxoFlowError::Config {
            message: format!("invalid glob pattern '{}': {}", full_glob, e),
        })? {
            let path = entry.map_err(|e| OxoFlowError::Config {
                message: format!("glob error: {}", e),
            })?;

            // Get relative path from base_dir for extraction
            let rel_path = path.strip_prefix(base_dir).unwrap_or(&path);
            let path_str = rel_path.to_string_lossy();

            // Extract wildcard values from the path using regex
            if let Some(captures) = re.captures(&path_str) {
                let mut wildcards: HashMap<String, String> = HashMap::new();
                for name in &wildcard_names {
                    if let Some(m) = captures.name(name) {
                        wildcards.insert(name.clone(), m.as_str().to_string());
                    }
                }

                if let Some(pair_id) = wildcards.get("pair_id") {
                    // Skip duplicates (same pair_id)
                    if seen_pair_ids.contains(pair_id) {
                        continue;
                    }
                    seen_pair_ids.insert(pair_id.clone());

                    if let Some(experiment) = wildcards.get("experiment")
                        && let Some(control) = wildcards.get("control")
                    {
                        let pair = Self {
                            pair_id: pair_id.clone(),
                            experiment: experiment.clone(),
                            control: Some(control.clone()),
                            experiment_type: wildcards.get("experiment_type").cloned(),
                            metadata: HashMap::new(),
                        };
                        pairs.push(pair);
                    }
                }
            }
        }

        if pairs.is_empty() {
            tracing::warn!(
                "pairs_pattern '{}' matched no files in {}",
                pattern,
                base_dir.display()
            );
        } else {
            tracing::info!(
                "Discovered {} pairs from pattern '{}'",
                pairs.len(),
                pattern
            );
        }

        Ok(pairs)
    }

    fn parse_json(content: &str, path: &Path) -> Result<Vec<Self>> {
        serde_json::from_str(content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid JSON pairs file: {}", e),
        })
    }

    fn parse_csv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, ',', path)
    }

    fn parse_tsv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, '\t', path)
    }

    fn parse_delimited(content: &str, delimiter: char, path: &Path) -> Result<Vec<Self>> {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter as u8)
            .has_headers(true)
            .trim(csv::Trim::All)
            .comment(Some(b'#'))
            .from_reader(content.as_bytes());

        let headers = reader
            .headers()
            .map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("pairs file is empty or has invalid headers: {}", e),
            })?
            .clone();

        let col_index: HashMap<&str, usize> =
            headers.iter().enumerate().map(|(i, h)| (h, i)).collect();

        // Required columns
        let pair_id_col = col_index
            .get("pair_id")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'pair_id' column".to_string(),
            })?;
        let experiment_col = col_index
            .get("experiment")
            .or_else(|| col_index.get("tumor"))
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'experiment' column (or 'tumor')".to_string(),
            })?;
        let control_col = col_index
            .get("control")
            .or_else(|| col_index.get("normal"))
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "pairs file missing 'control' column (or 'normal')".to_string(),
            })?;

        // Optional columns
        let experiment_type_col = col_index
            .get("experiment_type")
            .or(col_index.get("tumor_type"));

        let mut pairs = Vec::new();
        for (row_idx, result) in reader.records().enumerate() {
            let record = result.map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("error parsing row {}: {}", row_idx + 2, e),
            })?;

            let mut metadata = HashMap::new();
            for (i, header) in headers.iter().enumerate() {
                // If it's not one of the standard columns, add to metadata
                if i != *pair_id_col
                    && i != *experiment_col
                    && i != *control_col
                    && experiment_type_col.is_none_or(|&j| i != j)
                {
                    metadata.insert(header.to_string(), record.get(i).unwrap_or("").to_string());
                }
            }

            let pair = Self {
                pair_id: record.get(*pair_id_col).unwrap_or("").to_string(),
                experiment: record.get(*experiment_col).unwrap_or("").to_string(),
                control: Some(record.get(*control_col).unwrap_or("").to_string()),
                experiment_type: experiment_type_col
                    .and_then(|&i| record.get(i).map(|s| s.to_string())),
                metadata,
            };
            pairs.push(pair);
        }

        Ok(pairs)
    }
}

/// A named group of samples for cohort-level analysis.
///
/// Rules containing `{group}` or `{sample}` wildcards are expanded for every
/// (group, sample) combination across all defined groups.
///
/// # Example `.oxoflow` usage
///
/// ```toml
/// [[sample_groups]]
/// name    = "control"
/// samples = ["S001", "S002"]
///
/// [[sample_groups]]
/// name    = "case"
/// samples = ["S003", "S004"]
/// ```
/// A named list of values for arbitrary parameter wildcards (`[[values]]`).
///
/// Rules referencing `{name}` or `{values.name}` (where `name` matches a
/// table) fan out once per value — a Cartesian expansion for assembler /
/// bin-parameter style parameterization that previously forced hand-written
/// per-value rules. Multiple tables combine orthogonally with each other
/// and with `[[pairs]]` / `[[sample_groups]]`.
///
/// # Example `.oxoflow` usage
///
/// ```toml
/// [[values]]
/// name   = "assembler"
/// values = ["spades", "megahit"]
///
/// [[values]]
/// name   = "k"
/// values = ["21", "33"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ValueGroup {
    /// Wildcard name, e.g. `assembler` — usable as `{assembler}` in rule
    /// inputs, outputs, shells, and `expand_inputs` patterns, or as
    /// `{values.assembler}` in the namespaced form.
    pub name: String,

    /// The values to fan out, e.g. `["spades", "megahit"]`.
    #[serde(default)]
    pub values: Vec<String>,
}

/// Instance-name suffix for a `[[values]]` combo: one `_name_value` segment
/// per referenced table in declaration order, e.g. `_assembler_spades_k_21`.
/// Values are sanitized for shell-safe, unambiguous instance names.
pub(crate) fn value_instance_suffix(
    combo: &crate::wildcard::WildcardValues,
    tables: &[&ValueGroup],
) -> String {
    let mut suffix = String::new();
    for table in tables {
        if let Some(value) = combo.get(&table.name) {
            suffix.push('_');
            suffix.push_str(&table.name);
            suffix.push('_');
            suffix.push_str(&crate::wildcard::sanitize_instance_value(value));
        }
    }
    suffix
}

/// Expand every file pattern (List / Map / Dir) with a wildcard combo,
/// preserving the collection structure. `{values.name}` placeholders are
/// resolved alongside bare `{name}` ones.
pub(crate) fn expand_rule_patterns(
    patterns: &FilePatterns,
    combo: &crate::wildcard::WildcardValues,
) -> FilePatterns {
    let expand_one = |p: &String| -> String {
        if crate::wildcard::has_wildcards(p) || crate::wildcard::contains_values_namespace(p) {
            crate::wildcard::expand_values_namespace(
                &crate::wildcard::expand_pattern(p, combo).unwrap_or_else(|_| p.clone()),
                combo,
            )
        } else {
            p.clone()
        }
    };
    match patterns {
        FilePatterns::List(v) => FilePatterns::List(v.iter().map(expand_one).collect()),
        FilePatterns::Map(m) => {
            FilePatterns::Map(m.iter().map(|(k, v)| (k.clone(), expand_one(v))).collect())
        }
        FilePatterns::Dir { path, pattern } => FilePatterns::Dir {
            path: expand_one(path),
            pattern: pattern.clone(),
        },
    }
}

/// Expand a shell template with a wildcard combo, resolving both the bare
/// `{name}` and namespaced `{values.name}` placeholder forms.
pub(crate) fn expand_rule_shell(shell: &str, combo: &crate::wildcard::WildcardValues) -> String {
    if crate::wildcard::has_wildcards(shell) || crate::wildcard::contains_values_namespace(shell) {
        crate::wildcard::expand_values_namespace(
            &crate::wildcard::expand_pattern(shell, combo).unwrap_or_else(|_| shell.to_string()),
            combo,
        )
    } else {
        shell.to_string()
    }
}

/// Bake per-instance fan-out values into the free-text command fields
/// (issue #98): `script` plus the hook fields render through the same
/// execution-time placeholder pass as `shell`/`log`, and that pass never
/// sees pair/group/value/scatter names — so the values must be baked in
/// here. The `expand` closure matches the surrounding block's expansion
/// semantics (values-namespace-aware for the pair/group/value paths, plain
/// wildcard expansion for the scatter path).
pub(crate) fn expand_command_text_fields(
    expanded: &mut crate::rule::Rule,
    source: &crate::rule::Rule,
    expand: impl Fn(&str) -> String,
) {
    expanded.script = source.script.as_deref().map(&expand);
    expanded.pre_exec = source.pre_exec.as_deref().map(&expand);
    expanded.on_success = source.on_success.as_deref().map(&expand);
    expanded.on_failure = source.on_failure.as_deref().map(&expand);
}

/// A named collection of samples with optional metadata wildcards.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampleGroup {
    /// Group name (available as `{group}`).
    pub name: String,

    /// Sample identifiers belonging to this group (each available as `{sample}`).
    #[serde(default)]
    pub samples: Vec<String>,

    /// Arbitrary key-value metadata for the group; each key is a wildcard.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl SampleGroup {
    /// Load sample groups from a TSV, CSV, or JSON file.
    ///
    /// # File format
    ///
    /// **TSV**:
    /// ```text
    /// name    samples
    /// control    CTRL_001,CTRL_002,CTRL_003
    /// case    S001,S002,S003
    /// ```
    ///
    /// **JSON**:
    /// ```json
    /// [
    ///   {"name": "control", "samples": ["CTRL_001", "CTRL_002"]},
    ///   {"name": "case", "samples": ["S001", "S002"]}
    /// ]
    /// ```
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        let metadata = std::fs::metadata(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read metadata for sample_groups file: {}", e),
        })?;

        // 50MB limit to prevent OOM on accidental binary file input
        if metadata.len() > 50 * 1024 * 1024 {
            return Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "sample_groups file is too large ({} bytes). Maximum allowed size is 50MB.",
                    metadata.len()
                ),
            });
        }

        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("failed to read sample_groups file: {}", e),
        })?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match extension {
            "json" => Self::parse_json(&content, path),
            "csv" => Self::parse_csv(&content, path),
            "tsv" | "txt" | "" => Self::parse_tsv(&content, path),
            _ => Err(OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("unsupported sample_groups file format: {}", extension),
            }),
        }
    }

    fn parse_json(content: &str, path: &Path) -> Result<Vec<Self>> {
        serde_json::from_str(content).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid JSON sample_groups file: {}", e),
        })
    }

    fn parse_csv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, ',', path)
    }

    fn parse_tsv(content: &str, path: &Path) -> Result<Vec<Self>> {
        Self::parse_delimited(content, '\t', path)
    }

    fn parse_delimited(content: &str, delimiter: char, path: &Path) -> Result<Vec<Self>> {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter as u8)
            .has_headers(true)
            .trim(csv::Trim::All)
            .comment(Some(b'#'))
            .from_reader(content.as_bytes());

        let headers = reader
            .headers()
            .map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("sample_groups file is empty or has invalid headers: {}", e),
            })?
            .clone();

        let col_index: HashMap<&str, usize> =
            headers.iter().enumerate().map(|(i, h)| (h, i)).collect();

        let name_col = col_index.get("name").ok_or_else(|| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: "sample_groups file missing 'name' column".to_string(),
        })?;
        let samples_col = col_index
            .get("samples")
            .ok_or_else(|| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: "sample_groups file missing 'samples' column".to_string(),
            })?;

        let mut groups = Vec::new();
        for (row_idx, result) in reader.records().enumerate() {
            let record = result.map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: format!("error parsing row {}: {}", row_idx + 2, e),
            })?;

            // Samples can be comma-separated within the field
            let samples: Vec<String> = record
                .get(*samples_col)
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let group = Self {
                name: record.get(*name_col).unwrap_or("").to_string(),
                samples,
                metadata: HashMap::new(),
            };
            groups.push(group);
        }

        Ok(groups)
    }
}

/// Resource group configuration for limiting shared resources like API rate
/// limits or database connections.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ResourceGroupConfig {
    /// Maximum capacity of the resource (e.g., 10 for 10 concurrent connections).
    pub max: u32,
    /// Optional wait strategy: "queue" (default) or "fail".
    #[serde(default = "default_wait_strategy")]
    pub wait: String,
}

fn default_wait_strategy() -> String {
    "queue".to_string()
}

/// Complete workflow configuration parsed from an `.oxoflow` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// Workflow metadata.
    pub workflow: WorkflowMeta,

    /// Configuration variables — every key is a CLI `--key` flag.
    ///
    /// Supports two forms:
    ///   key = "value"              → implicit default, simple string
    ///   key = { default = "…", … } → declared with metadata (see `config_meta`)
    ///
    /// Referenced in shells / inputs / outputs / `when` as `{config.key}`.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, toml::Value>,

    /// Metadata for declarative `[config]` entries — parsed from inline-table values.
    /// Only populated for keys using the `key = { default, required, … }` form.
    /// Simple `key = "value"` entries do NOT appear here.
    #[serde(default, skip_deserializing, skip_serializing_if = "HashMap::is_empty")]
    pub config_meta: HashMap<String, ConfigDef>,

    /// Config keys injected by the engine rather than written by the user:
    /// reference keyed-config values (`config.<name>` = output) and
    /// reference_dir-derived paths. Catalog tooling (CLI `info`, the
    /// oxo-community drift gate) uses this to report only user-declared
    /// parameters. Run-time injections (samples_list / pairs_list /
    /// samples_*) are covered separately by `is_engine_injected_key`.
    #[serde(skip)]
    pub(crate) injected_config_keys: BTreeSet<String>,

    /// Declared reference artifacts — pre-built indexes and data files.
    ///
    /// The engine auto-builds any reference whose output is missing, using
    /// the declared build command. Built references are tracked in the
    /// checkpoint and not rebuilt on resume.
    #[serde(default, rename = "references")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceDef>,

    /// Base directory for reference files.
    ///
    /// When set, standard reference paths are auto-derived:
    /// - `reference_fasta` → `{reference_dir}/genome.fa`
    /// - `gene_annotation` → `{reference_dir}/genes.gtf`
    /// - `bwa_index` → `{reference_dir}/bwa/genome.fa`
    /// - `bowtie2_index` → `{reference_dir}/bowtie2/genome.fa`
    /// - `star_index` → `{reference_dir}/star`
    /// - `hisat2_index` → `{reference_dir}/hisat2/genome.fa`
    ///
    /// Explicit values in config override these defaults.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_dir: Option<String>,

    /// Default settings for all rules.
    #[serde(default)]
    #[serde(skip_serializing_if = "is_defaults_empty")]
    pub defaults: Defaults,

    /// Report configuration.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportConfig>,

    /// List of rules (pipeline steps).
    #[serde(default, rename = "rules")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,

    /// Plugin configuration for extending oxo-flow with custom types.
    #[serde(default, rename = "plugins")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<crate::plugin::PluginsConfig>,

    /// Include directives for importing rules from other workflow files.
    #[serde(default, rename = "include")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<IncludeDirective>,

    /// Explicit execution groups for sequential/parallel rule ordering.
    #[serde(default, rename = "execution_group")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub execution_groups: Vec<ExecutionGroup>,

    /// Citation information for reproducibility.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation: Option<CitationInfo>,

    /// Cluster execution profile.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterProfile>,

    /// Resource budget for the workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_budget: Option<ResourceBudget>,

    /// Shared resource groups for limiting concurrent access to APIs or databases.
    #[serde(default, rename = "resource_groups")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub resource_groups: HashMap<String, ResourceGroupConfig>,

    /// Reference database versions used by this workflow.
    #[serde(default, rename = "reference_db")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reference_databases: Vec<ReferenceDatabase>,

    /// Named environment groups for sharing environments across rules.
    ///
    /// Rules can reference these via `env_group = "name"` instead of
    /// specifying `environment` directly.
    #[serde(default, rename = "env_groups")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env_groups: HashMap<String, EnvironmentSpec>,

    /// Global wildcard constraints (regular expressions).
    /// Each key is a wildcard name, value is the regex pattern it must match.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub wildcard_constraints: HashMap<String, String>,

    /// Experiment-control sample pairs for comparative analysis workflows.
    ///
    /// Rules containing `{experiment}`, `{control}`, or `{pair_id}` wildcards
    /// are expanded once per pair by [`WorkflowConfig::expand_wildcards`].
    ///
    /// Backward compatibility:
    /// - `{tumor}` aliases `{experiment}`
    /// - `{normal}` aliases `{control}`
    #[serde(default, rename = "pairs")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<ExperimentControlPair>,

    /// Sample groups for cohort-level analysis.
    ///
    /// Rules containing `{group}` or `{sample}` wildcards are expanded for
    /// every (group, sample) combination by [`WorkflowConfig::expand_wildcards`].
    #[serde(default, rename = "sample_groups")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_groups: Vec<SampleGroup>,

    /// Named value lists for arbitrary parameter wildcards (`[[values]]`).
    ///
    /// Rules containing `{assembler}` or `{values.assembler}` (for a table
    /// named `assembler`) are expanded once per value combination by
    /// [`WorkflowConfig::expand_wildcards`], orthogonally with pairs and
    /// sample groups.
    #[serde(default, rename = "values")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<ValueGroup>,

    /// Engine-internal: the `[[values]]` bindings each expanded rule came
    /// from.
    ///
    /// Populated by [`WorkflowConfig::expand_wildcards`] (value fan-out)
    /// and consumed when resolving `expand_inputs` patterns per instance,
    /// so `{assembler}` binds to that instance's own value only. Never
    /// serialized — user TOML cannot set it.
    #[serde(skip)]
    pub expansion_values: HashMap<String, crate::wildcard::WildcardValues>,

    /// Populated by [`WorkflowConfig::expand_wildcards`] (pair fan-out)
    /// and consumed the same way: `{pair_id}` (and the other pair
    /// wildcards) inside an `expand_inputs` pattern resolve to the
    /// instance's own pair. Never serialized — user TOML cannot set it.
    #[serde(skip)]
    pub expansion_pairs: HashMap<String, crate::wildcard::WildcardValues>,
    /// Instance → template rule name for every fan-out expansion (issue #74
    /// phase 3): the cluster driver groups instances into job arrays by
    /// their TEMPLATE, never by guessing name suffixes. Never serialized.
    #[serde(skip)]
    pub expansion_templates: HashMap<String, String>,
    /// Resolved include contracts (issue #112 module slice): one entry per
    /// `[[include]]` that declares an interface. Never serialized.
    #[serde(skip)]
    pub include_contracts: Vec<ResolvedIncludeContract>,
    /// Rule → include-contract index provenance (which module a rule came
    /// from). Never serialized.
    #[serde(skip)]
    pub module_of: HashMap<String, usize>,
    /// Module name → its rule names (issue #112 elasticity: `--module`
    /// partial runs). Built at include resolution; never serialized.
    #[serde(skip)]
    pub module_rules: HashMap<String, Vec<String>>,

    /// Engine-internal: the sample names each expanded rule came from.
    ///
    /// Populated by [`WorkflowConfig::expand_wildcards`]: pair expansion
    /// records the experiment/control names, group expansion records the
    /// sample name. Used for per-sample input-readiness attribution
    /// (issue #63). Never serialized — user TOML cannot set it.
    #[serde(skip)]
    pub expansion_samples: HashMap<String, Vec<String>>,

    /// Pre-expansion rule templates, captured on the first `expand_wildcards`
    /// call — the source for checkpoint re-entry re-expansion (issue #78 P3).
    #[serde(skip)]
    pub rule_templates: Vec<Rule>,
}

// ---------------------------------------------------------------------------
// Type-state pattern for workflow lifecycle
// ---------------------------------------------------------------------------

/// Marker type for a parsed (but not validated) workflow.
#[derive(Debug, Clone)]
pub struct Parsed;

/// Marker type for a validated workflow.
#[derive(Debug, Clone)]
pub struct Validated;

/// Marker type for a workflow that is ready to execute.
#[derive(Debug, Clone)]
pub struct Ready;

/// Type-state wrapper for [`WorkflowConfig`] that enforces lifecycle transitions
/// at compile time: Parsed → Validated → Ready.
#[derive(Debug, Clone)]
pub struct WorkflowState<S> {
    pub config: WorkflowConfig,
    _state: std::marker::PhantomData<S>,
}

impl WorkflowState<Parsed> {
    /// Create a new parsed workflow state from a config.
    #[must_use]
    pub fn new(config: WorkflowConfig) -> Self {
        Self {
            config,
            _state: std::marker::PhantomData,
        }
    }

    /// Validate the workflow and transition to Validated state.
    pub fn validate(self) -> crate::Result<WorkflowState<Validated>> {
        self.config.validate()?;
        for rule in &self.config.rules {
            rule.validate()?;
        }
        Ok(WorkflowState {
            config: self.config,
            _state: std::marker::PhantomData,
        })
    }
}

impl WorkflowState<Validated> {
    /// Build the DAG and transition to Ready state.
    pub fn prepare(self) -> crate::Result<WorkflowState<Ready>> {
        let _dag = crate::dag::WorkflowDag::from_rules(&self.config.rules)?;
        Ok(WorkflowState {
            config: self.config,
            _state: std::marker::PhantomData,
        })
    }
}

impl<S> WorkflowState<S> {
    /// Access the underlying config.
    #[must_use]
    pub fn config(&self) -> &WorkflowConfig {
        &self.config
    }
}

/// Expand `{config.name}` placeholders in a path using provided config values.
pub(crate) fn expand_config_vars_in_path(
    path: &str,
    config: &HashMap<String, toml::Value>,
) -> String {
    // Stringify every value once, then expand to a fixed point so nested
    // `{config.x}` references resolve regardless of map iteration order.
    let stringified: HashMap<String, String> = config
        .iter()
        .map(|(key, value)| {
            let string_val = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (format!("config.{key}"), string_val)
        })
        .collect();
    crate::executor::expand_to_fixed_point(path, &stringified, |value| value.to_owned())
}
