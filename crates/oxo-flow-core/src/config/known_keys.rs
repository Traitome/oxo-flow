//! Known-key whitelist for the `.oxoflow` TOML surface (E017).
//!
//! Serde silently drops every key it does not recognise, so a typo'd key
//! (`envgroup`, `samle_pattern`) or a misplaced section (`[sample_pattern]`
//! instead of `sample_pattern = …` inside `[workflow]`) parsed through
//! without a sound and the setting simply never took effect.
//!
//! Deserialization itself stays lenient — `deny_unknown_fields` would reject
//! every older workflow and every engine-injected field the moment a key is
//! renamed — instead the raw table is walked after parsing and each key
//! outside the whitelists below is reported as an **E017** error naming the
//! closest known key. Sections whose keys are user-defined by design
//! (`[config]`, `params`, `metadata`, `wildcard_constraints`, …) are exempt.
//!
//! The whitelists mirror the serde structs they describe, and the
//! `*_keys_*` tests at the bottom fail when a whitelist entry stops being a
//! struct field or is missing from the shipped `oxoflow-v1.schema.json`, so
//! struct, whitelist and schema cannot drift apart.

use crate::error::{OxoFlowError, Result};

/// Top-level keys/sections of a `.oxoflow` file.
const TOP_LEVEL_KEYS: &[&str] = &[
    "citation",
    "cluster",
    "config",
    "defaults",
    "env_groups",
    "execution_group",
    "include",
    "metadata",
    "pairs",
    "plugins",
    "reference_db",
    "reference_dir",
    "references",
    "report",
    "resource_budget",
    "resource_groups",
    "rules",
    "sample_groups",
    "values",
    "wildcard_constraints",
    "workflow",
];

/// What a top-level typo most likely meant: a top-level section, or the
/// `[workflow]` key the author wrote as a section of its own.
const TOP_LEVEL_SUGGESTIONS: &[&str] = &[
    "citation",
    "cluster",
    "config",
    "defaults",
    "env_groups",
    "execution_group",
    "include",
    "metadata",
    "pairs",
    "plugins",
    "reference_db",
    "reference_dir",
    "references",
    "report",
    "resource_budget",
    "resource_groups",
    "rules",
    "sample_groups",
    "values",
    "wildcard_constraints",
    "workflow",
    "workflow.author",
    "workflow.description",
    "workflow.format_version",
    "workflow.genome_build",
    "workflow.interpreter_map",
    "workflow.metadata_file",
    "workflow.min_version",
    "workflow.name",
    "workflow.on_complete",
    "workflow.on_error",
    "workflow.pairs_file",
    "workflow.pairs_pattern",
    "workflow.profile_mode",
    "workflow.sample_groups_file",
    "workflow.sample_pattern",
    "workflow.version",
];

/// Keys of the `[workflow]` table.
const WORKFLOW_KEYS: &[&str] = &[
    "author",
    "description",
    "format_version",
    "genome_build",
    "interpreter_map",
    "metadata_file",
    "min_version",
    "name",
    "on_complete",
    "on_error",
    "pairs_file",
    "pairs_pattern",
    "profile_mode",
    "sample_groups_file",
    "sample_pattern",
    "version",
];

/// Keys of the `[defaults]` table.
const DEFAULTS_KEYS: &[&str] = &["environment", "memory", "shell_prelude", "threads"];

/// Keys of the `[report]` table.
const REPORT_KEYS: &[&str] = &["format", "sections", "template"];

/// Keys of one `[[include]]` entry.
const INCLUDE_KEYS: &[&str] = &[
    "inputs", "name", "namespace", "outputs", "params", "path", "ref", "repo",
];

/// Keys of one `[[execution_group]]` entry.
const EXECUTION_GROUP_KEYS: &[&str] = &["mode", "name", "rules"];

/// Keys of the `[citation]` table.
const CITATION_KEYS: &[&str] = &["authors", "doi", "title", "url"];

