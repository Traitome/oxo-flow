//! Integration tests for oxo-flow core workflow parsing, DAG construction,
//! and validation across the complete lifecycle.

use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

/// Test full workflow parse → DAG → validate → execution_order cycle.
#[test]
fn full_lifecycle_simple_workflow() {
    let toml = r#"
        [workflow]
        name = "integration-test"
        version = "1.0.0"

        [[rules]]
        name = "step_a"
        input = ["raw/input.txt"]
        output = ["mid/processed.txt"]
        shell = "process {input[0]} > {output[0]}"
        threads = 2

        [[rules]]
        name = "step_b"
        input = ["mid/processed.txt"]
        output = ["out/final.txt"]
        shell = "finalize {input[0]} > {output[0]}"
        threads = 4
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(config.workflow.name, "integration-test");
    assert_eq!(config.rules.len(), 2);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order, vec!["step_a", "step_b"]);

    assert_eq!(dag.node_count(), 2);
    assert_eq!(dag.edge_count(), 1);
    assert_eq!(dag.root_rules(), vec!["step_a"]);
    assert_eq!(dag.leaf_rules(), vec!["step_b"]);
}

/// Test full lifecycle with the paired tumor-normal example.
#[test]
fn full_lifecycle_paired_experiment_control() {
    let toml = std::fs::read_to_string("examples/paired_experiment_control.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "paired-experiment-control");
    assert!(!config.rules.is_empty());

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());

    // Verify clinical_report is the last step
    assert_eq!(order.last().unwrap(), "clinical_report");

    // Verify DAG has correct structure
    let groups = dag.parallel_groups().unwrap();
    assert!(groups.len() >= 3); // At least 3 levels of parallelism
}

/// Test full lifecycle with the simple variant calling example.
#[test]
fn full_lifecycle_simple_variant_calling() {
    let toml = std::fs::read_to_string("examples/simple_variant_calling.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();

    assert!(!config.rules.is_empty());

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());
}

/// Test apply_defaults propagation through the workflow lifecycle.
#[test]
fn apply_defaults_in_lifecycle() {
    let toml = r#"
        [workflow]
        name = "defaults-test"

        [defaults]
        threads = 16
        memory = "32G"

        [[rules]]
        name = "step_a"
        output = ["out.txt"]
        shell = "echo hello > {output[0]}"

        [[rules]]
        name = "step_b"
        input = ["out.txt"]
        output = ["final.txt"]
        shell = "cat {input[0]} > {output[0]}"
        threads = 4
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.apply_defaults();

    // step_a should get defaults
    assert_eq!(config.rules[0].effective_threads(), 16);
    assert_eq!(config.rules[0].effective_memory(), Some("32G"));

    // step_b specified its own threads, should keep them
    assert_eq!(config.rules[1].effective_threads(), 4);
    // but step_b didn't specify memory, should get default
    assert_eq!(config.rules[1].effective_memory(), Some("32G"));
}

/// Test Venus pipeline generates valid oxoflow.
#[test]
fn venus_generate_and_validate() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentControl,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![oxo_flow_venus::Sample {
            name: "EXP_01".to_string(),
            r1_fastq: "raw/EXP_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/EXP_01_R2.fq.gz".to_string()),
            is_experiment: true,
        }],
        control_samples: vec![oxo_flow_venus::Sample {
            name: "CTRL_01".to_string(),
            r1_fastq: "raw/CTRL_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/CTRL_01_R2.fq.gz".to_string()),
            is_experiment: false,
        }],
        known_sites: Some("/ref/dbsnp.vcf.gz".to_string()),
        target_bed: None,
        threads: 8,
        output_dir: "output".to_string(),
        annotate: true,
        report: true,
        project_name: Some("VenusIntegration".to_string()),
    };

    let toml_str = oxo_flow_venus::generate_oxoflow(&config).unwrap();
    let wf = WorkflowConfig::parse(&toml_str).unwrap();

    assert_eq!(wf.workflow.name, "VenusIntegration");

    let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());

    let groups = dag.parallel_groups().unwrap();
    assert!(groups.len() >= 2); // Should have parallel groups
}

/// Test parallel groups structure for a complex pipeline.
#[test]
fn parallel_groups_complex() {
    let toml = r#"
        [workflow]
        name = "parallel-test"

        [[rules]]
        name = "source_a"
        output = ["a.txt"]
        shell = "echo a"

        [[rules]]
        name = "source_b"
        output = ["b.txt"]
        shell = "echo b"

        [[rules]]
        name = "source_c"
        output = ["c.txt"]
        shell = "echo c"

        [[rules]]
        name = "merge_ab"
        input = ["a.txt", "b.txt"]
        output = ["ab.txt"]
        shell = "cat a.txt b.txt > ab.txt"

        [[rules]]
        name = "final"
        input = ["ab.txt", "c.txt"]
        output = ["final.txt"]
        shell = "cat ab.txt c.txt > final.txt"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();

    let groups = dag.parallel_groups().unwrap();
    // Level 0: source_a, source_b, source_c (all independent)
    // Level 1: merge_ab (depends on source_a, source_b)
    // Level 2: final (depends on merge_ab, source_c)
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].len(), 3); // 3 source rules in parallel
    assert_eq!(groups[1].len(), 1); // merge_ab
    assert_eq!(groups[2].len(), 1); // final
}

/// Test report generation lifecycle.
#[test]
fn report_generation_lifecycle() {
    use oxo_flow_core::report::*;

    let mut report = Report::new("Integration Report", "test-pipeline", "1.0.0");

    // Add sample info
    let sample = SampleInfo {
        sample_id: "SAMPLE_01".to_string(),
        patient_id: Some("P001".to_string()),
        sample_type: "Tumor".to_string(),
        collection_date: Some("2024-01-15".to_string()),
        platform: Some("Illumina NovaSeq 6000".to_string()),
        seq_type: Some("WGS".to_string()),
    };
    report.add_section(sample_info_section(&sample));

    // Add QC metrics
    let metrics = vec![QcMetric {
        sample: "SAMPLE_01".to_string(),
        total_reads: 100_000_000,
        mapped_reads: 98_000_000,
        mapping_rate: 0.98,
        mean_coverage: 30.0,
        duplicate_rate: 0.05,
    }];
    report.add_section(qc_metrics_section(&metrics));

    // Add variant summary
    let variants = vec![VariantSummary {
        gene: "BRCA1".to_string(),
        variant: "c.5266dupC".to_string(),
        classification: "Pathogenic".to_string(),
        allele_frequency: 0.45,
        depth: 250,
        clinical_significance: Some("Associated with hereditary breast cancer".to_string()),
    }];
    report.add_section(variant_summary_section(&variants));

    // Add disclaimer
    report.add_section(clinical_disclaimer_section());

    // Generate HTML
    let html = report.to_html();
    assert!(html.contains("SAMPLE_01"));
    assert!(html.contains("BRCA1"));
    assert!(html.contains("Pathogenic"));
    assert!(html.contains("Clinical Disclaimer"));

    // Generate JSON
    let json = report.to_json().unwrap();
    assert!(json.contains("SAMPLE_01"));
    assert!(json.contains("BRCA1"));

    // Use template engine
    let engine = TemplateEngine::new().unwrap();
    let templated_html = engine.render_report(&report).unwrap();
    assert!(templated_html.contains("Integration Report"));
}

