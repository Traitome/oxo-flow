//! Sample input readiness for incremental data arrival (issue #63).
//!
//! Sequencing centers deliver data in batches: fastq files for a cohort trickle
//! in over days. Before any rule runs, `compute_readiness` answers two
//! questions per sample:
//!
//! - which samples have **complete external inputs** and can be processed now
//!   ("ready"), and
//! - which samples are still waiting for data, and exactly which files.
//!
//! Readiness is computed on an **expanded** workflow ([`WorkflowConfig`] after
//! [`WorkflowConfig::expand_wildcards`]), so `{sample}` placeholders are already
//! concrete and `expand_inputs` injections are visible. A sample is ready when
//! every external input belonging to it exists; intermediate products are
//! excluded (producing them is the DAG's job), optional rules never block
//! readiness (the executor skips them when their inputs are absent), and rules
//! whose `when` gate is statically false under the current `[config]` never
//! block readiness either (the DAG prunes those nodes and the executor skips
//! them — their inputs would cite paths no rule will ever produce).

use crate::config::WorkflowConfig;

/// Readiness status of a single sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleStatus {
    /// Sample name as declared in `[[sample_groups]]` or `[[pairs]]`.
    pub name: String,
    /// Whether every external input belonging to this sample exists.
    pub ready: bool,
    /// Missing external input paths (empty when ready).
    pub missing: Vec<String>,
}

/// Per-sample readiness report for a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadinessReport {
    /// Total number of samples examined.
    pub total: usize,
    /// Samples with complete external inputs, in workflow order.
    pub ready: Vec<SampleStatus>,
    /// Samples still waiting for data, in workflow order.
    pub waiting: Vec<SampleStatus>,
    /// Missing external inputs that belong to no specific sample (e.g. shared
    /// reference files). These block the workflow but not any one sample.
    pub missing_global: Vec<String>,
}