/// Keys of the `[cluster]` table.
const CLUSTER_KEYS: &[&str] = &[
    "account",
    "backend",
    "extra_args",
    "max_array_size",
    "max_submitted",
    "partition",
    "poll_interval",
    "walltime",
];

/// Keys of the declarative inline-table form of a `[config]` entry
/// (`key = { default = "…", required = true, … }`).
const CONFIG_DEF_KEYS: &[&str] = &[
    "choices", "default", "help", "must_exist", "range", "required", "sensitive", "type",
];

/// Keys of one `[[references]]` entry.
const REFERENCE_KEYS: &[&str] = &[
    "build", "description", "environment", "memory", "name", "output", "source", "threads",
];

/// Keys of the `[resource_budget]` table.
const RESOURCE_BUDGET_KEYS: &[&str] = &["max_jobs", "max_memory", "max_threads"];

/// Keys of one `[resource_groups]` entry.
const RESOURCE_GROUP_KEYS: &[&str] = &["max", "wait"];

/// Keys of one `[[reference_db]]` entry.
const REFERENCE_DB_KEYS: &[&str] = &["accessed_date", "checksum", "name", "source", "version"];

/// Keys of one `[[pairs]]` entry (including the `tumor`/`normal` aliases).
const PAIR_KEYS: &[&str] = &[
    "control",
    "experiment",
    "experiment_type",
    "metadata",
    "normal",
    "pair_id",
    "tumor",
    "tumor_type",
    "when",
];

/// Keys of one `[[sample_groups]]` entry.
const SAMPLE_GROUP_KEYS: &[&str] = &["metadata", "name", "samples"];

/// Keys of one `[[values]]` entry.
const VALUE_KEYS: &[&str] = &["name", "values"];

/// Keys of the `[plugins]` table.
const PLUGINS_KEYS: &[&str] = &["executor", "reports", "rules", "trusted_keys_file"];

/// Keys of one `[[rules]]` entry.
const RULE_KEYS: &[&str] = &[
    "ancient",
    "benchmark",
    "cache_key",
    "checkpoint",
    "checkpoint_manifest",
    "checksum",
    "depends_on",
    "description",
    "env_group",
    "environment",
    "envvars",
    "expand_inputs",
    "extends",
    "format_hint",
    "group",
    "input",
    "input_function",
    "input_groups",
    "interpreter",
    "localrule",
    "log",
    "memory",
    "name",
    "on_failure",
    "on_success",
    "optional",
    "output",
    "output_pattern",
    "params",
    "pipe",
    "pre_exec",
    "priority",
    "protected_output",
    "required",
    "resource_hint",
    "resources",
    "retries",
    "retry_delay",
    "rule_metadata",
    "scatter",
    "scratch",
    "shadow",
    "shell",
    "script",
    "tags",
    "target",
    "temp_output",
    "temporary",
    "threads",
    "transform",
    "when",
];

/// Keys of the `[resources]` table.
const RESOURCES_KEYS: &[&str] = &[
    "disk", "gpu", "gpu_spec", "groups", "memory", "partition", "threads", "time_limit",
];

/// Keys of the `[resources.gpu_spec]` table.
const GPU_SPEC_KEYS: &[&str] = &["compute_capability", "count", "memory_gb", "model"];

/// Keys of an `environment` table (rule-level, `[defaults]`, `[[references]]`).
const ENVIRONMENT_KEYS: &[&str] = &[
    "conda",
    "conda_prefix",
    "docker",
    "mamba",
    "mamba_prefix",
    "modules",
    "pixi",
    "singularity",
    "venv",
    "venv_requirements",
];

/// Keys of a `[scatter]` table.
const SCATTER_KEYS: &[&str] = &["gather", "values", "values_from", "variable"];

/// Keys of a `[transform]` table.
const TRANSFORM_KEYS: &[&str] = &["cleanup", "combine", "map", "split"];