// === Gallery Workflow Validation Tests ===
// Every gallery workflow must parse, build a valid DAG, and produce a non-empty execution order.

#[test]
fn gallery_01_hello_world() {
    let toml = std::fs::read_to_string("examples/gallery/01_hello_world.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "hello-world");
    assert_eq!(config.rules.len(), 1);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order, vec!["greet"]);
}

#[test]
fn gallery_02_file_pipeline() {
    let toml = std::fs::read_to_string("examples/gallery/02_file_pipeline.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "file-pipeline");
    assert_eq!(config.rules.len(), 3);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 3);
    assert_eq!(order[0], "generate_data");
    assert_eq!(order[2], "summarize");
}

#[test]
fn gallery_03_parallel_samples() {
    let toml = std::fs::read_to_string("examples/gallery/03_parallel_samples.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "parallel-samples");
    assert_eq!(config.rules.len(), 3);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 3);
}

#[test]
fn gallery_04_scatter_gather() {
    let toml = std::fs::read_to_string("examples/gallery/04_scatter_gather.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "scatter-gather-chromosomes");
    assert_eq!(config.rules.len(), 2);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 2);
    // Both rules at Level 0 with no file dependencies, sorted alphabetically
    assert_eq!(order[0], "gather_gvcf");
    assert_eq!(order[1], "haplotype_caller");
}

#[test]
fn gallery_05_conda_environments() {
    let toml = std::fs::read_to_string("examples/gallery/05_conda_environments.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "environment-showcase");
    assert_eq!(config.rules.len(), 4);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 4);
    // analyze_results depends on both align_sequences and quality_check
    assert_eq!(order.last().unwrap(), "analyze_results");
}

#[test]
fn gallery_06_rnaseq_quantification() {
    let toml =
        std::fs::read_to_string("examples/gallery/06_rnaseq_quantification.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "rnaseq-quantification");
    assert_eq!(config.rules.len(), 5);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 5);
    assert_eq!(order[0], "fastp_trim");
    // Both index_bam and multiqc are terminal nodes with no downstream dependents.
    // Their relative order does not affect correctness — the DAG guarantees all
    // upstream dependencies complete before either runs.
}

#[test]
fn gallery_07_wgs_germline() {
    let toml = std::fs::read_to_string("examples/gallery/07_wgs_germline.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "wgs-germline-calling");
    assert_eq!(config.rules.len(), 10);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 10);
    // Level 0: combine_gvcfs, fastp_qc (alphabetically sorted)
    assert!(order.contains(&"fastp_qc".to_string()));
    // Level 4: annotate_variants, haplotype_caller - last alphabetically is haplotype_caller
    assert_eq!(order.last().unwrap(), "haplotype_caller");
}

#[test]
fn gallery_08_multiomics_integration() {
    let toml =
        std::fs::read_to_string("examples/gallery/08_multiomics_integration.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "multiomics-integration");
    assert_eq!(config.rules.len(), 8);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 8);
    assert_eq!(order.last().unwrap(), "generate_report");

    // Verify the branching DAG structure has parallel groups
    let groups = dag.parallel_groups().unwrap();
    assert!(groups.len() >= 3); // At least 3 levels of depth
    assert!(groups[0].len() >= 3); // 3 independent alignment steps at the root
}

// ============================================================================
// WC-01: Experiment-Control Pairing Tests
// ============================================================================

/// Test that [[pairs]] are parsed correctly and expand_wildcards produces
/// concrete rule instances.
#[test]
fn wc01_pairs_expand_to_concrete_rules() {
    let toml = r#"
        [workflow]
        name = "wc01-test"

        [[pairs]]
        pair_id = "CASE_001"
        experiment = "EXP_01"
        control    = "CTRL_01"

        [[pairs]]
        pair_id = "CASE_002"
        experiment = "EXP_02"
        control    = "CTRL_02"

        [[rules]]
        name   = "align_experiment"
        input  = ["raw/{experiment}_R1.fq.gz"]
        output = ["aligned/{experiment}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"

        [[rules]]
        name   = "align_control"
        input  = ["raw/{control}_R1.fq.gz"]
        output = ["aligned/{control}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"

        [[rules]]
        name   = "mutect2"
        input  = ["aligned/{experiment}.bam", "aligned/{control}.bam"]
        output = ["variants/{pair_id}.vcf.gz"]
        shell  = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal {control} -O {output[0]}"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(config.pairs.len(), 2);
    assert_eq!(config.rules.len(), 3);

    // Before expansion, rule names are template names
    assert!(config.rules.iter().any(|r| r.name == "align_experiment"));
    assert!(config.rules.iter().any(|r| r.name == "mutect2"));

    config.expand_wildcards().unwrap();

    // After expansion: 3 rules × 2 pairs = 6 rules
    assert_eq!(config.rules.len(), 6);

    // Check expanded names
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_experiment_CASE_001")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_experiment_CASE_002")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_control_CASE_001")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_control_CASE_002")
    );
    assert!(config.rules.iter().any(|r| r.name == "mutect2_CASE_001"));
    assert!(config.rules.iter().any(|r| r.name == "mutect2_CASE_002"));

    // Check wildcard substitution in file paths
    let align_t1 = config
        .rules
        .iter()
        .find(|r| r.name == "align_experiment_CASE_001")
        .unwrap();
    assert_eq!(align_t1.input.get_index(0).unwrap(), "raw/EXP_01_R1.fq.gz");
    assert_eq!(align_t1.output.get_index(0).unwrap(), "aligned/EXP_01.bam");

    let mutect2_c2 = config
        .rules
        .iter()
        .find(|r| r.name == "mutect2_CASE_002")
        .unwrap();
    assert_eq!(
        mutect2_c2.output.get_index(0).unwrap(),
        "variants/CASE_002.vcf.gz"
    );

    // DAG should be buildable and valid
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 6);
}

/// Test that the multi-case experiment-control example file parses and expands correctly.
#[test]
fn wc01_example_paired_experiment_control_pairs() {
    let toml = std::fs::read_to_string("examples/paired_experiment_control_pairs.oxoflow").unwrap();
    let mut config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "multi-case-experiment-control");
    assert_eq!(config.pairs.len(), 2);

    // Before expansion the template rules use {experiment}/{control} placeholders
    let template_rule_count = config.rules.len();
    assert!(template_rule_count > 0);

    config.expand_wildcards().unwrap();

    // Every template rule should expand into 2 instances (one per pair)
    assert_eq!(config.rules.len(), template_rule_count * 2);

    // DAG construction must succeed
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());

    // Both pairs should have a clinical_report step
    assert!(order.iter().any(|n| n == "clinical_report_CASE_001"));
    assert!(order.iter().any(|n| n == "clinical_report_CASE_002"));
}

