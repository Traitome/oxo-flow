//! Issue #1154 (ledger WF-03): `transform`-generated rules must preserve
//! every `Rule` field the transform operator does not itself replace.
//!
//! The evidence is deliberately exhaustive rather than example-based:
//!
//! - [`exhaustive_transform_source`] fills EVERY field of
//!   [`crate::rule::Rule`] with a non-default value, including the
//!   engine-internal ones (`cleanup_chunks`, `expand_inputs_baked`) that
//!   TOML cannot deserialize.
//! - [`assert_transform_generated_rule`] destructures the whole `Rule`
//!   struct WITHOUT a `..` rest pattern, so adding a field to `Rule` breaks
//!   compilation here instead of silently escaping coverage (the
//!   compile-time guard the issue asks for).
//! - The three user-visible transform modes are covered: split → map →
//!   combine with an explicit combine shell, split → map without a combine
//!   stage, and split → map → aggregate (`concat`).
//!
//! Fields that are mutually exclusive with `transform` by construction stay
//! at their defaults in the source: `scatter` (the scatter branch runs
//! first), `output_pattern` (validated v1 exclusion), and `input_groups`
//! (the input_groups fan-out pass diverts rules before the transform
//! branch). The generated rules are still asserted to keep those defaults.

#![allow(deprecated)] // `Rule::threads` / `Rule::memory` shorthand fields

use super::*;
use crate::rule::{
    CombineConfig, EnvironmentSpec, ExpandConfig, FilePatterns, OptionalMode, ResourceHint,
    Resources, Rule, RuleReport, SplitConfig, TransformConfig,
};
use crate::storage::StagingMode;
use crate::types::RuleTypesSpec;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const WORKFLOW_TOML: &str = r#"
    [workflow]
    name = "transform-field-preservation"

    [config]
    genomes = ["hg38"]
    enable_qc = true
"#;

const SPLIT_VALUES: [&str; 2] = ["chr1", "chr2"];

/// A valid non-transform rule used to make `depends_on` / `extends`
/// references resolvable (parse-time `extends` resolution is bypassed here
/// because the source rule is injected programmatically, but the authored
/// state must still be a legal workflow shape).
fn support_rule(name: &str) -> Rule {
    Rule {
        name: name.to_string(),
        shell: Some("true".to_string()),
        ..Default::default()
    }
}