/// Keys of a `[transform.split]` table.
const SPLIT_KEYS: &[&str] = &["by", "glob", "n", "values", "values_from"];

/// Keys of a `[transform.combine]` table.
const COMBINE_KEYS: &[&str] = &["aggregate", "header", "method", "shell"];

/// Keys of one `[[expand_inputs]]` entry.
const EXPAND_INPUT_KEYS: &[&str] = &["pattern", "variables"];

/// Keys of one `[[input_groups]]` entry.
const INPUT_GROUP_KEYS: &[&str] = &["group_by", "keep", "pattern"];

/// Keys of a `[resource_hint]` table.
const RESOURCE_HINT_KEYS: &[&str] = &["input_size", "io_bound", "memory_scale", "runtime"];

/// A key the whitelist does not know about.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UnknownKey {
    /// Human-readable location, e.g. `[[rules]] #0.resources`; empty at the
    /// top level.
    pub(crate) location: String,
    /// The key as written in the TOML.
    pub(crate) key: String,
    /// The closest known key, when one is close enough to be a likely typo.
    pub(crate) suggestion: Option<String>,
}

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.location.as_str() {
            "" => write!(f, "unknown top-level key '{}'", self.key)?,
            loc => write!(f, "unknown key '{}' in {}", self.key, loc)?,
        }
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " — did you mean '{suggestion}'?")?;
        }
        Ok(())
    }
}

/// Levenshtein edit distance between two keys, capped so unrelated pairs exit
/// early.
fn edit_distance(a: &str, b: &str, cap: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > cap {
        return cap + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The known key closest to `key` — the same key ignoring case, or within a
/// two-edit typo. Case-insensitive equality wins: `Shell` is a case slip, not
/// a guess. Qualified candidates (`workflow.sample_pattern`) compare by their
/// last segment and are reported in full, so a top-level typo is pointed at
/// the section it belongs in.
fn closest<'a>(key: &str, candidates: &[&'a str]) -> Option<&'a str> {
    if let Some(exact) = candidates.iter().find(|c| c.eq_ignore_ascii_case(key)) {
        return Some(exact);
    }
    let lowered = key.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|c| !c.eq_ignore_ascii_case(key))
        .filter_map(|c| {
            let tail = c.rsplit('.').next().unwrap_or(c);
            let distance = edit_distance(&lowered, &tail.to_ascii_lowercase(), 2);
            (distance <= 2).then_some((c, distance))
        })
        .min_by_key(|&(_, distance)| distance)
        .map(|(c, _)| *c)
}

/// Build the report for one unknown key found under `location`.
fn unknown(location: &str, key: &str, candidates: &[&str]) -> UnknownKey {
    UnknownKey {
        location: location.to_string(),
        key: key.to_string(),
        suggestion: closest(key, candidates).map(str::to_string),
    }
}

/// A known key whose value is a table with its own whitelist.
fn nested_table(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "citation" => Some(CITATION_KEYS),
        "cluster" => Some(CLUSTER_KEYS),
        "combine" => Some(COMBINE_KEYS),
        "defaults" => Some(DEFAULTS_KEYS),
        "environment" => Some(ENVIRONMENT_KEYS),
        "gpu_spec" => Some(GPU_SPEC_KEYS),
        "plugins" => Some(PLUGINS_KEYS),
        "report" => Some(REPORT_KEYS),
        "resource_budget" => Some(RESOURCE_BUDGET_KEYS),
        "resource_hint" => Some(RESOURCE_HINT_KEYS),
        "resources" => Some(RESOURCES_KEYS),
        "scatter" => Some(SCATTER_KEYS),
        "split" => Some(SPLIT_KEYS),
        "transform" => Some(TRANSFORM_KEYS),
        "workflow" => Some(WORKFLOW_KEYS),
        _ => None,
    }
}