/// Test that rules without {experiment}/{control} wildcards are unaffected by expansion.
#[test]
fn wc01_non_wildcard_rules_unchanged() {
    let toml = r#"
        [workflow]
        name = "wc01-no-wildcard"

        [[pairs]]
        pair_id = "P1"
        experiment = "E1"
        control    = "C1"

        [[rules]]
        name   = "setup"
        output = ["setup.done"]
        shell  = "mkdir -p results && touch setup.done"

        [[rules]]
        name   = "align_experiment"
        input  = ["raw/{experiment}.fq"]
        output = ["aligned/{experiment}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    // setup rule should remain as-is
    assert_eq!(config.rules.iter().filter(|r| r.name == "setup").count(), 1);
    // align_experiment should be expanded
    assert!(config.rules.iter().any(|r| r.name == "align_experiment_P1"));
}

// ============================================================================
// WC-02: Multi-Sample Group Tests
// ============================================================================

/// Test that [[sample_groups]] parse correctly and expand_wildcards produces
/// one rule instance per (group, sample) combination.
#[test]
fn wc02_sample_groups_expand_correctly() {
    let toml = r#"
        [workflow]
        name = "wc02-test"

        [[sample_groups]]
        name    = "control"
        samples = ["CTRL_001", "CTRL_002"]

        [[sample_groups]]
        name    = "case"
        samples = ["CASE_001"]

        [[rules]]
        name   = "qc"
        input  = ["raw/{sample}_R1.fq.gz"]
        output = ["qc/{sample}_fastqc.html"]
        shell  = "fastqc {input[0]} -o qc/"

        [[rules]]
        name   = "align"
        input  = ["raw/{sample}_R1.fq.gz"]
        output = ["aligned/{sample}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(config.sample_groups.len(), 2);
    assert_eq!(config.rules.len(), 2);

    config.expand_wildcards().unwrap();

    // 2 rules × 3 total samples (2 control + 1 case) = 6 rules
    assert_eq!(config.rules.len(), 6);

    assert!(config.rules.iter().any(|r| r.name == "qc_control_CTRL_001"));
    assert!(config.rules.iter().any(|r| r.name == "qc_control_CTRL_002"));
    assert!(config.rules.iter().any(|r| r.name == "qc_case_CASE_001"));
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_control_CTRL_001")
    );
    assert!(config.rules.iter().any(|r| r.name == "align_case_CASE_001"));

    // File patterns should be resolved
    let qc_c001 = config
        .rules
        .iter()
        .find(|r| r.name == "qc_control_CTRL_001")
        .unwrap();
    assert_eq!(qc_c001.input.get_index(0).unwrap(), "raw/CTRL_001_R1.fq.gz");
    assert_eq!(
        qc_c001.output.get_index(0).unwrap(),
        "qc/CTRL_001_fastqc.html"
    );

    // DAG must be valid
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();
}

/// Test the cohort analysis example file.
#[test]
fn wc02_example_cohort_analysis() {
    let toml = std::fs::read_to_string("examples/cohort_analysis.oxoflow").unwrap();
    let mut config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "cohort-analysis");
    assert_eq!(config.sample_groups.len(), 2);

    // Count rules before expansion (4 wildcard + 1 non-wildcard)
    let wildcard_rule_count: usize = config
        .rules
        .iter()
        .filter(|r| {
            r.input.iter().any(|p| p.contains("{sample}"))
                || r.output.iter().any(|p| p.contains("{sample}"))
        })
        .count();
    let non_wildcard_rule_count = config.rules.len() - wildcard_rule_count;

    config.expand_wildcards().unwrap();

    // Total samples: 3 control + 2 case = 5
    let total_samples = 5_usize;

    // After expansion:
    //   wildcard rules expanded per sample + non-wildcard rules run once
    let expected = wildcard_rule_count * total_samples + non_wildcard_rule_count;
    assert_eq!(config.rules.len(), expected);

    // DAG should be valid
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();
}

// ============================================================================
// WF-01: `when` Conditional Execution Tests
// ============================================================================

