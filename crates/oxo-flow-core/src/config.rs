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
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
/// [`crate::wildcard::expand_values_namespace`].
static VALUES_NS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{values\.(\w+)\}").expect("valid values-namespace regex"));

fn is_defaults_empty(d: &Defaults) -> bool {
    d.threads.is_none() && d.memory.is_none() && d.environment.is_none()
}

/// Maximum depth for nested include directives to prevent infinite recursion.
const MAX_INCLUDE_DEPTH: usize = 16;

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
    fn merge_from(&mut self, other: &ClusterProfile, override_mode: bool) {
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
fn value_instance_suffix(
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
fn expand_rule_patterns(
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
fn expand_rule_shell(shell: &str, combo: &crate::wildcard::WildcardValues) -> String {
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
fn expand_command_text_fields(
    expanded: &mut crate::rule::Rule,
    source: &crate::rule::Rule,
    expand: impl Fn(&str) -> String,
) {
    expanded.script = source.script.as_deref().map(&expand);
    expanded.pre_exec = source.pre_exec.as_deref().map(&expand);
    expanded.on_success = source.on_success.as_deref().map(&expand);
    expanded.on_failure = source.on_failure.as_deref().map(&expand);
}

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
    injected_config_keys: BTreeSet<String>,

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
fn expand_config_vars_in_path(path: &str, config: &HashMap<String, toml::Value>) -> String {
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

impl WorkflowConfig {
    /// Parse a workflow configuration from a TOML string.
    #[must_use = "parsing a config returns a Result that must be used"]
    pub fn parse(content: &str) -> Result<Self> {
        let mut config: WorkflowConfig = toml::from_str(content)?;
        config.extract_declarative_config()?;
        config = config.with_reference_builder_templates()?;
        config.validate()?;
        Ok(config)
    }

    /// Extract declarative `ConfigDef` entries from inline-table `[config]` values.
    fn extract_declarative_config(&mut self) -> Result<()> {
        for (key, val) in self.config.clone().iter() {
            let toml::Value::Table(t) = val else {
                continue;
            };
            if (t.contains_key("default")
                || t.contains_key("required")
                || t.contains_key("help")
                || t.contains_key("sensitive"))
                && let Ok(def) = toml::Value::Table(t.clone()).try_into::<ConfigDef>()
            {
                self.config_meta.insert(key.clone(), def);
                let runtime_val = self.config_meta[key].default.clone().unwrap_or_default();
                self.config
                    .insert(key.clone(), toml::Value::String(runtime_val));
            }
        }
        Ok(())
    }

    /// Parse a workflow configuration from a `.oxoflow` file.
    #[must_use = "parsing a config file returns a Result that must be used"]
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let mut config: WorkflowConfig =
            toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        config.extract_declarative_config()?;

        // Format Versioning check (S5)
        const SUPPORTED_FORMAT_VERSION: &str = "1.0";
        if let Some(ref version) = config.workflow.format_version
            && version != SUPPORTED_FORMAT_VERSION
        {
            tracing::warn!(
                "Workflow format version mismatch in {}: expected {}, found {}. Some features may not work as expected.",
                path.display(),
                SUPPORTED_FORMAT_VERSION,
                version
            );
        }

        // Resolve modular includes
        let parent = crate::parent_dir(path);
        config.resolve_includes(parent)?;

        // Load pairs from external file if specified
        if let Some(ref pairs_file) = config.workflow.pairs_file {
            let pairs_path = parent.join(pairs_file);
            let file_pairs = ExperimentControlPair::load_from_file(&pairs_path)?;
            let count = file_pairs.len();
            // Merge with inline pairs
            config.pairs.extend(file_pairs);
            tracing::info!("Loaded {} pairs from {}", count, pairs_file);
        }

        // Discover pairs from pattern if specified
        if let Some(ref pairs_pattern) = config.workflow.pairs_pattern {
            let discovered_pairs =
                ExperimentControlPair::discover_from_pattern(pairs_pattern, parent)?;
            let count = discovered_pairs.len();
            // Merge with inline/file pairs
            config.pairs.extend(discovered_pairs);
            tracing::info!(
                "Discovered {} pairs from pattern '{}'",
                count,
                pairs_pattern
            );
        }

        // Load sample_groups from external file if specified
        if let Some(ref groups_file) = config.workflow.sample_groups_file {
            let groups_path = parent.join(groups_file);
            let file_groups = SampleGroup::load_from_file(&groups_path)?;
            let count = file_groups.len();
            // Merge with inline groups
            config.sample_groups.extend(file_groups);
            tracing::info!("Loaded {} sample groups from {}", count, groups_file);
        }

        // Auto-discover samples from filesystem pattern
        if let Some(ref sample_pattern) = config.workflow.sample_pattern {
            // Expand config variables in the pattern (e.g. {config.data_dir}/.../{sample}_R1.fq.gz)
            // Declared config defaults (from `key = { default = … }` entries) are
            // included alongside plain values so the pattern always resolves.
            let mut expand_config = config.config.clone();
            for (key, def) in &config.config_meta {
                if !expand_config.contains_key(key)
                    && let Some(ref default) = def.default
                {
                    expand_config.insert(key.clone(), toml::Value::String(default.clone()));
                }
            }
            let expanded_pattern = expand_config_vars_in_path(sample_pattern, &expand_config);

            // Validate pattern contains {sample} wildcard
            if !expanded_pattern.contains("{sample}") {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "sample_pattern must contain {{sample}} wildcard after expansion: '{}'",
                        expanded_pattern
                    ),
                });
            }

            // Resolve the base directory and filename pattern.
            let sp = std::path::Path::new(&expanded_pattern);
            let (search_dir, file_pattern) = if sp.is_absolute() {
                if let Some(file_name) = sp.file_name() {
                    let dir = sp.parent().unwrap_or(std::path::Path::new("/"));
                    (dir.to_path_buf(), file_name.to_string_lossy().to_string())
                } else {
                    (parent.to_path_buf(), expanded_pattern)
                }
            } else if let Some(file_name) = sp.file_name() {
                let dir = sp.parent().unwrap_or(std::path::Path::new(""));
                (parent.join(dir), file_name.to_string_lossy().to_string())
            } else {
                (parent.to_path_buf(), expanded_pattern)
            };

            let discovered =
                crate::wildcard::discover_wildcards_from_pattern(&search_dir, &file_pattern)?;
            if discovered.is_empty() {
                tracing::warn!(
                    "sample_pattern '{}' matched no files in {}",
                    sample_pattern,
                    search_dir.display()
                );
            } else {
                // Extract sample values from discovered combinations
                let auto_samples: Vec<String> = discovered
                    .iter()
                    .filter_map(|combo| combo.get("sample").cloned())
                    .collect();

                if !auto_samples.is_empty() {
                    let auto_group = SampleGroup {
                        name: "auto-discovered".to_string(),
                        samples: auto_samples.clone(),
                        metadata: HashMap::new(),
                    };
                    config.sample_groups.push(auto_group);
                    tracing::info!(
                        "Auto-discovered {} samples from pattern '{}'",
                        auto_samples.len(),
                        sample_pattern
                    );
                } else {
                    tracing::warn!(
                        "sample_pattern '{}' matched files but no {{sample}} values were extracted",
                        sample_pattern
                    );
                }
            }
        }

        // ── Consolidate all sample sources into samples_list ──────────────
        // Merges auto-discovered samples (from sample_pattern), file-loaded
        // samples (from sample_groups_file), and inline [[sample_groups]].
        let mut all_samples: Vec<String> = Vec::new();
        for group in &config.sample_groups {
            for s in &group.samples {
                if !all_samples.contains(s) {
                    all_samples.push(s.clone());
                }
            }
        }
        // [[pairs]] members are samples too: experiment/control names are
        // what {sample}-wildcarded rules fan out over in pair workflows, so
        // the merged config.samples_list must include them (live: a
        // pairs-only workflow rendered {config.samples_list} as a literal
        // because nothing fed the merged list).
        for pair in &config.pairs {
            for s in std::iter::once(&pair.experiment).chain(pair.control.iter()) {
                if !all_samples.contains(s) {
                    all_samples.push(s.clone());
                }
            }
        }
        if !all_samples.is_empty() {
            let existing = config
                .config
                .get("samples_list")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let merged = merge_comma_list(existing, &all_samples);
            config.config.insert(
                "samples_list".to_string(),
                toml::Value::String(merged.join(",")),
            );
        }
        // Also inject each sample group's name as a config variable
        for group in &config.sample_groups {
            config
                .config
                .entry(format!("samples_{}", group.name))
                .or_insert_with(|| toml::Value::String(group.samples.join(",")));
        }

        // ── Inject config.pairs_list (symmetric with samples_list) ────────
        // [[pairs]], pairs_file, and pairs_pattern entries are consolidated
        // above; their pair_ids are injected as a sorted, comma-joined list
        // so rules can reference `{config.pairs_list}` instead of keeping a
        // hand-written `[config] pair_ids = [...]` in sync with [[pairs]].
        if !config.pairs.is_empty() {
            let existing = config
                .config
                .get("pairs_list")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let pair_ids: Vec<String> = config.pairs.iter().map(|p| p.pair_id.clone()).collect();
            let merged = merge_comma_list(existing, &pair_ids);
            config.config.insert(
                "pairs_list".to_string(),
                toml::Value::String(merged.join(",")),
            );
        }

        // Derive standard reference paths from reference_dir (e.g., reference_fasta = reference_dir + "/genome.fa")
        config = config.with_derived_references();

        // Expand [[references]] builder templates (build = "bwa_index" → canonical command)
        // and inject keyed config values (config.<name> = output) for each reference.
        config = config.with_reference_builder_templates()?;

        config.validate()?;
        Ok(config)
    }

    /// Filter the workflow's samples to a pilot subset.
    ///
    /// Specs are `first:N` (the first N samples in workflow order) and/or
    /// explicit comma-separated sample names — both forms may be combined
    /// and repeated. Filtering is applied to every sample source
    /// (`[[sample_groups]]`, `sample_pattern` auto-discovery, sample-group
    /// files), the merged `config.samples_list` / `config.pairs_list`, and
    /// experiment/control `[[pairs]]` whose samples were filtered out.
    ///
    /// Returns `(kept, unknown)` — the kept samples in workflow order and
    /// any explicitly named samples that were not found.
    pub fn filter_samples(&mut self, specs: &[String]) -> Result<(Vec<String>, Vec<String>)> {
        let mut take_first: Option<usize> = None;
        let mut explicit: Vec<String> = Vec::new();
        for spec in specs {
            for part in spec.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                if let Some(n) = part.strip_prefix("first:") {
                    let n: usize = n.trim().parse().map_err(|_| OxoFlowError::Config {
                        message: format!(
                            "invalid --samples spec '{part}': expected first:<N> or a sample name"
                        ),
                    })?;
                    take_first = Some(take_first.map_or(n, |cur| cur.max(n)));
                } else {
                    explicit.push(part.to_string());
                }
            }
        }

        // Workflow order: group order, then within-group order, deduplicated.
        let ordered: Vec<String> = {
            let mut out = Vec::new();
            for group in &self.sample_groups {
                for s in &group.samples {
                    if !out.contains(s) {
                        out.push(s.clone());
                    }
                }
            }
            out
        };

        let allowed: std::collections::HashSet<String> = if let Some(n) = take_first {
            ordered
                .iter()
                .take(n)
                .cloned()
                .chain(explicit.iter().cloned())
                .collect()
        } else {
            explicit.iter().cloned().collect()
        };
        let kept: Vec<String> = ordered
            .iter()
            .filter(|s| allowed.contains(*s))
            .cloned()
            .collect();
        // Pair experiment/control names are valid sample identifiers too —
        // they must not be reported as unknown (issue #63 feeds resolved
        // `ready` names through this path).
        let unknown: Vec<String> = explicit
            .iter()
            .filter(|name| {
                !ordered.iter().any(|s| s == name.as_str())
                    && !self.pairs.iter().any(|p| {
                        p.experiment == name.as_str()
                            || p.control.as_deref().is_some_and(|c| c == name.as_str())
                    })
            })
            .cloned()
            .collect();

        // Filter every sample source and the merged samples_list.
        for group in &mut self.sample_groups {
            group.samples.retain(|s| allowed.contains(s));
        }
        self.pairs.retain(|p| {
            allowed.contains(&p.experiment)
                && p.control.as_ref().is_none_or(|c| allowed.contains(c))
        });
        // Keep the injected config.pairs_list / samples_list in sync with
        // the surviving sets — including the empty case, so a filter that
        // drops EVERY pair/sample cannot leave a stale list behind for
        // expand_inputs to resolve against rules that no longer exist.
        let mut pair_ids: Vec<String> = self.pairs.iter().map(|p| p.pair_id.clone()).collect();
        pair_ids.sort();
        pair_ids.dedup();
        self.config.insert(
            "pairs_list".to_string(),
            toml::Value::String(pair_ids.join(",")),
        );
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(kept.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }

        Ok((kept, unknown))
    }

    /// Replace the workflow's sample groups outright and keep the injected
    /// config lists (`samples_list` / `samples_<group>` / `pairs_list`) and
    /// `[[pairs]]` in sync with the new set.
    ///
    /// This is the "override" path: the given groups REPLACE the inline /
    /// auto-discovered / file-loaded groups instead of filtering them. It is
    /// how the CLI lets a workflow ship with fixture samples (e.g. `S1`/`S2`)
    /// and a caller swap in real identifiers without editing the file.
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn override_sample_groups(&mut self, groups: Vec<SampleGroup>) -> Result<Vec<String>> {
        let mut final_samples: Vec<String> = Vec::new();
        for group in &groups {
            for sample in &group.samples {
                if !final_samples.iter().any(|s| s == sample) {
                    final_samples.push(sample.clone());
                }
            }
        }

        // Group names that existed before the override: their injected
        // `samples_<group>` keys must not survive when the group is gone
        // (expand_inputs would keep resolving the stale list).
        let old_group_names: std::collections::HashSet<String> =
            self.sample_groups.iter().map(|g| g.name.clone()).collect();

        self.sample_groups = groups;

        // Prune stale injected samples_<group> keys for dropped groups.
        let new_group_names: std::collections::HashSet<String> =
            self.sample_groups.iter().map(|g| g.name.clone()).collect();
        for stale in old_group_names.difference(&new_group_names) {
            self.config.remove(&format!("samples_{stale}"));
        }

        // Drop pairs whose experiment/control are no longer selected.
        self.pairs.retain(|p| {
            final_samples.iter().any(|s| s == &p.experiment)
                && p.control
                    .as_ref()
                    .is_none_or(|c| final_samples.iter().any(|s| s == c))
        });

        // Keep the injected config lists in sync with the surviving set.
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(final_samples.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }
        let mut pair_ids: Vec<String> = self.pairs.iter().map(|p| p.pair_id.clone()).collect();
        pair_ids.sort();
        pair_ids.dedup();
        self.config.insert(
            "pairs_list".to_string(),
            toml::Value::String(pair_ids.join(",")),
        );

        Ok(final_samples)
    }

    /// Append sample groups on top of the workflow's current set — the
    /// "add" counterpart of [`Self::override_sample_groups`] (`+@path` on
    /// the CLI). A sheet group whose name matches an existing group extends
    /// it (union, dedup, order-preserving); new group names are added
    /// as-is. `[[pairs]]` are left untouched: appending can only ADD
    /// samples, never remove a pair's side.
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn append_sample_groups(&mut self, groups: Vec<SampleGroup>) -> Result<Vec<String>> {
        for incoming in groups {
            if let Some(existing) = self
                .sample_groups
                .iter_mut()
                .find(|g| g.name == incoming.name)
            {
                for sample in incoming.samples {
                    if !existing.samples.contains(&sample) {
                        existing.samples.push(sample);
                    }
                }
            } else {
                self.sample_groups.push(incoming);
            }
        }

        let mut final_samples: Vec<String> = Vec::new();
        for group in &self.sample_groups {
            for sample in &group.samples {
                if !final_samples.iter().any(|s| s == sample) {
                    final_samples.push(sample.clone());
                }
            }
        }
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(final_samples.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }
        Ok(final_samples)
    }

    /// Override the workflow's samples with a flat list — collapses every
    /// group into a single group (reusing the first group's name, or
    /// `"samples"` when the workflow declares no groups) so `{group}` /
    /// `{sample}` expansion keeps working. See [`Self::override_sample_groups`].
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn override_samples(&mut self, names: &[String]) -> Result<Vec<String>> {
        let mut final_samples: Vec<String> = Vec::new();
        for name in names {
            let name = name.trim();
            if !name.is_empty() && !final_samples.iter().any(|s| s == name) {
                final_samples.push(name.to_string());
            }
        }

        // Reuse the first group's name so `{group}` references keep resolving;
        // fall back to `"samples"` when the workflow declares no groups.
        let group_name = self
            .sample_groups
            .first()
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "samples".to_string());

        self.override_sample_groups(vec![SampleGroup {
            name: group_name,
            samples: final_samples,
            metadata: HashMap::new(),
        }])
    }

    /// Validate the workflow configuration for internal consistency.
    #[must_use = "validation returns a Result that must be checked"]
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

        // Ensure each rule has either shell, script, or transform
        for rule in &self.rules {
            if rule.shell.is_none()
                && rule.script.is_none()
                && rule.transform.is_none()
                && !rule.output.is_empty()
            {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "rule '{}' has outputs but no shell command, script, or transform",
                        rule.name
                    ),
                });
            }
        }

        self.validate_execution_groups()?;

        // Include interface contracts (issue #112): contract errors fail
        // fast with the wiring gap named; encapsulation gaps warn.
        let (contract_errors, contract_warnings) = self.check_include_contracts();
        if let Some(first) = contract_errors.first() {
            return Err(OxoFlowError::Config {
                message: first.clone(),
            });
        }
        for warning in &contract_warnings {
            tracing::warn!("{warning}");
        }

        // Validate [[references]] entries: builder template names must be
        // known, template builds must declare an output, names must be unique.
        crate::references::validate_reference_defs(&self.references)?;

        // Warn about rules exceeding system capacity (but don't block)
        let system_threads = num_cpus::get() as u32;
        let system_memory_mb = {
            use sysinfo::System;
            // Only memory is needed here; `System::new_all()` would walk all of
            // /proc (every process, disk, and network interface) just to read
            // total RAM, adding ~50ms to every parse/validate/dry-run/run call.
            let mut sys = System::new();
            sys.refresh_memory();
            sys.total_memory() / 1024 / 1024
        };

        for rule in &self.rules {
            for warning in crate::scheduler::validate_resources_against_system(
                rule,
                system_threads,
                system_memory_mb,
            ) {
                tracing::warn!("{}", warning);
            }
        }

        // Validate wildcard constraints
        for (name, pattern) in &self.wildcard_constraints {
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(OxoFlowError::Config {
                    message: format!("invalid regex for wildcard constraint '{}': {}", name, e),
                });
            }
        }

        Ok(())
    }

    /// Derive standard reference paths from `reference_dir`.
    ///
    /// Returns a map of derived paths for keys that are not explicitly set.
    pub fn derive_reference_paths(&self) -> HashMap<String, String> {
        // Support both top-level `reference_dir` and `[config]` reference_dir
        let base = self
            .reference_dir
            .as_deref()
            .or_else(|| self.config.get("reference_dir").and_then(|v| v.as_str()));
        let Some(base) = base else {
            return HashMap::new();
        };

        let derivations = [
            ("reference_fasta", "genome.fa"),
            ("gene_annotation", "genes.gtf"),
            ("bwa_index", "bwa/genome.fa"),
            ("bwamem2_index", "bwamem2/genome.fa"),
            ("bowtie2_index", "bowtie2/genome.fa"),
            ("star_index", "star"),
            ("hisat2_index", "hisat2/genome.fa"),
            ("minimap2_index", "genome.fa.mmi"),
            ("gatk_dict", "genome.dict"),
            ("samtools_faidx", "genome.fa.fai"),
        ];

        let mut result = HashMap::new();
        for (key, suffix) in derivations {
            // Only derive if not explicitly set
            if !self.config.contains_key(key) {
                result.insert(key.to_string(), format!("{}/{}", base, suffix));
            }
        }
        result
    }

    /// Merge derived reference paths into config, and auto-generate default
    /// `[[references]]` entries for standard indexes when `reference_dir` is set
    /// and no explicit references block exists.
    ///
    /// This connects the Reference Discovery API (`reference_dir`) with the
    /// auto-build system (`[[references]]`), so pipelines using only
    /// `reference_dir` get automatic index building without explicit declarations.
    #[must_use]
    pub fn with_derived_references(mut self) -> Self {
        let derived = self.derive_reference_paths();
        for (key, value) in &derived {
            self.config
                .entry(key.clone())
                .or_insert_with(|| toml::Value::String(value.clone()));
            self.injected_config_keys.insert(key.clone());
        }

        // If reference_dir is set but no [[references]] block exists, auto-derive
        // default index-building references so users don't need to declare them.
        if self.references.is_empty()
            && let Some(ref base) = self
                .reference_dir
                .as_deref()
                .or_else(|| self.config.get("reference_dir").and_then(|v| v.as_str()))
        {
            let defaults: Vec<ReferenceDef> = vec![
                // --- Universal: FASTA index (.fai) — required by virtually every tool ---
                ReferenceDef {
                    name: "samtools_faidx".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.fa.fai"),
                    build: format!("samtools faidx {base}/genome.fa"),
                    threads: Some(1),
                    memory: Some("2G".into()),
                    environment: None,
                    description: Some("FASTA index (.fai) — required by IGV, GATK, samtools, and most viewers".into()),
                },
                // --- Short-read DNA alignment: BWA (classic, widely used) ---
                ReferenceDef {
                    name: "bwa_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bwa/genome.fa.bwt"),
                    build: format!(
                        "mkdir -p {base}/bwa && bwa index -p {base}/bwa/genome.fa {base}/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("BWA index for short-read DNA alignment (BWA-MEM/BWA-SW)".into()),
                },
                // --- Short-read DNA alignment: BWA-MEM2 (1.3-3.1x faster, identical output) ---
                ReferenceDef {
                    name: "bwamem2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bwamem2/genome.fa.0123"),
                    build: format!(
                        "mkdir -p {base}/bwamem2 && bwa-mem2 index -p {base}/bwamem2/genome.fa {base}/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("16G".into()),
                    environment: None,
                    description: Some("BWA-MEM2 index — faster BWA replacement, identical alignment output".into()),
                },
                // --- Short-read DNA alignment: Bowtie2 ---
                ReferenceDef {
                    name: "bowtie2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bowtie2/genome.fa.1.bt2"),
                    build: format!(
                        "mkdir -p {base}/bowtie2 && bowtie2-build --threads 8 {base}/genome.fa {base}/bowtie2/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("Bowtie2 index for short-read DNA alignment".into()),
                },
                // --- Long-read alignment: Minimap2 (Nanopore, PacBio) ---
                ReferenceDef {
                    name: "minimap2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.fa.mmi"),
                    build: format!("minimap2 -d {base}/genome.fa.mmi {base}/genome.fa"),
                    threads: Some(4),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("Minimap2 index (.mmi) for long-read alignment (Nanopore/PacBio)".into()),
                },
                // --- RNA-seq alignment: STAR (splice-aware, gold standard) ---
                ReferenceDef {
                    name: "star_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/star/SAindex"),
                    build: format!(
                        "mkdir -p {base}/star && STAR --runMode genomeGenerate --genomeDir {base}/star --genomeFastaFiles {base}/genome.fa --sjdbGTFfile {base}/genes.gtf --runThreadN 16"
                    ),
                    threads: Some(16),
                    memory: Some("64G".into()),
                    environment: None,
                    description: Some("STAR index for splice-aware RNA-seq alignment (~30 GB, 2-6 hours)".into()),
                },
                // --- RNA-seq alignment: HISAT2 (hierarchical indexing, smaller memory) ---
                ReferenceDef {
                    name: "hisat2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/hisat2/genome.fa.1.ht2"),
                    build: format!(
                        "mkdir -p {base}/hisat2 && hisat2-build -p 8 {base}/genome.fa {base}/hisat2/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("HISAT2 index for splice-aware RNA-seq alignment (hierarchical, smaller memory)".into()),
                },
                // --- Variant calling: Sequence dictionary (.dict) for GATK/Picard ---
                ReferenceDef {
                    name: "gatk_dict".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.dict"),
                    build: format!("samtools dict {base}/genome.fa -o {base}/genome.dict"),
                    threads: Some(1),
                    memory: Some("4G".into()),
                    environment: None,
                    description: Some("Sequence dictionary (.dict) for GATK/Picard variant calling".into()),
                },
            ];
            self.references = defaults;
        }
        self
    }

    /// Expand `[[references]]` builder templates and inject keyed config values.
    ///
    /// Every reference's `build` may name a built-in builder template
    /// (e.g. `build = "bwa_index"`) instead of a handwritten shell command;
    /// this step replaces the template name with its canonical command (see
    /// [`crate::references`] for the registry and the naming standard).
    /// Handwritten shell commands pass through unchanged, and unknown template
    /// names are rejected by [`Self::validate`].
    ///
    /// Each reference also becomes a keyed config value: `config.<name>` is
    /// set to the reference's `output` path (with `{config.x}` placeholders
    /// pre-expanded) unless the key is already declared, so rules reference
    /// the artifact as `{config.genome}`.
    #[must_use = "template expansion returns a Result that must be checked"]
    pub fn with_reference_builder_templates(mut self) -> Result<Self> {
        // Keyed references: config.<name> = output (unless already declared).
        // Iterate to a fixpoint so an output embedding another reference's
        // keyed config (`{config.other}`) resolves regardless of
        // declaration order; at most one expansion per reference per pass,
        // bounded by the reference count (an unresolvable `{config.x}` is
        // left literal and terminates the loop).
        for _ in 0..=self.references.len() {
            let mut changed = false;
            for def in &self.references {
                if def.output.trim().is_empty() {
                    continue;
                }
                // Fill missing keyed values, and re-expand previously
                // INJECTED values that still carry an unresolved
                // `{config.x}` (a reference whose output embeds another
                // reference's key). User-declared values are never touched.
                let needs_fill = !self.injected_config_keys.contains(&def.name)
                    && !self.config.contains_key(&def.name);
                let needs_reexpand = self.injected_config_keys.contains(&def.name)
                    && self
                        .config
                        .get(&def.name)
                        .and_then(toml::Value::as_str)
                        .is_some_and(|v| v.contains("{config."));
                if needs_fill || needs_reexpand {
                    let value = expand_config_vars_in_path(&def.output, &self.config);
                    changed |=
                        self.config.get(&def.name) != Some(&toml::Value::String(value.clone()));
                    self.config
                        .insert(def.name.clone(), toml::Value::String(value));
                    self.injected_config_keys.insert(def.name.clone());
                }
            }
            if !changed {
                break;
            }
        }
        // Builder templates: replace template names with canonical commands.
        for def in &mut self.references {
            def.build = crate::references::expand_build_command(def)?;
        }
        Ok(self)
    }

    /// True when `key` was injected by the engine at parse time (reference
    /// keyed-config values or reference_dir-derived paths), not written by
    /// the user. Run-time injections (samples_list / pairs_list /
    /// samples_*) are covered by `config_impact::is_engine_injected_key`.
    pub fn is_injected_config_key(&self, key: &str) -> bool {
        self.injected_config_keys.contains(key)
    }

    /// Resolve include directives by loading and merging rules from included files.
    /// Rules from included files are optionally prefixed with the namespace.
    #[must_use = "resolving includes returns a Result that must be checked"]
    pub fn resolve_includes(&mut self, base_dir: &Path) -> Result<()> {
        self.resolve_includes_with_depth(base_dir, 0)
    }

    fn resolve_includes_with_depth(&mut self, base_dir: &Path, depth: usize) -> Result<()> {
        if depth >= MAX_INCLUDE_DEPTH {
            return Err(OxoFlowError::Config {
                message: format!(
                    "include depth exceeds maximum of {} — possible circular includes",
                    MAX_INCLUDE_DEPTH
                ),
            });
        }
        let includes = std::mem::take(&mut self.includes);
        for inc in &includes {
            let (content, inc_base_dir) = if let Some(repo) = &inc.repo {
                // Pinned git include (issue #112): clone into the module
                // cache (keyed repo@ref), then resolve `path` inside the
                // checkout. ensure_pinned clones on a cache miss (healing
                // partial dirs), re-fetches branch/tag refs on every
                // activation, and pins full commit SHAs by fetch (issue
                // #136).
                let git_ref = inc.git_ref.as_deref().ok_or_else(|| OxoFlowError::Config {
                    message: format!(
                        "include '{}' declares `repo` without `ref` — pin the module version",
                        inc.path
                    ),
                })?;
                let cache_dir =
                    crate::git::module_cache_root().join(crate::git::cache_dir_name(repo, git_ref));
                std::fs::create_dir_all(cache_dir.parent().unwrap_or(&cache_dir)).map_err(|e| {
                    OxoFlowError::Config {
                        message: format!("cannot create module cache: {e}"),
                    }
                })?;
                crate::git::ensure_pinned(repo, git_ref, &cache_dir).map_err(|e| {
                    OxoFlowError::Config {
                        message: format!(
                            "failed to clone or refresh include repo '{repo}' at '{git_ref}': {e}"
                        ),
                    }
                })?;
                let inc_path = cache_dir.join(&inc.path);
                let text = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: format!(
                        "failed to read include '{}' from repo '{repo}': {e}",
                        inc.path
                    ),
                })?;
                let next_base = inc_path.parent().unwrap_or(&cache_dir).to_path_buf();
                (text, next_base)
            } else if inc.path.starts_with("http://") || inc.path.starts_with("https://") {
                // Fetch remote include on a dedicated thread to avoid tokio runtime conflicts
                tracing::info!(url = %inc.path, "fetching remote include");
                let url = inc.path.clone();
                let text =
                    std::thread::spawn(move || reqwest::blocking::get(&url).and_then(|r| r.text()))
                        .join()
                        .map_err(|_| OxoFlowError::Parse {
                            path: PathBuf::from(&inc.path),
                            message: "remote include fetch panicked".to_string(),
                        })?
                        .map_err(|e| OxoFlowError::Parse {
                            path: PathBuf::from(&inc.path),
                            message: format!("failed to fetch remote include: {}", e),
                        })?;
                (text, base_dir.to_path_buf()) // Remote includes don't change base_dir for now
            } else {
                let inc_path = base_dir.join(&inc.path);
                let text = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: format!("failed to read include '{}': {}", inc.path, e),
                })?;
                let next_base = inc_path.parent().unwrap_or(base_dir).to_path_buf();
                (text, next_base)
            };

            let mut inc_config: WorkflowConfig =
                toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                    path: PathBuf::from(&inc.path),
                    message: e.to_string(),
                })?;

            // Recursively resolve nested includes
            inc_config.resolve_includes_with_depth(&inc_base_dir, depth + 1)?;
            // Nested contracts merge first (their provenance indices shift
            // by the host's current contract count).
            let offset = self.include_contracts.len();
            self.include_contracts
                .extend(std::mem::take(&mut inc_config.include_contracts));
            for (rule, idx) in std::mem::take(&mut inc_config.module_of) {
                self.module_of.insert(rule, idx + offset);
            }
            // Record THIS include's contract (issue #112) and its rules'
            // provenance.
            let contract_idx = if !inc.inputs.is_empty() || !inc.outputs.is_empty() {
                self.include_contracts.push(ResolvedIncludeContract {
                    inputs: inc.inputs.clone(),
                    outputs: inc.outputs.clone(),
                });
                Some(self.include_contracts.len() - 1)
            } else {
                None
            };
            // Collect original rule names from included file for dependency resolution
            // Module identity: explicit `name` field, else the file stem.
            let module_name = inc.name.clone().unwrap_or_else(|| {
                Path::new(&inc.path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| inc.path.clone())
            });
            let original_rule_names: std::collections::HashSet<String> =
                inc_config.rules.iter().map(|r| r.name.clone()).collect();
            let mut module_members: Vec<String> = Vec::new();
            for mut rule in inc_config.rules {
                if let Some(ref ns) = inc.namespace {
                    // Prefix rule name with namespace
                    rule.name = format!("{}::{}", ns, rule.name);
                    // Prefix depends_on references that point to rules in the same included file
                    for dep in &mut rule.depends_on {
                        if original_rule_names.contains(dep) {
                            *dep = format!("{}::{}", ns, dep);
                        }
                    }
                }
                if !self.rules.iter().any(|r| r.name == rule.name) {
                    if let Some(idx) = contract_idx {
                        self.module_of.insert(rule.name.clone(), idx);
                    }
                    module_members.push(rule.name.clone());
                    self.rules.push(rule);
                }
            }
            self.module_rules
                .entry(module_name)
                .or_default()
                .extend(module_members);
            // Contract params fill config keys in profile-style — host
            // values win (or_insert).
            for (key, value) in &inc.params {
                self.config
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        self.includes = includes;
        Ok(())
    }

    /// The rule-name closure of a module for partial runs (issue #112
    /// elasticity): the module's own rules plus every HOST rule producing
    /// one of its declared concrete inputs. Upstream DAG dependents are
    /// added by the caller through the regular target machinery.
    pub fn module_closure(&self, module: &str) -> Option<Vec<String>> {
        let members = self.module_rules.get(module)?.clone();
        let mut closure: Vec<String> = members.clone();
        // Declared concrete inputs the module needs wired in.
        for (idx, contract) in self.include_contracts.iter().enumerate() {
            let module_uses_this_contract = members
                .iter()
                .any(|r| self.module_of.get(r).is_some_and(|p| *p == idx));
            if !module_uses_this_contract {
                continue;
            }
            let mut producers: HashMap<String, String> = HashMap::new();
            for rule in &self.rules {
                for out in rule.output.to_vec() {
                    producers.entry(out).or_insert_with(|| rule.name.clone());
                }
            }
            for input in &contract.inputs {
                if input.contains('{') {
                    continue;
                }
                if let Some(producer) = producers.get(input)
                    && !members.contains(producer)
                    && !closure.contains(producer)
                {
                    closure.push(producer.clone());
                }
            }
        }
        Some(closure)
    }

    /// Check the resolved include contracts (issue #112 module slice).
    ///
    /// Returns `(errors, warnings)`. Errors cover the fail-fast contract:
    /// a declared concrete input nobody produces, and a declared output no
    /// module rule produces (or produced outside the module). Warnings
    /// cover encapsulation: a host rule reading a module-internal file the
    /// contract does not declare. Wildcarded patterns are skipped here —
    /// their wiring is verified by DAG edge inference at run time.
    pub fn check_include_contracts(&self) -> (Vec<String>, Vec<String>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        if self.include_contracts.is_empty() {
            return (errors, warnings);
        }
        // Producer map: output path → producing rule name (owned — rule
        // outputs are borrowed across iterations).
        let mut producers: HashMap<String, String> = HashMap::new();
        for rule in &self.rules {
            for out in rule.output.to_vec() {
                producers.entry(out).or_insert_with(|| rule.name.clone());
            }
        }
        for (idx, contract) in self.include_contracts.iter().enumerate() {
            for input in &contract.inputs {
                if input.contains('{') {
                    continue; // wildcard wiring: DAG-time, documented
                }
                if !producers.contains_key(input) {
                    errors.push(format!(
                        "include contract: declared input '{input}' is not produced by any rule — wire it to a host rule output"
                    ));
                }
            }
            for output in &contract.outputs {
                if output.contains('{') {
                    continue;
                }
                match producers.get(output) {
                    Some(producer)
                        if self
                            .module_of
                            .get(producer)
                            .is_some_and(|p| *p == idx) => {}
                    Some(producer) => errors.push(format!(
                        "include contract: declared output '{output}' is produced by '{producer}', which is outside the module — module outputs must come from module rules"
                    )),
                    None => errors.push(format!(
                        "include contract: declared output '{output}' is not produced by any module rule"
                    )),
                }
            }
            // Encapsulation: host rules reading undeclared module-internal
            // files.
            let declared: std::collections::HashSet<&str> =
                contract.outputs.iter().map(String::as_str).collect();
            for rule in &self.rules {
                if self.module_of.get(&rule.name).is_some_and(|p| *p == idx) {
                    continue;
                }
                for inp in rule.input.to_vec() {
                    if inp.contains('{') {
                        continue;
                    }
                    let internal = producers
                        .get(&inp)
                        .is_some_and(|p| self.module_of.get(p).is_some_and(|mp| *mp == idx));
                    if internal && !declared.contains(inp.as_str()) {
                        warnings.push(format!(
                            "rule '{}' reads module-internal file '{inp}' which the include contract does not declare — add it to `outputs` to keep the coupling explicit",
                            rule.name
                        ));
                    }
                }
            }
        }
        (errors, warnings)
    }

    /// Validate that all execution group references point to existing rules.
    #[must_use = "validation returns a Result that must be checked"]
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

    /// The template rule a fanned-out instance was expanded from, if any
    /// (issue #74 phase 3). Rules that never fan out have no entry.
    pub fn template_of(&self, instance: &str) -> Option<&str> {
        self.expansion_templates.get(instance).map(String::as_str)
    }

    /// Apply global defaults to all rules that don't have explicit overrides.
    pub fn apply_defaults(&mut self) {
        for rule in &mut self.rules {
            // Apply default threads if rule doesn't specify one (either field).
            // resources.threads defaults to 1 via serde, and 1 means "unset"
            // in the engine's own convention (skip_serializing_if), so values
            // > 1 are treated as explicit rule-level declarations.
            if rule.threads.is_none() && rule.resources.threads <= 1 {
                rule.threads = self.defaults.threads;
            }
            // Apply default memory if rule doesn't specify one (either field)
            if rule.memory.is_none()
                && rule.resources.memory.is_none()
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

    /// Merge a `profiles/<NAME>.toml` document into this workflow — the
    /// semantics `run --profile` / `dry-run --profile` apply.
    ///
    /// Merges the profile's `[config]` (every key becomes a `{config.key}`
    /// variable) and `[defaults]` (feeds `rules.resources` defaults via
    /// [`WorkflowConfig::apply_defaults`]) sections. The mode comes from
    /// `[workflow] profile_mode`:
    ///
    /// - `"fill"` (default): profile values only FILL IN keys the workflow
    ///   does not set — existing workflow values always win.
    /// - `"override"`: profile values REPLACE workflow values. Nested
    ///   tables deep-merge recursively (keys from both sides survive);
    ///   scalars and arrays replace the workflow value wholesale.
    ///
    /// Callers must pass a profile document parsed with `toml::from_str`
    /// (a document), not `toml::Value::from_str` — in toml 1.x the latter
    /// parses a SINGLE inline value and would reject `[config]` tables.
    pub fn merge_profile(&mut self, profile_toml: &toml::Value) -> Result<()> {
        fn coerce_profile_defaults(table: toml::Table) -> toml::Table {
            // Profiles historically tolerated quoted numerics in [defaults]
            // (e.g. `threads = "16"`). Keep that tolerance: coerce string
            // values for integer fields, let genuinely wrong types fail the
            // typed conversion below with the same clear error.
            let mut table = table;
            if let Some(toml::Value::String(s)) = table.get("threads")
                && let Ok(n) = s.trim().parse::<u32>()
            {
                table.insert("threads".into(), toml::Value::Integer(i64::from(n)));
            }
            table
        }

        let mode = self.workflow.profile_mode.unwrap_or_default();
        if let Some(config_table) = profile_toml.get("config").and_then(toml::Value::as_table) {
            for (key, value) in config_table {
                match mode {
                    ProfileMode::Fill => {
                        self.config
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                    ProfileMode::Override => match self.config.get_mut(key) {
                        Some(existing) => deep_merge_value(existing, value.clone()),
                        None => {
                            self.config.insert(key.clone(), value.clone());
                        }
                    },
                }
            }
        }
        if let Some(defaults_table) = profile_toml.get("defaults").and_then(toml::Value::as_table) {
            let profile_defaults: Defaults =
                toml::Value::Table(coerce_profile_defaults(defaults_table.clone()))
                    .try_into()
                    .map_err(|e| OxoFlowError::Config {
                        message: format!("invalid [defaults] section in profile: {e}"),
                    })?;
            match mode {
                ProfileMode::Fill => {
                    if self.defaults.threads.is_none() {
                        self.defaults.threads = profile_defaults.threads;
                    }
                    if self.defaults.memory.is_none() {
                        self.defaults.memory = profile_defaults.memory;
                    }
                    if self.defaults.environment.is_none() {
                        self.defaults.environment = profile_defaults.environment;
                    }
                }
                ProfileMode::Override => {
                    if profile_defaults.threads.is_some() {
                        self.defaults.threads = profile_defaults.threads;
                    }
                    if profile_defaults.memory.is_some() {
                        self.defaults.memory = profile_defaults.memory;
                    }
                    if profile_defaults.environment.is_some() {
                        self.defaults.environment = profile_defaults.environment;
                    }
                }
            }
        }
        // `[cluster]` — the block whose presence routes `run --profile` to a
        // scheduler (issue #74). Same fill/override semantics as the rest of
        // the merge, so site config lives in one shared profile file.
        if let Some(cluster_table) = profile_toml.get("cluster") {
            let profile_cluster: ClusterProfile =
                cluster_table
                    .clone()
                    .try_into()
                    .map_err(|e| OxoFlowError::Config {
                        message: format!("invalid [cluster] section in profile: {e}"),
                    })?;
            self.cluster
                .get_or_insert_with(ClusterProfile::default)
                .merge_from(&profile_cluster, mode == ProfileMode::Override);
        }
        Ok(())
    }

    /// Expand rules that contain pair or group wildcards into concrete instances.
    ///
    /// Scans each rule for wildcard placeholders:
    /// - Rules containing `{experiment}`, `{control}`, or `{pair_id}` are
    ///   expanded once per entry in `self.pairs`.
    /// - Backward-compatible aliases `{tumor}` and `{normal}` are also
    ///   recognized.
    /// - Rules containing `{group}` or `{sample}` are expanded once per
    ///   (group, sample) combination in `self.sample_groups`.
    /// - Rules without any of these wildcards are kept unchanged.
    ///
    /// The expanded rule names follow the pattern `{original_name}_{suffix}`,
    /// where the suffix is the `pair_id` for pair rules or `{group}_{sample}`
    /// for group rules.
    ///
    /// After calling this method, `self.rules` contains only concrete rules
    /// (no pair/group wildcards) and the DAG can be built normally.
    ///
    /// # Errors
    ///
    /// Returns an error if duplicate rule names would be produced (e.g., two
    /// pairs with the same `pair_id`), or if a pair/group is defined but no
    /// rules reference its wildcards (this is not an error—those pairs are
    /// simply ignored).
    pub fn expand_wildcards(&mut self) -> Result<()> {
        // Preserve the unexpanded templates on first expansion — checkpoint
        // re-entry (issue #78 P3) re-expands from them with merged values.
        if self.rule_templates.is_empty() {
            self.rule_templates = self.rules.clone();
        }
        use crate::wildcard::{
            expand_pattern, has_wildcards, validate_wildcard_constraints_compiled,
            wildcard_combinations_from_groups, wildcard_combinations_from_pairs,
        };
        use regex::Regex;

        let pair_combos = wildcard_combinations_from_pairs(&self.pairs);
        let group_combos = wildcard_combinations_from_groups(&self.sample_groups);

        // Rebuild expansion provenance from scratch — this method may run on a
        // config that was expanded before.
        self.expansion_samples.clear();
        self.expansion_values.clear();

        // Validate [[values]] tables: non-empty names/values, unique names,
        // and no collisions with built-in wildcards (a rule referencing
        // `{sample}` must not be ambiguous between group and value fan-out).
        let mut seen_value_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        const RESERVED_VALUE_NAMES: &[&str] = &[
            "sample",
            "group",
            "pair_id",
            "experiment",
            "control",
            "tumor",
            "normal",
            "experiment_type",
            "tumor_type",
            // Executor placeholders — a value table named `input` (etc.)
            // would replace the placeholder in every rule's shell.
            "input",
            "output",
            "log",
            "threads",
            "memory",
        ];
        for table in &self.values {
            if table.name.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: "[[values]] table must have a non-empty name".to_string(),
                    rule: None,
                    suggestion: None,
                });
            }
            if table.values.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: format!("[[values]] table '{}' has no values", table.name),
                    rule: None,
                    suggestion: Some("add at least one value, or remove the table".to_string()),
                });
            }
            if RESERVED_VALUE_NAMES.contains(&table.name.as_str()) {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "[[values]] name '{}' collides with a built-in wildcard",
                        table.name
                    ),
                    rule: None,
                    suggestion: Some(
                        "rename the table (e.g. use a tool-specific parameter name)".to_string(),
                    ),
                });
            }
            if !seen_value_names.insert(table.name.clone()) {
                return Err(OxoFlowError::Validation {
                    message: format!("duplicate [[values]] table '{}'", table.name),
                    rule: None,
                    suggestion: Some("merge the tables or rename one of them".to_string()),
                });
            }
        }

        // Pre-compile constraints for performance
        let mut compiled_constraints = HashMap::new();
        for (name, pattern) in &self.wildcard_constraints {
            let re = Regex::new(pattern).map_err(|e| OxoFlowError::Wildcard {
                rule: String::new(),
                message: format!(
                    "invalid regex constraint '{}' for wildcard '{}': {}",
                    pattern, name, e
                ),
            })?;
            compiled_constraints.insert(name.clone(), re);
        }

        // Wildcards that trigger pair expansion.
        // Include backward-compatible aliases `{tumor}`/`{normal}`.
        let mut pair_wildcards = vec![
            "experiment",
            "control",
            "tumor",
            "normal",
            "pair_id",
            "experiment_type",
            "tumor_type",
        ];
        // Also include any metadata keys from defined pairs
        for pair in &self.pairs {
            for key in pair.metadata.keys() {
                pair_wildcards.push(key.as_str());
            }
        }

        // Wildcards that trigger group expansion
        const GROUP_WILDCARDS: &[&str] = &["group", "sample"];

        let mut expanded_rules: Vec<Rule> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Track original → expanded name mapping for depends_on resolution
        let mut name_map: HashMap<String, Vec<String>> = HashMap::new();

        for rule in &self.rules {
            // Collect all text fields that might contain wildcards. The fan-out
            // TRIGGER set is input/output/shell only — script and the hooks
            // substitute per instance when the rule fans out, but never start
            // a fan-out themselves (cloning on a hook-only wildcard would
            // duplicate the whole rule execution, and `${name}` bash
            // spellings inside script would false-trigger).
            let mut all_text: Vec<&str> = rule.input.iter().map(String::as_str).collect();
            all_text.extend(rule.output.iter().map(String::as_str));
            if let Some(ref shell) = rule.shell {
                all_text.push(shell);
            }

            let uses_pair_wildcard = !pair_combos.is_empty()
                && all_text.iter().any(|t| {
                    pair_wildcards
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            let uses_group_wildcard = !group_combos.is_empty()
                && all_text.iter().any(|t| {
                    GROUP_WILDCARDS
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            // `[[values]]` fan-out: tables whose names appear in the rule —
            // in inputs/outputs/shells and in `expand_inputs` patterns —
            // become an additional Cartesian dimension, orthogonal to pairs
            // and groups. `{values.name}` is the namespaced sibling of the
            // bare `{name}` form.
            let expand_texts: Vec<&str> = all_text
                .iter()
                .copied()
                .chain(rule.expand_inputs.iter().map(|e| e.pattern.as_str()))
                .collect();
            let mut active_value_tables: Vec<&ValueGroup> = Vec::new();
            for table in &self.values {
                let referenced = expand_texts.iter().any(|t| {
                    t.contains(&format!("{{{}}}", table.name))
                        || t.contains(&format!("{{values.{}}}", table.name))
                });
                if referenced {
                    active_value_tables.push(table);
                }
            }
            let uses_value_wildcard = !active_value_tables.is_empty();

            // Unbound `{values.name}` namespace references have no fan-out
            // source: warn (never error — same stance as unbound `{sample}`)
            // so the author notices before runtime. Bare `{name}` is
            // indistinguishable from engine placeholders (`{input}`,
            // `{threads}`) and stays silent.
            let mut warned_value_ns: Vec<String> = Vec::new();
            for text in &expand_texts {
                for cap in VALUES_NS_RE.captures_iter(text) {
                    let name = cap[1].to_string();
                    let has_table = self.values.iter().any(|v| v.name == name);
                    if !has_table && !warned_value_ns.contains(&name) {
                        tracing::warn!(
                            rule = %rule.name,
                            "rule references '{{values.{name}}}' but no [[values]] table named '{name}' exists; the placeholder will be left unexpanded"
                        );
                        warned_value_ns.push(name);
                    }
                }
            }

            // Cartesian product of the active tables, deterministic: the
            // last referenced table varies fastest, mirroring declaration
            // order. Rules using no value table get a single empty combo —
            // the identity element, so the pair/group branches below run
            // exactly as before.
            let mut value_combos: Vec<crate::wildcard::WildcardValues> =
                vec![crate::wildcard::WildcardValues::new()];
            for table in &active_value_tables {
                let mut next = Vec::with_capacity(value_combos.len() * table.values.len());
                for combo in &value_combos {
                    for value in &table.values {
                        let mut c = combo.clone();
                        c.insert(table.name.clone(), value.clone());
                        next.push(c);
                    }
                }
                value_combos = next;
            }

            if uses_pair_wildcard {
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                // Expand for each value-combo × pair combination (orthogonal
                // fan-out — [[values]] adds a dimension on top of pairs).
                for value_combo in &value_combos {
                    for pair_combo in &pair_combos {
                        // Merge the binding sources so `{assembler}` and
                        // `{experiment}` both resolve during expansion.
                        let mut combo = pair_combo.clone();
                        combo.extend(value_combo.clone());

                        // Filter by constraints (non-matching combos are
                        // skipped, per docs).
                        if validate_wildcard_constraints_compiled(&combo, &compiled_constraints)
                            .is_err()
                        {
                            continue;
                        }

                        let suffix = pair_combo.get("pair_id").cloned().unwrap_or_else(|| {
                            pair_combo.values().cloned().collect::<Vec<_>>().join("_")
                        });
                        let new_name = format!(
                            "{}{}_{}",
                            rule.name,
                            value_instance_suffix(value_combo, &active_value_tables),
                            suffix
                        );
                        expanded_names.push(new_name.clone());

                        if !seen_names.insert(new_name.clone()) {
                            return Err(OxoFlowError::DuplicateRule { name: new_name });
                        }

                        let mut expanded = rule.clone();
                        expanded.name = new_name;

                        // Expand input/output/shell/log patterns
                        expanded.input = expand_rule_patterns(&rule.input, &combo);
                        expanded.output = expand_rule_patterns(&rule.output, &combo);
                        if let Some(ref shell) = rule.shell {
                            expanded.shell = Some(expand_rule_shell(shell, &combo));
                        }
                        if let Some(ref log) = rule.log {
                            expanded.log = Some(expand_rule_shell(log, &combo));
                        }
                        // Script and hooks carry the per-instance
                        // substitution too (issue #98) — same class as
                        // shell/log.
                        expand_command_text_fields(&mut expanded, rule, |s| {
                            expand_rule_shell(s, &combo)
                        });

                        // Record which sample names this expansion belongs to
                        // (issue #63 readiness attribution).
                        let mut involved: Vec<String> = Vec::new();
                        for key in ["experiment", "control"] {
                            if let Some(value) = combo.get(key)
                                && !value.is_empty()
                                && !involved.contains(value)
                            {
                                involved.push(value.clone());
                            }
                        }
                        self.expansion_samples
                            .insert(expanded.name.clone(), involved);

                        if !value_combo.is_empty() {
                            self.expansion_values
                                .insert(expanded.name.clone(), value_combo.clone());
                        }
                        self.expansion_templates
                            .insert(expanded.name.clone(), orig_name.clone());

                        expanded_rules.push(expanded);
                    }
                }
                name_map.insert(orig_name, expanded_names);
            } else if uses_group_wildcard {
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                // Expand for each value-combo × (group, sample) combination
                // (orthogonal fan-out — [[values]] adds a dimension on top
                // of sample groups).
                for value_combo in &value_combos {
                    for combo in &group_combos {
                        let mut merged = combo.clone();
                        merged.extend(value_combo.clone());

                        // Filter by constraints (non-matching combos are
                        // skipped, per docs).
                        if validate_wildcard_constraints_compiled(&merged, &compiled_constraints)
                            .is_err()
                        {
                            continue;
                        }

                        let group = combo.get("group").map(String::as_str).unwrap_or("group");
                        let sample = combo.get("sample").map(String::as_str).unwrap_or("sample");
                        let new_name = format!(
                            "{}{}_{}_{}",
                            rule.name,
                            value_instance_suffix(value_combo, &active_value_tables),
                            group,
                            sample
                        );
                        expanded_names.push(new_name.clone());

                        if !seen_names.insert(new_name.clone()) {
                            return Err(OxoFlowError::DuplicateRule { name: new_name });
                        }

                        let mut expanded = rule.clone();
                        expanded.name = new_name;

                        expanded.input = rule
                            .input
                            .iter()
                            .map(|p| {
                                if has_wildcards(p) || crate::wildcard::contains_values_namespace(p)
                                {
                                    crate::wildcard::expand_values_namespace(
                                        &expand_pattern(p, &merged).unwrap_or_else(|_| p.clone()),
                                        &merged,
                                    )
                                } else {
                                    p.clone()
                                }
                            })
                            .collect();
                        expanded.output = rule
                            .output
                            .iter()
                            .map(|p| {
                                if has_wildcards(p) || crate::wildcard::contains_values_namespace(p)
                                {
                                    crate::wildcard::expand_values_namespace(
                                        &expand_pattern(p, &merged).unwrap_or_else(|_| p.clone()),
                                        &merged,
                                    )
                                } else {
                                    p.clone()
                                }
                            })
                            .collect();
                        if let Some(ref shell) = rule.shell {
                            expanded.shell = if has_wildcards(shell)
                                || crate::wildcard::contains_values_namespace(shell)
                            {
                                Some(crate::wildcard::expand_values_namespace(
                                    &expand_pattern(shell, &merged)
                                        .unwrap_or_else(|_| shell.clone()),
                                    &merged,
                                ))
                            } else {
                                Some(shell.clone())
                            };
                        }
                        if let Some(ref log) = rule.log {
                            expanded.log = if has_wildcards(log)
                                || crate::wildcard::contains_values_namespace(log)
                            {
                                Some(crate::wildcard::expand_values_namespace(
                                    &expand_pattern(log, &merged).unwrap_or_else(|_| log.clone()),
                                    &merged,
                                ))
                            } else {
                                Some(log.clone())
                            };
                        }
                        // Free-text command fields take the same per-instance
                        // substitution as shell/log (issue #98): script and
                        // the hooks render through the same placeholder pass
                        // at execution time, which never sees pair/group
                        // names, so the values must be baked in here.
                        expand_command_text_fields(&mut expanded, rule, |s| {
                            expand_rule_shell(s, &merged)
                        });

                        // Record which sample this expansion belongs to
                        // (issue #63 readiness attribution).
                        let involved: Vec<String> = combo
                            .get("sample")
                            .cloned()
                            .into_iter()
                            .filter(|name| !name.is_empty())
                            .collect();
                        self.expansion_samples
                            .insert(expanded.name.clone(), involved);

                        if !value_combo.is_empty() {
                            self.expansion_values
                                .insert(expanded.name.clone(), value_combo.clone());
                        }
                        self.expansion_templates
                            .insert(expanded.name.clone(), orig_name.clone());

                        expanded_rules.push(expanded);
                    }
                }
                name_map.insert(orig_name, expanded_names);
            } else if uses_value_wildcard {
                // [[values]] fan-out without pair/group wildcards.
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                for combo in &value_combos {
                    // Filter by constraints (non-matching combos are skipped,
                    // per docs).
                    if validate_wildcard_constraints_compiled(combo, &compiled_constraints).is_err()
                    {
                        continue;
                    }

                    let new_name = format!(
                        "{}{}",
                        rule.name,
                        value_instance_suffix(combo, &active_value_tables)
                    );
                    expanded_names.push(new_name.clone());

                    if !seen_names.insert(new_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule { name: new_name });
                    }

                    let mut expanded = rule.clone();
                    expanded.name = new_name.clone();

                    // Structure-preserving expansion (List / Map / Dir).
                    expanded.input = expand_rule_patterns(&rule.input, combo);
                    expanded.output = expand_rule_patterns(&rule.output, combo);
                    if let Some(ref shell) = rule.shell {
                        expanded.shell = Some(expand_rule_shell(shell, combo));
                    }
                    if let Some(ref log) = rule.log {
                        expanded.log = Some(expand_rule_shell(log, combo));
                    }
                    // Script and hooks carry the per-instance substitution
                    // too (issue #98) — same class as shell/log.
                    expand_command_text_fields(&mut expanded, rule, |s| {
                        expand_rule_shell(s, combo)
                    });

                    self.expansion_values
                        .insert(new_name.clone(), combo.clone());
                    self.expansion_templates
                        .insert(new_name.clone(), orig_name.clone());
                    expanded_rules.push(expanded);
                }
                name_map.insert(orig_name, expanded_names);
            } else {
                // No expansion needed — keep rule as-is
                if !seen_names.insert(rule.name.clone()) {
                    return Err(OxoFlowError::DuplicateRule {
                        name: rule.name.clone(),
                    });
                }
                expanded_rules.push(rule.clone());
            }
        }

        // Resolve depends_on references: replace template names with expanded names
        if !name_map.is_empty() {
            for rule in &mut expanded_rules {
                if rule.depends_on.is_empty() {
                    continue;
                }
                let mut resolved_deps = Vec::new();
                for dep in &rule.depends_on {
                    if let Some(expanded_names) = name_map.get(dep.as_str()) {
                        resolved_deps.extend(expanded_names.clone());
                    } else {
                        resolved_deps.push(dep.clone());
                    }
                }
                rule.depends_on = resolved_deps;
            }
        }

        let mut final_rules = Vec::new();
        let mut gather_injections: HashMap<String, Vec<String>> = HashMap::new();

        for rule in expanded_rules {
            if let Some(ref scatter) = rule.scatter {
                let mut values = scatter.values.clone();
                if values.is_empty()
                    && let Some(ref v_from) = scatter.values_from
                    && let Some(resolved) = self.resolve_config_list(v_from)
                {
                    values = resolved;
                }

                let mut scatter_outputs = Vec::new();

                for val in &values {
                    let mut combo = HashMap::new();
                    combo.insert(scatter.variable.clone(), val.clone());

                    let mut scattered_rule = rule.clone();
                    scattered_rule.name = format!("{}_{}", rule.name, val);
                    scattered_rule.scatter = None; // remove scatter from generated rule

                    scattered_rule.input = match scattered_rule.input {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                        FilePatterns::Dir {
                            ref path,
                            ref pattern,
                        } => FilePatterns::Dir {
                            path: expand_pattern(path, &combo).unwrap_or_else(|_| path.clone()),
                            pattern: pattern.clone(),
                        },
                    };
                    scattered_rule.output = match scattered_rule.output {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                        FilePatterns::Dir {
                            ref path,
                            ref pattern,
                        } => FilePatterns::Dir {
                            path: expand_pattern(path, &combo).unwrap_or_else(|_| path.clone()),
                            pattern: pattern.clone(),
                        },
                    };
                    if let Some(ref shell) = scattered_rule.shell {
                        scattered_rule.shell =
                            Some(expand_pattern(shell, &combo).unwrap_or_else(|_| shell.clone()));
                    }
                    if let Some(ref log) = scattered_rule.log {
                        scattered_rule.log =
                            Some(expand_pattern(log, &combo).unwrap_or_else(|_| log.clone()));
                    }
                    // Script and hooks take the same substitution (issue #98):
                    // a script whose path or content depends on the scatter
                    // variable must resolve per instance (live: the pca rule
                    // had to be split into three explicit rules).
                    expand_command_text_fields(&mut scattered_rule, &rule, |text| {
                        expand_pattern(text, &combo).unwrap_or_else(|_| text.to_string())
                    });

                    // Carry the pre-scatter [[values]] bindings over to the
                    // scattered name and add the scatter variable, so
                    // expand_inputs patterns referencing {assembler} (etc.)
                    // still resolve per instance after the rename.
                    let mut scatter_bindings = self
                        .expansion_values
                        .get(&rule.name)
                        .cloned()
                        .unwrap_or_default();
                    scatter_bindings.insert(scatter.variable.clone(), val.clone());
                    self.expansion_values
                        .insert(scattered_rule.name.clone(), scatter_bindings);
                    self.expansion_templates
                        .insert(scattered_rule.name.clone(), rule.name.clone());

                    scatter_outputs.extend(scattered_rule.output.to_vec());
                    final_rules.push(scattered_rule);
                }

                if let Some(ref gather_rule) = scatter.gather {
                    gather_injections
                        .entry(gather_rule.clone())
                        .or_default()
                        .extend(scatter_outputs);
                }
            } else if let Some(ref transform) = rule.transform {
                // Handle transform operator: split -> map -> combine
                let split_values = self.resolve_split_values(&transform.split)?;

                // Validate that split values are not empty
                if split_values.is_empty() {
                    return Err(OxoFlowError::Validation {
                        message: format!("transform rule '{}' has no split values", rule.name),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "provide values, values_from, n, or glob in split config".to_string(),
                        ),
                    });
                }

                let split_var = &transform.split.by;
                let mut all_chunk_outputs: Vec<String> = Vec::new();

                // Generate map rules for each split value
                for value in &split_values {
                    // Determine chunk output path
                    let chunk_output = if rule.output.is_empty() {
                        format!(".oxo-flow/chunks/{split_var}/{value}.out")
                    } else if rule
                        .output
                        .get_index(0)
                        .map(|o| o.contains(&format!("{{{split_var}}}")))
                        .unwrap_or(false)
                    {
                        // Replace only {split_var} in output
                        rule.output
                            .get_index(0)
                            .unwrap()
                            .replace(&format!("{{{split_var}}}"), value)
                    } else {
                        let base = rule
                            .output
                            .get_index(0)
                            .expect("output non-empty: guarded by is_empty() check");
                        // Keep the full multi-part extension (e.g. "vcf.gz", not
                        // just "gz") so tools can infer the format from the name.
                        let file_part = base.rsplit('/').next().unwrap_or(base);
                        let ext = file_part.split_once('.').map(|(_, e)| e).unwrap_or("out");
                        format!(".oxo-flow/chunks/{split_var}/{value}.{ext}")
                    };

                    all_chunk_outputs.push(chunk_output.clone());

                    let map_rule_name = format!("{}_{}", rule.name, value);
                    // Replace only {split_var} in map shell, keep other placeholders for execution
                    let map_shell = transform.map.replace(&format!("{{{split_var}}}"), value);

                    let mut map_rule = Rule {
                        name: map_rule_name,
                        input: rule.input.clone(),
                        output: vec![chunk_output].into(),
                        shell: Some(map_shell),
                        // Inherit the parent's required semantics (issue
                        // #142 H5): Rule's plain Default makes bools false
                        // while serde defaults `required` to true — an
                        // engine-generated chunk must not silently become
                        // best-effort.
                        required: rule.required,

                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        retries: rule.retries,
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        map_rule.threads = rule.threads;
                        map_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(map_rule);
                }

                // Generate combine rule if specified
                if let Some(ref combine) = transform.combine {
                    let combine_rule_name = format!("{}_combine", rule.name);
                    let combine_shell = if let Some(ref shell) = combine.shell {
                        let chunks_str = all_chunk_outputs.join(" ");
                        shell
                            .replace("{chunks}", &chunks_str)
                            .replace("{input}", &chunks_str)
                            .replace("{output}", &rule.output.join(" "))
                    } else if combine.aggregate {
                        let method = combine.method.as_deref().unwrap_or("concat");
                        let chunks_str = all_chunk_outputs.join(" ");
                        let output_str = rule.output.join(" ");

                        match method {
                            "concat" => {
                                let header = combine
                                    .header
                                    .as_deref()
                                    .map(|h| format!("echo '{}' && ", h))
                                    .unwrap_or_default();
                                format!("{}cat {} > {}", header, chunks_str, output_str)
                            }
                            "json_merge" => {
                                format!("jq -s 'add' {} > {}", chunks_str, output_str)
                            }
                            _ => {
                                return Err(OxoFlowError::Validation {
                                    message: format!("unknown aggregation method: {}", method),
                                    rule: Some(rule.name.clone()),
                                    suggestion: Some("use 'concat' or 'json_merge'".to_string()),
                                });
                            }
                        }
                    } else {
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "transform rule '{}' has combine but no shell or aggregate method",
                                rule.name
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "specify combine.shell or combine.aggregate".to_string(),
                            ),
                        });
                    };

                    let mut combine_rule = Rule {
                        name: combine_rule_name,
                        input: FilePatterns::List(all_chunk_outputs.clone()),
                        output: rule.output.clone(),
                        shell: Some(combine_shell),
                        required: rule.required,
                        cleanup_chunks: transform.cleanup,
                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        combine_rule.threads = rule.threads;
                        combine_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(combine_rule);
                }
            } else {
                final_rules.push(rule);
            }
        }

        // Apply gather injections and expand_inputs
        for rule in &mut final_rules {
            if let Some(injected) = gather_injections.get(&rule.name) {
                let mut current_input = rule.input.to_vec();
                current_input.extend(injected.clone());
                rule.input = FilePatterns::List(current_input);
            }

            // process expand_inputs
            for exp in &rule.expand_inputs {
                let mut variables = HashMap::new();
                for (var_name, var_ref) in &exp.variables {
                    if let Some(vals) = self.resolve_config_list(var_ref) {
                        variables.insert(var_name.clone(), vals);
                    } else if var_ref.starts_with('[') && var_ref.ends_with(']') {
                        let inner = &var_ref[1..var_ref.len() - 1];
                        let vals = inner
                            .split(',')
                            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        variables.insert(var_name.clone(), vals);
                    } else {
                        variables.insert(var_name.clone(), vec![var_ref.clone()]);
                    }
                }

                // Bind this instance's own [[values]] values so `{assembler}`
                // inside the expand pattern resolves per instance — the
                // spades instance never sees the megahit value.
                let bindings = self.expansion_values.get(&rule.name).cloned();
                if let Some(bindings) = &bindings {
                    for (name, value) in bindings {
                        variables
                            .entry(name.clone())
                            .or_insert_with(|| vec![value.clone()]);
                    }
                }

                let mut expanded = crate::wildcard::cartesian_expand(&exp.pattern, &variables);
                // Resolve the `{values.name}` namespace form per instance.
                if let Some(bindings) = &bindings {
                    for path in &mut expanded {
                        *path = crate::wildcard::expand_values_namespace(path, bindings);
                    }
                }
                let mut current_input = rule.input.to_vec();
                current_input.extend(expanded);
                rule.input = FilePatterns::List(current_input);
            }
        }

        self.rules = final_rules;
        Ok(())
    }

    /// Resolve split values from SplitConfig.
    fn resolve_split_values(&self, split: &crate::rule::SplitConfig) -> Result<Vec<String>> {
        // Priority: values > values_from > n > glob
        if !split.values.is_empty() {
            return Ok(split.values.clone());
        }

        if let Some(ref values_from) = split.values_from {
            if let Some(vals) = self.resolve_config_list(values_from) {
                return Ok(vals);
            }
            return Err(OxoFlowError::Validation {
                message: format!("cannot resolve split.values_from: {}", values_from),
                rule: None,
                suggestion: Some("ensure config variable exists and is an array".to_string()),
            });
        }

        if let Some(ref n_str) = split.n {
            // Resolve n from config (config.<key> or bare <key>) or parse as number
            let n = self
                .resolve_config_list(n_str)
                .and_then(|v| v.first().and_then(|s| s.parse::<usize>().ok()))
                .unwrap_or_else(|| n_str.parse::<usize>().unwrap_or(1));
            // Generate chunk indices: 0, 1, 2, ..., n-1
            return Ok((0..n).map(|i| i.to_string()).collect());
        }

        if let Some(ref glob) = split.glob {
            // Glob expansion - find matching files
            let matches: Vec<String> = glob::glob(glob)
                .map_err(|e| OxoFlowError::Validation {
                    message: format!("invalid glob pattern: {}", e),
                    rule: None,
                    suggestion: None,
                })?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            if matches.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: format!("glob pattern '{}' matched no files", glob),
                    rule: None,
                    suggestion: Some("check the glob path and ensure files exist".to_string()),
                });
            }
            return Ok(matches);
        }

        Ok(Vec::new())
    }

    /// Resolve a config variable (e.g., "config.samples") into a list of strings.
    ///
    /// Accepts both the `config.<key>` form and a bare `<key>` reference.
    ///
    /// String values are split on commas (trimmed, empties dropped) so
    /// engine-injected comma-joined lists like `config.samples_list`,
    /// `config.pairs_list`, and `config.samples_<group>` expand per value.
    /// A string without commas
    /// still resolves to a single value; use a single-element array
    /// (e.g. `["a,b"]`) to force a comma-containing string to stay whole.
    pub fn resolve_config_list(&self, var: &str) -> Option<Vec<String>> {
        let key = var.strip_prefix("config.").unwrap_or(var);
        let val = self.config.get(key)?;
        if let Some(arr) = val.as_array() {
            return Some(
                arr.iter()
                    .map(|v| match v {
                        toml::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect(),
            );
        } else if let Some(s) = val.as_str() {
            return Some(
                s.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
        None
    }

    /// Compute a SHA-256 checksum of the workflow configuration for reproducibility.
    ///
    /// The checksum is computed from a deterministic hash of the config,
    /// ensuring consistent results regardless of field ordering.
    pub fn checksum(&self) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::hash::DefaultHasher::new();
        self.workflow.name.hash(&mut hasher);
        self.workflow.version.hash(&mut hasher);
        self.rules.len().hash(&mut hasher);
        for rule in &self.rules {
            rule.name.hash(&mut hasher);
            rule.input.hash(&mut hasher);
            rule.output.hash(&mut hasher);
            rule.shell.hash(&mut hasher);
            rule.script.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// Validate that a reference genome file path has a recognized extension
    /// (`.fa`, `.fasta`, `.fa.gz`, `.fasta.gz`) and optionally check that
    /// it exists on disk.
    #[must_use]
    pub fn validate_reference(path: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let valid_extensions = [".fa", ".fasta", ".fa.gz", ".fasta.gz"];
        let has_valid_ext = valid_extensions.iter().any(|ext| path.ends_with(ext));
        if !has_valid_ext {
            warnings.push(format!(
                "Reference path '{}' does not have a recognized extension (.fa, .fasta, .fa.gz, .fasta.gz)",
                path
            ));
        }
        // Check for .fai index
        let fai_path = format!("{}.fai", path);
        let p = std::path::Path::new(&fai_path);
        if !p.exists() && std::path::Path::new(path).exists() {
            warnings.push(format!(
                "Reference index '{}' not found; you may need to run 'samtools faidx'",
                fai_path
            ));
        }
        warnings
    }

    /// Validate a sample sheet CSV/TSV: check that it has a header row,
    /// no duplicate sample IDs, and at least one data row.
    #[must_use]
    pub fn validate_sample_sheet(content: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            warnings.push("Sample sheet is empty".to_string());
            return warnings;
        }
        // Detect delimiter
        let delimiter = if lines[0].contains('\t') { '\t' } else { ',' };
        let header: Vec<&str> = lines[0].split(delimiter).collect();
        if header.is_empty() {
            warnings.push("Sample sheet header is empty".to_string());
            return warnings;
        }
        if lines.len() < 2 {
            warnings.push("Sample sheet has no data rows".to_string());
            return warnings;
        }
        // Check for duplicate IDs in the first column
        let mut seen = std::collections::HashSet::new();
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(delimiter).collect();
            if let Some(id) = fields.first()
                && !seen.insert(*id)
            {
                warnings.push(format!("Duplicate sample ID '{}' at line {}", id, i + 1));
            }
        }
        warnings
    }

    /// Resolve environment for a rule, checking env_group first.
    ///
    /// Returns the environment spec from the following sources (in order):
    /// 1. `env_groups[group_name]` if `rule.env_group` is set
    /// 2. `rule.environment` if not empty
    /// 3. `defaults.environment` as fallback
    pub fn resolve_environment(&self, rule: &Rule) -> Option<EnvironmentSpec> {
        // Check env_group first
        if let Some(ref group_name) = rule.env_group
            && let Some(env) = self.env_groups.get(group_name)
        {
            return Some(env.clone());
        }
        // Fall back to rule's environment if not empty
        if !rule.environment.is_empty() {
            return Some(rule.environment.clone());
        }
        // Fall back to defaults
        self.defaults.environment.clone()
    }
}

/// Resolve rule template inheritance.
///
/// For each rule with an `extends` field, copy missing fields from the
/// named base rule. Only fields that are at their default values in the
/// child rule are inherited; explicitly set fields are preserved.
///
/// Returns an error if an `extends` target does not exist or if a
/// circular inheritance chain is detected.
pub fn resolve_rule_templates(rules: &mut [crate::rule::Rule]) -> crate::Result<()> {
    // Build a name→index map
    let name_to_idx: std::collections::HashMap<String, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.name.clone(), i))
        .collect();

    // Detect circular inheritance
    for rule in rules.iter() {
        if let Some(ref base_name) = rule.extends {
            let mut visited = std::collections::HashSet::new();
            visited.insert(rule.name.clone());
            let mut current = base_name.clone();
            while let Some(&idx) = name_to_idx.get(&current) {
                if !visited.insert(current.clone()) {
                    return Err(crate::OxoFlowError::Config {
                        message: format!(
                            "circular extends chain detected: rule '{}' extends '{}' which forms a cycle",
                            rule.name, base_name
                        ),
                    });
                }
                match &rules[idx].extends {
                    Some(next) => current = next.clone(),
                    None => break,
                }
            }
        }
    }

    // Resolve templates (iterate by index to avoid borrow issues)
    let snapshot: Vec<crate::rule::Rule> = rules.to_vec();

    for rule in rules.iter_mut() {
        if let Some(ref base_name) = rule.extends.clone() {
            let base_idx =
                name_to_idx
                    .get(base_name)
                    .ok_or_else(|| crate::OxoFlowError::Config {
                        message: format!(
                            "rule '{}' extends '{}' which does not exist",
                            rule.name, base_name
                        ),
                    })?;
            let base = &snapshot[*base_idx];

            // Inherit fields that are at their default values
            if rule.threads.is_none() && base.threads.is_some() {
                rule.threads = base.threads;
            }
            if rule.memory.is_none() && base.memory.is_some() {
                rule.memory = base.memory.clone();
            }
            if rule.resources == crate::rule::Resources::default()
                && base.resources != crate::rule::Resources::default()
            {
                rule.resources = base.resources.clone();
            }
            if rule.environment.is_empty() && !base.environment.is_empty() {
                rule.environment = base.environment.clone();
            }
            if rule.tags.is_empty() && !base.tags.is_empty() {
                rule.tags = base.tags.clone();
            }
            if rule.retries == 0 && base.retries > 0 {
                rule.retries = base.retries;
            }
            if rule.retry_delay.is_none() && base.retry_delay.is_some() {
                rule.retry_delay = base.retry_delay.clone();
            }
            if rule.group.is_none() && base.group.is_some() {
                rule.group = base.group.clone();
            }
            if rule.log.is_none() && base.log.is_some() {
                rule.log = base.log.clone();
            }
            // Inherit params that are not already set
            for (key, value) in &base.params {
                let k: String = key.clone();
                let v: toml::Value = value.clone();
                rule.params.entry(k).or_insert(v);
            }
        }
    }

    Ok(())
}