/// Compute per-sample readiness on an expanded workflow config.
///
/// Relative input paths resolve against `base_dir` — the directory the
/// executor runs rules from (the workflow file's directory, or `--workdir`).
/// The executor spawns shells with that working directory, so readiness must
/// judge existence from the same place or it predicts execution wrong.
pub fn compute_readiness(config: &WorkflowConfig, base_dir: &std::path::Path) -> ReadinessReport {
    // Sample universe: group samples in workflow order, then pair
    // experiment/control names, deduplicated. Pairs without a control side
    // contribute only the experiment name.
    let mut universe: Vec<String> = Vec::new();
    for group in &config.sample_groups {
        for sample in &group.samples {
            if !universe.contains(sample) {
                universe.push(sample.clone());
            }
        }
    }
    for pair in &config.pairs {
        for name in [
            pair.experiment.as_str(),
            pair.control.as_deref().unwrap_or(""),
        ] {
            if !name.is_empty() && !universe.iter().any(|u| u == name) {
                universe.push(name.to_string());
            }
        }
    }

    // `{config.x}` placeholders resolve against [config] values, exactly like
    // the executor does at execution time.
    let wildcard_values = config_vars(config);

    // Bare-key [config] values for `when` evaluation — the same shape
    // `expand_wildcards` builds for its pair gate (keys WITHOUT the
    // `config.` prefix, `toml::Value`-typed). Deliberately NOT
    // [`config_vars`]: that map is `config.<key>` → String, the path-
    // expansion namespace; the `when` evaluator resolves `config.x`
    // references itself and would see no bare keys through it.
    let config_values: std::collections::HashMap<String, toml::Value> = config
        .config
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Inputs produced by the workflow itself are not readiness gatekeepers —
    // producing them is the DAG's job.
    let mut produced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for rule in &config.rules {
        for output in rule.output.iter() {
            produced.insert(crate::executor::checkpoint::expand_config_in_path(
                output,
                &wildcard_values,
            ));
        }
        // `protected_output` side effects are workflow-produced too
        // (issue #865): a rule may declare expensive secondary artifacts
        // only in `protected_output` (deliberately absent from `output`),
        // and downstream `expand_inputs` consumers legitimately read them.
        // Fan-out expansion bakes `input`/`output` but never touches
        // `protected_output`, so per-instance declarations stay raw
        // `{wildcard}` templates — concretize them with the same
        // per-instance bindings the `expand_inputs` pass resolves
        // (`expansion_values` winning over `expansion_pairs`, the
        // `expand_pattern_variables` order), then resolve `{config.*}`.
        // Report-only: this set never feeds the executor DAG. Shell-glob
        // declarations (`results/*.bam`, issue #473) are left out — a glob
        // has no finite path set here, and glob-shaped *inputs* are skipped
        // in the loop below anyway.
        let mut bindings = std::collections::HashMap::new();
        for source in [
            config.expansion_values.get(&rule.name),
            config.expansion_pairs.get(&rule.name),
        ]
        .into_iter()
        .flatten()
        {
            for (name, value) in source {
                bindings
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        for pattern in rule.protected_output.iter() {
            let concrete = crate::config::expand_rule_shell(pattern, &bindings);
            let concrete =
                crate::executor::checkpoint::expand_config_in_path(&concrete, &wildcard_values);
            if concrete.is_empty() || concrete.contains('{') || concrete.contains('*') {
                continue;
            }
            produced.insert(concrete);
        }
    }

    let mut missing_by_sample: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    let mut missing_global: Vec<String> = Vec::new();

    for rule in &config.rules {
        // Optional rules are skipped by the executor when their inputs are
        // absent, so they must not block readiness.
        if rule.optional.is_optional() {
            continue;
        }
        // A rule whose `when` gate is statically false under the current
        // [config] never executes (the DAG prunes it; the executor would
        // skip it), so its inputs must not be cited as missing. Config-only
        // conditions survive expansion un-baked — expansion only bakes
        // wildcard/meta-dependent `when` text — so they are evaluated here.
        // Unbound references → false (#199) matches the executor's verdict;
        // wildcard/meta conditions were already baked to concrete verdicts
        // during expansion, so re-evaluating them reproduces the same result.
        if let Some(when) = rule.when.as_deref()
            && !crate::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                when,
                &config_values,
                &std::collections::HashMap::new(),
                Some(base_dir),
            )
        {
            continue;
        }
        let scoped: &[String] = config
            .expansion_samples
            .get(&rule.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for input in rule.input.iter() {
            let input = crate::executor::checkpoint::expand_config_in_path(input, &wildcard_values);
            // Skip what cannot be resolved yet (unknown config vars or globs)
            // and what the workflow itself produces.
            if input.is_empty()
                || input.contains('{')
                || input.contains('*')
                || produced.contains(&input)
            {
                continue;
            }
            if base_dir.join(&input).exists() {
                continue;
            }
            if let Some(name) = longest_matching_sample(&input, scoped) {
                missing_by_sample
                    .entry(name)
                    .or_default()
                    .push(input.to_string());
            } else {
                missing_global.push(input.to_string());
            }
        }
    }

    let mut report = ReadinessReport {
        total: universe.len(),
        ..Default::default()
    };
    for name in &universe {
        let mut missing = missing_by_sample.remove(name.as_str()).unwrap_or_default();
        missing.sort();
        missing.dedup();
        if missing.is_empty() {
            report.ready.push(SampleStatus {
                name: name.clone(),
                ready: true,
                missing: Vec::new(),
            });
        } else {
            report.waiting.push(SampleStatus {
                name: name.clone(),
                ready: false,
                missing,
            });
        }
    }
    missing_global.sort();
    missing_global.dedup();
    report.missing_global = missing_global;
    report
}

/// Map every `[config]` value to the `config.<key>` wildcard form the
/// executor resolves in paths (`{config.data_dir}` → value).
fn config_vars(config: &WorkflowConfig) -> std::collections::HashMap<String, String> {
    config
        .config
        .iter()
        .map(|(key, value)| {
            let string_val = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (format!("config.{key}"), string_val)
        })
        .collect()
}

/// The longest scoped sample name mentioned in `path`.
///
/// Attribution is bounded by expansion provenance: the rule's inputs can only
/// belong to the samples the rule was expanded for. Among those, prefer the
/// longest name so `S1` never steals a path belonging to `S1_extra`.
fn longest_matching_sample<'a>(path: &str, scoped: &'a [String]) -> Option<&'a str> {
    scoped
        .iter()
        .filter(|name| !name.is_empty() && path.contains(name.as_str()))
        .max_by_key(|name| name.len())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    /// Parse a TOML workflow, expand it, and compute readiness against `dir`.
    /// Fixture paths are made absolute so tests are parallel-safe (no chdir).
    fn readiness_for(toml: &str, dir: &std::path::Path) -> ReadinessReport {
        let mut config = WorkflowConfig::parse(toml).expect("parse workflow");
        config.apply_defaults();
        config.expand_wildcards().expect("expand wildcards");
        compute_readiness(&config, dir)
    }

    #[test]
    fn ready_sample_all_entry_inputs_exist() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1_R1.fastq.gz"), b"x").unwrap();
        std::fs::write(data.join("S1_R2.fastq.gz"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "qc"
            input = ["{d}/data/{{sample}}_R1.fastq.gz", "{d}/data/{{sample}}_R2.fastq.gz"]
            output = ["{d}/results/qc/{{sample}}.txt"]
            shell = "cat {{{{input}}}} > {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(report.ready[0].missing, Vec::<String>::new());
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
    }

    #[test]
    fn waiting_sample_lists_exact_missing_entry_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1_R1.fastq.gz"), b"x").unwrap();
        std::fs::write(data.join("S1_R2.fastq.gz"), b"x").unwrap();
        std::fs::write(data.join("S2_R1.fastq.gz"), b"x").unwrap();
        // data/S2_R2.fastq.gz deliberately absent — S2 is waiting.

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "qc"
            input = ["{d}/data/{{sample}}_R1.fastq.gz", "{d}/data/{{sample}}_R2.fastq.gz"]
            output = ["{d}/results/qc/{{sample}}.txt"]
            shell = "cat {{{{input}}}} > {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 2);
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(report.waiting.len(), 1);
        assert_eq!(report.waiting[0].name, "S2");
        assert_eq!(
            report.waiting[0].missing,
            vec![data.join("S2_R2.fastq.gz").display().to_string()]
        );
    }

    #[test]
    fn intermediate_outputs_never_block_readiness() {
        // align consumes qc's output, which does not exist yet on disk.
        // Producing it is the DAG's job — the sample must stay ready.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1_R1.fastq.gz"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "qc"
            input = ["{d}/data/{{sample}}_R1.fastq.gz"]
            output = ["{d}/results/qc/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"

            [[rules]]
            name = "align"
            depends_on = ["qc"]
            input = ["{d}/results/qc/{{sample}}.txt"]
            output = ["{d}/results/align/{{sample}}.bam"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(
            report.ready.len(),
            1,
            "intermediates must not block: {report:#?}"
        );
        assert_eq!(report.ready[0].name, "S1");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
    }

    #[test]
    fn optional_rules_do_not_block_readiness() {
        // A missing optional input skips the rule at execution — the sample
        // itself is still processable.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1_R1.fastq.gz"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "qc"
            input = ["{d}/data/{{sample}}_R1.fastq.gz"]
            output = ["{d}/results/qc/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"

            [[rules]]
            name = "extra_qc"
            optional = true
            input = ["{d}/data/{{sample}}_R3.fastq.gz"]
            output = ["{d}/results/extra/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(
            report.ready.len(),
            1,
            "optional inputs must not block: {report:#?}"
        );
        assert_eq!(report.ready[0].name, "S1");
        assert!(report.waiting.is_empty());
    }

    #[test]
    fn config_vars_in_paths_are_resolved_before_checking() {
        // `{config.data_dir}` must be expanded from [config] before the
        // existence check, mirroring how the executor resolves paths at
        // execution time.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("raw");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1_R1.fastq.gz"), b"x").unwrap();
        // S2_R1.fastq.gz absent — S2 must be waiting.

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [config]
            data_dir = "{d}/raw"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "qc"
            input = ["{{config.data_dir}}/{{sample}}_R1.fastq.gz"]
            output = ["{d}/results/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 2);
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(report.waiting.len(), 1);
        assert_eq!(report.waiting[0].name, "S2");
        assert_eq!(
            report.waiting[0].missing,
            vec![data.join("S2_R1.fastq.gz").display().to_string()]
        );
    }

    #[test]
    fn pair_samples_reported_per_name() {
        // Pair workflows: experiment and control are the sample identifiers.
        // Each side is reported independently — the waiting side lists the
        // exact missing file.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("T1.fq"), b"x").unwrap();
        // N1.fq deliberately absent — N1 is waiting.

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[pairs]]
            pair_id = "P1"
            experiment = "T1"
            control = "N1"

            [[rules]]
            name = "align"
            input = ["{d}/data/{{experiment}}.fq", "{d}/data/{{control}}.fq"]
            output = ["{d}/results/{{experiment}}_{{control}}.bam"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 2, "{report:#?}");
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "T1");
        assert_eq!(report.waiting.len(), 1);
        assert_eq!(report.waiting[0].name, "N1");
        assert_eq!(
            report.waiting[0].missing,
            vec![data.join("N1.fq").display().to_string()]
        );
    }

    #[test]
    fn pair_without_control_has_no_empty_sample() {
        // A pair without a control side must not invent an empty sample name.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("T2.fq"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[pairs]]
            pair_id = "P2"
            experiment = "T2"

            [[rules]]
            name = "align"
            input = ["{d}/data/{{experiment}}.fq"]
            output = ["{d}/results/{{experiment}}.bam"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1, "{report:#?}");
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "T2");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
    }

    #[test]
    fn relative_inputs_resolve_against_base_dir_not_cwd() {
        // The executor runs shells in the workflow's directory (or
        // --workdir), not the process CWD. Readiness must judge relative
        // paths from the same place or it predicts execution wrong.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1.fq"), b"x").unwrap();

        // Relative path in the workflow; only base_dir/data/S1.fq exists.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "qc"
            input = ["data/{sample}.fq"]
            output = ["results/{sample}.txt"]
            shell = "touch {output}"
        "#;

        let report = readiness_for(toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1, "{report:#?}");
        assert!(report.waiting.is_empty());

        // Against an unrelated base dir the same workflow sees S1 waiting.
        let elsewhere = tempfile::tempdir().unwrap();
        let report = readiness_for(toml, elsewhere.path());
        assert_eq!(report.waiting.len(), 1, "{report:#?}");
        assert_eq!(report.waiting[0].name, "S1");
        assert_eq!(report.waiting[0].missing, vec!["data/S1.fq".to_string()]);
    }

    #[test]
    fn workflow_without_samples_reports_empty() {
        // A workflow with no sample sources has nothing to gate — the report
        // is empty and the CLI skips the readiness section.
        let toml = r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[rules]]
            name = "hello"
            output = ["hello.txt"]
            shell = "echo hi > hello.txt"
        "#;
        let dir = tempfile::tempdir().unwrap();
        let report = readiness_for(toml, dir.path());
        assert_eq!(report, ReadinessReport::default());
    }

    #[test]
    fn missing_path_attributed_to_longest_sample_name() {
        // With samples "S1" and "S1_extra", a missing path mentioning
        // "S1_extra" must never be blamed on "S1".
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1.fq"), b"x").unwrap();
        // S1_extra.fq absent.

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S1_extra"]

            [[rules]]
            name = "qc"
            input = ["{d}/data/{{sample}}.fq"]
            output = ["{d}/results/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 2, "{report:#?}");
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(report.waiting.len(), 1);
        assert_eq!(report.waiting[0].name, "S1_extra");
        assert_eq!(
            report.waiting[0].missing,
            vec![data.join("S1_extra.fq").display().to_string()]
        );
    }

    #[test]
    fn unresolvable_paths_never_block_readiness() {
        // A path whose config vars are not resolvable yet (e.g. injected at
        // execution time) cannot be verified — it must not mark the sample
        // waiting on a guess.
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "qc"
            input = ["{{config.unknown_dir}}/{{sample}}.fq"]
            output = ["{d}/results/{{sample}}.txt"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1, "{report:#?}");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
    }

    #[test]
    fn when_false_rules_do_not_block_readiness() {
        // A rule whose `when` gate is statically false under the current
        // [config] never executes (the DAG prunes it), so its expanded
        // input — a path NO rule produces under this config — must not be
        // cited as missing. Mirrors the methylseq shape: default
        // `aligner = "bismark"` keeps the bwameth-only `sort` rule off, and
        // `results/bwameth/alignments/{sample}.bam` is a phantom.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1.fq"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [config]
            out_dir = "results"
            aligner = "bismark"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "align"
            input = ["{d}/data/{{sample}}.fq"]
            output = ["{{config.out_dir}}/{{config.aligner}}/alignments/{{sample}}.bam"]
            shell = "touch {{{{output}}}}"

            [[rules]]
            name = "sort"
            depends_on = ["align"]
            when = "config.aligner == \"bwameth\" || config.aligner == \"bwamem\""
            input = ["{{config.out_dir}}/{{config.aligner}}/alignments/{{sample}}.bam"]
            output = ["{{config.out_dir}}/{{config.aligner}}/alignments/{{sample}}.sorted.bam"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(
            report.ready.len(),
            1,
            "when-false rule inputs must not block: {report:#?}"
        );
        assert_eq!(report.ready[0].name, "S1");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());

        // The mirrored true-config case: flipping the gate on makes the same
        // rule's input a real gatekeeper again (the align output is produced
        // by the workflow, so the sample stays ready — but the gate is live).
        let toml_on = toml.replace("aligner = \"bismark\"", "aligner = \"bwameth\"");
        let mut config = WorkflowConfig::parse(&toml_on).expect("parse workflow");
        config.apply_defaults();
        config.expand_wildcards().expect("expand wildcards");
        let report = compute_readiness(&config, dir.path());
        assert_eq!(report.ready.len(), 1, "{report:#?}");
    }

    #[test]
    fn missing_global_inputs_reported_separately() {
        // A shared reference file belongs to no sample — it is reported as
        // workflow-level and does not mark any specific sample waiting.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("S1.fq"), b"x").unwrap();

        let toml = format!(
            r#"
            [workflow]
            name = "t"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1"]

            [[rules]]
            name = "align"
            input = ["{d}/refs/genome.fa", "{d}/data/{{sample}}.fq"]
            output = ["{d}/results/{{sample}}.bam"]
            shell = "touch {{{{output}}}}"
        "#,
            d = dir.path().display()
        );

        let report = readiness_for(&toml, dir.path());
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1, "{report:#?}");
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(
            report.missing_global,
            vec![dir.path().join("refs/genome.fa").display().to_string()]
        );
    }

    #[test]
    fn expand_inputs_side_effect_from_protected_output_not_reported_missing() {
        // Issue #865: a producer declares expensive secondary artifacts only
        // in `protected_output` (deliberately absent from `output`), and a
        // cohort consumer reads them via `expand_inputs`. On a cold workdir
        // the consumer's baked inputs are not on disk yet — but they ARE
        // workflow-produced, so they must not be cited as "workflow-level
        // inputs missing (block every sample)". Mirrors the issue's
        // msi_paired/msi_cohort shape with [[pairs]] + config.pairs_list.
        let dir = tempfile::tempdir().unwrap();

        let toml = r#"
            [workflow]
            name = "msi"
            version = "1.0.0"

            [config]
            pairs_list = "Pt01,Pt02"

            [[pairs]]
            pair_id = "Pt01"
            experiment = "Pt01_T"

            [[pairs]]
            pair_id = "Pt02"
            experiment = "Pt02_T"

            [[rules]]
            name = "msi_paired"
            input = ["{input}"]
            output = ["deliver/{pair_id}.msi.tsv"]
            protected_output = ["deliver/{pair_id}.msi.tsv", "msi/{pair_id}_all", "msi/{pair_id}_unstable"]
            shell = "touch {{output[0]}}"

            [[rules]]
            name = "msi_cohort"
            input = []
            expand_inputs = [
                { pattern = "deliver/{pair_id}.msi.tsv", variables = { pair_id = "config.pairs_list" } },
                { pattern = "msi/{pair_id}_all", variables = { pair_id = "config.pairs_list" } }
            ]
            output = ["msi_summary.tsv"]
            shell = "cat {{input}} > {{output[0]}}"
        "#
        .to_string();

        let report = readiness_for(&toml, dir.path());
        // The side-effect files (msi/Pt0x_all) are produced by msi_paired —
        // before the fix they landed in missing_global and "blocked every
        // sample" on a cold workdir.
        assert_eq!(report.missing_global, Vec::<String>::new(), "{report:#?}");
        assert_eq!(report.waiting, Vec::<SampleStatus>::new());
        assert_eq!(report.total, 2);
        assert_eq!(report.ready.len(), 2);
    }
}