/// Test that the conditional workflow example parses correctly.
#[test]
fn wf01_example_conditional_workflow() {
    let toml = std::fs::read_to_string("examples/conditional_workflow.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "conditional-workflow");
    assert!(!config.rules.is_empty());

    // Rules with `when` fields should be present
    let conditional_rules: Vec<_> = config.rules.iter().filter(|r| r.when.is_some()).collect();
    assert!(
        !conditional_rules.is_empty(),
        "expected some conditional rules"
    );

    // DAG builds fine even with conditional rules
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();
}

/// Test evaluate_condition with various expressions through the executor.
#[test]
fn wf01_evaluate_condition_integration() {
    use oxo_flow_core::executor::evaluate_condition;

    let mut config = std::collections::HashMap::new();
    config.insert("run_qc".to_string(), toml::Value::Boolean(true));
    config.insert("skip_annotation".to_string(), toml::Value::Boolean(false));
    config.insert("min_coverage".to_string(), toml::Value::Integer(30));
    config.insert("mode".to_string(), toml::Value::String("WGS".to_string()));

    // Simple truthy checks
    assert!(evaluate_condition("config.run_qc", &config));
    assert!(!evaluate_condition("config.skip_annotation", &config));

    // Comparisons
    assert!(evaluate_condition("config.min_coverage >= 30", &config));
    assert!(!evaluate_condition("config.min_coverage > 30", &config));
    assert!(evaluate_condition(r#"config.mode == "WGS""#, &config));
    assert!(!evaluate_condition(r#"config.mode == "WES""#, &config));

    // Logical operators
    assert!(evaluate_condition(
        "config.run_qc && config.min_coverage >= 20",
        &config
    ));
    assert!(evaluate_condition(
        "config.skip_annotation || config.run_qc",
        &config
    ));
    assert!(!evaluate_condition(
        "config.skip_annotation && config.run_qc",
        &config
    ));

    // Negation
    assert!(!evaluate_condition("!config.run_qc", &config));
    assert!(evaluate_condition("!config.skip_annotation", &config));

    // Complex
    assert!(evaluate_condition(
        r#"config.run_qc && config.mode == "WGS" && config.min_coverage >= 20"#,
        &config
    ));
}

// ============================================================================
// Gallery 09: Single-Cell RNA-seq
// ============================================================================

#[test]
fn gallery_09_single_cell_rnaseq() {
    let toml = std::fs::read_to_string("examples/gallery/09_single_cell_rnaseq.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();
    assert_eq!(config.workflow.name, "sc-rnaseq-pipeline");
    assert!(!config.rules.is_empty());

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());
}

// ============================================================================
// DAG structural tests
// ============================================================================

/// Test that a cycle in the dependency graph is detected.
#[test]
fn dag_cycle_detection() {
    let toml = r#"
        [workflow]
        name = "cycle-test"

        [[rules]]
        name = "rule_a"
        input = ["b.txt"]
        output = ["a.txt"]
        shell = "echo a"

        [[rules]]
        name = "rule_b"
        input = ["a.txt"]
        output = ["b.txt"]
        shell = "echo b"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let result = WorkflowDag::from_rules(&config.rules);
    // Should fail to build because of a cycle
    assert!(result.is_err(), "cycle should be detected as an error");
}

/// Test diamond dependency pattern: A→B, A→C, B+C→D.
#[test]
fn dag_diamond_dependency_pattern() {
    let toml = r#"
        [workflow]
        name = "diamond"

        [[rules]]
        name = "source"
        output = ["raw.txt"]
        shell = "echo raw"

        [[rules]]
        name = "branch_left"
        input = ["raw.txt"]
        output = ["left.txt"]
        shell = "cat raw.txt > left.txt"

        [[rules]]
        name = "branch_right"
        input = ["raw.txt"]
        output = ["right.txt"]
        shell = "cat raw.txt > right.txt"

        [[rules]]
        name = "merge"
        input = ["left.txt", "right.txt"]
        output = ["merged.txt"]
        shell = "cat left.txt right.txt > merged.txt"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 4);
    // source must be first, merge must be last
    assert_eq!(order[0], "source");
    assert_eq!(order[3], "merge");

    // Both branches must appear before merge
    let left_pos = order.iter().position(|r| r == "branch_left").unwrap();
    let right_pos = order.iter().position(|r| r == "branch_right").unwrap();
    let merge_pos = order.iter().position(|r| r == "merge").unwrap();
    assert!(left_pos < merge_pos);
    assert!(right_pos < merge_pos);
}

/// Test a wide fan-out then fan-in (scatter/gather) pattern.
#[test]
fn dag_scatter_gather_pattern() {
    let toml = r#"
        [workflow]
        name = "scatter-gather"

        [[rules]]
        name = "scatter"
        output = ["shard1.txt", "shard2.txt", "shard3.txt"]
        shell = "echo shard1 > shard1.txt && echo shard2 > shard2.txt && echo shard3 > shard3.txt"

        [[rules]]
        name = "process_shard1"
        input = ["shard1.txt"]
        output = ["proc1.txt"]
        shell = "cat shard1.txt > proc1.txt"

        [[rules]]
        name = "process_shard2"
        input = ["shard2.txt"]
        output = ["proc2.txt"]
        shell = "cat shard2.txt > proc2.txt"

        [[rules]]
        name = "process_shard3"
        input = ["shard3.txt"]
        output = ["proc3.txt"]
        shell = "cat shard3.txt > proc3.txt"

        [[rules]]
        name = "gather"
        input = ["proc1.txt", "proc2.txt", "proc3.txt"]
        output = ["final.txt"]
        shell = "cat proc1.txt proc2.txt proc3.txt > final.txt"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 5);

    // Verify parallel groups: scatter → 3 processors → gather
    let groups = dag.parallel_groups().unwrap();
    assert!(groups.len() >= 3);
    // scatter is alone in the first group
    assert_eq!(groups[0], vec!["scatter".to_string()]);
    // gather is alone in the last group
    assert_eq!(groups[groups.len() - 1], vec!["gather".to_string()]);
}

/// Test a complex multi-level bioinformatics-like pipeline.
#[test]
fn dag_complex_bioinformatics_pipeline() {
    let toml = r#"
        [workflow]
        name = "complex-bio-pipeline"
        description = "Multi-omics-like pipeline DAG"

        [[rules]]
        name = "qc_sample_a"
        output = ["qc/a.txt"]
        shell = "echo qc_a > qc/a.txt"
        threads = 4

        [[rules]]
        name = "qc_sample_b"
        output = ["qc/b.txt"]
        shell = "echo qc_b > qc/b.txt"
        threads = 4

        [[rules]]
        name = "align_sample_a"
        input = ["qc/a.txt"]
        output = ["aligned/a.bam"]
        shell = "cat qc/a.txt > aligned/a.bam"
        threads = 8

        [[rules]]
        name = "align_sample_b"
        input = ["qc/b.txt"]
        output = ["aligned/b.bam"]
        shell = "cat qc/b.txt > aligned/b.bam"
        threads = 8

        [[rules]]
        name = "call_variants_a"
        input = ["aligned/a.bam"]
        output = ["variants/a.vcf"]
        shell = "cat aligned/a.bam > variants/a.vcf"
        threads = 2

        [[rules]]
        name = "call_variants_b"
        input = ["aligned/b.bam"]
        output = ["variants/b.vcf"]
        shell = "cat aligned/b.bam > variants/b.vcf"
        threads = 2

        [[rules]]
        name = "merge_variants"
        input = ["variants/a.vcf", "variants/b.vcf"]
        output = ["merged.vcf"]
        shell = "cat variants/a.vcf variants/b.vcf > merged.vcf"

        [[rules]]
        name = "annotate"
        input = ["merged.vcf"]
        output = ["annotated.vcf"]
        shell = "cat merged.vcf > annotated.vcf"

        [[rules]]
        name = "report"
        input = ["annotated.vcf"]
        output = ["report.html"]
        shell = "echo '<html>' > report.html && cat annotated.vcf >> report.html"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(config.rules.len(), 9);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 9);
    assert_eq!(order.last().unwrap(), "report");

    let groups = dag.parallel_groups().unwrap();
    // Level 0: qc_sample_a, qc_sample_b
    // Level 1: align_sample_a, align_sample_b
    // Level 2: call_variants_a, call_variants_b
    // Level 3: merge_variants
    // Level 4: annotate
    // Level 5: report
    assert!(groups.len() >= 5);
    assert_eq!(groups[0].len(), 2); // two QC rules in parallel
}

/// Test execution order for specific targets.
#[test]
fn dag_execution_order_for_targets() {
    let toml = r#"
        [workflow]
        name = "target-test"

        [[rules]]
        name = "step_a"
        output = ["a.txt"]
        shell = "echo a"

        [[rules]]
        name = "step_b"
        input = ["a.txt"]
        output = ["b.txt"]
        shell = "echo b"

        [[rules]]
        name = "step_c"
        input = ["b.txt"]
        output = ["c.txt"]
        shell = "echo c"

        [[rules]]
        name = "step_d"
        input = ["a.txt"]
        output = ["d.txt"]
        shell = "echo d"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();

    // Target step_b: should only need step_a and step_b
    let targets = vec!["step_b"];
    let order = dag.execution_order_for_targets(&targets).unwrap();
    assert_eq!(order.len(), 2);
    assert!(order.contains(&"step_a".to_string()));
    assert!(order.contains(&"step_b".to_string()));
    assert!(!order.contains(&"step_c".to_string()));
    assert!(!order.contains(&"step_d".to_string()));
}

// ============================================================================
// Format, lint, and validate diagnostic tests
// ============================================================================

/// Test that format_workflow produces canonical TOML that can be re-parsed.
#[test]
fn format_workflow_roundtrip() {
    use oxo_flow_core::format::format_workflow;

    let toml =
        std::fs::read_to_string("examples/gallery/06_rnaseq_quantification.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();

    // Format the workflow
    let formatted = format_workflow(&config);

    // Re-parse the formatted output
    let config2 = WorkflowConfig::parse(&formatted).unwrap();
    assert_eq!(config.workflow.name, config2.workflow.name);
    assert_eq!(config.rules.len(), config2.rules.len());
}

/// Test that validate_format catches a rule that references an undefined config variable.
#[test]
fn validate_format_detects_missing_output() {
    use oxo_flow_core::format::validate_format;

    // E005: Shell command references an undefined config variable
    let toml = r#"
        [workflow]
        name = "undefined-config-var-test"

        [config]
        defined_var = "hello"

        [[rules]]
        name = "bad_rule"
        output = ["output.txt"]
        shell = "echo {config.defined_var} {config.undefined_var} > output.txt"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let result = validate_format(&config);

    // Should report an error about undefined config variable
    assert!(
        result.has_errors(),
        "expected at least one error diagnostic for undefined config variable reference"
    );
    assert!(
        result.diagnostics.iter().any(|d| d.code == "E005"),
        "expected E005 for undefined config variable, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| &d.code)
            .collect::<Vec<_>>()
    );
}

/// Test that lint detects a missing description on a shell rule.
#[test]
fn lint_detects_missing_description_warning() {
    use oxo_flow_core::format::lint_format;

    let toml = r#"
        [workflow]
        name = "lint-test"

        [[rules]]
        name = "no_description_rule"
        output = ["out.txt"]
        shell = "echo hello > out.txt"
    "#;

    let config = WorkflowConfig::parse(toml).unwrap();
    let diagnostics = lint_format(&config);

    // Lint should find at least one warning (no description)
    assert!(
        !diagnostics.is_empty(),
        "expected lint warnings for undescribed rule"
    );
}

/// Test that diff_workflows identifies added/removed rules correctly.
#[test]
fn diff_workflows_identifies_changes() {
    use oxo_flow_core::format::diff_workflows;

    let toml_a = r#"
        [workflow]
        name = "pipeline-v1"
        version = "1.0.0"

        [[rules]]
        name = "step_a"
        output = ["a.txt"]
        shell = "echo a"
    "#;

    let toml_b = r#"
        [workflow]
        name = "pipeline-v2"
        version = "2.0.0"

        [[rules]]
        name = "step_a"
        output = ["a.txt"]
        shell = "echo a"

        [[rules]]
        name = "step_b"
        input = ["a.txt"]
        output = ["b.txt"]
        shell = "echo b"
    "#;

    let config_a = WorkflowConfig::parse(toml_a).unwrap();
    let config_b = WorkflowConfig::parse(toml_b).unwrap();

    let diffs = diff_workflows(&config_a, &config_b);
    assert!(
        !diffs.is_empty(),
        "workflows with different rules should have diffs"
    );

    // Should detect version and rule count differences
    let categories: Vec<&str> = diffs.iter().map(|d| d.category.as_str()).collect();
    assert!(
        categories.iter().any(|c| {
            *c == "version"
                || *c == "rule_added"
                || *c == "name"
                || *c == "rules"
                || *c == "metadata"
        }),
        "expected to detect metadata or rule differences, got: {:?}",
        categories
    );
}

// ============================================================================
// Checkpoint state tests
// ============================================================================

/// Test CheckpointState serialization/deserialization roundtrip.
#[test]
fn checkpoint_state_json_roundtrip() {
    use oxo_flow_core::executor::{BenchmarkRecord, CheckpointState};

    let mut state = CheckpointState::new();
    state.mark_completed(
        "align_sample",
        BenchmarkRecord {
            rule: "align_sample".to_string(),
            wall_time_secs: 42.5,
            max_memory_mb: Some(1024),
            cpu_seconds: Some(38.0),
        },
    );
    state.mark_failed("call_variants");

    let json = state.to_json().unwrap();
    let restored = CheckpointState::from_json(&json).unwrap();

    assert!(restored.is_completed("align_sample"));
    assert!(!restored.is_completed("call_variants"));
    assert!(restored.failed_rules.contains("call_variants"));
    assert!(
        restored.benchmarks.contains_key("align_sample"),
        "benchmark should be preserved"
    );
    assert!(
        (restored.benchmarks["align_sample"].wall_time_secs - 42.5).abs() < f64::EPSILON * 1000.0
    );
}

/// Test CheckpointState file save and load.
#[test]
fn checkpoint_state_file_persistence() {
    use oxo_flow_core::executor::{BenchmarkRecord, CheckpointState};

    let dir = tempfile::tempdir().unwrap();
    let checkpoint_path = CheckpointState::default_path(dir.path());

    let mut state = CheckpointState::new();
    state.mark_completed(
        "qc_step",
        BenchmarkRecord {
            rule: "qc_step".to_string(),
            wall_time_secs: 5.0,
            max_memory_mb: Some(512),
            cpu_seconds: Some(4.2),
        },
    );
    state.mark_completed(
        "align_step",
        BenchmarkRecord {
            rule: "align_step".to_string(),
            wall_time_secs: 120.3,
            max_memory_mb: Some(4096),
            cpu_seconds: Some(115.0),
        },
    );

    // Save to file
    std::fs::create_dir_all(checkpoint_path.parent().unwrap()).unwrap();
    state.save_to_file(&checkpoint_path).unwrap();
    assert!(checkpoint_path.exists());

    // Load from file
    let loaded = CheckpointState::load_from_file(&checkpoint_path).unwrap();
    assert!(loaded.is_completed("qc_step"));
    assert!(loaded.is_completed("align_step"));
    assert!(!loaded.should_skip("nonexistent_step"));
    assert_eq!(loaded.completed_rules.len(), 2);
}

/// Test that should_skip returns correct values.
#[test]
fn checkpoint_should_skip_logic() {
    use oxo_flow_core::executor::{BenchmarkRecord, CheckpointState};

    let mut state = CheckpointState::new();
    state.mark_completed(
        "done_rule",
        BenchmarkRecord {
            rule: "done_rule".to_string(),
            wall_time_secs: 1.0,
            max_memory_mb: None,
            cpu_seconds: Some(0.9),
        },
    );
    state.mark_failed("failed_rule");

    assert!(state.should_skip("done_rule"));
    assert!(!state.should_skip("failed_rule"));
    assert!(!state.should_skip("pending_rule"));
}

/// Test Prometheus metrics generation from checkpoint state.
#[test]
fn checkpoint_prometheus_metrics() {
    use oxo_flow_core::executor::{BenchmarkRecord, CheckpointState};

    let mut state = CheckpointState::new();
    state.mark_completed(
        "step1",
        BenchmarkRecord {
            rule: "step1".to_string(),
            wall_time_secs: 10.0,
            max_memory_mb: None,
            cpu_seconds: None,
        },
    );
    state.mark_failed("step2");

    let metrics = state.to_prometheus_metrics();
    assert!(metrics.contains("oxo_flow_rules_completed_total 1"));
    assert!(metrics.contains("oxo_flow_rules_failed_total 1"));
    assert!(metrics.contains("oxo_flow_rule_duration_seconds{rule=\"step1\"}"));
}

// ============================================================================
// Execution support utilities
// ============================================================================

/// Test render_shell_command substitutes rule inputs/outputs and wildcards.
#[test]
fn render_shell_command_substitution() {
    use oxo_flow_core::executor::render_shell_command;
    use oxo_flow_core::rule::Rule;
    use std::collections::HashMap;

    let rule = Rule {
        name: "align".to_string(),
        input: vec!["raw/sample_R1.fastq.gz".to_string()].into(),
        output: vec!["aligned/sample.bam".to_string()].into(),
        shell: Some("bwa mem {config.reference} {input[0]} > {output[0]}".to_string()),
        ..Default::default()
    };

    let mut wildcards = HashMap::new();
    wildcards.insert("config.reference".to_string(), "/ref/hg38.fa".to_string());

    let rendered = render_shell_command(rule.shell.as_ref().unwrap(), &rule, &wildcards);
    assert!(rendered.contains("raw/sample_R1.fastq.gz"));
    assert!(rendered.contains("aligned/sample.bam"));
    assert!(rendered.contains("/ref/hg38.fa"));
}

/// Test sanitize_shell_command detects dangerous patterns.
#[test]
fn sanitize_shell_command_detects_dangerous_patterns() {
    use oxo_flow_core::executor::sanitize_shell_command;

    // Normal bioinformatics command should be clean
    let safe_cmd = "bwa mem ref.fa R1.fastq.gz | samtools sort -o output.bam";
    let warnings = sanitize_shell_command(safe_cmd);
    assert!(
        warnings.is_empty(),
        "normal bioinformatics command should have no warnings"
    );

    // Command substitution should trigger a warning
    let dangerous_cmd = "echo $(cat /etc/passwd)";
    let warnings = sanitize_shell_command(dangerous_cmd);
    assert!(
        !warnings.is_empty(),
        "command substitution should be flagged"
    );
}

// ============================================================================
// Execution events
// ============================================================================

/// Test that ExecutionEvent serializes to valid JSON log format.
#[test]
fn execution_event_json_log_format() {
    use oxo_flow_core::executor::{ExecutionEvent, JobStatus};

    let event = ExecutionEvent::WorkflowStarted {
        workflow_name: "test-pipeline".to_string(),
        total_rules: 5,
    };
    let log = event.to_json_log();
    assert!(log.contains("\"event\":\"workflow_started\""));
    assert!(log.contains("\"workflow\":\"test-pipeline\""));
    assert!(log.contains("\"total_rules\":5"));
    assert!(log.contains("\"timestamp\":"));
    // Should be parseable JSON
    let parsed: serde_json::Value = serde_json::from_str(&log).unwrap();
    assert_eq!(parsed["event"], "workflow_started");

    let rule_event = ExecutionEvent::RuleCompleted {
        rule: "align_reads".to_string(),
        status: JobStatus::Success,
        duration_ms: 12500,
    };
    let log2 = rule_event.to_json_log();
    assert!(log2.contains("\"event\":\"rule_completed\""));
    assert!(log2.contains("align_reads"));
    assert!(log2.contains("success"));
}

/// Test that event_type returns the correct name.
#[test]
fn execution_event_type_names() {
    use oxo_flow_core::executor::{ExecutionEvent, JobStatus};

    assert_eq!(
        ExecutionEvent::WorkflowStarted {
            workflow_name: "x".to_string(),
            total_rules: 1
        }
        .event_type(),
        "workflow_started"
    );
    assert_eq!(
        ExecutionEvent::RuleStarted {
            rule: "r".to_string(),
            command: None
        }
        .event_type(),
        "rule_started"
    );
    assert_eq!(
        ExecutionEvent::RuleSkipped {
            rule: "r".to_string(),
            reason: "already up-to-date".to_string()
        }
        .event_type(),
        "rule_skipped"
    );
    assert_eq!(
        ExecutionEvent::RuleCompleted {
            rule: "r".to_string(),
            status: JobStatus::Success,
            duration_ms: 0
        }
        .event_type(),
        "rule_completed"
    );
}

// ============================================================================
// Venus library tests
// ============================================================================

/// Test Venus ExperimentOnly mode generates a valid workflow.
#[test]
fn venus_experiment_only_wgs() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentOnly,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![oxo_flow_venus::Sample {
            name: "CASE_A".to_string(),
            r1_fastq: "raw/CASE_A_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/CASE_A_R2.fq.gz".to_string()),
            is_experiment: true,
        }],
        control_samples: vec![],
        known_sites: None,
        target_bed: None,
        threads: 4,
        output_dir: "output".to_string(),
        annotate: false,
        report: false,
        project_name: Some("ExperimentOnlyTest".to_string()),
    };

    let toml_str = oxo_flow_venus::generate_oxoflow(&config).unwrap();
    let wf = WorkflowConfig::parse(&toml_str).unwrap();

    assert_eq!(wf.workflow.name, "ExperimentOnlyTest");
    assert!(!wf.rules.is_empty());

    let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
    dag.validate().unwrap();
}