/// Build a transform rule carrying a NON-DEFAULT value in every `Rule`
/// field.
fn exhaustive_transform_source(transform: TransformConfig) -> Rule {
    let mut params: HashMap<String, toml::Value> = HashMap::new();
    params.insert("seed".to_string(), toml::Value::Integer(7));
    params.insert("mode".to_string(), toml::Value::String("deep".to_string()));

    let mut envvars: HashMap<String, String> = HashMap::new();
    envvars.insert("SAMPLE_SHEET".to_string(), "sheet.tsv".to_string());

    let mut rule_metadata: HashMap<String, toml::Value> = HashMap::new();
    rule_metadata.insert("assay".to_string(), toml::Value::String("wes".to_string()));

    let mut resource_groups: HashMap<String, u32> = HashMap::new();
    resource_groups.insert("db_connection".to_string(), 2);

    let mut expand_variables: HashMap<String, String> = HashMap::new();
    expand_variables.insert("genome".to_string(), "config.genomes".to_string());

    let mut type_inputs = BTreeMap::new();
    type_inputs.insert("bam".to_string(), "alignment.bam".to_string());
    let mut type_outputs = BTreeMap::new();
    type_outputs.insert("vcf".to_string(), "variants.vcf".to_string());

    let mut expand_inputs_baked = BTreeSet::new();
    expand_inputs_baked.insert("refs/baked.fa".to_string());

    Rule {
        name: "splitter".to_string(),
        input: FilePatterns::List(vec![
            "shards/{chr}.bam".to_string(),
            "shared.bam".to_string(),
        ]),
        expand_inputs: vec![ExpandConfig {
            pattern: "refs/{genome}.fa".to_string(),
            variables: expand_variables,
        }],
        expand_inputs_baked,
        // transform × input_groups has no combined expansion path: the
        // input_groups pass diverts such rules before the transform branch.
        input_groups: Vec::new(),
        output: FilePatterns::List(vec!["merged/all.vcf.gz".to_string()]),
        // transform × output_pattern is a validated v1 exclusion.
        output_pattern: None,
        // Engine-internal: set here only to prove the generator replaces it
        // (map rules always false; the combine rule uses transform.cleanup).
        cleanup_chunks: true,
        // Replaced by transform.map / the combine command.
        shell: Some("authored shell must be replaced".to_string()),
        // Preserved: runs after the generated shell (docs: shell, then script).
        script: Some("scripts/post_process.sh".to_string()),
        threads: Some(16),
        memory: Some("64G".to_string()),
        resources: Resources {
            threads: 8,
            memory: Some("32G".to_string()),
            gpu: None,
            gpu_spec: None,
            disk: Some("500G".to_string()),
            time_limit: Some("12h".to_string()),
            partition: Some("batch".to_string()),
            groups: resource_groups,
            numa_pin: true,
            numa_node: Some(1),
        },
        environment: EnvironmentSpec {
            conda: Some("envs/tools.yaml".to_string()),
            mamba: Some("envs/tools.mamba.yaml".to_string()),
            pixi: Some("envs/pixi.toml".to_string()),
            docker: Some("biocontainers/gatk:4.5".to_string()),
            gpus: Some("all".to_string()),
            singularity: Some("docker://biocontainers/gatk:4.5".to_string()),
            venv: Some("envs/venv".to_string()),
            modules: vec!["gcc/11.2".to_string()],
            conda_prefix: Some(".oxo-flow/envs/conda".to_string()),
            mamba_prefix: Some(".oxo-flow/envs/mamba".to_string()),
            venv_requirements: Some("envs/requirements.txt".to_string()),
        },
        env_group: Some("aligners".to_string()),
        log: Some("logs/{chr}.log".to_string()),
        benchmark: Some("bench/{chr}.tsv".to_string()),
        params,
        priority: 42,
        target: true,
        optional: OptionalMode::Any,
        group: Some("variant-calling".to_string()),
        description: Some("split-map-combine field-preservation evidence".to_string()),
        report: Some(RuleReport::Detailed {
            file: Some("report/caption.md".to_string()),
            caption: Some("Variant calling caption".to_string()),
        }),
        when: Some("config.enable_qc".to_string()),
        temporary: true,
        scratch: true,
        singleton: true,
        // A rule declaring both scatter and transform takes the scatter
        // branch first; transform would never run.
        scatter: None,
        transform: Some(transform),
        temp_output: vec![".tmp/partial.vcf".to_string()],
        protected_output: vec!["merged/all.vcf.gz".to_string()],
        input_function: Some("resolve_inputs".to_string()),
        retries: 5,
        tags: vec!["variant".to_string(), "production".to_string()],
        ancient: vec!["references/genome.fa".to_string()],
        localrule: true,
        envvars,
        checkpoint: true,
        checkpoint_manifest: Some("reentry.toml".to_string()),
        required: false,
        depends_on: vec!["setup".to_string()],
        extends: Some("base".to_string()),
        retry_delay: Some("30s".to_string()),
        pre_exec: Some("mkdir -p logs".to_string()),
        on_success: Some("echo done".to_string()),
        on_failure: Some("echo failed".to_string()),
        format_hint: vec!["vcf".to_string()],
        pipe: true,
        staging_mode: Some(StagingMode::StreamFifo),
        checksum: Some("sha256".to_string()),
        resource_hint: Some(ResourceHint {
            input_size: Some("large".to_string()),
            memory_scale: Some(1.5),
            runtime: Some("slow".to_string()),
            io_bound: Some(true),
        }),
        rule_metadata,
        cache_key: Some("variant-v1".to_string()),
        interpreter: Some("/usr/bin/python3".to_string()),
        types: Some(RuleTypesSpec {
            inputs: type_inputs,
            outputs: type_outputs,
        }),
    }
}