/// A known key whose value is an array of tables with their own whitelist.
fn array_of_tables(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "execution_group" => Some(EXECUTION_GROUP_KEYS),
        "expand_inputs" => Some(EXPAND_INPUT_KEYS),
        "include" => Some(INCLUDE_KEYS),
        "input_groups" => Some(INPUT_GROUP_KEYS),
        "pairs" => Some(PAIR_KEYS),
        "reference_db" => Some(REFERENCE_DB_KEYS),
        "references" => Some(REFERENCE_KEYS),
        "rules" => Some(RULE_KEYS),
        "sample_groups" => Some(SAMPLE_GROUP_KEYS),
        "values" => Some(VALUE_KEYS),
        _ => None,
    }
}

/// A known key whose value maps arbitrary names to whitelisted tables
/// (`[env_groups.<name>]`, `[resource_groups.<name>]`).
fn table_of_tables(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "env_groups" => Some(ENVIRONMENT_KEYS),
        "resource_groups" => Some(RESOURCE_GROUP_KEYS),
        _ => None,
    }
}

/// How a known key's value nests into further whitelisted tables.
enum Nesting {
    /// No further keys below it (`config` is user vocabulary, scalars end).
    None,
    /// A single table: `[workflow]`, `rules[i].resources`.
    Table(&'static [&'static str]),
    /// An array of tables, located by index: `[[rules]] #0`.
    Array(&'static [&'static str]),
    /// A table of named tables, located by name: `[env_groups.star]`.
    NamedTables(&'static [&'static str]),
    /// The declarative inline tables inside the user-defined `[config]`
    /// (`genome = { default = "hg38", required = true }`).
    Config,
}

/// How the value stored under `key` nests.
fn nesting(key: &str, value: &toml::Value) -> Nesting {
    match value {
        toml::Value::Table(_) if key == "config" => Nesting::Config,
        toml::Value::Table(_) => match nested_table(key) {
            Some(keys) => Nesting::Table(keys),
            None => Nesting::None,
        },
        toml::Value::Array(_) => {
            if let Some(keys) = array_of_tables(key) {
                Nesting::Array(keys)
            } else if let Some(keys) = table_of_tables(key) {
                Nesting::NamedTables(keys)
            } else {
                Nesting::None
            }
        }
        _ => Nesting::None,
    }
}

/// Append every unknown key of `table` to `out`, recursing into known
/// sub-tables. `location` is empty at the top level and `[workflow]`-style
/// below it.
fn walk(
    table: &toml::Table,
    keys: &[&str],
    location: &str,
    candidates: &[&str],
    out: &mut Vec<UnknownKey>,
) {
    for (key, value) in table {
        if !keys.contains(&key.as_str()) {
            out.push(unknown(location, key, candidates));
            continue;
        }
        let label = if location.is_empty() {
            format!("[{key}]")
        } else {
            format!("{location}.{key}")
        };
        match nesting(key, value) {
            Nesting::None => {}
            Nesting::Table(child_keys) => {
                let toml::Value::Table(child) = value else {
                    continue;
                };
                walk(child, child_keys, &label, child_keys, out);
            }
            Nesting::Array(child_keys) => {
                let Some(toml::Value::Array(entries)) = table.get(key) else {
                    continue;
                };
                for (index, entry) in entries.iter().enumerate() {
                    let toml::Value::Table(child) = entry else {
                        continue;
                    };
                    walk(child, child_keys, &format!("{label} #{index}"), child_keys, out);
                }
            }
            Nesting::NamedTables(child_keys) => {
                let toml::Value::Table(children) = value else {
                    continue;
                };
                for (name, child) in children {
                    let toml::Value::Table(child) = child else {
                        continue;
                    };
                    walk(child, child_keys, &format!("{label}.{name}"), child_keys, out);
                }
            }
            Nesting::Config => {
                // Declarative entries live in the user-defined `[config]`
                // table, so they are located by their own key. A table
                // whose keys are NOT all declarative fields is user
                // vocabulary (e.g. `[config] tool = { mem = "4G" }`) and
                // stays fully exempt — only declarative-shaped tables
                // (`{ default = ..., help = ... }`) are walked.
                let toml::Value::Table(entries) = value else {
                    continue;
                };
                for (name, entry) in entries {
                    let toml::Value::Table(child) = entry else {
                        continue;
                    };
                    // `[config]` is user vocabulary: scalars and
                    // vocabulary tables (`tool = { mem = "4G" }`) are
                    // fully exempt. The only thing worth flagging is a
                    // near-miss of a declarative field (`defualt` →
                    // `default`) — reported individually; never walk the
                    // table itself, its other keys are the user's own.
                    for k in child.keys() {
                        if !CONFIG_DEF_KEYS.contains(&k.as_str())
                            && closest(k, CONFIG_DEF_KEYS).is_some()
                        {
                            out.push(unknown(&format!("[config].{name}"), k, CONFIG_DEF_KEYS));
                        }
                    }
                }
            }
        }
    }
}