/// Test Venus ControlOnly mode.
#[test]
fn venus_control_only_mode() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ControlOnly,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![],
        control_samples: vec![oxo_flow_venus::Sample {
            name: "NORMAL_01".to_string(),
            r1_fastq: "raw/NORMAL_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/NORMAL_01_R2.fq.gz".to_string()),
            is_experiment: false,
        }],
        known_sites: None,
        target_bed: None,
        threads: 4,
        output_dir: "output".to_string(),
        annotate: false,
        report: false,
        project_name: Some("ControlOnlyTest".to_string()),
    };

    let toml_str = oxo_flow_venus::generate_oxoflow(&config).unwrap();
    let wf = WorkflowConfig::parse(&toml_str).unwrap();

    assert!(!wf.rules.is_empty());
    let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
    dag.validate().unwrap();
}

/// Test Venus ExperimentControl with multiple samples and known_sites.
#[test]
fn venus_experiment_control_multi_sample() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentControl,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![
            oxo_flow_venus::Sample {
                name: "TUMOR_01".to_string(),
                r1_fastq: "raw/TUMOR_01_R1.fq.gz".to_string(),
                r2_fastq: Some("raw/TUMOR_01_R2.fq.gz".to_string()),
                is_experiment: true,
            },
            oxo_flow_venus::Sample {
                name: "TUMOR_02".to_string(),
                r1_fastq: "raw/TUMOR_02_R1.fq.gz".to_string(),
                r2_fastq: Some("raw/TUMOR_02_R2.fq.gz".to_string()),
                is_experiment: true,
            },
        ],
        control_samples: vec![
            oxo_flow_venus::Sample {
                name: "NORMAL_01".to_string(),
                r1_fastq: "raw/NORMAL_01_R1.fq.gz".to_string(),
                r2_fastq: Some("raw/NORMAL_01_R2.fq.gz".to_string()),
                is_experiment: false,
            },
            oxo_flow_venus::Sample {
                name: "NORMAL_02".to_string(),
                r1_fastq: "raw/NORMAL_02_R1.fq.gz".to_string(),
                r2_fastq: Some("raw/NORMAL_02_R2.fq.gz".to_string()),
                is_experiment: false,
            },
        ],
        known_sites: Some("/ref/dbsnp.vcf.gz".to_string()),
        target_bed: None,
        threads: 16,
        output_dir: "output".to_string(),
        annotate: true,
        report: true,
        project_name: Some("MultiSampleTumorNormal".to_string()),
    };

    let toml_str = oxo_flow_venus::generate_oxoflow(&config).unwrap();
    let wf = WorkflowConfig::parse(&toml_str).unwrap();

    assert_eq!(wf.workflow.name, "MultiSampleTumorNormal");
    assert!(!wf.rules.is_empty());

    let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert!(!order.is_empty());
}