/// Expected values for the fields the transform generator replaces.
struct ExpectedTransformRule {
    name: String,
    input: Vec<String>,
    output: Vec<String>,
    shell: String,
    cleanup_chunks: bool,
    expand_inputs_baked: BTreeSet<String>,
}

/// Assert, field by field, that a generated rule differs from the authored
/// source rule ONLY in the fields the transform operator owns.
fn assert_transform_generated_rule(
    generated: &Rule,
    source: &Rule,
    expected: &ExpectedTransformRule,
) {
    // Exhaustive destructure WITHOUT `..`: a new `Rule` field is a compile
    // error here until this suite covers it.
    let Rule {
        name,
        input,
        expand_inputs,
        expand_inputs_baked,
        input_groups,
        output,
        output_pattern,
        cleanup_chunks,
        shell,
        script,
        threads,
        memory,
        resources,
        environment,
        env_group,
        log,
        benchmark,
        params,
        priority,
        target,
        optional,
        group,
        description,
        report,
        when,
        temporary,
        scratch,
        singleton,
        scatter,
        transform,
        temp_output,
        protected_output,
        input_function,
        retries,
        tags,
        ancient,
        localrule,
        envvars,
        checkpoint,
        checkpoint_manifest,
        required,
        depends_on,
        extends,
        retry_delay,
        pre_exec,
        on_success,
        on_failure,
        format_hint,
        pipe,
        staging_mode,
        checksum,
        resource_hint,
        rule_metadata,
        cache_key,
        interpreter,
        types,
    } = generated;

    // ── Fields the transform operator intentionally replaces ──
    assert_eq!(name, &expected.name, "generated rule name");
    assert_eq!(
        input,
        &FilePatterns::List(expected.input.clone()),
        "generated rule input (transform substitution + expand_inputs materialization)"
    );
    assert_eq!(
        output,
        &FilePatterns::List(expected.output.clone()),
        "generated rule output"
    );
    assert_eq!(
        shell.as_deref(),
        Some(expected.shell.as_str()),
        "generated rule shell must be the transform command"
    );
    assert_eq!(
        cleanup_chunks, &expected.cleanup_chunks,
        "cleanup_chunks is generator-owned"
    );
    assert!(
        transform.is_none(),
        "the transform operator itself must be cleared on generated rules"
    );
    assert!(
        scatter.is_none(),
        "generated transform rules must not carry scatter"
    );
    assert!(
        output_pattern.is_none(),
        "generated transform rules must not carry output_pattern"
    );

    // ── Every other field is the authored rule's value, byte for byte ──
    assert_eq!(expand_inputs, &source.expand_inputs, "expand_inputs");
    // Materialization (a later pass, shared with hand-written rules)
    // appends the expanded entry to the authored baked provenance.
    assert_eq!(
        expand_inputs_baked, &expected.expand_inputs_baked,
        "expand_inputs_baked"
    );
    assert_eq!(input_groups, &source.input_groups, "input_groups");
    assert_eq!(script, &source.script, "script");
    assert_eq!(threads, &source.threads, "threads");
    assert_eq!(memory, &source.memory, "memory");
    assert_eq!(resources, &source.resources, "resources");
    assert_eq!(environment, &source.environment, "environment");
    assert_eq!(env_group, &source.env_group, "env_group");
    assert_eq!(log, &source.log, "log");
    assert_eq!(benchmark, &source.benchmark, "benchmark");
    assert_eq!(params, &source.params, "params");
    assert_eq!(priority, &source.priority, "priority");
    assert_eq!(target, &source.target, "target");
    assert_eq!(optional, &source.optional, "optional");
    assert_eq!(group, &source.group, "group");
    assert_eq!(description, &source.description, "description");
    assert_eq!(report, &source.report, "report");
    assert_eq!(when, &source.when, "when");
    assert_eq!(temporary, &source.temporary, "temporary");
    assert_eq!(scratch, &source.scratch, "scratch");
    assert_eq!(singleton, &source.singleton, "singleton");
    assert_eq!(temp_output, &source.temp_output, "temp_output");
    assert_eq!(
        protected_output, &source.protected_output,
        "protected_output"
    );
    assert_eq!(input_function, &source.input_function, "input_function");
    assert_eq!(retries, &source.retries, "retries");
    assert_eq!(tags, &source.tags, "tags");
    assert_eq!(ancient, &source.ancient, "ancient");
    assert_eq!(localrule, &source.localrule, "localrule");
    assert_eq!(envvars, &source.envvars, "envvars");
    assert_eq!(checkpoint, &source.checkpoint, "checkpoint");
    assert_eq!(
        checkpoint_manifest, &source.checkpoint_manifest,
        "checkpoint_manifest"
    );
    assert_eq!(required, &source.required, "required");
    assert_eq!(depends_on, &source.depends_on, "depends_on");
    assert_eq!(extends, &source.extends, "extends");
    assert_eq!(retry_delay, &source.retry_delay, "retry_delay");
    assert_eq!(pre_exec, &source.pre_exec, "pre_exec");
    assert_eq!(on_success, &source.on_success, "on_success");
    assert_eq!(on_failure, &source.on_failure, "on_failure");
    assert_eq!(format_hint, &source.format_hint, "format_hint");
    assert_eq!(pipe, &source.pipe, "pipe");
    assert_eq!(staging_mode, &source.staging_mode, "staging_mode");
    assert_eq!(checksum, &source.checksum, "checksum");
    assert_eq!(resource_hint, &source.resource_hint, "resource_hint");
    assert_eq!(rule_metadata, &source.rule_metadata, "rule_metadata");
    assert_eq!(cache_key, &source.cache_key, "cache_key");
    assert_eq!(interpreter, &source.interpreter, "interpreter");
    assert_eq!(types, &source.types, "types");
}