/// Merge a comma-joined config value with newly consolidated entries.
///
/// Existing entries keep their order and never duplicate; the combined
/// list is then sorted and deduplicated. Shared by the engine-injected
/// `config.samples_list` and `config.pairs_list` consolidation.
fn merge_comma_list(existing: &str, new: &[String]) -> Vec<String> {
    let existing_set: std::collections::HashSet<&str> =
        existing.split(',').filter(|s| !s.is_empty()).collect();
    let mut merged: Vec<String> = existing
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    for s in new {
        if !existing_set.contains(s.as_str()) {
            merged.push(s.clone());
        }
    }
    merged.sort();
    merged.dedup();
    merged
}

/// Deep-merge `src` into `dst` in place (profile override semantics):
/// nested tables recurse — keys from both sides survive, `src` wins on
/// conflict — while scalars and arrays replace `dst` wholesale.
fn deep_merge_value(dst: &mut toml::Value, src: toml::Value) {
    match (dst, src) {
        (toml::Value::Table(dst_table), toml::Value::Table(src_table)) => {
            for (key, src_value) in src_table {
                match dst_table.get_mut(&key) {
                    Some(dst_value) => deep_merge_value(dst_value, src_value),
                    None => {
                        dst_table.insert(key, src_value);
                    }
                }
            }
        }
        (dst_value, src_value) => *dst_value = src_value,
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
    fn apply_defaults_respects_resources_field() {
        // resources.threads / resources.memory (non-deprecated style) must
        // take precedence over [defaults]. A rule that declares only
        // resources.threads = 16 must not be overwritten by defaults.threads.
        let toml_str = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 8
            memory = "16G"

            [[rules]]
            name = "wide_rule"
            shell = "echo wide"

            [rules.resources]
            threads = 16
            memory = "32G"

            [[rules]]
            name = "inherit_rule"
            shell = "echo inherit"
        "#;

        let mut config = WorkflowConfig::parse(toml_str).unwrap();
        config.apply_defaults();

        // wide_rule declares resources.threads=16/resources.memory=32G —
        // defaults must NOT override these.
        let wide = config.get_rule("wide_rule").unwrap();
        assert_eq!(
            wide.effective_threads(),
            16,
            "resources.threads must win over defaults"
        );
        assert_eq!(
            wide.effective_memory(),
            Some("32G"),
            "resources.memory must win over defaults"
        );

        // inherit_rule has neither field — defaults apply.
        let inherit = config.get_rule("inherit_rule").unwrap();
        assert_eq!(inherit.effective_threads(), 8);
        assert_eq!(inherit.effective_memory(), Some("16G"));
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
    fn resolve_includes_with_namespace_and_depends_on() {
        let dir = tempfile::tempdir().unwrap();

        // Included file has rules with internal dependencies
        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "qc_step"
            shell = "fastqc"

            [[rules]]
            name = "trim_step"
            shell = "fastp"
            depends_on = ["qc_step"]
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
        // Find trim_step rule and check its depends_on
        let trim_rule = config
            .rules
            .iter()
            .find(|r| r.name == "qc::trim_step")
            .unwrap();
        assert_eq!(trim_rule.depends_on, vec!["qc::qc_step"]);
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
    fn resolve_includes_with_namespace_external_depends_on() {
        let dir = tempfile::tempdir().unwrap();

        // Included file has rule that depends on external (main workflow) rule
        let included_content = r#"
            [workflow]
            name = "included"

            [[rules]]
            name = "post_process"
            shell = "samtools stats"
            depends_on = ["align"]  # External dependency - should NOT be prefixed
        "#;
        let inc_path = dir.path().join("post.oxoflow");
        std::fs::write(&inc_path, included_content).unwrap();

        let main_content = r#"
            [workflow]
            name = "main"

            [[include]]
            path = "post.oxoflow"
            namespace = "post"

            [[rules]]
            name = "align"
            shell = "bwa"
        "#;

        let mut config: WorkflowConfig = toml::from_str(main_content).unwrap();
        config.resolve_includes(dir.path()).unwrap();

        assert_eq!(config.rules.len(), 2);
        // Find post_process rule and check its depends_on is NOT prefixed
        let post_rule = config
            .rules
            .iter()
            .find(|r| r.name == "post::post_process")
            .unwrap();
        assert_eq!(post_rule.depends_on, vec!["align"]); // Not prefixed because "align" is external
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

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn resolve_includes_depth_limit() {
        let dir = tempfile::tempdir().unwrap();

        // A file that includes itself recurses forever unless the depth guard
        // stops it — each level re-reads the same content from disk.
        let circular = r#"
            [workflow]
            name = "circular"

            [[include]]
            path = "circular.oxoflow"
        "#;
        std::fs::write(dir.path().join("circular.oxoflow"), circular).unwrap();

        let mut config: WorkflowConfig = toml::from_str(circular).unwrap();
        let err = config
            .resolve_includes(dir.path())
            .expect_err("self-including workflow should hit the depth limit");

        let message = err.to_string();
        // The limit must stay high enough for legitimate nested includes —
        // the behavioral check above only proves the guard fires at *some*
        // depth, not that the depth is reasonable.
        assert!(
            MAX_INCLUDE_DEPTH >= 8,
            "include depth limit should be at least 8"
        );
        assert!(
            message.contains(&MAX_INCLUDE_DEPTH.to_string()),
            "error should name the depth limit, got: {message}"
        );
        assert!(
            message.contains("circular includes"),
            "error should point at circular includes, got: {message}"
        );
    }

    #[test]
    fn checksum_deterministic() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#;
        let c1 = WorkflowConfig::parse(toml).unwrap();
        let c2 = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(c1.checksum(), c2.checksum());
    }

    #[test]
    fn checksum_differs_for_different_configs() {
        let c1 = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test1"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#,
        )
        .unwrap();
        let c2 = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test2"
            [[rules]]
            name = "step1"
            shell = "echo hello"
        "#,
        )
        .unwrap();
        assert_ne!(c1.checksum(), c2.checksum());
    }

    #[test]
    fn parse_citation_info() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [citation]
            doi = "10.1234/test"
            url = "https://github.com/example/test"
            authors = ["Alice", "Bob"]
            title = "My Workflow Paper"
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let citation = config.citation.unwrap();
        assert_eq!(citation.doi.as_deref(), Some("10.1234/test"));
        assert_eq!(
            citation.url.as_deref(),
            Some("https://github.com/example/test")
        );
        assert_eq!(citation.authors, vec!["Alice", "Bob"]);
        assert_eq!(citation.title.as_deref(), Some("My Workflow Paper"));
    }

    #[test]
    fn citation_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.citation.is_none());
    }

    #[test]
    fn parse_cluster_profile() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [cluster]
            backend = "slurm"
            partition = "gpu"
            account = "proj123"
            extra_args = ["--exclusive", "--gres=gpu:1"]
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let cluster = config.cluster.unwrap();
        assert_eq!(cluster.backend.as_deref(), Some("slurm"));
        assert_eq!(cluster.partition.as_deref(), Some("gpu"));
        assert_eq!(cluster.account.as_deref(), Some("proj123"));
        assert_eq!(cluster.extra_args, vec!["--exclusive", "--gres=gpu:1"]);
    }

    #[test]
    fn cluster_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.cluster.is_none());
    }

    #[test]
    fn parse_resource_budget() {
        let toml_str = r#"
            [workflow]
            name = "test"

            [resource_budget]
            max_threads = 64
            max_memory = "256G"
            max_jobs = 10
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        let budget = config.resource_budget.unwrap();
        assert_eq!(budget.max_threads, Some(64));
        assert_eq!(budget.max_memory.as_deref(), Some("256G"));
        assert_eq!(budget.max_jobs, Some(10));
    }

    #[test]
    fn resource_budget_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.resource_budget.is_none());
    }

    #[test]
    fn parse_format_version_in_workflow_meta() {
        let toml_str = r#"
            [workflow]
            name = "test"
            format_version = "1.0"
        "#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.workflow.format_version.as_deref(), Some("1.0"));
    }

    #[test]
    fn format_version_defaults_to_none() {
        let config = WorkflowConfig::parse(MINIMAL_WORKFLOW).unwrap();
        assert!(config.workflow.format_version.is_none());
    }

    #[test]
    fn workflow_state_lifecycle() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            [[rules]]
            name = "step1"
            input = ["a.txt"]
            output = ["b.txt"]
            shell = "cat a.txt > b.txt"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let parsed = WorkflowState::new(config);
        assert_eq!(parsed.config().workflow.name, "test");
        let validated = parsed.validate().unwrap();
        assert_eq!(validated.config().workflow.name, "test");
        let ready = validated.prepare().unwrap();
        assert_eq!(ready.config().workflow.name, "test");
    }

    #[test]
    fn validate_reference_valid_path() {
        let warnings = WorkflowConfig::validate_reference("ref.fa");
        assert!(warnings.is_empty() || warnings.iter().all(|w| w.contains("index")));
    }

    #[test]
    fn validate_reference_invalid_extension() {
        let warnings = WorkflowConfig::validate_reference("ref.txt");
        assert!(warnings.iter().any(|w| w.contains("recognized extension")));
    }

    #[test]
    fn validate_sample_sheet_valid() {
        let csv =
            "sample_id,fastq_r1,fastq_r2\nS1,s1_R1.fq.gz,s1_R2.fq.gz\nS2,s2_R1.fq.gz,s2_R2.fq.gz";
        let warnings = WorkflowConfig::validate_sample_sheet(csv);
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_sample_sheet_empty() {
        let warnings = WorkflowConfig::validate_sample_sheet("");
        assert!(warnings.iter().any(|w| w.contains("empty")));
    }

    #[test]
    fn validate_sample_sheet_duplicates() {
        let csv = "sample_id,fastq\nS1,a.fq\nS1,b.fq";
        let warnings = WorkflowConfig::validate_sample_sheet(csv);
        assert!(warnings.iter().any(|w| w.contains("Duplicate")));
    }

    #[test]
    fn variant_classification_display() {
        assert_eq!(VariantClassification::TierI.to_string(), "Tier I");
        assert_eq!(VariantClassification::Vus.to_string(), "VUS");
        assert_eq!(VariantClassification::Benign.to_string(), "Benign");
    }

    #[test]
    fn biomarker_result_display() {
        let br = BiomarkerResult {
            name: "TMB".to_string(),
            value: 12.5,
            unit: "mutations/Mb".to_string(),
            classification: Some("TMB-High".to_string()),
            threshold: Some(10.0),
        };
        let s = br.to_string();
        assert!(s.contains("TMB"));
        assert!(s.contains("12.50"));
        assert!(s.contains("TMB-High"));
    }

    #[test]
    fn qc_threshold_passes() {
        let t = QcThreshold {
            metric: "coverage".to_string(),
            min: Some(30.0),
            max: Some(1000.0),
            description: None,
        };
        assert!(t.passes(50.0));
        assert!(!t.passes(10.0));
        assert!(!t.passes(2000.0));
    }

    #[test]
    fn gene_panel_display() {
        let gp = GenePanel {
            name: "Test Panel".to_string(),
            version: Some("1.0".to_string()),
            genes: vec!["BRCA1".to_string(), "BRCA2".to_string()],
            bed_file: None,
        };
        assert_eq!(gp.to_string(), "Test Panel (2 genes) v1.0");
    }

    #[test]
    fn rule_name_newtype() {
        let rn = RuleName::from("align");
        assert_eq!(rn.to_string(), "align");
        assert_eq!(rn, RuleName("align".to_string()));
    }

    #[test]
    fn wildcard_pattern_newtype() {
        let wp = WildcardPattern::from("{sample}.bam");
        assert_eq!(wp.to_string(), "{sample}.bam");
    }

    #[test]
    fn execution_mode_display() {
        assert_eq!(ExecutionMode::Sequential.to_string(), "sequential");
        assert_eq!(ExecutionMode::Parallel.to_string(), "parallel");
    }

    #[test]
    fn genome_build_in_workflow_meta() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            genome_build = "GRCh38"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(config.workflow.genome_build.as_deref(), Some("GRCh38"));
    }

    #[test]
    fn clinical_report_section_display() {
        assert_eq!(
            ClinicalReportSection::SpecimenInfo.to_string(),
            "Specimen Information"
        );
        assert_eq!(
            ClinicalReportSection::Methodology.to_string(),
            "Methodology"
        );
    }

    #[test]
    fn reference_database_display() {
        let db = ReferenceDatabase {
            name: "GRCh38".to_string(),
            version: Some("p14".to_string()),
            source: None,
            checksum: None,
            accessed_date: None,
        };
        assert_eq!(db.to_string(), "GRCh38 vp14");
    }

    #[test]
    fn reference_database_default() {
        let db = ReferenceDatabase::default();
        assert!(db.name.is_empty());
        assert!(db.version.is_none());
    }

    #[test]
    fn parse_workflow_with_reference_db() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[reference_db]]
            name = "GRCh38"
            version = "p14"
            source = "https://ftp.ncbi.nlm.nih.gov/genomes/all/GCA/000/001/405/GCA_000001405.15_GRCh38/GCA_000001405.15_GRCh38_genomic.fna.gz"
            checksum = "sha256:abc123"

            [[reference_db]]
            name = "dbSNP"
            version = "b156"

            [[rules]]
            name = "align"
            input = ["reads.fastq"]
            output = ["aligned.bam"]
            shell = "bwa mem ref.fa reads.fastq > aligned.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(config.reference_databases.len(), 2);
        assert_eq!(config.reference_databases[0].name, "GRCh38");
        assert_eq!(
            config.reference_databases[1].version,
            Some("b156".to_string())
        );
    }

    #[test]
    fn resolve_rule_templates_basic() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "base_align".to_string(),
                threads: Some(16),
                memory: Some("32G".to_string()),
                environment: crate::rule::EnvironmentSpec {
                    docker: Some("biocontainers/bwa:0.7.17".to_string()),
                    ..Default::default()
                },
                tags: vec!["alignment".to_string()],
                retries: 2,
                ..Default::default()
            },
            crate::rule::Rule {
                name: "align_sample".to_string(),
                extends: Some("base_align".to_string()),
                input: vec!["reads.fq".to_string()].into(),
                output: vec!["aligned.bam".to_string()].into(),
                shell: Some("bwa mem ref.fa {input} > {output}".to_string()),
                ..Default::default()
            },
        ];

        resolve_rule_templates(&mut rules).unwrap();

        let child = &rules[1];
        assert_eq!(child.threads, Some(16));
        assert_eq!(child.memory.as_deref(), Some("32G"));
        assert_eq!(
            child.environment.docker.as_deref(),
            Some("biocontainers/bwa:0.7.17")
        );
        assert_eq!(child.tags, vec!["alignment"]);
        assert_eq!(child.retries, 2);
        // Shell should NOT be inherited (it's set on the child)
        assert_eq!(
            child.shell.as_deref(),
            Some("bwa mem ref.fa {input} > {output}")
        );
    }

    #[test]
    fn resolve_rule_templates_override() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "base".to_string(),
                threads: Some(16),
                memory: Some("32G".to_string()),
                ..Default::default()
            },
            crate::rule::Rule {
                name: "child".to_string(),
                extends: Some("base".to_string()),
                threads: Some(8), // Override
                ..Default::default()
            },
        ];

        resolve_rule_templates(&mut rules).unwrap();

        let child = &rules[1];
        assert_eq!(child.threads, Some(8)); // Kept child's value
        assert_eq!(child.memory.as_deref(), Some("32G")); // Inherited
    }

    #[test]
    fn resolve_rule_templates_missing_base() {
        let mut rules = vec![crate::rule::Rule {
            name: "child".to_string(),
            extends: Some("nonexistent".to_string()),
            ..Default::default()
        }];

        let result = resolve_rule_templates(&mut rules);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn resolve_rule_templates_circular() {
        let mut rules = vec![
            crate::rule::Rule {
                name: "a".to_string(),
                extends: Some("b".to_string()),
                ..Default::default()
            },
            crate::rule::Rule {
                name: "b".to_string(),
                extends: Some("a".to_string()),
                ..Default::default()
            },
        ];

        let result = resolve_rule_templates(&mut rules);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("circular"));
    }

    // ── Transform Operator Tests ───────────────────────────────────────────────

    #[test]
    fn parse_transform_with_split_by_values() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "parallel_qc"
            input = ["sample.bam"]
            threads = 4

            [rules.transform.split]
            by = "chr"
            values = ["chr1", "chr2", "chr3"]

            [rules.transform]
            map = "samtools view -b {input} {chr} > qc/{chr}.bam"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(transform.split.by, "chr");
        assert_eq!(
            transform.split.values,
            vec!["chr1".to_string(), "chr2".to_string(), "chr3".to_string()]
        );
        assert_eq!(
            transform.map,
            "samtools view -b {input} {chr} > qc/{chr}.bam"
        );
        assert!(transform.combine.is_none());
    }

    #[test]
    fn parse_transform_with_values_from() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "variant_calling"
            input = ["sample.bam"]
            output = ["sample.vcf.gz"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "call {input} {chr}"

            [rules.transform.combine]
            shell = "merge {chunks} > {output}"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(
            transform.split.values_from,
            Some("config.chromosomes".to_string())
        );
        let combine = transform.combine.as_ref().unwrap();
        assert_eq!(combine.shell, Some("merge {chunks} > {output}".to_string()));
    }

    #[test]
    fn parse_transform_with_aggregate_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "collect_stats"
            input = ["data.txt"]

            [rules.transform.split]
            by = "chunk"
            n = "5"

            [rules.transform]
            map = "process {input} > .oxo-flow/chunks/{chunk}.txt"

            [rules.transform.combine]
            aggregate = true
            method = "concat"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = &config.rules[0];
        let transform = rule.transform.as_ref().unwrap();
        assert_eq!(transform.split.n, Some("5".to_string()));
        let combine = transform.combine.as_ref().unwrap();
        assert!(combine.aggregate);
        assert_eq!(combine.method, Some("concat".to_string()));
    }

    #[test]
    fn resolve_split_values_from_config() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"

            [config]
            chromosomes = ["chr1", "chr2", "chr3"]

            [[rules]]
            name = "test_rule"
            shell = "echo test"
        "#,
        )
        .unwrap();

        let split = crate::rule::SplitConfig {
            by: "chr".to_string(),
            values: vec![], // empty, use values_from
            values_from: Some("config.chromosomes".to_string()),
            n: None,
            glob: None,
        };

        let values = config.resolve_split_values(&split).unwrap();
        assert_eq!(
            values,
            vec!["chr1".to_string(), "chr2".to_string(), "chr3".to_string()]
        );
    }

    #[test]
    fn resolve_split_values_direct() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "test_rule"
            shell = "echo test"
        "#,
        )
        .unwrap();

        let split = crate::rule::SplitConfig {
            by: "chr".to_string(),
            values: vec!["chr1".to_string(), "chr2".to_string()],
            values_from: None,
            n: None,
            glob: None,
        };

        let values = config.resolve_split_values(&split).unwrap();
        assert_eq!(values, vec!["chr1".to_string(), "chr2".to_string()]);
    }

    #[test]
    fn expand_transform_split_map_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "variant_calling"
            input = ["sample.bam"]
            output = ["sample.vcf.gz"]
            threads = 8

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "gatk HaplotypeCaller -I {input} -L {chr} -O .oxo-flow/chunks/{chr}.g.vcf.gz"

            [rules.transform.combine]
            shell = "gatk GatherVcfs {chunks} -O {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 2 map rules + 1 combine rule = 3 rules
        assert_eq!(config.rules.len(), 3);

        // Check map rules
        let map1 = &config.rules[0];
        assert!(map1.name.contains("chr1"));
        assert!(map1.shell.as_ref().unwrap().contains("chr1"));

        let map2 = &config.rules[1];
        assert!(map2.name.contains("chr2"));
        assert!(map2.shell.as_ref().unwrap().contains("chr2"));

        // Check combine rule
        let combine = &config.rules[2];
        assert!(combine.name.contains("combine"));
        assert!(combine.shell.as_ref().unwrap().contains("GatherVcfs"));
    }

    #[test]
    fn expand_transform_split_map_no_combine() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2", "chr3"]

            [[rules]]
            name = "parallel_qc"
            input = ["sample.bam"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "samtools flagstat {input} > qc/{chr}.flagstat.txt"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 3 map rules (no combine)
        assert_eq!(config.rules.len(), 3);

        // Each rule should have its own output based on chr
        for (i, rule) in config.rules.iter().enumerate() {
            let expected_chr = ["chr1", "chr2", "chr3"][i];
            assert!(rule.name.contains(expected_chr));
        }
    }

    #[test]
    fn expand_transform_keeps_full_extension_in_chunk_outputs() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "variant_calling"
            input = ["aligned/sample.bam"]
            output = ["variants/sample.g.vcf.gz"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "gatk HaplotypeCaller -R {config.reference} -I {input} -L {chr} -O {output}"
            cleanup = true

            [rules.transform.combine]
            shell = "gatk GatherVcfs {chunks} -O {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // 2 map rules + 1 combine rule
        assert_eq!(config.rules.len(), 3);

        // Chunk outputs must keep the full extension so tools like GATK
        // can infer the file format (.g.vcf.gz, not a bare .gz)
        let map1 = &config.rules[0];
        assert_eq!(
            map1.output.to_vec(),
            vec![".oxo-flow/chunks/chr/chr1.g.vcf.gz".to_string()]
        );
        let map2 = &config.rules[1];
        assert_eq!(
            map2.output.to_vec(),
            vec![".oxo-flow/chunks/chr/chr2.g.vcf.gz".to_string()]
        );

        // The combine rule keeps the declared output and consumes the chunks
        let combine = &config.rules[2];
        assert_eq!(
            combine.output.to_vec(),
            vec!["variants/sample.g.vcf.gz".to_string()]
        );

        // cleanup = true propagates to the combine rule; map rules never clean up
        assert!(combine.cleanup_chunks);
        assert!(!map1.cleanup_chunks);
        assert!(!map2.cleanup_chunks);
    }

    #[test]
    fn expand_transform_cleanup_defaults_to_false() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            chromosomes = ["chr1"]

            [[rules]]
            name = "variant_calling"
            input = ["sample.bam"]
            output = ["sample.vcf.gz"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "gatk HaplotypeCaller -L {chr} -O {output}"

            [rules.transform.combine]
            shell = "gatk GatherVcfs {chunks} -O {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        let combine = &config.rules[1];
        assert!(!combine.cleanup_chunks);
    }

    #[test]
    fn sample_pattern_expands_config_vars() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("S1_R1.fastq.gz"), b"x").unwrap();

        let wf_path = dir.path().join("sp.oxoflow");
        std::fs::write(
            &wf_path,
            r#"
            [workflow]
            name = "sp"
            version = "1.0.0"
            sample_pattern = "{config.samples_dir}/{sample}_R1.fastq.gz"

            [config]
            samples_dir = "raw"
            "#,
        )
        .unwrap();
        let config = WorkflowConfig::from_file(&wf_path).unwrap();
        let group = config
            .sample_groups
            .iter()
            .find(|g| g.name == "auto-discovered")
            .expect("auto-discovered group");
        assert_eq!(group.samples, vec!["S1".to_string()]);
    }

    #[test]
    fn filter_samples_first_n_and_explicit() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2", "S3", "S4"]

            [[pairs]]
            pair_id = "P1"
            experiment = "S1"
            control = "S2"

            [[pairs]]
            pair_id = "P2"
            experiment = "S3"
            control = "S4"
        "#;

        // first:N takes the first N samples in workflow order and prunes
        // pairs whose samples were filtered out.
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let (kept, unknown) = config.filter_samples(&["first:2".to_string()]).unwrap();
        assert_eq!(kept, vec!["S1", "S2"]);
        assert!(unknown.is_empty());
        assert_eq!(config.pairs.len(), 1);
        assert_eq!(config.pairs[0].pair_id, "P1");
        assert_eq!(
            config.config.get("samples_list").and_then(|v| v.as_str()),
            Some("S1,S2")
        );
        assert_eq!(
            config.config.get("samples_cohort").and_then(|v| v.as_str()),
            Some("S1,S2")
        );

        // Explicit names combine with first:N and preserve workflow order.
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let (kept, unknown) = config
            .filter_samples(&["first:2".to_string(), "S4".to_string()])
            .unwrap();
        assert_eq!(kept, vec!["S1", "S2", "S4"]);
        assert!(unknown.is_empty());

        // Unknown names are reported, known ones still applied.
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let (kept, unknown) = config.filter_samples(&["S2,S9".to_string()]).unwrap();
        assert_eq!(kept, vec!["S2"]);
        assert_eq!(unknown, vec!["S9"]);
    }

    #[test]
    fn filter_samples_knows_pair_names() {
        // Pair experiment/control names are valid sample identifiers — an
        // explicit selection must not be reported as unknown (issue #63:
        // `--samples ready` feeds pair names into this path).
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[rules]]
            name = "align"
            input = ["data/{experiment}.fq", "data/{control}.fq"]
            output = ["results/{experiment}_{control}.bam"]
            shell = "touch {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let (kept, unknown) = config
            .filter_samples(&["T1".to_string(), "N1".to_string()])
            .unwrap();
        assert!(kept.is_empty()); // pairs-only workflow: kept tracks group samples
        assert!(unknown.is_empty(), "pair names are known: {unknown:?}");
        // Both sides selected → the pair survives filtering.
        assert_eq!(config.pairs.len(), 1);

        // A truly unknown name is still reported.
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let (_, unknown) = config.filter_samples(&["BOGUS".to_string()]).unwrap();
        assert_eq!(unknown, vec!["BOGUS".to_string()]);
    }

    #[test]
    fn filter_samples_invalid_spec() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        assert!(config.filter_samples(&["first:abc".to_string()]).is_err());
    }

    #[test]
    fn override_samples_replaces_inline_samples() {
        // `--samples` explicit names replace inline [[sample_groups]] fixture
        // names instead of filtering them — the fix for "inline samples can't
        // be replaced from the CLI".
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "align"
            input = ["raw/{sample}.fq"]
            output = ["aln/{sample}.bam"]
            shell = "touch {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let kept = config
            .override_samples(&["SRR6357072".to_string(), "SRR6357076".to_string()])
            .unwrap();

        // The explicit list becomes the final set (order preserved).
        assert_eq!(kept, vec!["SRR6357072", "SRR6357076"]);
        // One group remains, reusing the original group name.
        assert_eq!(config.sample_groups.len(), 1);
        assert_eq!(config.sample_groups[0].name, "cohort");
        assert_eq!(
            config.sample_groups[0].samples,
            vec!["SRR6357072".to_string(), "SRR6357076".to_string()]
        );
        // Injected config lists track the new set.
        assert_eq!(
            config.config.get("samples_list").and_then(|v| v.as_str()),
            Some("SRR6357072,SRR6357076")
        );
        assert_eq!(
            config.config.get("samples_cohort").and_then(|v| v.as_str()),
            Some("SRR6357072,SRR6357076")
        );

        // {sample} expansion now binds to the override, not S1/S2.
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let rule_names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        assert!(rule_names.iter().any(|n| n.contains("SRR6357072")));
        assert!(rule_names.iter().any(|n| n.contains("SRR6357076")));
        assert!(
            !rule_names
                .iter()
                .any(|n| n.contains("S1") || n.contains("S2"))
        );
    }

    #[test]
    fn override_samples_dedups_and_prunes_pairs() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[pairs]]
            pair_id = "P1"
            experiment = "S1"
            control = "S2"

            [[pairs]]
            pair_id = "P2"
            experiment = "S3"
            control = "S4"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let kept = config
            .override_samples(&[
                "S3".to_string(),
                "S3".to_string(), // duplicate dropped
                "S4".to_string(),
            ])
            .unwrap();
        assert_eq!(kept, vec!["S3", "S4"]);
        // P1 (S1/S2) is gone; P2 (S3/S4) survives.
        assert_eq!(config.pairs.len(), 1);
        assert_eq!(config.pairs[0].pair_id, "P2");
        assert_eq!(
            config.config.get("pairs_list").and_then(|v| v.as_str()),
            Some("P2")
        );
    }

    #[test]
    fn override_samples_prunes_stale_group_keys() {
        // Override drops the 'case' group — its injected samples_case key
        // must be pruned too, or expand_inputs keeps resolving the stale
        // list (a silent phantom-group reference). Loaded via from_file:
        // the samples_<group> injection happens on file load, not parse.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prune.oxoflow");
        std::fs::write(
            &path,
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[sample_groups]]
            name = "case"
            samples = ["S3"]
            "#,
        )
        .unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        assert!(config.config.contains_key("samples_case"));
        config
            .override_sample_groups(vec![SampleGroup {
                name: "cohort".to_string(),
                samples: vec!["A".to_string(), "B".to_string()],
                metadata: HashMap::new(),
            }])
            .unwrap();
        assert!(
            !config.config.contains_key("samples_case"),
            "stale samples_<group> key must be pruned"
        );
        assert_eq!(
            config
                .config
                .get("samples_cohort")
                .and_then(toml::Value::as_str),
            Some("A,B")
        );
    }

    #[test]
    fn append_sample_groups_merges_and_adds_without_touching_pairs() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[pairs]]
            pair_id = "P1"
            experiment = "S1"
            control = "S2"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let kept = config
            .append_sample_groups(vec![
                SampleGroup {
                    name: "cohort".to_string(),
                    samples: vec!["S2".to_string(), "S3".to_string()], // S2 dup, S3 new
                    metadata: HashMap::new(),
                },
                SampleGroup {
                    name: "case".to_string(),
                    samples: vec!["C1".to_string()],
                    metadata: HashMap::new(),
                },
            ])
            .unwrap();
        // Union with dedup, original order preserved; new group appended.
        assert_eq!(kept, vec!["S1", "S2", "S3", "C1"]);
        assert_eq!(config.sample_groups.len(), 2);
        assert_eq!(
            config.sample_groups[0].samples,
            vec!["S1".to_string(), "S2".to_string(), "S3".to_string()]
        );
        assert_eq!(config.sample_groups[1].name, "case");
        // Pairs untouched: append can only add samples, never drop sides.
        assert_eq!(config.pairs.len(), 1);
        assert_eq!(
            config
                .config
                .get("samples_list")
                .and_then(toml::Value::as_str),
            Some("S1,S2,S3,C1")
        );
        assert_eq!(
            config
                .config
                .get("samples_case")
                .and_then(toml::Value::as_str),
            Some("C1")
        );
    }

    #[test]
    fn override_samples_uses_default_group_name_without_groups() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "align"
            input = ["raw/{sample}.fq"]
            output = ["aln/{sample}.bam"]
            shell = "touch {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let kept = config
            .override_samples(&["A".to_string(), "B".to_string()])
            .unwrap();
        assert_eq!(kept, vec!["A", "B"]);
        assert_eq!(config.sample_groups.len(), 1);
        assert_eq!(config.sample_groups[0].name, "samples");
    }

    #[test]
    fn cleanup_chunks_is_not_settable_from_user_toml() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            input = ["in.bam"]
            output = ["out.bam"]
            shell = "cp {input} {output}"
            cleanup_chunks = true
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        let rule = config.rules.iter().find(|r| r.name == "step1").unwrap();
        // The flag is engine-internal: a user setting it on a plain rule
        // would silently delete their real input files after success.
        assert!(
            !rule.cleanup_chunks,
            "user TOML must not be able to set cleanup_chunks"
        );
    }

    #[test]
    fn temporary_rule_field_parses_and_defaults_false() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "keep"
            shell = "echo keep"

            [[rules]]
            name = "ephemeral"
            output = ["intermediate.bam"]
            shell = "echo ephemeral"
            temporary = true
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert!(
            !config
                .rules
                .iter()
                .find(|r| r.name == "keep")
                .unwrap()
                .temporary,
            "temporary defaults to false"
        );
        assert!(
            config
                .rules
                .iter()
                .find(|r| r.name == "ephemeral")
                .unwrap()
                .temporary,
            "temporary = true parses"
        );
    }

    #[test]
    fn transform_validation_missing_split_values() {
        let toml = r#"
            [workflow]
            name = "test"

            [[rules]]
            name = "bad_transform"
            input = ["sample.bam"]
            output = ["result.txt"]

            [rules.transform.split]
            by = "chr"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            shell = "merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        let result = config.expand_wildcards();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no split values"));
    }

    #[test]
    fn transform_validation_combine_without_shell_or_aggregate() {
        let toml = r###"
            [workflow]
            name = "test"

            [config]
            chromosomes = ["chr1"]

            [[rules]]
            name = "bad_combine"
            input = ["sample.bam"]
            output = ["result.vcf"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            header = "# header without shell"
        "###;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        let result = config.expand_wildcards();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no shell or aggregate method"));
    }

    #[test]
    fn transform_inherits_threads_and_memory() {
        let toml = r#"
            [workflow]
            name = "test"

            [defaults]
            threads = 8
            memory = "16G"

            [config]
            chromosomes = ["chr1", "chr2"]

            [[rules]]
            name = "inherited_transform"
            input = ["sample.bam"]
            output = ["result.vcf"]

            [rules.transform.split]
            by = "chr"
            values_from = "config.chromosomes"

            [rules.transform]
            map = "process {chr}"

            [rules.transform.combine]
            shell = "merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // All expanded rules should inherit defaults
        for rule in &config.rules {
            assert_eq!(rule.threads, Some(8));
            assert_eq!(rule.memory.as_deref(), Some("16G"));
        }
    }

    #[test]
    fn transform_with_aggregate_concat() {
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            chunks = ["part1", "part2"]

            [[rules]]
            name = "aggregate_test"
            input = ["data.txt"]
            output = ["combined.txt"]

            [rules.transform.split]
            by = "part"
            values_from = "config.chunks"

            [rules.transform]
            map = "process > .oxo-flow/chunks/{part}.txt"

            [rules.transform.combine]
            aggregate = true
            method = "concat"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 2 map rules + 1 aggregate rule
        assert_eq!(config.rules.len(), 3);

        // Last rule should be aggregate
        let aggregate_rule = &config.rules[2];
        // Aggregate rule should use concat method
        assert!(aggregate_rule.shell.as_ref().unwrap().contains("cat"));
    }

    #[test]
    fn transform_with_aggregate_json_merge() {
        let toml = r#"
            [workflow]
            name = "test"

            [config]
            chunks = ["part1"]

            [[rules]]
            name = "json_test"
            input = ["data.json"]
            output = ["merged.json"]

            [rules.transform.split]
            by = "part"
            values_from = "config.chunks"

            [rules.transform]
            map = "process > .oxo-flow/chunks/{part}.json"

            [rules.transform.combine]
            aggregate = true
            method = "json_merge"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Should have 1 map rule + 1 aggregate rule = 2 rules (only 1 chunk)
        assert_eq!(config.rules.len(), 2);

        // Aggregate rule should handle json
        let aggregate_rule = &config.rules[1];
        // For json_merge, the shell should use jq
        assert!(aggregate_rule.shell.as_ref().unwrap().contains("jq"));
    }

    #[test]
    fn reference_def_parses_optional_environment() {
        let config: WorkflowConfig = toml::from_str(
            r#"
[workflow]
name = "test"

[[references]]
name = "bowtie2_index"
output = "refs/bowtie2/genome.fa.1.bt2"
build = "bowtie2-build refs/genome.fa refs/bowtie2/genome.fa"

[references.environment]
conda = "envs/bowtie2.yaml"
"#,
        )
        .unwrap();

        let reference = &config.references[0];
        let env = reference.environment.as_ref().expect("environment parsed");
        assert_eq!(env.conda.as_deref(), Some("envs/bowtie2.yaml"));
        // A reference without an environment leaves the field None.
        let bare: WorkflowConfig = toml::from_str(
            r#"
[workflow]
name = "test"

[[references]]
name = "faidx"
output = "refs/genome.fa.fai"
build = "samtools faidx refs/genome.fa"
"#,
        )
        .unwrap();
        assert!(bare.references[0].environment.is_none());
    }

    #[test]
    fn reference_dir_derives_standard_paths() {
        let config: WorkflowConfig = toml::from_str(
            r#"
reference_dir = "/data/GRCh38"

[workflow]
name = "test"
"#,
        )
        .unwrap();

        let derived = config.derive_reference_paths();
        assert_eq!(
            derived.get("reference_fasta"),
            Some(&"/data/GRCh38/genome.fa".to_string())
        );
        assert_eq!(
            derived.get("gene_annotation"),
            Some(&"/data/GRCh38/genes.gtf".to_string())
        );
        assert_eq!(
            derived.get("bwa_index"),
            Some(&"/data/GRCh38/bwa/genome.fa".to_string())
        );
    }

    #[test]
    fn reference_dir_explicit_overrides_derived() {
        let config: WorkflowConfig = toml::from_str(
            r#"
reference_dir = "/data/GRCh38"

[workflow]
name = "test"

[config]
reference_fasta = "/custom/genome.fa"
"#,
        )
        .unwrap();

        let derived = config.derive_reference_paths();
        // Should not derive reference_fasta since it's explicitly set
        assert_eq!(derived.get("reference_fasta"), None);
        // But should still derive others
        assert_eq!(
            derived.get("gene_annotation"),
            Some(&"/data/GRCh38/genes.gtf".to_string())
        );
    }

    #[test]
    fn reference_dir_none_derives_nothing() {
        let config: WorkflowConfig = toml::from_str(
            r#"
[workflow]
name = "test"
"#,
        )
        .unwrap();

        let derived = config.derive_reference_paths();
        assert!(derived.is_empty());
    }

    #[test]
    fn config_def_declarative_syntax() {
        let toml_str = r#"
[workflow]
name = "test"
version = "1.0.0"

[config]
database = { required = true, help = "Path to DB" }
threshold = { default = "1e-5", help = "E-value" }

[[rules]]
name = "s"
output = ["out.txt"]
shell = "echo {config.database} > {output[0]}"
"#;
        let config = WorkflowConfig::parse(toml_str).unwrap();
        assert_eq!(config.config_meta.len(), 2);
        assert!(config.config_meta["database"].required);
        assert_eq!(
            config.config_meta["database"].help.as_deref(),
            Some("Path to DB")
        );
        assert_eq!(
            config.config_meta["threshold"].default.as_deref(),
            Some("1e-5")
        );
        assert!(!config.config_meta["threshold"].required);
        // Config values are resolved from defaults when no CLI override
        assert_eq!(
            config.config.get("database").and_then(|v| v.as_str()),
            Some("") // required, no default → empty string
        );
        assert_eq!(
            config.config.get("threshold").and_then(|v| v.as_str()),
            Some("1e-5")
        );
    }

    #[test]
    fn sensitive_only_inline_config_registers_metadata() {
        // issue #99 B1: the declarative-config promotion trigger was
        // default/required/help only, so a sensitive-ONLY declaration
        // silently stayed an unparsed inline table and the value was never
        // masked. The declaration must register its metadata; the value
        // itself comes from a CLI override or profile at run time.
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            api_token = { sensitive = true }
            "#,
        )
        .unwrap();
        assert!(
            config.config_meta["api_token"].sensitive,
            "sensitive flag must register in config_meta"
        );
        assert_eq!(
            config.config.get("api_token").and_then(|v| v.as_str()),
            Some(""),
            "no default: the runtime value is empty until overridden"
        );
    }

    #[test]
    fn expansion_templates_track_the_fan_out_source() {
        // Issue #74 phase 3: array grouping needs the TEMPLATE name each
        // expanded instance came from. The expansion records it for every
        // fan-out path (scatter, values, pairs) so the cluster driver never
        // guesses from instance-name suffixes.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[values]]
            name = "assembler"
            values = ["spades"]

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "align"
            output = ["out/{pair_id}/{assembler}.bam"]
            shell = "echo hi"

            [[rules]]
            name = "qc"
            scatter = { variable = "treatment", values = ["control", "treated"] }
            output = ["qc/{treatment}.tsv"]
            shell = "echo hi"

            [[rules]]
            name = "plain"
            output = ["p.txt"]
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();

        // The pair-expanded instance maps back to "align".
        let align_instance = config
            .rules
            .iter()
            .find(|r| r.name.starts_with("align_"))
            .expect("pair instance must exist");
        assert_eq!(
            config.template_of(&align_instance.name),
            Some("align"),
            "pair-expanded instances must track their template"
        );

        // Scatter instances map back to "qc".
        assert_eq!(config.template_of("qc_control"), Some("qc"));
        assert_eq!(config.template_of("qc_treated"), Some("qc"));

        // A rule that never fanned out has no template entry.
        assert_eq!(config.template_of("plain"), None);
    }

    #[test]
    fn module_closure_includes_contract_input_producers() {
        // Issue #112 elasticity: `--module` must include the host rules
        // producing the module's declared concrete inputs, so a partial
        // run of the module alone has everything it needs wired.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("m.oxoflow"),
            r#"[workflow]
name = "m"
version = "1.0.0"

[[rules]]
name = "step"
input = ["raw.fq"]
output = ["out.bam"]
shell = "true"
"#,
        )
        .unwrap();
        let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "m.oxoflow"