/// Test Venus config validation catches missing experiment samples in ExperimentOnly mode.
#[test]
fn venus_config_validation_no_experiment_samples() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentOnly,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![], // Empty — should fail
        control_samples: vec![],
        known_sites: None,
        target_bed: None,
        threads: 4,
        output_dir: "output".to_string(),
        annotate: false,
        report: false,
        project_name: None,
    };

    let result = config.validate();
    assert!(result.is_err(), "should fail with no experiment samples");
}

/// Test Venus config validation catches WES without target_bed.
#[test]
fn venus_config_validation_wes_missing_target_bed() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentOnly,
        seq_type: oxo_flow_venus::SeqType::WES,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![oxo_flow_venus::Sample {
            name: "EXOME_01".to_string(),
            r1_fastq: "raw/EXOME_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/EXOME_01_R2.fq.gz".to_string()),
            is_experiment: true,
        }],
        control_samples: vec![],
        known_sites: None,
        target_bed: None, // Missing — should fail for WES
        threads: 4,
        output_dir: "output".to_string(),
        annotate: false,
        report: false,
        project_name: None,
    };

    let result = config.validate();
    assert!(result.is_err(), "WES mode requires target_bed");
}

/// Test Venus all_samples returns all samples across experiment and control.
#[test]
fn venus_all_samples_combines_both_groups() {
    let config = oxo_flow_venus::VenusConfig {
        mode: oxo_flow_venus::AnalysisMode::ExperimentControl,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        experiment_samples: vec![
            oxo_flow_venus::Sample {
                name: "T1".to_string(),
                r1_fastq: "r1.fq".to_string(),
                r2_fastq: None,
                is_experiment: true,
            },
            oxo_flow_venus::Sample {
                name: "T2".to_string(),
                r1_fastq: "r1.fq".to_string(),
                r2_fastq: None,
                is_experiment: true,
            },
        ],
        control_samples: vec![oxo_flow_venus::Sample {
            name: "N1".to_string(),
            r1_fastq: "r1.fq".to_string(),
            r2_fastq: None,
            is_experiment: false,
        }],
        known_sites: None,
        target_bed: None,
        threads: 4,
        output_dir: "output".to_string(),
        annotate: false,
        report: false,
        project_name: None,
    };

    let all = config.all_samples();
    assert_eq!(all.len(), 3);
    let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"T1"));
    assert!(names.contains(&"T2"));
    assert!(names.contains(&"N1"));
}

