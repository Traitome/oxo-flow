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