name = "mapper"
inputs = ["raw.fq"]
outputs = ["out.bam"]

[[rules]]
name = "fetch"
output = ["raw.fq"]
shell = "true"

[[rules]]
name = "unrelated"
output = ["u.txt"]
shell = "true"
"#;
        let wf = dir.path().join("host.oxoflow");
        std::fs::write(&wf, host).unwrap();
        let config = WorkflowConfig::from_file(&wf).unwrap();
        let closure = config.module_closure("mapper").expect("module exists");
        assert!(
            closure.contains(&"step".to_string()),
            "module rules: {closure:?}"
        );
        assert!(
            closure.contains(&"fetch".to_string()),
            "the declared-input producer must join the closure: {closure:?}"
        );
        assert!(
            !closure.contains(&"unrelated".to_string()),
            "unrelated rules must stay out: {closure:?}"
        );
    }

    #[test]
    fn include_from_git_repo_resolves_path_inside_checkout() {
        // Issue #112: includes may come from a pinned git repository
        // (repo + ref + path) — the versioned-module composition story.
        // A LOCAL git repo keeps the test network-free (file:// clone).
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("mods");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("qc.oxoflow"),
            r#"[workflow]
name = "qc"
version = "1.0.0"

[[rules]]
name = "fastqc"
output = ["qc.html"]
shell = "true"
"#,
        )
        .unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
        };
        git(&["init"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "qc.oxoflow"]);
        git(&["commit", "-m", "qc"]);
        git(&["tag", "v1.0.0"]);

        let host = format!(
            r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
repo = "file://{}"
ref = "v1.0.0"
path = "qc.oxoflow"

[[rules]]
name = "use"
input = ["qc.html"]
output = ["u.txt"]
shell = "true"
"#,
            repo.display()
        );
        let wf = dir.path().join("host.oxoflow");
        std::fs::write(&wf, host).unwrap();
        let config = WorkflowConfig::from_file(&wf).unwrap();
        assert!(
            config.rules.iter().any(|r| r.name == "fastqc"),
            "the module's rule must resolve from the pinned repo"
        );
    }

    #[test]
    fn include_contract_unwired_input_is_an_error() {
        // Issue #112 module slice: a module that DECLARES an input nobody
        // produces must fail validation with the wiring gap named — instead
        // of the rule dying at runtime on a missing file.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("qc.oxoflow"),
            r#"[workflow]
name = "qc"
version = "1.0.0"

[[rules]]
name = "fastqc"
input = ["raw/sample.fq"]
output = ["qc/sample.html"]
shell = "true"
"#,
        )
        .unwrap();
        let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/sample.fq"]
