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
fn full_lifecycle_paired_tumor_normal() {
    let toml = std::fs::read_to_string("examples/paired_tumor_normal.oxoflow").unwrap();
    let config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "paired-tumor-normal");
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
        mode: oxo_flow_venus::AnalysisMode::TumorNormal,
        seq_type: oxo_flow_venus::SeqType::WGS,
        genome_build: oxo_flow_venus::GenomeBuild::GRCh38,
        reference_fasta: "/ref/hg38.fa".to_string(),
        tumor_samples: vec![oxo_flow_venus::Sample {
            name: "TUMOR_01".to_string(),
            r1_fastq: "raw/TUMOR_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/TUMOR_01_R2.fq.gz".to_string()),
            is_tumor: true,
        }],
        normal_samples: vec![oxo_flow_venus::Sample {
            name: "NORMAL_01".to_string(),
            r1_fastq: "raw/NORMAL_01_R1.fq.gz".to_string(),
            r2_fastq: Some("raw/NORMAL_01_R2.fq.gz".to_string()),
            is_tumor: false,
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
    assert_eq!(config.workflow.name, "scatter-gather");
    assert_eq!(config.rules.len(), 4);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], "prepare_input");
    assert_eq!(order[3], "gather");
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
    assert_eq!(config.rules.len(), 8);

    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 8);
    assert_eq!(order[0], "fastp_qc");
    assert_eq!(order.last().unwrap(), "annotate_variants");
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
// WC-01: Tumor-Normal Pairing Tests
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
        tumor   = "TUMOR_01"
        normal  = "NORMAL_01"

        [[pairs]]
        pair_id = "CASE_002"
        tumor   = "TUMOR_02"
        normal  = "NORMAL_02"

        [[rules]]
        name   = "align_tumor"
        input  = ["raw/{tumor}_R1.fq.gz"]
        output = ["aligned/{tumor}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"

        [[rules]]
        name   = "align_normal"
        input  = ["raw/{normal}_R1.fq.gz"]
        output = ["aligned/{normal}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"

        [[rules]]
        name   = "mutect2"
        input  = ["aligned/{tumor}.bam", "aligned/{normal}.bam"]
        output = ["variants/{pair_id}.vcf.gz"]
        shell  = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal {normal} -O {output[0]}"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(config.pairs.len(), 2);
    assert_eq!(config.rules.len(), 3);

    // Before expansion, rule names are template names
    assert!(config.rules.iter().any(|r| r.name == "align_tumor"));
    assert!(config.rules.iter().any(|r| r.name == "mutect2"));

    config.expand_wildcards().unwrap();

    // After expansion: 3 rules × 2 pairs = 6 rules
    assert_eq!(config.rules.len(), 6);

    // Check expanded names
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_tumor_CASE_001")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_tumor_CASE_002")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_normal_CASE_001")
    );
    assert!(
        config
            .rules
            .iter()
            .any(|r| r.name == "align_normal_CASE_002")
    );
    assert!(config.rules.iter().any(|r| r.name == "mutect2_CASE_001"));
    assert!(config.rules.iter().any(|r| r.name == "mutect2_CASE_002"));

    // Check wildcard substitution in file paths
    let align_t1 = config
        .rules
        .iter()
        .find(|r| r.name == "align_tumor_CASE_001")
        .unwrap();
    assert_eq!(align_t1.input[0], "raw/TUMOR_01_R1.fq.gz");
    assert_eq!(align_t1.output[0], "aligned/TUMOR_01.bam");

    let mutect2_c2 = config
        .rules
        .iter()
        .find(|r| r.name == "mutect2_CASE_002")
        .unwrap();
    assert_eq!(mutect2_c2.output[0], "variants/CASE_002.vcf.gz");

    // DAG should be buildable and valid
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.validate().unwrap();

    let order = dag.execution_order().unwrap();
    assert_eq!(order.len(), 6);
}

/// Test that the multi-case tumor-normal example file parses and expands correctly.
#[test]
fn wc01_example_paired_tumor_normal_pairs() {
    let toml = std::fs::read_to_string("examples/paired_tumor_normal_pairs.oxoflow").unwrap();
    let mut config = WorkflowConfig::parse(&toml).unwrap();

    assert_eq!(config.workflow.name, "multi-case-tumor-normal");
    assert_eq!(config.pairs.len(), 2);

    // Before expansion the template rules use {tumor}/{normal} placeholders
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

/// Test that rules without {tumor}/{normal} wildcards are unaffected by expansion.
#[test]
fn wc01_non_wildcard_rules_unchanged() {
    let toml = r#"
        [workflow]
        name = "wc01-no-wildcard"

        [[pairs]]
        pair_id = "P1"
        tumor   = "T1"
        normal  = "N1"

        [[rules]]
        name   = "setup"
        output = ["setup.done"]
        shell  = "mkdir -p results && touch setup.done"

        [[rules]]
        name   = "align_tumor"
        input  = ["raw/{tumor}.fq"]
        output = ["aligned/{tumor}.bam"]
        shell  = "bwa mem ref.fa {input[0]} > {output[0]}"
    "#;

    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    // setup rule should remain as-is
    assert_eq!(config.rules.iter().filter(|r| r.name == "setup").count(), 1);
    // align_tumor should be expanded
    assert!(config.rules.iter().any(|r| r.name == "align_tumor_P1"));
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
    assert_eq!(qc_c001.input[0], "raw/CTRL_001_R1.fq.gz");
    assert_eq!(qc_c001.output[0], "qc/CTRL_001_fastqc.html");

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