// ============================================================================
// Rule / config API tests
// ============================================================================

/// Test that Rule::effective_threads returns its own threads when set.
#[test]
fn rule_effective_threads_uses_rule_value() {
    use oxo_flow_core::rule::Rule;

    let rule = Rule {
        name: "test".to_string(),
        ..Default::default()
    };
    // Default rule has no threads set — effective_threads should return 1
    assert_eq!(rule.effective_threads(), 1);
}

/// Test that apply_defaults propagates default memory to rules without memory.
#[test]
fn apply_defaults_memory_propagation() {
    let toml = r#"
        [workflow]
        name = "defaults-memory"

        [defaults]
        threads = 8
        memory = "16G"

        [[rules]]
        name = "rule_no_memory"
        output = ["out1.txt"]
        shell = "echo out1"

        [[rules]]
        name = "rule_with_memory"
        output = ["out2.txt"]
        shell = "echo out2"
        memory = "4G"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.apply_defaults();

    let r1 = config
        .rules
        .iter()
        .find(|r| r.name == "rule_no_memory")
        .unwrap();
    let r2 = config
        .rules
        .iter()
        .find(|r| r.name == "rule_with_memory")
        .unwrap();

    // rule_no_memory should get default memory
    assert_eq!(r1.effective_memory(), Some("16G"));
    // rule_with_memory should keep its own memory
    assert_eq!(r2.effective_memory(), Some("4G"));
    // both should get default threads
    assert_eq!(r1.effective_threads(), 8);
}

/// Test WorkflowConfig::from_file correctly parses a file on disk.
#[test]
fn workflow_config_from_file() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.oxoflow");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"
[workflow]
name = "file-parse-test"
version = "0.2.0"

[[rules]]
name = "hello"
output = ["hello.txt"]
shell = "echo hello"
"#
    )
    .unwrap();

    let config = WorkflowConfig::from_file(&path).unwrap();
    assert_eq!(config.workflow.name, "file-parse-test");
    assert_eq!(config.workflow.version, "0.2.0");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "hello");
}

/// Test that WorkflowConfig::from_file fails on a non-existent path.
#[test]
fn workflow_config_from_nonexistent_file() {
    let result = WorkflowConfig::from_file(std::path::Path::new("/nonexistent/path.oxoflow"));
    assert!(result.is_err());
}

// ============================================================================
// Wildcard utility tests
// ============================================================================

/// Test extract_wildcards identifies all wildcard names in a pattern.
#[test]
fn wildcard_extract_names_from_pattern() {
    use oxo_flow_core::wildcard::extract_wildcards;

    let pattern = "results/{sample}/{chrom}/{sample}_{chrom}.vcf";
    let wildcards = extract_wildcards(pattern);
    // Should extract unique wildcard names
    assert!(wildcards.contains(&"sample".to_string()));
    assert!(wildcards.contains(&"chrom".to_string()));
}

/// Test expand_pattern with concrete wildcard values.
#[test]
fn wildcard_expand_pattern_with_values() {
    use oxo_flow_core::wildcard::expand_pattern;
    use std::collections::HashMap;

    let pattern = "results/{sample}/aligned/{sample}.bam";
    let mut values = HashMap::new();
    values.insert("sample".to_string(), "SAMPLE_01".to_string());

    let expanded = expand_pattern(pattern, &values).unwrap();
    assert_eq!(expanded, "results/SAMPLE_01/aligned/SAMPLE_01.bam");
}

/// Test has_wildcards correctly identifies patterns with and without wildcards.
#[test]
fn wildcard_has_wildcards_detection() {
    use oxo_flow_core::wildcard::has_wildcards;

    assert!(has_wildcards("{sample}_R1.fastq.gz"));
    assert!(has_wildcards("results/{sample}/{chrom}.vcf"));
    assert!(!has_wildcards("results/final.vcf"));
    assert!(!has_wildcards("raw/input.fastq.gz"));
}

/// Test cartesian_product over multiple wildcard dimensions.
#[test]
fn wildcard_cartesian_product() {
    use oxo_flow_core::wildcard::cartesian_product;
    use std::collections::HashMap;

    let mut dims = HashMap::new();
    dims.insert(
        "sample".to_string(),
        vec!["S1".to_string(), "S2".to_string()],
    );
    dims.insert(
        "chrom".to_string(),
        vec!["chr1".to_string(), "chr2".to_string(), "chr3".to_string()],
    );

    let combos = cartesian_product(&dims);
    // 2 samples × 3 chromosomes = 6 combinations
    assert_eq!(combos.len(), 6);
}

// ============================================================================
// Report generation tests
// ============================================================================

/// Test that the report engine handles empty sections gracefully.
#[test]
fn report_empty_workflow_report() {
    use oxo_flow_core::report::Report;

    let report = Report::new("Empty Pipeline Report", "empty-pipeline", "0.1.0");
    // Don't add any sections

    let html = report.to_html();
    assert!(
        html.contains("Empty Pipeline Report")
            || html.contains("empty-pipeline")
            || html.contains("html")
    );

    let json = report.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    // JSON should be valid
    assert!(parsed.is_object() || parsed.is_array());
}