outputs = ["qc/sample.html"]

[[rules]]
name = "final"
input = ["qc/sample.html"]
output = ["final.txt"]
shell = "true"
"#;
        let wf = dir.path().join("host.oxoflow");
        std::fs::write(&wf, host).unwrap();
        let err = WorkflowConfig::from_file(&wf).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("raw/sample.fq"),
            "the error must name the unwired input: {msg}"
        );
    }

    #[test]
    fn include_contract_checks_declared_outputs_and_encapsulation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("qc.oxoflow"),
            r#"[workflow]
name = "qc"
version = "1.0.0"

[[rules]]
name = "fastqc"
input = ["raw/sample.fq"]
output = ["qc/sample.html"]
shell = "true"

[[rules]]
name = "internal"
input = ["qc/sample.html"]
output = ["qc/tmp.bin"]
shell = "true"
"#,
        )
        .unwrap();
        // (a) a declared output that no module rule produces = error
        let bad_host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/sample.fq"]
outputs = ["qc/nope.html"]

[[rules]]
name = "rawmaker"
output = ["raw/sample.fq"]
shell = "true"
"#;
        let wf = dir.path().join("bad.oxoflow");
        std::fs::write(&wf, bad_host).unwrap();
        let err = WorkflowConfig::from_file(&wf).unwrap_err();
        assert!(
            format!("{err}").contains("qc/nope.html"),
            "the error must name the unproduced declared output"
        );

        // (b) host reading an UNDECLARED module-internal file = warning
        let host2 = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/sample.fq"]