/// Expand `source` (plus the support rules its `extends`/`depends_on`
/// reference) and return the expanded config.
fn expand_source(source: Rule) -> WorkflowConfig {
    let mut config = WorkflowConfig::parse(WORKFLOW_TOML).expect("minimal workflow parses");
    config.rules = vec![support_rule("base"), support_rule("setup"), source];
    config
        .expand_wildcards()
        .expect("transform expansion succeeds");
    config
}

fn find_rule<'a>(config: &'a WorkflowConfig, name: &str) -> &'a Rule {
    config
        .rules
        .iter()
        .find(|rule| rule.name == name)
        .unwrap_or_else(|| {
            panic!(
                "generated rule '{name}' missing; rules = {:?}",
                config.rules.iter().map(|r| &r.name).collect::<Vec<_>>()
            )
        })
}

fn chunk_output(value: &str) -> String {
    format!(".oxo-flow/chunks/chr/{value}.vcf.gz")
}

/// The authored baked provenance plus the `expand_inputs` entry the shared
/// materialization pass appends (`refs/{genome}.fa` → `refs/hg38.fa`).
fn expected_baked(source: &Rule) -> BTreeSet<String> {
    let mut baked = source.expand_inputs_baked.clone();
    baked.insert("refs/hg38.fa".to_string());
    baked
}

fn map_expected_input(source: &Rule, value: &str) -> Vec<String> {
    let mut input: Vec<String> = source
        .input
        .to_vec()
        .iter()
        .map(|pattern| pattern.replace("{chr}", value))
        .collect();
    input.push("refs/hg38.fa".to_string());
    input
}

fn combine_expected_input() -> Vec<String> {
    let mut input: Vec<String> = SPLIT_VALUES.iter().map(|v| chunk_output(v)).collect();
    input.push("refs/hg38.fa".to_string());
    input
}