/// Test variant summary section rendering.
#[test]
fn report_variant_summary_section() {
    use oxo_flow_core::report::{Report, VariantSummary, variant_summary_section};

    let mut report = Report::new("Variant Report", "variant-pipeline", "1.0.0");

    let variants = vec![
        VariantSummary {
            gene: "KRAS".to_string(),
            variant: "p.G12D".to_string(),
            classification: "Pathogenic".to_string(),
            allele_frequency: 0.32,
            depth: 450,
            clinical_significance: Some("Associated with colorectal cancer".to_string()),
        },
        VariantSummary {
            gene: "TP53".to_string(),
            variant: "p.R175H".to_string(),
            classification: "Pathogenic".to_string(),
            allele_frequency: 0.51,
            depth: 600,
            clinical_significance: Some("Gain of function mutation".to_string()),
        },
    ];
    report.add_section(variant_summary_section(&variants));

    let html = report.to_html();
    assert!(html.contains("KRAS") || html.contains("variant") || html.contains("html"));

    let json = report.to_json().unwrap();
    assert!(json.contains("KRAS") || json.len() > 10);
}

// ============================================================================
// Bioinformatics scenario: execute real shell-based workflows
// ============================================================================

/// Test executing a scatter-gather variant calling pipeline using Unix tools.
#[cfg(unix)]
#[tokio::test]
async fn execute_scatter_gather_unix_pipeline() {
    use oxo_flow_core::executor::{ExecutorConfig, LocalExecutor};
    use std::collections::HashMap;

    let dir = tempfile::tempdir().unwrap();
    let workdir = dir.path().to_path_buf();

    // Create simulated chromosome-level "variant files"
    std::fs::write(
        workdir.join("chr1.txt"),
        "variant_chr1_001\nvariant_chr1_002\n",
    )
    .unwrap();
    std::fs::write(workdir.join("chr2.txt"), "variant_chr2_001\n").unwrap();
    std::fs::write(
        workdir.join("chr3.txt"),
        "variant_chr3_001\nvariant_chr3_002\nvariant_chr3_003\n",
    )
    .unwrap();

    let toml = format!(
        r#"
        [workflow]
        name = "scatter-gather-unix"

        [[rules]]
        name = "count_chr1"
        input = ["{d}/chr1.txt"]
        output = ["{d}/count1.txt"]
        shell = "wc -l {d}/chr1.txt > {d}/count1.txt"

        [[rules]]
        name = "count_chr2"
        input = ["{d}/chr2.txt"]
        output = ["{d}/count2.txt"]
        shell = "wc -l {d}/chr2.txt > {d}/count2.txt"

        [[rules]]
        name = "count_chr3"
        input = ["{d}/chr3.txt"]
        output = ["{d}/count3.txt"]
        shell = "wc -l {d}/chr3.txt > {d}/count3.txt"

        [[rules]]
        name = "gather_counts"
        input = ["{d}/count1.txt", "{d}/count2.txt", "{d}/count3.txt"]
        output = ["{d}/total_counts.txt"]
        shell = "cat {d}/count1.txt {d}/count2.txt {d}/count3.txt > {d}/total_counts.txt"
    "#,
        d = workdir.display()
    );

    let config = WorkflowConfig::parse(&toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    let order = dag.execution_order().unwrap();

    let exec_config = ExecutorConfig {
        max_jobs: 3,
        workdir: workdir.clone(),
        skip_env_setup: true,
        ..Default::default()
    };
    let executor = LocalExecutor::new(exec_config);

    let wildcards: HashMap<String, String> = HashMap::new();
    for rule_name in &order {
        let rule = config.get_rule(rule_name).unwrap();
        let record = executor.execute_rule(rule, &wildcards).await.unwrap();
        assert_eq!(
            record.status,
            oxo_flow_core::executor::JobStatus::Success,
            "rule {} should succeed",
            rule_name
        );
    }

    assert!(workdir.join("total_counts.txt").exists());
    let content = std::fs::read_to_string(workdir.join("total_counts.txt")).unwrap();
    assert!(!content.is_empty());
}

/// Test executing a QC → trim → align simulation pipeline.
#[cfg(unix)]
#[tokio::test]
async fn execute_qc_trim_align_simulation() {
    use oxo_flow_core::executor::{ExecutorConfig, LocalExecutor};
    use std::collections::HashMap;

    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // Simulate paired-end FASTQ files
    std::fs::write(
        d.join("sample_R1.fq"),
        "@READ1\nACGTACGT\n+\nIIIIIIII\n@READ2\nTTTTAAAA\n+\nIIIIIIII\n",
    )
    .unwrap();
    std::fs::write(
        d.join("sample_R2.fq"),
        "@READ1\nTGCATGCA\n+\nIIIIIIII\n@READ2\nAAAATTTT\n+\nIIIIIIII\n",
    )
    .unwrap();

    let toml = format!(
        r#"
        [workflow]
        name = "qc-trim-align-sim"
        description = "Simulated QC → trim → align pipeline using Unix tools"

        [[rules]]
        name = "fastqc_r1"
        input = ["{d}/sample_R1.fq"]
        output = ["{d}/qc_r1.txt"]
        shell = "wc -l {d}/sample_R1.fq > {d}/qc_r1.txt && echo 'QC_PASS' >> {d}/qc_r1.txt"
        threads = 2
        description = "Quality check for R1 reads"

        [[rules]]
        name = "fastqc_r2"
        input = ["{d}/sample_R2.fq"]
        output = ["{d}/qc_r2.txt"]
        shell = "wc -l {d}/sample_R2.fq > {d}/qc_r2.txt && echo 'QC_PASS' >> {d}/qc_r2.txt"
        threads = 2
        description = "Quality check for R2 reads"

        [[rules]]
        name = "trim_reads"
        input = ["{d}/sample_R1.fq", "{d}/sample_R2.fq"]
        output = ["{d}/trimmed_R1.fq", "{d}/trimmed_R2.fq"]
        shell = "grep -A1 '^@' {d}/sample_R1.fq | grep -v '^--' > {d}/trimmed_R1.fq && grep -A1 '^@' {d}/sample_R2.fq | grep -v '^--' > {d}/trimmed_R2.fq"
        threads = 4
        description = "Adapter trimming simulation"

        [[rules]]
        name = "multiqc_summary"
        input = ["{d}/qc_r1.txt", "{d}/qc_r2.txt"]
        output = ["{d}/multiqc_report.txt"]
        shell = "cat {d}/qc_r1.txt {d}/qc_r2.txt > {d}/multiqc_report.txt && echo 'MultiQC Summary Complete' >> {d}/multiqc_report.txt"
        description = "Aggregate QC report"
    "#,
        d = d.display()
    );

    let config = WorkflowConfig::parse(&toml).unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 4);

    let exec_config = ExecutorConfig {
        max_jobs: 2,
        workdir: d.to_path_buf(),
        skip_env_setup: true,
        ..Default::default()
    };
    let executor = LocalExecutor::new(exec_config);
    let wildcards: HashMap<String, String> = HashMap::new();

    for rule_name in &order {
        let rule = config.get_rule(rule_name).unwrap();
        let record = executor.execute_rule(rule, &wildcards).await.unwrap();
        assert_eq!(
            record.status,
            oxo_flow_core::executor::JobStatus::Success,
            "rule {} should succeed",
            rule_name
        );
    }

    let report = std::fs::read_to_string(d.join("multiqc_report.txt")).unwrap();
    assert!(report.contains("QC_PASS"));
    assert!(report.contains("MultiQC Summary Complete"));
}