outputs = ["qc/sample.html"]

[[rules]]
name = "rawmaker"
output = ["raw/sample.fq"]
shell = "true"

[[rules]]
name = "peeker"
input = ["qc/tmp.bin"]
output = ["peek.txt"]
shell = "true"
"#;
        let wf2 = dir.path().join("ok.oxoflow");
        std::fs::write(&wf2, host2).unwrap();
        let config = WorkflowConfig::from_file(&wf2).unwrap();
        let (errors, warnings) = config.check_include_contracts();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(
            warnings.iter().any(|w| w.contains("qc/tmp.bin")),
            "encapsulation warning must name the internal file: {warnings:?}"
        );
    }

    #[test]
    fn include_contract_params_fill_in_config_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mod.oxoflow"),
            r#"[workflow]
name = "mod"
version = "1.0.0"

[[rules]]
name = "step"
output = ["o.txt"]
shell = "echo {config.threads} > o.txt"
"#,
        )
        .unwrap();
        let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"
outputs = ["o.txt"]
params = { threads = "8" }

[[rules]]
name = "use"
input = ["o.txt"]
output = ["u.txt"]
shell = "true"
"#;
        let wf = dir.path().join("host.oxoflow");
        std::fs::write(&wf, host).unwrap();
        let config = WorkflowConfig::from_file(&wf).unwrap();
        assert_eq!(
            config.config.get("threads").and_then(toml::Value::as_str),
            Some("8"),
            "params defaults must fill in config keys"
        );
    }

    #[test]
    fn include_without_contract_is_unchanged() {
        // Backward compatibility: includes that declare no interface fields
        // trigger none of the new checks.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mod.oxoflow"),
            r#"[workflow]
name = "mod"
version = "1.0.0"

[[rules]]
name = "step"
output = ["o.txt"]
shell = "true"
"#,
        )
        .unwrap();
        let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"