/// Every unknown key in a raw workflow table, in file order.
pub(crate) fn unknown_keys(raw: &toml::Table) -> Vec<UnknownKey> {
    let mut out = Vec::new();
    walk(raw, TOP_LEVEL_KEYS, "", TOP_LEVEL_SUGGESTIONS, &mut out);
    out
}

/// Fail with E017 when `raw` holds any key outside the whitelists.
///
/// Every offender is named (capped at five) so one validate round reports the
/// whole typo cluster instead of one typo per run.
pub(crate) fn check(raw: &toml::Table) -> Result<()> {
    let unknown = unknown_keys(raw);
    if unknown.is_empty() {
        return Ok(());
    }
    const MAX_LISTED: usize = 5;
    let listed: Vec<String> = unknown
        .iter()
        .take(MAX_LISTED)
        .map(ToString::to_string)
        .collect();
    let mut message = format!(
        "{} — not a .oxoflow key. A misspelled key is silently ignored, so the setting never applies.",
        listed.join("; ")
    );
    let hidden = unknown.len() - listed.len();
    if hidden > 0 {
        message.push_str(&format!(" … and {hidden} more"));
    }
    Err(OxoFlowError::Config {
        message: format!("E017: {message}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> toml::Table {
        toml::from_str(content).expect("test TOML must parse")
    }

    fn errors(content: &str) -> Vec<String> {
        unknown_keys(&parse(content))
            .into_iter()
            .map(|u| u.to_string())
            .collect()
    }

    // ---- clean workflows pass ------------------------------------------------

    #[test]
    fn accepts_a_workflow_using_every_section() {
        let toml = r#"
            [workflow]
            name = "wf"
            sample_pattern = "raw/{sample}.fastq.gz"
            on_complete = "echo done"

            [config]
            genome = "hg38"

            [defaults]
            threads = 2

            [[rules]]
            name = "align"
            input = "raw/{sample}.fastq.gz"
            output = "out/{sample}.bam"
            shell = "bwa mem {input} > {output}"
            env_group = "aligner"

            [[rules]]
            name = "index"
            input = "out/{sample}.bam"
            output = "out/{sample}.bam.bai"
            depends_on = ["align"]

            [env_groups.aligner]
            conda = "bioconda::bwa"

            [[pairs]]
            pair_id = "P1"
            tumor = "T"
            normal = "N"

            [[values]]
            name = "assembler"
            values = ["spades"]
        "#;
        assert!(errors(toml).is_empty(), "{:?}", errors(toml));
    }

    #[test]
    fn accepts_every_gallery_workflow() {
        // The gallery is the compatibility floor: all 16 must stay E017-clean.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("gallery directory exists") {
            let path = entry.expect("readable gallery entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("oxoflow") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("gallery file readable");
            let unknown = unknown_keys(&parse(&content));
            assert!(
                unknown.is_empty(),
                "{} has unknown keys: {unknown:?}",
                path.display()
            );
            checked += 1;
        }
        assert_eq!(checked, 16, "expected the full 16-workflow gallery");
    }

    // ---- typo'd keys ---------------------------------------------------------

    #[test]
    fn reports_a_rule_key_typo_with_a_suggestion() {
        let toml = r#"
            [[rules]]
            name = "r"
            envgroup = "aligner"
            shell = "true"
        "#;
        let errs = errors(toml);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("unknown key 'envgroup' in [rules] #0")
                && errs[0].contains("did you mean 'env_group'?"),
            "{errs:?}"
        );
    }

    #[test]
    fn reports_a_workflow_key_typo() {
        let toml = r#"
            [workflow]
            name = "wf"
            samle_pattern = "raw/{sample}.fastq.gz"
        "#;
        let errs = errors(toml);
        assert!(
            errs[0].contains("'samle_pattern'") && errs[0].contains("did you mean 'sample_pattern'?"),
            "{errs:?}"
        );
    }

    #[test]
    fn reports_typos_in_nested_tables() {
        let toml = r#"
            [defaults]
            threadz = 4

            [[rules]]
            name = "r"
            shell = "true"
            [rules.resources]
            threadz = 4
        "#;
        let errs = errors(toml);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().all(|e| e.contains("'threadz'")), "{errs:?}");
        assert!(
            errs.iter().any(|e| e.contains("in [rules] #0.resources")),
            "{errs:?}"
        );
    }

    #[test]
    fn reports_an_unknown_top_level_section() {
        // P9-7: `sample_pattern` belongs inside [workflow] — as a section of
        // its own it is dropped on the floor.
        let toml = r#"
            [sample_pattern]
            raw = "data/{sample}.fastq.gz"

            [workflow]
            name = "wf"

            [[rules]]
            name = "r"
            shell = "true"
        "#;
        let errs = errors(toml);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("unknown top-level key 'sample_pattern'")
                && errs[0].contains("did you mean 'workflow.sample_pattern'?"),
            "{errs:?}"
        );
    }

    #[test]
    fn reports_a_declarative_config_key_typo() {
        let toml = r#"
            [workflow]
            name = "wf"

            [config]
            genome = { defualt = "hg38", required = true }

            [[rules]]
            name = "r"
            shell = "true"
        "#;
        let errs = errors(toml);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("'defualt'") && errs[0].contains("'default'"),
            "{errs:?}"
        );
    }

    #[test]
    fn reports_engine_internal_keys_as_unknown() {
        // `cleanup_chunks` is engine-internal (skip_deserializing): writing it
        // in TOML has never done anything.
        let toml = r#"
            [[rules]]
            name = "r"
            shell = "true"
            cleanup_chunks = true
        "#;
        assert!(
            errors(toml)[0].contains("'cleanup_chunks'"),
            "{:?}",
            errors(toml)
        );
    }

    #[test]
    fn user_defined_vocabularies_are_exempt() {
        let toml = r#"
            [config]
            anything_goes = "yes"
            declared = { default = "1", help = "x" }

            [wildcard_constraints]
            sample = 'S\d+'

            [env_groups.conda_env]
            conda = "bioconda::samtools"

            [resource_groups.api]
            max = 2
            wait = "queue"

            [[rules]]
            name = "r"
            shell = "true"
            params = { arbitrary = 1, nested = { deep = 2 } }
            rule_metadata = { assay = "rna" }
            envvars = { PATH_EXTRA = "/tmp" }
            resources.groups = { database = 1 }

            [metadata.SE1]
            endedness = "SE"
        "#;
        assert!(errors(toml).is_empty(), "{:?}", errors(toml));
    }

    #[test]
    fn check_collects_the_whole_cluster_of_typos() {
        let toml = r#"
            [workflow]
            name = "wf"
            versionn = "1.0"

            [defaluts]
            threads = 1

            [[rules]]
            name = "r"
            shell = "true"
            local_rules = true
        "#;
        let err = check(&parse(toml)).expect_err("unknown keys must fail");
        let message = err.to_string();
        assert!(message.contains("E017"), "{message}");
        assert!(message.contains("'versionn'"), "{message}");
        assert!(message.contains("'local_rules'"), "{message}");
        // All three typos are named in full — no truncation.
        assert!(message.contains("'defaluts'"), "{message}");
    }

    // ---- suggestion machinery ------------------------------------------------

    #[test]
    fn case_insensitive_match_wins() {
        assert_eq!(closest("Shell", RULE_KEYS), Some("shell"));
    }

    #[test]
    fn distant_keys_get_no_suggestion() {
        assert_eq!(closest("zzzzzzzz", RULE_KEYS), None);
        // A top-level typo is answered with the qualified `[workflow]` key.
        assert_eq!(
            closest("sample_pattern", TOP_LEVEL_SUGGESTIONS),
            Some("workflow.sample_pattern")
        );
        assert_eq!(closest("output_pattern", TOP_LEVEL_SUGGESTIONS), None);
    }

    #[test]
    fn edit_distance_is_standard_levenshtein() {
        assert_eq!(edit_distance("kitten", "sitting", 8), 3);
        assert_eq!(edit_distance("abc", "abc", 2), 0);
        assert_eq!(edit_distance("abc", "abd", 2), 1);
        // abc -> wxyz: three substitutions plus one insertion.
        assert_eq!(edit_distance("abc", "wxyz", 2), 4);
    }

    // ---- struct / whitelist / schema sync ------------------------------------

    /// The whitelist must name only keys the structs really deserialize: each
    /// entry below is set to a non-default value, and the round-trip through
    /// `Rule` has to give it back. A key that stops being a `Rule` field —
    /// renamed, removed, or `skip_deserializing` — disappears here.
    #[test]
    fn rule_keys_round_trip_through_the_struct() {
        let toml = r#"
            name = "probe"
            description = "d"
            input = ["a.txt"]
            output = ["b.txt"]
            output_pattern = "c/*.txt"
            expand_inputs = [{ pattern = "p/*.txt" }]
            input_groups = [{ pattern = "p/{lane}/*.fq", group_by = "lane" }]
            shell = "true"
            script = "x.py"
            threads = 2
            memory = "4G"
            resources = { threads = 2, memory = "4G", gpu = 1, disk = "10G",
                          time_limit = "1h", partition = "main", groups = { db = 1 } }
            environment = { conda = "x" }
            env_group = "e"
            log = "l.log"
            benchmark = "b.tsv"
            params = { k = 1 }
            priority = 1
            target = true
            optional = "any"
            group = "g"
            when = "true"
            temporary = true
            scratch = true
            scatter = { variable = "v", values = ["a"] }
            transform = { split = { by = "s" }, map = "m" }
            temp_output = ["t.txt"]
            protected_output = ["p.txt"]
            input_function = "f"
            retries = 2
            tags = ["t"]
            shadow = "minimal"
            ancient = ["a.txt"]
            localrule = true
            envvars = { A = "B" }
            checkpoint = true
            checkpoint_manifest = "m.toml"
            required = false
            depends_on = ["d"]
            extends = "base"
            retry_delay = "5s"
            pre_exec = "echo"
            on_success = "echo"
            on_failure = "echo"
            format_hint = ["bam"]
            pipe = true
            checksum = "md5"
            resource_hint = { input_size = "small" }
            rule_metadata = { assay = "rna" }
            cache_key = "k"
            interpreter = "python"
        "#;
        let rule: crate::rule::Rule = toml::from_str(toml).expect("probe rule deserializes");
        let written = toml::to_string(&rule)
            .expect("rule serializes")
            .parse::<toml::Table>()
            .expect("serialized rule is a table")
            .keys()
            .cloned()
            .collect::<Vec<_>>();

        // Every whitelist key must come back out of the struct. `gpu_spec`
        // nests under `resources` (its own RESOURCES_KEYS list), so it is
        // deliberately absent from RULE_KEYS and must not appear here as a
        // dropped key.
        let dropped: Vec<&str> = RULE_KEYS
            .iter()
            .copied()
            .filter(|k| !written.contains(&k.to_string()))
            .collect();
        assert!(
            dropped.is_empty(),
            "whitelist keys no longer on Rule: {dropped:?}"
        );
    }

    /// Same contract for `[workflow]`.
    #[test]
    fn workflow_keys_round_trip_through_the_struct() {
        let toml = r#"
            name = "probe"
            version = "1.0.0"
            description = "d"
            author = "a"
            min_version = "0.1.0"
            format_version = "1.0"
            on_complete = "echo"
            on_error = "echo"
            genome_build = "hg38"
            interpreter_map = { ".m" = "octave" }
            pairs_file = "pairs.tsv"
            sample_groups_file = "groups.tsv"
            pairs_pattern = "p/{pair_id}.bam"
            metadata_file = "samples.tsv"
            sample_pattern = "raw/{sample}.fastq.gz"
            profile_mode = "override"
        "#;
        let meta: crate::config::WorkflowMeta =
            toml::from_str(toml).expect("probe metadata deserializes");
        let written = toml::to_string(&meta)
            .expect("metadata serializes")
            .parse::<toml::Table>()
            .expect("serialized metadata is a table")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for key in WORKFLOW_KEYS {
            assert!(
                written.iter().any(|k| k == key),
                "[workflow] round-trip lost '{key}'"
            );
        }
    }

    /// The shipped JSON schema must cover every whitelisted key — a key the
    /// schema omits is a key editors and `oxo-flow schema` never surface.
    #[test]
    fn whitelisted_keys_are_in_the_schema() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../oxo-flow-cli/schema/oxoflow-v1.schema.json");
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("schema file is readable"),
        )
        .expect("schema is valid JSON");

        let properties = |pointer: &str| -> Vec<String> {
            schema
                .pointer(pointer)
                .and_then(|v| v.get("properties"))
                .and_then(|p| p.as_object())
                .map(|p| p.keys().cloned().collect())
                .unwrap_or_default()
        };

        let rule_props = properties("/$defs/rule");
        let workflow_props = properties("/properties/workflow");
        assert!(!rule_props.is_empty() && !workflow_props.is_empty(), "schema shape changed");

        for key in RULE_KEYS {
            assert!(rule_props.iter().any(|p| p == key), "schema $defs/rule omits '{key}'");
        }
        for key in WORKFLOW_KEYS {
            assert!(
                workflow_props.iter().any(|p| p == key),
                "schema properties.workflow omits '{key}'"
            );
        }
        for (keys, pointer) in [
            (TOP_LEVEL_KEYS, ""),
            (DEFAULTS_KEYS, "/properties/defaults"),
            (REPORT_KEYS, "/properties/report"),
            (CLUSTER_KEYS, "/properties/cluster"),
            (CITATION_KEYS, "/properties/citation"),
            (RESOURCE_BUDGET_KEYS, "/properties/resource_budget"),
            (PLUGINS_KEYS, "/properties/plugins"),
            (RESOURCES_KEYS, "/$defs/resources"),
            (ENVIRONMENT_KEYS, "/$defs/environmentSpec"),
            (SCATTER_KEYS, "/$defs/scatterConfig"),
            (TRANSFORM_KEYS, "/$defs/transformConfig"),
            (RESOURCE_HINT_KEYS, "/$defs/resourceHint"),
        ] {
            let pointer = if pointer.is_empty() {
                "/properties"
            } else {
                pointer
            };
            let props = properties(pointer);
            for key in keys {
                assert!(props.iter().any(|p| p == key), "schema {pointer} omits '{key}'");
            }
        }
    }
}
