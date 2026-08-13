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
//! excluded (producing them is the DAG's job), and optional rules never block
//! readiness (the executor skips them when their inputs are absent).

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
pub fn compute_readiness(config: &WorkflowConfig) -> ReadinessReport {
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
    }

    let mut missing_by_sample: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    let mut missing_global: Vec<String> = Vec::new();

    for rule in &config.rules {
        // Optional rules are skipped by the executor when their inputs are
        // absent, so they must not block readiness.
        if rule.optional {
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
            if std::path::Path::new(&input).exists() {
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

    /// Parse a TOML workflow, expand it, and compute readiness. Fixture paths
    /// are made absolute so tests are parallel-safe (no chdir).
    fn readiness_for(toml: &str) -> ReadinessReport {
        let mut config = WorkflowConfig::parse(toml).expect("parse workflow");
        config.apply_defaults();
        config.expand_wildcards().expect("expand wildcards");
        compute_readiness(&config)
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
        assert_eq!(report.total, 1, "{report:#?}");
        assert_eq!(report.ready.len(), 1);
        assert_eq!(report.ready[0].name, "T2");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
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
        let report = readiness_for(toml);
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

        let report = readiness_for(&toml);
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

        let report = readiness_for(&toml);
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1, "{report:#?}");
        assert!(report.waiting.is_empty());
        assert!(report.missing_global.is_empty());
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

        let report = readiness_for(&toml);
        assert_eq!(report.total, 1);
        assert_eq!(report.ready.len(), 1, "{report:#?}");
        assert_eq!(report.ready[0].name, "S1");
        assert_eq!(
            report.missing_global,
            vec![dir.path().join("refs/genome.fa").display().to_string()]
        );
    }
}