[[rules]]
name = "use"
input = ["o.txt"]
output = ["u.txt"]
shell = "true"
"#;
        let wf = dir.path().join("host.oxoflow");
        std::fs::write(&wf, host).unwrap();
        let config = WorkflowConfig::from_file(&wf).unwrap();
        let (errors, warnings) = config.check_include_contracts();
        assert!(
            errors.is_empty() && warnings.is_empty(),
            "{errors:?} {warnings:?}"
        );
    }

    #[test]
    fn defaults_shell_prelude_parses_and_applies() {
        // issue #92: a workflow-global shell prelude (e.g. set -euo
        // pipefail) is opt-in, parsed from [defaults], and prepended to a
        // command on its own line.
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [defaults]
            shell_prelude = "set -euo pipefail"

            [[rules]]
            name = "s"
            output = ["out.txt"]
            shell = "echo hi > out.txt"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.defaults.shell_prelude.as_deref(),
            Some("set -euo pipefail")
        );
        assert_eq!(
            config.defaults.apply_shell_prelude("echo hi > out.txt"),
            "set -euo pipefail\necho hi > out.txt"
        );

        // No prelude: the command passes through unchanged.
        let plain = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "s"
            output = ["out.txt"]
            shell = "echo hi > out.txt"
            "#,
        )
        .unwrap();
        assert_eq!(
            plain.defaults.apply_shell_prelude("echo hi > out.txt"),
            "echo hi > out.txt"
        );
    }

    #[test]
    fn resolve_config_list_splits_comma_joined_strings() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"

            [config]
            plain = "single_value"
            comma_list = "S1,S2,S3"
            messy_list = " S1, S2 ,,S3,"
            string_array = ["A", "B"]
            "#,
        )
        .unwrap();

        // Strings without commas keep behaving as a single value.
        assert_eq!(
            config.resolve_config_list("config.plain"),
            Some(vec!["single_value".to_string()])
        );
        // Comma-joined strings split into individual values (the form the
        // engine uses for config.samples_list / config.samples_<group>).
        assert_eq!(
            config.resolve_config_list("config.comma_list"),
            Some(vec!["S1".to_string(), "S2".to_string(), "S3".to_string()])
        );
        // Entries are trimmed and empty segments dropped.
        assert_eq!(
            config.resolve_config_list("config.messy_list"),
            Some(vec!["S1".to_string(), "S2".to_string(), "S3".to_string()])
        );
        // Arrays resolve unchanged.
        assert_eq!(
            config.resolve_config_list("config.string_array"),
            Some(vec!["A".to_string(), "B".to_string()])
        );
        // Bare keys (without the config. prefix) work too.
        assert_eq!(
            config.resolve_config_list("comma_list"),
            Some(vec!["S1".to_string(), "S2".to_string(), "S3".to_string()])
        );
    }

    #[test]
    fn expand_inputs_resolves_injected_samples_list_per_sample() {
        // Mirrors examples/gallery/07_wgs_germline.oxoflow: the sample
        // group is the single source of truth and expand_inputs consumes
        // the auto-injected config.samples_list (a comma-joined string).
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("wgs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "wgs"

            [[sample_groups]]
            name = "cohort"
            samples = ["NA12878", "NA12879", "NA12880"]

            [[rules]]
            name = "combine_gvcfs"
            input = []
            expand_inputs = [
                { pattern = "variants/{sample}.g.vcf.gz", variables = { sample = "config.samples_list" } }
            ]
            output = ["variants/cohort.g.vcf.gz"]
            shell = "gatk CombineGVCFs {input} -O {output[0]}"
            "#,
        )
        .unwrap();

        let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        let combine = config
            .rules
            .iter()
            .find(|r| r.name == "combine_gvcfs")
            .expect("combine_gvcfs rule should survive expansion");
        assert_eq!(
            combine.input.to_vec(),
            vec![
                "variants/NA12878.g.vcf.gz".to_string(),
                "variants/NA12879.g.vcf.gz".to_string(),
                "variants/NA12880.g.vcf.gz".to_string(),
            ]
        );
    }

    #[test]
    fn from_file_injects_pairs_list_from_pairs() {
        // [[pairs]] is the single source of truth: the engine injects
        // config.pairs_list (a sorted, comma-joined string) exactly like
        // config.samples_list, so rules can reference `{config.pairs_list}`
        // instead of hand-writing `[config] pair_ids = [...]`.
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"

            [[pairs]]
            pair_id = "CASE_002"
            experiment = "EXP_02"
            control = "CTR_02"

            [[pairs]]
            pair_id = "CASE_001"
            experiment = "EXP_01"
            control = "CTR_01"

            [[rules]]
            name = "step1"
            shell = "echo hi"
            "#,
        )
        .unwrap();

        let config = WorkflowConfig::from_file(&workflow_path).unwrap();
        // Sorted, comma-joined — the string `{config.pairs_list}` renders.
        assert_eq!(
            config
                .config
                .get("pairs_list")
                .and_then(toml::Value::as_str),
            Some("CASE_001,CASE_002")
        );
        // resolve_config_list splits the injected list per value.
        assert_eq!(
            config.resolve_config_list("config.pairs_list"),
            Some(vec!["CASE_001".to_string(), "CASE_002".to_string(),])
        );
    }

    #[test]
    fn from_file_feeds_pair_members_into_samples_list() {
        // [[pairs]] members are samples too: a pairs-only workflow renders
        // {config.samples_list} as a literal before this (live: pair-driven
        // workflow, shell probe showed the unexpanded placeholder) — the
        // consolidated list only collected sample_groups members.
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[pairs]]
            pair_id = "P2"
            experiment = "T2"

            [[rules]]
            name = "step1"
            shell = "echo hi"
            "#,
        )
        .unwrap();

        let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        // Deduplicated experiment+control names, sorted (merge_comma_list
        // sorts the consolidated list, same as pairs_list).
        assert_eq!(
            config
                .config
                .get("samples_list")
                .and_then(toml::Value::as_str),
            Some("N1,T1,T2")
        );
        assert_eq!(
            config.resolve_config_list("config.samples_list"),
            Some(vec!["N1".to_string(), "T1".to_string(), "T2".to_string(),])
        );
    }

    #[test]
    fn from_file_injects_pairs_list_merging_user_value_and_pairs_file() {
        // Manually declared config.pairs_list entries survive (merged like
        // samples_list) and pairs_file entries are included too.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pairs.tsv"),
            "pair_id\texperiment\tcontrol\nP3\tT3\tN3\nP4\tT4\tN4\n",
        )
        .unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"
            pairs_file = "pairs.tsv"

            [config]
            pairs_list = "P1,P2"

            [[pairs]]
            pair_id = "P2"
            experiment = "T2"
            control = "N2"

            [[rules]]
            name = "step1"
            shell = "echo hi"
            "#,
        )
        .unwrap();

        let config = WorkflowConfig::from_file(&workflow_path).unwrap();
        assert_eq!(
            config
                .config
                .get("pairs_list")
                .and_then(toml::Value::as_str),
            Some("P1,P2,P3,P4")
        );
    }

    #[test]
    fn expand_inputs_resolves_injected_pairs_list_per_pair() {
        // Mirrors the samples_list test: [[pairs]] is the single source of
        // truth and expand_inputs consumes the auto-injected
        // config.pairs_list (a comma-joined string).
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"

            [[pairs]]
            pair_id = "CASE_001"
            experiment = "EXP_01"
            control = "CTR_01"

            [[pairs]]
            pair_id = "CASE_002"
            experiment = "EXP_02"
            control = "CTR_02"

            [[rules]]
            name = "combine_calls"
            input = []
            expand_inputs = [
                { pattern = "calls/{pair_id}.vcf.gz", variables = { pair_id = "config.pairs_list" } }
            ]
            output = ["calls/cohort.vcf.gz"]
            shell = "bcftools concat {input} -O z -o {output[0]}"
            "#,
        )
        .unwrap();

        let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        let combine = config
            .rules
            .iter()
            .find(|r| r.name == "combine_calls")
            .expect("combine_calls rule should survive expansion");
        assert_eq!(
            combine.input.to_vec(),
            vec![
                "calls/CASE_001.vcf.gz".to_string(),
                "calls/CASE_002.vcf.gz".to_string(),
            ]
        );
    }

    #[test]
    fn filter_samples_syncs_injected_pairs_list() {
        // --samples filtering drops pairs whose side samples are excluded;
        // config.pairs_list must follow (mirrors the samples_list rewrite).
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[pairs]]
            pair_id = "P2"
            experiment = "T2"
            control = "N2"

            [[rules]]
            name = "step1"
            shell = "echo hi"
            "#,
        )
        .unwrap();

        let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
        let (kept, unknown) = config
            .filter_samples(&["T1".to_string(), "N1".to_string()])
            .unwrap();
        assert!(kept.is_empty());
        assert!(unknown.is_empty());
        assert_eq!(config.pairs.len(), 1);
        assert_eq!(
            config
                .config
                .get("pairs_list")
                .and_then(toml::Value::as_str),
            Some("P1")
        );
    }

    #[test]
    fn filter_samples_clears_injected_lists_when_everything_dropped() {
        // A filter that drops EVERY pair/sample must clear the injected
        // pairs_list/samples_list — expand_inputs resolving against the
        // stale list would target rules that no longer exist.
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("pairs.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "pairs"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[rules]]
            name = "step1"
            shell = "echo hi"
            "#,
        )
        .unwrap();

        let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
        assert_eq!(
            config
                .config
                .get("pairs_list")
                .and_then(toml::Value::as_str),
            Some("P1")
        );
        let (kept, _) = config.filter_samples(&["T9".to_string()]).unwrap();
        assert!(kept.is_empty());
        assert!(config.pairs.is_empty());
        assert_eq!(
            config
                .config
                .get("pairs_list")
                .and_then(toml::Value::as_str),
            Some("")
        );
    }

    #[test]
    fn merge_profile_tolerates_quoted_threads_in_defaults() {
        // Profiles historically tolerated quoted numerics in [defaults]
        // (`threads = "16"`): coercion keeps that tolerance, while a
        // genuinely wrong type still fails with the same clear error.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [defaults]
            threads = 8

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [defaults]
            threads = "16"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();
        config.apply_defaults();
        assert_eq!(config.rules[0].threads, Some(16));

        let bad: toml::Value = toml::from_str(
            r#"
            [defaults]
            threads = "lots"
            "#,
        )
        .unwrap();
        let err = WorkflowConfig::parse(toml).unwrap().merge_profile(&bad);
        assert!(err.is_err(), "non-numeric quoted threads must fail");
    }

    #[test]
    fn values_name_colliding_with_executor_placeholder_rejected() {
        // A [[values]] table named like an executor placeholder (`input`,
        // `output`, `log`, `threads`, `memory`) would replace the
        // placeholder in every rule's shell — expansion must reject it
        // (run/dry-run both expand before executing).
        for name in ["input", "output", "log", "threads", "memory"] {
            let toml = format!(
                r#"
                [workflow]
                name = "test"
                version = "1.0.0"

                [[values]]
                name = "{name}"
                values = ["a", "b"]

                [[rules]]
                name = "step1"
                output = ["out.txt"]
                shell = "echo hi"
                "#
            );
            let mut config = WorkflowConfig::parse(&toml).unwrap();
            let err = config.expand_wildcards().unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains("collides with a built-in wildcard"),
                "{name}: {message}"
            );
        }
    }

    #[test]
    fn reference_keyed_injection_resolves_cross_references_any_order() {
        // A reference whose output embeds another reference's keyed config
        // resolves regardless of declaration order (fixpoint expansion).
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[references]]
            name = "genome_bwa"
            source = "refs/genome.fa"
            output = "{config.genome}.bwt"
            build = "bwa_index"

            [[references]]
            name = "genome"
            source = "refs/genome.fa"
            output = "refs/genome.fa"
            build = "cp refs/genome.fa refs/genome.fa.idx"

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(
            config
                .config
                .get("genome_bwa")
                .and_then(toml::Value::as_str),
            Some("refs/genome.fa.bwt"),
            "cross-reference must expand despite later declaration"
        );
    }

    #[test]
    fn scatter_keeps_values_bindings_for_expand_inputs() {
        // scatter renames the instance, which used to orphan the per-name
        // [[values]] bindings — expand_inputs patterns referencing the
        // value stayed literal. The bindings must ride along.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[values]]
            name = "assembler"
            values = ["spades"]

            [[rules]]
            name = "combine"
            scatter = { variable = "b", values = ["1", "2"] }
            expand_inputs = [{ pattern = "asm/{assembler}/x.txt", variables = {} }]
            output = ["out/{assembler}/{b}.txt"]
            shell = "cat {input} > {output}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();
        let rule = config
            .rules
            .iter()
            .find(|r| r.name.contains("spades") && r.name.ends_with("_1"))
            .expect("scattered instance must exist");
        assert!(
            rule.input
                .to_vec()
                .contains(&"asm/spades/x.txt".to_string()),
            "{{assembler}} must resolve per instance, got {:?}",
            rule.input.to_vec()
        );
    }

    #[test]
    fn log_field_expands_wildcards_per_instance() {
        // log = "logs/{assembler}.log" must expand per [[values]] instance
        // (and per pair) — every instance writing to the same literal
        // brace path would corrupt the log contract.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "do"
            output = ["out/{assembler}/{pair_id}.txt"]
            log = "logs/{assembler}/{pair_id}.log"
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();
        let logs: std::collections::BTreeSet<String> =
            config.rules.iter().filter_map(|r| r.log.clone()).collect();
        assert_eq!(
            logs,
            [
                "logs/megahit/P1.log".to_string(),
                "logs/spades/P1.log".to_string()
            ]
            .into_iter()
            .collect(),
            "every instance must own its log path: {logs:?}"
        );
    }

    #[test]
    fn scatter_expands_script_and_hooks_with_scatter_variable() {
        // issue #98: the scatter variable must substitute into script (and
        // the hook fields) per instance — shell/log were the only text
        // fields covered before. Live: the star-deseq2 pca rule had to be
        // split into three explicit rules because the per-treatment script
        // invocation could not be expressed.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[rules]]
            name = "pca"
            scatter = { variable = "treatment", values = ["control", "treated"] }
            output = ["pca/{treatment}.tsv"]
            script = "scripts/pca_{treatment}.R --out {treatment}.tsv"
            pre_exec = "mkdir -p tmp/{treatment}"
            on_success = "echo done {treatment}"
            on_failure = "echo failed {treatment}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();

        assert_eq!(
            config.rules.len(),
            2,
            "scatter over 2 values must produce 2 instances"
        );
        for treatment in ["control", "treated"] {
            let rule = config
                .rules
                .iter()
                .find(|r| r.name == format!("pca_{treatment}"))
                .unwrap_or_else(|| panic!("scattered instance pca_{treatment} must exist"));
            assert_eq!(
                rule.script.as_deref(),
                Some(format!("scripts/pca_{treatment}.R --out {treatment}.tsv").as_str()),
                "script must carry the per-instance scatter value"
            );
            assert_eq!(
                rule.pre_exec.as_deref(),
                Some(format!("mkdir -p tmp/{treatment}").as_str())
            );
            assert_eq!(
                rule.on_success.as_deref(),
                Some(format!("echo done {treatment}").as_str())
            );
            assert_eq!(
                rule.on_failure.as_deref(),
                Some(format!("echo failed {treatment}").as_str())
            );
        }
    }

    #[test]
    fn values_expansion_expands_script_per_instance() {
        // Same class as issue #98 on the [[values]] fan-out path: script
        // must carry the per-value substitution, not only shell/log.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[rules]]
            name = "asm"
            output = ["out/{assembler}.fa"]
            script = "scripts/asm_{assembler}.R"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();
        let scripts: std::collections::BTreeSet<String> = config
            .rules
            .iter()
            .filter_map(|r| r.script.clone())
            .collect();
        assert_eq!(
            scripts,
            [
                "scripts/asm_megahit.R".to_string(),
                "scripts/asm_spades.R".to_string()
            ]
            .into_iter()
            .collect(),
            "every value instance must own its script invocation: {scripts:?}"
        );
    }

    #[test]
    fn pair_expansion_expands_script_and_hooks_per_pair() {
        // Same class as issue #98 on the pairs path: {pair_id} must
        // substitute into script/hooks per instance.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "do"
            output = ["out/{pair_id}.txt"]
            script = "scripts/run_{pair_id}.R"
            on_success = "echo ok {pair_id}"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();
        let scripts: std::collections::BTreeSet<String> = config
            .rules
            .iter()
            .filter_map(|r| r.script.clone())
            .collect();
        assert_eq!(
            scripts,
            ["scripts/run_P1.R".to_string()].into_iter().collect(),
            "the pair instance must own its script invocation: {scripts:?}"
        );
        let hooks: std::collections::BTreeSet<String> = config
            .rules
            .iter()
            .filter_map(|r| r.on_success.clone())
            .collect();
        assert_eq!(hooks, ["echo ok P1".to_string()].into_iter().collect());
    }

    #[test]
    fn script_only_wildcards_do_not_trigger_fan_out() {
        // The fan-out trigger set is input/output/shell only. A rule whose
        // ONLY wildcard use is the script field must stay a single rule —
        // cloning it would duplicate the whole rule execution over
        // identical paths, and `${name}` bash spellings inside script must
        // never be mistaken for wildcards. Script substitution applies
        // when the rule fans out through its path fields (issue #98).
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "E1"
            control = "C1"

            [[rules]]
            name = "s"
            script = "scripts/run_${pair_id}.R"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.expand_wildcards().unwrap();
        assert_eq!(
            config.rules.len(),
            1,
            "script-only wildcard usage must not clone the rule"
        );
        assert_eq!(
            config.rules[0].script.as_deref(),
            Some("scripts/run_${pair_id}.R"),
            "a non-fanned rule keeps its script untouched"
        );
    }

    #[test]
    fn merge_profile_fill_mode_preserves_workflow_keys() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [config]
            threads = "8"
            genome = "hg38"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [config]
            threads = "32"
            scheduler = "slurm"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();

        // fill mode: existing workflow keys win, missing keys are filled in.
        assert_eq!(config.config["threads"].as_str(), Some("8"));
        assert_eq!(config.config["scheduler"].as_str(), Some("slurm"));
        assert_eq!(config.config["genome"].as_str(), Some("hg38"));
    }

    #[test]
    fn merge_profile_override_mode_replaces_scalars_and_keeps_workflow_only_keys() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [config]
            threads = "8"
            genome = "hg38"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [config]
            threads = "32"
            scheduler = "slurm"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();

        assert_eq!(config.config["threads"].as_str(), Some("32"));
        assert_eq!(config.config["scheduler"].as_str(), Some("slurm"));
        assert_eq!(config.config["genome"].as_str(), Some("hg38"));
    }

    #[test]
    fn cluster_profile_merge_carries_max_array_size() {
        // M4 (#142): profile-level max_array_size was silently dropped by
        // merge_from — the driver always fell back to its default chunking.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [cluster]
            backend = "slurm"
            max_array_size = 25
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [cluster]
            max_array_size = 50
            poll_interval = "10s"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();
        let cluster = config.cluster.as_ref().expect("cluster block present");
        assert_eq!(
            cluster.max_array_size,
            Some(50),
            "override mode must replace"
        );
        assert_eq!(cluster.poll_interval.as_deref(), Some("10s"));
        assert_eq!(
            cluster.backend.as_deref(),
            Some("slurm"),
            "other keys intact"
        );
    }

    #[test]
    fn cluster_profile_merge_fill_mode_keeps_own_max_array_size() {
        // Fill mode (default): a profile value must not clobber a value the
        // workflow already declares.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [cluster]
            backend = "slurm"
            max_array_size = 25
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [cluster]
            max_array_size = 50
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();
        let cluster = config.cluster.as_ref().expect("cluster block present");
        assert_eq!(
            cluster.max_array_size,
            Some(25),
            "fill mode keeps the workflow's own value"
        );
    }

    #[test]
    fn transform_chunks_inherit_required_from_the_parent_rule() {
        // H5 (#142): engine-generated map/combine chunk rules were built
        // with Rule::default() — bools false — so a required=true transform
        // produced best-effort chunks whose failure exited 0. The serde
        // default for `required` is true; the plain Default is false.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "t"
            required = false
            output = ["combined.txt"]
            transform = { split = { by = "part", values = ["a", "b"] },
                          map = "echo {part} > chunk",
                          combine = { shell = "cat {chunks} > {output}" } }
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        for name in ["t_a", "t_b", "t_combine"] {
            let r = config
                .get_rule(name)
                .unwrap_or_else(|| panic!("{name} generated"));
            assert!(!r.required, "{name} must inherit required=false");
        }
    }

    #[test]
    fn transform_chunks_default_to_required_like_the_parent() {
        // Default parent (serde: required=true) → chunks must be required.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[rules]]
            name = "t"
            output = ["combined.txt"]
            transform = { split = { by = "part", values = ["a", "b"] },
                          map = "echo {part} > chunk",
                          combine = { shell = "cat {chunks} > {output}" } }
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        for name in ["t_a", "t_b", "t_combine"] {
            let r = config
                .get_rule(name)
                .unwrap_or_else(|| panic!("{name} generated"));
            assert!(r.required, "{name} must inherit required=true");
        }
    }

    #[test]
    fn merge_profile_override_mode_deep_merges_nested_tables() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [config]
            tool = { threads = "8", mem = "4G" }
            genome = "hg38"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [config]
            tool = { threads = "32" }
            scheduler = "slurm"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();

        // Nested table deep-merges: profile's threads wins, workflow's mem
        // survives, sibling keys untouched.
        let tool = config.config["tool"].as_table().unwrap();
        assert_eq!(tool["threads"].as_str(), Some("32"));
        assert_eq!(tool["mem"].as_str(), Some("4G"));
        assert_eq!(config.config["genome"].as_str(), Some("hg38"));
    }

    #[test]
    fn merge_profile_override_mode_replaces_arrays_wholesale() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [config]
            samples = ["S1", "S2"]
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [config]
            samples = ["S1", "S3"]
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();

        let samples: Vec<&str> = config.config["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(samples, vec!["S1", "S3"]);
    }

    #[test]
    fn merge_profile_override_mode_flows_defaults_into_rules_resources() {
        // profile [defaults] threads=32 overrides workflow [defaults]
        // threads=8 in override mode and reaches rules.resources via
        // apply_defaults — the "cluster vs local" profile use case.
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "override"

            [defaults]
            threads = 8

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [defaults]
            threads = 32
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();
        config.apply_defaults();

        assert_eq!(config.rules[0].threads, Some(32));
    }

    #[test]
    fn merge_profile_fill_mode_fills_defaults_only_when_unset() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [defaults]
            threads = 8

            [[rules]]
            name = "step1"
            shell = "echo hi"
        "#;
        let mut config = WorkflowConfig::parse(toml).unwrap();
        let profile: toml::Value = toml::from_str(
            r#"
            [defaults]
            threads = 32
            memory = "16G"
            "#,
        )
        .unwrap();
        config.merge_profile(&profile).unwrap();
        config.apply_defaults();

        // fill mode: workflow's threads wins, profile's memory fills in.
        assert_eq!(config.rules[0].threads, Some(8));
        assert_eq!(config.rules[0].memory.as_deref(), Some("16G"));
    }

    #[test]
    fn merge_profile_invalid_profile_mode_is_rejected_at_parse() {
        let toml = r#"
            [workflow]
            name = "test"
            version = "1.0.0"
            profile_mode = "clobber"
        "#;
        assert!(WorkflowConfig::parse(toml).is_err());
    }

    // ---- [[values]] arbitrary-parameter fan-out (wave 2-2) ------------------

    fn values_workflow(tables: &str, rules: &str) -> String {
        format!(
            r#"
            [workflow]
            name = "values"
            version = "1.0.0"

            {tables}

            {rules}
            "#
        )
    }

    #[test]
    fn values_single_table_fans_out_rule() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["reads/{assembler}/in.fq"]
            output = ["contigs/{assembler}/out.fa"]
            shell = "{assembler} -o {output[0]} {input[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["assemble_assembler_spades", "assemble_assembler_megahit"]
        );
        let spades = &config.rules[0];
        assert_eq!(spades.input.to_vec(), vec!["reads/spades/in.fq"]);
        assert_eq!(spades.output.to_vec(), vec!["contigs/spades/out.fa"]);
        // {input[0]}/{output[0]} are executor-time placeholders; only the
        // {assembler} wildcard is substituted here.
        assert_eq!(
            spades.shell.as_deref(),
            Some("spades -o {output[0]} {input[0]}")
        );
    }

    #[test]
    fn values_multi_table_cartesian_product() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[values]]
            name = "k"
            values = ["21", "33"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["reads/{k}.fq"]
            output = ["contigs/{assembler}/k{k}/out.fa"]
            shell = "{assembler} -k {k} -o {output[0]} {input[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        // Last table varies fastest; instance names follow name_value style.
        assert_eq!(
            names,
            vec![
                "assemble_assembler_spades_k_21",
                "assemble_assembler_spades_k_33",
                "assemble_assembler_megahit_k_21",
                "assemble_assembler_megahit_k_33",
            ]
        );
        assert_eq!(
            config.rules[1].output.to_vec(),
            vec!["contigs/spades/k33/out.fa"]
        );
        assert_eq!(
            config.rules[3].shell.as_deref(),
            Some("megahit -k 33 -o {output[0]} {input[0]}")
        );
    }

    #[test]
    fn values_orthogonal_with_sample_groups() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["raw/{sample}.fq"]
            output = ["contigs/{sample}/{assembler}/out.fa"]
            shell = "{assembler} {input[0]} -o {output[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        // Values dimension is the outer loop: value-slowest, sample-fastest.
        assert_eq!(
            names,
            vec![
                "assemble_assembler_spades_cohort_S1",
                "assemble_assembler_spades_cohort_S2",
                "assemble_assembler_megahit_cohort_S1",
                "assemble_assembler_megahit_cohort_S2",
            ]
        );
        assert_eq!(
            config.rules[0].output.to_vec(),
            vec!["contigs/S1/spades/out.fa"]
        );
        assert_eq!(config.rules[3].input.to_vec(), vec!["raw/S2.fq"]);
    }

    #[test]
    fn values_namespace_form_expands_like_bare_form() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["reads/{values.assembler}/in.fq"]
            output = ["contigs/{values.assembler}/out.fa"]
            shell = "{values.assembler} -o {output[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].name, "assemble_assembler_spades");
        assert_eq!(config.rules[0].input.to_vec(), vec!["reads/spades/in.fq"]);
        assert_eq!(
            config.rules[0].shell.as_deref(),
            Some("spades -o {output[0]}")
        );
    }

    #[test]
    fn values_sanitized_instance_names() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "k"
            values = ["21", "1.5"]
            "#,
            r#"
            [[rules]]
            name = "filter"
            input = ["reads/{k}.fq"]
            output = ["filtered/{k}.fq"]
            shell = "echo {k}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["filter_k_21", "filter_k_1_5"]);
        assert_eq!(config.rules[1].input.to_vec(), vec!["reads/1.5.fq"]);
    }

    #[test]
    fn values_referenced_from_expand_inputs_binds_per_instance() {
        // The spades instance only ever sees spades outputs — no cross
        // fan-out between instances.
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]
            "#,
            r#"
            [[rules]]
            name = "combine"
            input = []
            expand_inputs = [
                { pattern = "contigs/{assembler}/out.fa", variables = { } }
            ]
            output = ["contigs/all/{values.assembler}.txt"]
            shell = "cat {input} > {output[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["combine_assembler_spades", "combine_assembler_megahit"]
        );
        assert_eq!(
            config.rules[0].input.to_vec(),
            vec!["contigs/spades/out.fa"]
        );
        assert_eq!(
            config.rules[0].output.to_vec(),
            vec!["contigs/all/spades.txt"]
        );
        assert_eq!(
            config.rules[1].input.to_vec(),
            vec!["contigs/megahit/out.fa"]
        );
    }

    #[test]
    fn values_expanded_rules_flow_into_dag() {
        // dry-run/plan/checkpoint share the post-expansion rule list, so a
        // producer/consumer pair fanned out by [[values]] must form edges
        // between the concrete instances.
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["reads/in.fq"]
            output = ["contigs/{assembler}/out.fa"]
            shell = "{assembler} {input[0]} -o {output[0]}"

            [[rules]]
            name = "quast"
            input = ["contigs/{assembler}/out.fa"]
            output = ["quast/{assembler}/report.txt"]
            shell = "quast {input[0]} -o {output[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let dag = crate::dag::WorkflowDag::from_rules(&config.rules).unwrap();
        assert_eq!(
            dag.dependencies("quast_assembler_spades").unwrap(),
            vec!["assemble_assembler_spades"]
        );
        assert_eq!(
            dag.dependencies("quast_assembler_megahit").unwrap(),
            vec!["assemble_assembler_megahit"]
        );
    }

    #[test]
    fn values_depends_on_resolves_to_expanded_instances() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]
            "#,
            r#"
            [[rules]]
            name = "assemble"
            input = ["reads/in.fq"]
            output = ["contigs/{assembler}/out.fa"]
            shell = "{assembler} {input[0]} -o {output[0]}"

            [[rules]]
            name = "report"
            input = []
            output = ["report.txt"]
            depends_on = ["assemble"]
            shell = "touch report.txt"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();

        let report = config
            .rules
            .iter()
            .find(|r| r.name == "report")
            .expect("report rule survives expansion");
        let mut deps = report.depends_on.clone();
        deps.sort();
        assert_eq!(
            deps,
            vec![
                "assemble_assembler_megahit".to_string(),
                "assemble_assembler_spades".to_string(),
            ]
        );
    }

    #[test]
    fn values_unused_tables_leave_rules_unchanged() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades", "megahit"]
            "#,
            r#"
            [[rules]]
            name = "plain"
            input = ["reads/in.fq"]
            output = ["out.txt"]
            shell = "cat {input[0]} > {output[0]}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].name, "plain");
    }

    #[test]
    fn values_duplicate_table_names_rejected() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = ["spades"]

            [[values]]
            name = "assembler"
            values = ["megahit"]
            "#,
            r#"
            [[rules]]
            name = "plain"
            shell = "echo hi"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        let err = config.expand_wildcards().unwrap_err();
        assert!(err.to_string().contains("duplicate [[values]] table"));
    }

    #[test]
    fn values_colliding_with_builtin_wildcard_rejected() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "sample"
            values = ["A", "B"]
            "#,
            r#"
            [[rules]]
            name = "plain"
            shell = "echo hi"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        let err = config.expand_wildcards().unwrap_err();
        assert!(
            err.to_string()
                .contains("collides with a built-in wildcard")
        );
    }

    #[test]
    fn values_empty_table_rejected() {
        let toml = values_workflow(
            r#"
            [[values]]
            name = "assembler"
            values = []
            "#,
            r#"
            [[rules]]
            name = "plain"
            shell = "echo hi"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        let err = config.expand_wildcards().unwrap_err();
        assert!(err.to_string().contains("has no values"));
    }

    #[test]
    fn unbound_values_namespace_keeps_rule_unchanged() {
        // `{values.assembler}` without a matching [[values]] table: rule is
        // kept as-is (a warning is emitted; never an error).
        let toml = values_workflow(
            "",
            r#"
            [[rules]]
            name = "plain"
            input = ["reads/{values.assembler}/in.fq"]
            output = ["out.txt"]
            shell = "echo {values.assembler}"
            "#,
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.expand_wildcards().unwrap();
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].name, "plain");
        assert_eq!(
            config.rules[0].input.to_vec(),
            vec!["reads/{values.assembler}/in.fq"]
        );
    }
}