fn transform_config(combine: Option<CombineConfig>) -> TransformConfig {
    TransformConfig {
        split: SplitConfig {
            by: "chr".to_string(),
            values: SPLIT_VALUES.iter().map(|v| v.to_string()).collect(),
            values_from: None,
            n: None,
            glob: None,
        },
        map: "process {chr} {input} > {output}".to_string(),
        combine,
        cleanup: true,
    }
}

/// Mode A — split → map → combine with an explicit combine shell.
#[test]
fn transform_generated_rules_preserve_every_rule_field() {
    let source = exhaustive_transform_source(transform_config(Some(CombineConfig {
        shell: Some("merge {chunks} > {output}".to_string()),
        aggregate: false,
        method: None,
        header: None,
    })));
    let config = expand_source(source.clone());

    let baked = expected_baked(&source);
    for value in SPLIT_VALUES {
        let map_rule = find_rule(&config, &format!("splitter_{value}"));
        assert_transform_generated_rule(
            map_rule,
            &source,
            &ExpectedTransformRule {
                name: format!("splitter_{value}"),
                input: map_expected_input(&source, value),
                output: vec![chunk_output(value)],
                shell: format!("process {value} {{input}} > {{output}}"),
                cleanup_chunks: false,
                expand_inputs_baked: baked.clone(),
            },
        );
    }

    let combine_rule = find_rule(&config, "splitter_combine");
    assert_transform_generated_rule(
        combine_rule,
        &source,
        &ExpectedTransformRule {
            name: "splitter_combine".to_string(),
            input: combine_expected_input(),
            output: source.output.to_vec(),
            shell: format!(
                "merge {} {} > merged/all.vcf.gz",
                chunk_output("chr1"),
                chunk_output("chr2")
            ),
            cleanup_chunks: true, // transform.cleanup
            expand_inputs_baked: baked,
        },
    );
}

/// Mode B — split → map without a combine stage.
#[test]
fn transform_without_combine_preserves_every_rule_field() {
    let source = exhaustive_transform_source(transform_config(None));
    let config = expand_source(source.clone());

    let baked = expected_baked(&source);
    for value in SPLIT_VALUES {
        let map_rule = find_rule(&config, &format!("splitter_{value}"));
        assert_transform_generated_rule(
            map_rule,
            &source,
            &ExpectedTransformRule {
                name: format!("splitter_{value}"),
                input: map_expected_input(&source, value),
                output: vec![chunk_output(value)],
                shell: format!("process {value} {{input}} > {{output}}"),
                cleanup_chunks: false,
                expand_inputs_baked: baked.clone(),
            },
        );
    }
    assert!(
        config
            .rules
            .iter()
            .all(|rule| rule.name != "splitter_combine"),
        "no combine config must generate no combine rule"
    );
}

/// Mode C — split → map → aggregate (`concat`).
#[test]
fn transform_aggregate_combine_preserves_every_rule_field() {
    let source = exhaustive_transform_source(transform_config(Some(CombineConfig {
        shell: None,
        aggregate: true,
        method: Some("concat".to_string()),
        header: None,
    })));
    let config = expand_source(source.clone());

    let baked = expected_baked(&source);
    for value in SPLIT_VALUES {
        let map_rule = find_rule(&config, &format!("splitter_{value}"));
        assert_transform_generated_rule(
            map_rule,
            &source,
            &ExpectedTransformRule {
                name: format!("splitter_{value}"),
                input: map_expected_input(&source, value),
                output: vec![chunk_output(value)],
                shell: format!("process {value} {{input}} > {{output}}"),
                cleanup_chunks: false,
                expand_inputs_baked: baked.clone(),
            },
        );
    }

    let combine_rule = find_rule(&config, "splitter_combine");
    assert_transform_generated_rule(
        combine_rule,
        &source,
        &ExpectedTransformRule {
            name: "splitter_combine".to_string(),
            input: combine_expected_input(),
            output: source.output.to_vec(),
            shell: format!(
                "cat {} {} > merged/all.vcf.gz",
                chunk_output("chr1"),
                chunk_output("chr2")
            ),
            cleanup_chunks: true, // transform.cleanup
            expand_inputs_baked: baked,
        },
    );
}
