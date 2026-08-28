//! Verbatim test corpus relocated per issue #206.
#![allow(unused_imports)]
#![allow(deprecated)] // carried over from monolithic config.rs head

use super::*;
use crate::config::parse::resolve_rule_templates;
use std::collections::HashMap;

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
    let csv = "sample_id,fastq_r1,fastq_r2\nS1,s1_R1.fq.gz,s1_R2.fq.gz\nS2,s2_R1.fq.gz,s2_R2.fq.gz";
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
fn workflow_meta_hooks_parse() {
    // Workflow-level terminal hooks (issue #227 item 1): on_complete
    // fires after a fully successful run, on_error after any failure.
    let toml = r#"
        [workflow]
        name = "test"
        version = "1.0.0"
        on_complete = "touch done.marker"
        on_error = "touch error.marker"

        [[rules]]
        name = "step1"
        shell = "echo hi"
    "#;
    let config = WorkflowConfig::parse(toml).unwrap();
    assert_eq!(
        config.workflow.on_complete.as_deref(),
        Some("touch done.marker")
    );
    assert_eq!(
        config.workflow.on_error.as_deref(),
        Some("touch error.marker")
    );
}

#[test]
fn webhook_section_parses_and_defaults() {
    // Issue #227 item 1: the `[webhook]` section reaches WorkflowConfig —
    // the previously dead surface. Partial sections take the serde
    // defaults (POST, [workflow_completed], 30s timeout, 3 retries).
    let toml = r#"
        [workflow]
        name = "test"
        version = "1.0.0"

        [webhook]
        url = "https://hooks.example.com/oxo"

        [[rules]]
        name = "step1"
        shell = "echo hi"
    "#;
    let config = WorkflowConfig::parse(toml).unwrap();
    let webhook = config.webhook.as_ref().expect("[webhook] parses");
    assert_eq!(webhook.url, "https://hooks.example.com/oxo");
    assert_eq!(
        webhook.events,
        vec![crate::webhook::WebhookEvent::WorkflowCompleted]
    );
    assert_eq!(webhook.max_retries, 3);
    assert_eq!(webhook.timeout_secs, 30);

    // Event selection parses.
    let full = toml.replace(
        "url = \"https://hooks.example.com/oxo\"",
        "url = \"https://hooks.example.com/oxo\"\n        events = [\"workflow_started\", \"workflow_completed\", \"workflow_failed\"]\n        secret = \"hunter2\"",
    );
    let config = WorkflowConfig::parse(&full).unwrap();
    let webhook = config.webhook.as_ref().unwrap();
    assert_eq!(webhook.events.len(), 3);
    assert_eq!(webhook.secret.as_deref(), Some("hunter2"));

    // A workflow without [webhook] stays None.
    let plain = WorkflowConfig::parse(
        "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"s\"\nshell = \"echo hi\"\n",
    )
    .unwrap();
    assert!(plain.webhook.is_none());
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
    // E017 (audit B1): the engine-internal key is no longer silently
    // ignored — it is rejected as unknown, which is strictly safer than
    // the old behavior (a user setting it on a plain rule would silently
    // delete their real input files after success).
    let err = WorkflowConfig::parse(toml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cleanup_chunks") && msg.contains("E017"),
        "parse must reject cleanup_chunks: {msg}"
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
fn include_contract_warns_on_wildcarded_internal_reads() {
    // Wildcarded host inputs cannot be resolved through the
    // exact-string producer map, and they form no DAG edge (placeholder
    // values only exist at run time), so the encapsulation check falls
    // back to a structural pattern match: identical literal prefix and
    // suffix against a module-internal output.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("qc.oxoflow"),
        r#"[workflow]
name = "qc"
version = "1.0.0"

[[rules]]
name = "fastqc"
input = ["raw/{sample}.fq"]
output = ["qc/{sample}.html"]
shell = "true"

[[rules]]
name = "internal"
input = ["qc/{sample}.html"]
output = ["qc/{sample}.tmp"]
shell = "true"
"#,
    )
    .unwrap();

    // (a) host reads a wildcarded pattern that structurally matches a
    // module-internal output, undeclared → warning
    let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/{sample}.fq"]
outputs = ["qc/{sample}.html"]

[[rules]]
name = "peeker"
input = ["qc/{sample}.tmp"]
output = ["peek.txt"]
shell = "true"
"#;
    let wf = dir.path().join("host.oxoflow");
    std::fs::write(&wf, host).unwrap();
    let config = WorkflowConfig::from_file(&wf).unwrap();
    let (errors, warnings) = config.check_include_contracts();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    assert!(
        warnings.iter().any(|w| w.contains("qc/{sample}.tmp")),
        "wildcarded encapsulation warning must fire: {warnings:?}"
    );

    // (b) the same pattern declared in the contract → no warning
    let host2 = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/{sample}.fq"]
outputs = ["qc/{sample}.html", "qc/{sample}.tmp"]

[[rules]]
name = "peeker"
input = ["qc/{sample}.tmp"]
output = ["peek.txt"]
shell = "true"
"#;
    let wf2 = dir.path().join("host2.oxoflow");
    std::fs::write(&wf2, host2).unwrap();
    let config2 = WorkflowConfig::from_file(&wf2).unwrap();
    let (errors2, warnings2) = config2.check_include_contracts();
    assert!(errors2.is_empty(), "unexpected errors: {errors2:?}");
    assert!(
        !warnings2.iter().any(|w| w.contains("qc/{sample}.tmp")),
        "declared wildcarded outputs must not warn: {warnings2:?}"
    );

    // (c) a pattern that cannot address the module's files (different
    // literal suffix) → no warning
    let host3 = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "qc.oxoflow"
inputs = ["raw/{sample}.fq"]
outputs = ["qc/{sample}.html"]

[[rules]]
name = "peeker"
input = ["qc/{sample}.bcf"]
output = ["peek.txt"]
shell = "true"
"#;
    let wf3 = dir.path().join("host3.oxoflow");
    std::fs::write(&wf3, host3).unwrap();
    let config3 = WorkflowConfig::from_file(&wf3).unwrap();
    let (errors3, warnings3) = config3.check_include_contracts();
    assert!(errors3.is_empty(), "unexpected errors: {errors3:?}");
    assert!(
        !warnings3.iter().any(|w| w.contains("qc/{sample}.bcf")),
        "patterns with different literals must not warn: {warnings3:?}"
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
fn include_module_config_defaults_fill_gaps() {
    // Issue #142 M3: a module declaring `[config]` defaults keeps them
    // when included without host params — previously the run failed
    // E005 (undefined config variable) because only `[[include]]
    // params` were merged into the host config.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mod.oxoflow"),
        r#"[workflow]
name = "mod"
version = "1.0.0"

[config]
trim_quality = "20"

[[rules]]
name = "step"
output = ["o.txt"]
shell = "echo {config.trim_quality} > o.txt"
"#,
    )
    .unwrap();

    // (a) module default fills the gap when no params are supplied
    let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"
outputs = ["o.txt"]

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
        config
            .config
            .get("trim_quality")
            .and_then(toml::Value::as_str),
        Some("20"),
        "module [config] defaults must fill gaps left by missing params"
    );

    // (b) host params win over the module default
    let host2 = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"
outputs = ["o.txt"]
params = { trim_quality = "30" }

[[rules]]
name = "use"
input = ["o.txt"]
output = ["u.txt"]
shell = "true"
"#;
    let wf2 = dir.path().join("host2.oxoflow");
    std::fs::write(&wf2, host2).unwrap();
    let config2 = WorkflowConfig::from_file(&wf2).unwrap();
    assert_eq!(
        config2
            .config
            .get("trim_quality")
            .and_then(toml::Value::as_str),
        Some("30"),
        "host params must override the module default"
    );

    // (c) declarative `{ default = ... }` entries follow the same
    // extraction semantics as standalone validation
    std::fs::write(
        dir.path().join("dec.oxoflow"),
        r#"[workflow]
name = "dec"
version = "1.0.0"

[config]
trim_quality = { default = "25" }

[[rules]]
name = "step"
output = ["o.txt"]
shell = "echo {config.trim_quality} > o.txt"
"#,
    )
    .unwrap();
    let host3 = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "dec.oxoflow"
outputs = ["o.txt"]

[[rules]]
name = "use"
input = ["o.txt"]
output = ["u.txt"]
shell = "true"
"#;
    let wf3 = dir.path().join("host3.oxoflow");
    std::fs::write(&wf3, host3).unwrap();
    let config3 = WorkflowConfig::from_file(&wf3).unwrap();
    assert_eq!(
        config3
            .config
            .get("trim_quality")
            .and_then(toml::Value::as_str),
        Some("25"),
        "declarative module defaults must be extracted and merged"
    );
}

#[test]
fn include_module_undefined_config_key_still_errors() {
    // Issue #142 M3 regression guard: when neither the host config,
    // `[[include]] params`, nor the module's own `[config]` define a
    // referenced key, the reference stays undefined — lint still
    // reports E005.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mod.oxoflow"),
        r#"[workflow]
name = "mod"
version = "1.0.0"

[[rules]]
name = "step"
output = ["o.txt"]
shell = "echo {config.trim_quality} > o.txt"
"#,
    )
    .unwrap();
    let host = r#"[workflow]
name = "host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"
outputs = ["o.txt"]

[[rules]]
name = "use"
input = ["o.txt"]
output = ["u.txt"]
shell = "true"
"#;
    let wf = dir.path().join("host.oxoflow");
    std::fs::write(&wf, host).unwrap();
    let config = WorkflowConfig::from_file(&wf).unwrap();
    assert!(
        !config.config.contains_key("trim_quality"),
        "a key defined nowhere must stay undefined"
    );
    let result = crate::format::validate_format(&config);
    assert!(
        !result.valid
            && result
                .errors()
                .iter()
                .any(|d| d.code == "E005" && d.message.contains("trim_quality")),
        "validation must still flag the undefined config reference, got: {:?}",
        result
            .errors()
            .iter()
            .filter(|d| d.code == "E005")
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
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
fn input_groups_config_parses() {
    // Issue #227 item 3 (groupTuple pattern): `input_groups` declares a
    // filesystem pattern whose per-group-key files feed ONE instance.
    // `keep` accepts both the single-string form (`keep = "lane"`) and an
    // array form (`keep = ["lane", "read"]`).
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("groups.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "groups"

        [[rules]]
        name = "lanemerge"
        input_groups = [
            { pattern = "results/adapterremoval/{sample}_{lane}_R1.fastq.gz", group_by = "sample", keep = "lane" }
        ]
        output = ["results/merged/{sample}_R1.fastq.gz"]
        shell = "cat {input} > {output}"

        [[rules]]
        name = "seqmerge"
        input_groups = [
            { pattern = "bams/{sample}_{replicate}_{seqtype}.bam", group_by = "sample", keep = ["replicate", "seqtype"] }
        ]
        output = ["merged/{sample}.bam"]
        shell = "samtools merge {output} {input}"
        "#,
    )
    .unwrap();

    let config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let lanemerge = config
        .rules
        .iter()
        .find(|r| r.name == "lanemerge")
        .expect("lanemerge rule");
    assert_eq!(lanemerge.input_groups.len(), 1);
    let g = &lanemerge.input_groups[0];
    assert_eq!(
        g.pattern,
        "results/adapterremoval/{sample}_{lane}_R1.fastq.gz"
    );
    assert_eq!(g.group_by, "sample");
    // Single-string `keep` normalizes to a one-element list.
    assert_eq!(g.keep.as_deref(), Some(&["lane".to_string()][..]));

    let seqmerge = config
        .rules
        .iter()
        .find(|r| r.name == "seqmerge")
        .expect("seqmerge rule");
    assert_eq!(
        seqmerge.input_groups[0].keep.as_deref(),
        Some(&["replicate".to_string(), "seqtype".to_string()][..])
    );
}

#[test]
fn input_groups_fans_rule_into_one_instance_per_group_key() {
    // The groupTuple expansion: files on disk matching the pattern are
    // grouped by the `group_by` wildcard; each group key becomes one rule
    // instance whose `{input}` renders ALL of the group's files (sorted)
    // and whose wildcard map binds the key plus the first occurrence of
    // every other pattern wildcard. Per-group value lists are exposed as
    // `{input_group.<wildcard>}`.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("merge.oxoflow");
    for file in [
        "raw/S1_L1_R1.fastq.gz",
        "raw/S1_L2_R1.fastq.gz",
        "raw/S2_L1_R1.fastq.gz",
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file).unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "merge"

        [[rules]]
        name = "lanemerge"
        input_groups = [
            { pattern = "raw/{sample}_{lane}_R1.fastq.gz", group_by = "sample" }
        ]
        output = ["merged/{sample}_R1.fastq.gz"]
        shell = "cat {input} > merged/{sample}_R1.fastq.gz && echo lanes: {input_group.lane}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["lanemerge_S1", "lanemerge_S2"]);

    let s1 = config
        .rules
        .iter()
        .find(|r| r.name == "lanemerge_S1")
        .expect("S1 instance");
    assert_eq!(
        s1.input.to_vec(),
        vec![
            "raw/S1_L1_R1.fastq.gz".to_string(),
            "raw/S1_L2_R1.fastq.gz".to_string(),
        ]
    );
    assert_eq!(s1.output.to_vec(), vec!["merged/S1_R1.fastq.gz"]);
    assert!(
        s1.input_groups.is_empty(),
        "instances must not carry the input_groups declaration"
    );
    // The shell baked the instance map ({sample} = key, {lane} = first
    // occurrence) and the space-joined {input_group.lane} list; {input}
    // stays for execution-time rendering.
    assert_eq!(
        s1.shell.as_deref(),
        Some("cat {input} > merged/S1_R1.fastq.gz && echo lanes: L1 L2")
    );
    // Readiness attribution (issue #63) records the group key.
    assert_eq!(
        config.expansion_samples.get("lanemerge_S1"),
        Some(&vec!["S1".to_string()])
    );
    // Per-instance bindings for expand_inputs pattern resolution.
    let s1_bindings = config.expansion_values.get("lanemerge_S1").unwrap();
    assert_eq!(s1_bindings.get("sample").map(String::as_str), Some("S1"));
    assert_eq!(s1_bindings.get("lane").map(String::as_str), Some("L1"));

    let s2 = config
        .rules
        .iter()
        .find(|r| r.name == "lanemerge_S2")
        .expect("S2 instance");
    assert_eq!(s2.input.to_vec(), vec!["raw/S2_L1_R1.fastq.gz"]);
}

#[test]
fn input_groups_intersects_declared_sample_set() {
    // #246: the discovery domain of an input_groups rule is the
    // FILESYSTEM, not the declared sample set — stale files for
    // undeclared samples (a prior run's leftovers) instantiated orphan
    // rules that executed with zero DAG consumers. When the workflow
    // declares a sample domain (sample_groups/pairs/sample_pattern),
    // group keys outside it are pruned at plan time with a warning.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("merge.oxoflow");
    for sample in ["S1", "S2", "S3"] {
        let path = dir.path().join(format!("raw/{sample}_R1.fastq.gz"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "merge"

        [[sample_groups]]
        name = "grp"
        samples = ["S1", "S2"]

        [[rules]]
        name = "merge"
        input_groups = [
            { pattern = "raw/{sample}_R1.fastq.gz", group_by = "sample" }
        ]
        output = ["merged/{sample}.fq"]
        shell = "cat {input} > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
    // S3 exists on disk but is not a declared sample — pruned.
    assert_eq!(names, vec!["merge_S1", "merge_S2"]);
}

#[test]
fn input_groups_non_sample_group_key_keeps_filesystem_domain() {
    // The declared-set intersection applies ONLY when the group key is
    // the sample dimension (`group_by = "sample"`). Other keys
    // (chipseq-style `group_by = "meta.antibody"`) are orthogonal to the
    // sample set and must keep their filesystem domain.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("ab.oxoflow");
    std::fs::write(
        dir.path().join("samples.tsv"),
        "sample\tantibody\nS1\tA1\nS2\tA2\n",
    )
    .unwrap();
    for sample in ["S1", "S2"] {
        let path = dir.path().join(format!("bams/{sample}.bam"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "ab"
        metadata_file = "samples.tsv"

        [[sample_groups]]
        name = "grp"
        samples = ["S1", "S2"]

        [[rules]]
        name = "concat"
        input_groups = [
            { pattern = "bams/{sample}.bam", group_by = "meta.antibody" }
        ]
        output = ["consensus/{antibody}.peaks.bed"]
        shell = "touch {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
    // Keys are antibody values (A1/A2), not sample names — no pruning.
    assert_eq!(names, vec!["concat_A1", "concat_A2"]);
}

#[test]
fn input_groups_matches_zero_files_drops_rule_with_warning() {
    // A group key that matches zero files is not instantiated — no
    // instance means nothing to run (issue #227 item 3 skip semantics).
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("empty.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "empty"

        [[rules]]
        name = "noop"
        input_groups = [
            { pattern = "missing/{sample}_{lane}_R1.fastq.gz", group_by = "sample" }
        ]
        output = ["merged/{sample}_R1.fastq.gz"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    assert!(
        config.rules.iter().all(|r| r.name != "noop"),
        "zero-match input_groups rule must not be instantiated"
    );
}

#[test]
fn input_groups_regular_input_appends_after_group_files() {
    // `input` and `input_groups` coexist: the group files come first in
    // the instance's input list, the declared inputs append after them.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("coexist.oxoflow");
    for file in ["raw/S1_L1_R1.fastq.gz", "raw/S1_L2_R1.fastq.gz"] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file).unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "coexist"

        [[rules]]
        name = "merge"
        input = ["meta/{sample}.txt"]
        input_groups = [
            { pattern = "raw/{sample}_{lane}_R1.fastq.gz", group_by = "sample" }
        ]
        output = ["merged/{sample}_R1.fastq.gz"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    let s1 = config
        .rules
        .iter()
        .find(|r| r.name == "merge_S1")
        .expect("S1 instance");
    assert_eq!(
        s1.input.to_vec(),
        vec![
            "raw/S1_L1_R1.fastq.gz".to_string(),
            "raw/S1_L2_R1.fastq.gz".to_string(),
            // Regular inputs append AFTER the group files, expanded with
            // the instance map ({sample} = group key).
            "meta/S1.txt".to_string(),
        ]
    );
}

#[test]
fn input_groups_rejects_invalid_declarations() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("invalid.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "invalid"

        [[rules]]
        name = "bad_group_by"
        input_groups = [
            { pattern = "raw/{sample}_{lane}_R1.fastq.gz", group_by = "read" }
        ]
        shell = "echo hi"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("group_by"),
        "group_by not in pattern must fail: {err}"
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
fn pair_when_gates_fan_out_per_config_toggle() {
    // A pair with `when` declares no rule instances while the condition is
    // false — one static [[pairs]] table serves profile-switched sample
    // sets (multi-sample chipseq/scrna ports gate extra pairs on a config
    // toggle flipped by `--profile` override).
    let toml = r#"
        [workflow]
        name = "test"
        version = "1.0.0"

        [config]
        extended = false

        [[pairs]]
        pair_id = "BASE_001"
        experiment = "EXP_01"
        control = "CTR_01"

        [[pairs]]
        pair_id = "EXTRA_001"
        experiment = "EXP_02"
        control = "CTR_02"
        when = "config.extended"

        [[rules]]
        name = "step1"
        input = ["raw/{pair_id}.fq"]
        output = ["out/{pair_id}.fq"]
        shell = "cp {input[0]} {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    // Gated pair excluded: only BASE_001 fans out.
    assert_eq!(
        config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["step1_BASE_001"]
    );

    // Flipping the toggle includes the gated pair (re-expand from the
    // preserved templates, the checkpoint re-entry pattern).
    config
        .config
        .insert("extended".to_string(), toml::Value::Boolean(true));
    config.rules = config.rule_templates.clone();
    config.expand_wildcards().unwrap();
    assert_eq!(
        config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["step1_BASE_001", "step1_EXTRA_001"]
    );
}

#[test]
fn pair_when_unknown_config_keys_reports_typos() {
    // A pair `when` referencing a config key absent from [config]
    // evaluates false — at pair scope that silently drops the pair's
    // whole rule set, so the plan-time typo guard must surface the
    // offending key (warn, never error — the #199 unbound→false stance).
    use crate::config::pair_when_unknown_config_keys;
    let mut config: std::collections::HashMap<String, toml::Value> =
        std::collections::HashMap::new();
    config.insert("extended".to_string(), toml::Value::Boolean(false));

    assert_eq!(
        pair_when_unknown_config_keys("config.extened", &config),
        vec!["extened"]
    );
    assert_eq!(
        pair_when_unknown_config_keys("config.extended", &config),
        Vec::<&str>::new()
    );
    assert_eq!(
        pair_when_unknown_config_keys("true", &config),
        Vec::<&str>::new()
    );
    // Comparison form, nesting, negation — all reference positions count.
    assert_eq!(
        pair_when_unknown_config_keys(
            "(config.extended && !config.missing) || config.other == \"yes\"",
            &config
        ),
        vec!["missing", "other"]
    );
    // One warning per key, in order of first appearance.
    assert_eq!(
        pair_when_unknown_config_keys("config.typo || config.typo", &config),
        vec!["typo"]
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
fn expand_inputs_bare_pair_id_binds_per_instance() {
    // A rule that fans out per pair (its input/output carry {pair_id})
    // and gathers with a bare {pair_id} inside the expand pattern:
    // each instance must pick up ITS OWN pair's files only — the
    // snakemake-style per-sample semantics (paired/tumor-only sinks).
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("perpair.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "perpair"

        [[pairs]]
        pair_id = "p1"
        experiment = "t1"
        control = "n1"

        [[pairs]]
        pair_id = "p2"
        experiment = "t2"

        [[rules]]
        name = "gather_pair"
        input = ["reads/{pair_id}.fq"]
        expand_inputs = [
            { pattern = "calls/{pair_id}.vcf.gz", variables = {} }
        ]
        output = ["done/{pair_id}.done"]
        shell = "touch {output[0]}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    let p1 = config
        .rules
        .iter()
        .find(|r| r.name == "gather_pair_p1")
        .expect("p1 instance survives");
    let p2 = config
        .rules
        .iter()
        .find(|r| r.name == "gather_pair_p2")
        .expect("p2 instance survives");
    assert_eq!(
        p1.input.to_vec(),
        vec!["reads/p1.fq".to_string(), "calls/p1.vcf.gz".to_string()]
    );
    assert_eq!(
        p2.input.to_vec(),
        vec!["reads/p2.fq".to_string(), "calls/p2.vcf.gz".to_string()]
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
fn values_from_resolves_fan_out_from_config() {
    // values_from resolves the fan-out from a config key at expansion time
    // (issue #18 wave) — CLI --arg / profile driven dimensions.
    let toml = values_workflow(
        r#"
        [config]
        library_ids = "u1,u2"

        [[values]]
        name = "assembler"
        values_from = "config.library_ids"
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
        vec!["assemble_assembler_u1", "assemble_assembler_u2"]
    );

    // Static values take a back seat to values_from.
    let toml = toml.replace(
        "values_from = \"config.library_ids\"",
        "values_from = \"config.library_ids\"\n        values = [\"ignored\"]",
    );
    let mut config = WorkflowConfig::parse(&toml).unwrap();
    config.expand_wildcards().unwrap();
    assert_eq!(config.rules.len(), 2);

    // A missing/unresolvable key fails validation with the key named.
    let bad = values_workflow(
        r#"
        [[values]]
        name = "assembler"
        values_from = "config.missing_key"
        "#,
        r#"
        [[rules]]
        name = "assemble"
        input = ["reads/{assembler}/in.fq"]
        output = ["contigs/{assembler}/out.fa"]
        shell = "{assembler} -o {output[0]} {input[0]}"
        "#,
    );
    let mut config = WorkflowConfig::parse(&bad).unwrap();
    let err = config.expand_wildcards().unwrap_err().to_string();
    assert!(err.contains("missing_key"), "{err}");
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
fn when_wildcard_scope_filters_instances_per_pair() {
    // Snakemake-style DAG morphing: one pair has a control, the other is
    // tumor-only. `wildcard.control` predicates keep exactly the matching
    // instance of each rule — the paired rule survives only for the
    // paired sample and vice versa.
    let toml = r#"
        [workflow]
        name = "t"

        [[pairs]]
        pair_id = "p1"
        experiment = "t1"
        control = "n1"

        [[pairs]]
        pair_id = "p2"
        experiment = "t2"

        [[rules]]
        name = "paired_step"
        input = ["reads/{pair_id}.fq"]
        output = ["out/{pair_id}.bam"]
        when = "wildcard.control != ''"
        shell = "touch {output[0]}"

        [[rules]]
        name = "unpaired_step"
        input = ["reads/{pair_id}.fq"]
        output = ["out/{pair_id}.vcf"]
        when = "wildcard.control == ''"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    let mut names: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["paired_step_p1".to_string(), "unpaired_step_p2".to_string()]
    );
}

#[test]
fn when_group_metadata_key_filters_instances() {
    // Live snparcher incident (issue #85): `when = "wildcard.input_type
    // == 'srr'"` must keep only the SRA cohort. The fastq cohort's
    // group declares no `input_type` metadata, so the key is unbound in
    // its combos — the unbound comparison is false and the SRA rule
    // never enters the DAG for fastq samples.
    let toml = r#"
        [workflow]
        name = "t"

        [[sample_groups]]
        name = "sra_cohort"
        samples = ["s1", "s2"]
        [sample_groups.metadata]
        input_type = "srr"

        [[sample_groups]]
        name = "fastq_cohort"
        samples = ["f1"]

        [[rules]]
        name = "download_sra"
        input = []
        output = ["raw/{sample}/{sample}_1.fastq.gz"]
        when = "wildcard.input_type == 'srr'"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    let mut names: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "download_sra_sra_cohort_s1".to_string(),
            "download_sra_sra_cohort_s2".to_string()
        ],
        "only the SRA-cohort instances survive; the fastq cohort has no input_type binding"
    );
}

#[test]
fn when_group_metadata_key_never_bound_filters_all_instances() {
    // No group declares `input_type` anywhere: every instance's
    // comparison is unbound → false → the rule never enters the DAG.
    let toml = r#"
        [workflow]
        name = "t"

        [[sample_groups]]
        name = "cohort"
        samples = ["s1"]

        [[rules]]
        name = "download_sra"
        input = []
        output = ["raw/{sample}.fq"]
        when = "wildcard.input_type == 'srr'"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    assert!(
        config.rules.is_empty(),
        "an unbound wildcard key in when must filter every instance"
    );
}

#[test]
fn when_pair_metadata_key_filters_instances() {
    // Pair metadata keys participate in when evaluation the same way
    // group metadata does.
    let toml = r#"
        [workflow]
        name = "t"

        [[pairs]]
        pair_id = "p1"
        experiment = "e1"
        control = "c1"
        [pairs.metadata]
        source = "sra"

        [[pairs]]
        pair_id = "p2"
        experiment = "e2"

        [[rules]]
        name = "sra_step"
        input = ["reads/{pair_id}.fq"]
        output = ["out/{pair_id}.sra.txt"]
        when = "wildcard.source == 'sra'"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    let mut names: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["sra_step_p1".to_string()]);
}

#[test]
fn when_wildcard_baked_into_kept_instance() {
    // Kept instances bake their per-instance bindings into `when` so
    // the execution-time re-check (no wildcard context there) re-
    // evaluates the same verdict instead of vetoing or re-running.
    let toml = r#"
        [workflow]
        name = "t"

        [[sample_groups]]
        name = "cohort"
        samples = ["s1"]
        [sample_groups.metadata]
        input_type = "srr"

        [[rules]]
        name = "download_sra"
        input = []
        output = ["raw/{sample}.fq"]
        when = "wildcard.input_type == 'srr'"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    assert_eq!(config.rules.len(), 1);
    let baked = config.rules[0].when.as_deref().expect("when survives");
    assert_eq!(baked, "'srr' == 'srr'");
    // And it evaluates true with no wildcard context, so execution
    // never vetoes the kept instance.
    assert!(crate::executor::process::evaluate_condition(
        baked,
        &HashMap::new()
    ));
}

#[test]
fn bake_wildcard_when_resolves_bindings_by_position() {
    let mut combo = crate::wildcard::WildcardValues::new();
    combo.insert("input_type".to_string(), "srr".to_string());
    combo.insert("control".to_string(), String::new());
    combo.insert("feature".to_string(), "on".to_string());

    // Comparison operands (either side) become quoted literals.
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("wildcard.input_type == 'srr'", &combo),
        "'srr' == 'srr'"
    );
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("'srr' != wildcard.input_type", &combo),
        "'srr' != 'srr'"
    );
    // Bare truthiness becomes true/false literals (handles `!`).
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("wildcard.feature", &combo),
        "true"
    );
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("!wildcard.feature", &combo),
        "!true"
    );
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("wildcard.control == ''", &combo),
        "'' == ''"
    );
    // Mixed with config predicates and unbound keys (left untouched:
    // strict unbound→false at execution matches the expansion verdict).
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("config.gate && wildcard.input_type == 'srr'", &combo),
        "config.gate && 'srr' == 'srr'"
    );
    assert_eq!(
        WorkflowConfig::bake_wildcard_when(
            "wildcard.input_type == 'srr' || wildcard.unbound_key == 'x'",
            &combo
        ),
        "'srr' == 'srr' || wildcard.unbound_key == 'x'"
    );
    // Every baked kept-instance form re-evaluates true with no context:
    // a `!=` form survives expansion only when the value differs from
    // the literal, and `!wildcard.feature` only when the value is falsy
    // (both baking to true forms, checked below). The config gate keeps
    // its own semantics.
    let mut fastq = crate::wildcard::WildcardValues::new();
    fastq.insert("input_type".to_string(), "fastq".to_string());
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("'srr' != wildcard.input_type", &fastq),
        "'srr' != 'fastq'"
    );
    let mut off = crate::wildcard::WildcardValues::new();
    off.insert("feature".to_string(), String::new()); // falsy → kept
    assert_eq!(
        WorkflowConfig::bake_wildcard_when("!wildcard.feature", &off),
        "!false"
    );
    let empty = HashMap::new();
    for baked in [
        "'srr' == 'srr'",
        "'srr' != 'fastq'",
        "true",
        "!false",
        "'' == ''",
    ] {
        assert!(
            crate::executor::process::evaluate_condition(baked, &empty),
            "baked form {baked:?} must re-evaluate true for a kept instance"
        );
    }
    let mut config = HashMap::new();
    config.insert("gate".to_string(), toml::Value::Boolean(true));
    assert!(crate::executor::process::evaluate_condition(
        "config.gate && 'srr' == 'srr'",
        &config
    ));
    config.insert("gate".to_string(), toml::Value::Boolean(false));
    assert!(!crate::executor::process::evaluate_condition(
        "config.gate && 'srr' == 'srr'",
        &config
    ));
}

#[test]
fn when_wildcard_scope_only_filters_when_referencing_wildcards() {
    // Conditions without `wildcard.` references keep the legacy
    // execution-time flow: the instance survives expansion (it will be
    // marked "condition evaluated to false" at execution).
    let toml = r#"
        [workflow]
        name = "t"

        [config]
        gate = false

        [[pairs]]
        pair_id = "p1"
        experiment = "t1"
        control = "n1"

        [[rules]]
        name = "config_gated"
        input = ["reads/{pair_id}.fq"]
        output = ["out/{pair_id}.bam"]
        when = "config.gate"
        shell = "touch {output[0]}"
    "#;
    let mut config = WorkflowConfig::parse(toml).unwrap();
    config.expand_wildcards().unwrap();

    assert_eq!(config.rules.len(), 1, "config-only when survives expansion");
    assert_eq!(config.rules[0].name, "config_gated_p1");
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

// ---------------------------------------------------------------------------
// Sample metadata table + {meta.<column>} namespace (issue #227 item 2)
// ---------------------------------------------------------------------------

/// Write a `metadata_file` (samples.tsv) plus the given sample groups and
/// rules into a tempdir; returns the tempdir and the workflow path.
fn metadata_workflow(
    metadata_tsv: &str,
    sample_groups: &str,
    rules: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("samples.tsv"), metadata_tsv).unwrap();
    let workflow_path = dir.path().join("meta.oxoflow");
    std::fs::write(
        &workflow_path,
        format!(
            r#"
        [workflow]
        name = "meta"
        metadata_file = "samples.tsv"

        {sample_groups}
        {rules}
        "#
        ),
    )
    .unwrap();
    (dir, workflow_path)
}

#[test]
fn metadata_file_loads_tsv_rows_into_config() {
    // `[workflow] metadata_file` loads a per-sample table: the first column
    // is the sample id (matching `{sample}` values), the remaining columns
    // are arbitrary keys addressed as `{meta.<column>}`.
    let (_dir, workflow_path) = metadata_workflow(
        "sample\tendedness\tadapters\nS1\tSE\tAGATCGGAAG\nS2\tPE\t\n",
        r#"
        [[sample_groups]]
        name = "control"
        samples = ["S1", "S2"]
        "#,
        r#"
        [[rules]]
        name = "trim"
        input = ["raw/{sample}_R1.fastq.gz"]
        output = ["trimmed/{sample}.fq"]
        shell = "cutadapt -a {meta.adapters}"
        "#,
    );
    let config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let s1 = config.metadata.get("S1").expect("S1 row");
    assert_eq!(s1.get("endedness").map(String::as_str), Some("SE"));
    assert_eq!(s1.get("adapters").map(String::as_str), Some("AGATCGGAAG"));
    let s2 = config.metadata.get("S2").expect("S2 row");
    assert_eq!(s2.get("endedness").map(String::as_str), Some("PE"));
    assert_eq!(s2.get("adapters"), Some(&String::new()));
}

#[test]
fn metadata_file_accepts_csv_and_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("samples.csv"),
        "sample,endedness\nS1,SE\nS2,PE\n",
    )
    .unwrap();
    let workflow_path = dir.path().join("csv.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "meta-csv"
        metadata_file = "samples.csv"
        "#,
    )
    .unwrap();
    let config = WorkflowConfig::from_file(&workflow_path).unwrap();
    assert_eq!(config.metadata["S1"]["endedness"], "SE");

    std::fs::write(
        dir.path().join("samples.json"),
        r#"[{"sample": "S1", "endedness": "SE"}, {"sample": "S2", "endedness": "PE"}]"#,
    )
    .unwrap();
    let workflow_path = dir.path().join("json.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "meta-json"
        metadata_file = "samples.json"
        "#,
    )
    .unwrap();
    let config = WorkflowConfig::from_file(&workflow_path).unwrap();
    assert_eq!(config.metadata["S2"]["endedness"], "PE");
}

#[test]
fn metadata_file_missing_errors() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("missing.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "meta"
        metadata_file = "nope.tsv"
        "#,
    )
    .unwrap();
    let err = WorkflowConfig::from_file(&workflow_path).unwrap_err();
    assert!(err.to_string().contains("nope.tsv"));
}

#[test]
fn plan_time_when_meta_only_gate_filters_instances() {
    // A `when` referencing ONLY `{meta.<col>}` must be decided at plan
    // time. The raw token would otherwise hit the evaluator's default-true
    // fallback and phantom instances survive planning (rnaseq-star's
    // `{meta.sra} != ''` gate). Empty and missing values close the gate;
    // a non-empty value opens it — plan-time instance set must equal the
    // runtime verdict.
    let (_dir, workflow_path) = metadata_workflow(
        "sample\trun_qc\nS1\tyes\nS2\t\n",
        r#"
        [[sample_groups]]
        name = "grp"
        samples = ["S1", "S2", "S3"]
        "#,
        r#"
        [[rules]]
        name = "qc"
        input = ["raw/{sample}.fq"]
        output = ["qc/{sample}.txt"]
        shell = "touch {output[0]}"
        when = "{meta.run_qc} != ''"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let names: Vec<_> = config
        .rules
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>();
    // S2 has an empty run_qc value and S3 has no row — both gates close.
    assert_eq!(names, vec!["qc_grp_S1"]);
}

#[test]
fn plan_time_when_mixed_wildcard_and_meta_gate_bakes_both() {
    // A `when` mixing `wildcard.<key>` and `{meta.<col>}` must bake BOTH
    // namespaces before plan-time evaluation: the wildcard binding comes
    // from group metadata, the {meta.} value from the metadata_file row.
    let (_dir, workflow_path) = metadata_workflow(
        "sample\trun_qc\nS1\tyes\nS2\t\n",
        r#"
        [[sample_groups]]
        name = "grp"
        samples = ["S1", "S2"]
        metadata = { input_type = "fastq" }
        "#,
        r#"
        [[rules]]
        name = "qc"
        input = ["raw/{sample}.fq"]
        output = ["qc/{sample}.txt"]
        shell = "touch {output[0]}"
        when = "wildcard.input_type == 'fastq' && {meta.run_qc} != ''"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let names: Vec<_> = config
        .rules
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["qc_grp_S1"]);
}

#[test]
fn meta_namespace_expands_in_shells_per_sample() {
    // rnaseq-star-style per-unit lookup: a metadata column feeds a shell
    // flag, resolved per instance from the instance's `{sample}` binding.
    // A column that exists but is empty on a row renders ""; a column no
    // row defines renders "" too.
    let (_dir, workflow_path) = metadata_workflow(
        "sample\tadapters\textra\nS1\tAGATCGGAAG\t--clip-r1 5\nS2\tCTGTCTCTTA\t\n",
        r#"
        [[sample_groups]]
        name = "control"
        samples = ["S1", "S2"]
        "#,
        r#"
        [[rules]]
        name = "trim"
        input = ["raw/{sample}_R1.fastq.gz"]
        output = ["trimmed/{sample}.fq"]
        shell = "cutadapt -a {meta.adapters} {meta.extra}"

        [[rules]]
        name = "qc"
        input = ["trimmed/{sample}.fq"]
        output = ["qc/{sample}.txt"]
        shell = "check {meta.unknown_column}"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();

    let s1 = config
        .rules
        .iter()
        .find(|r| r.name == "trim_control_S1")
        .expect("S1 instance");
    assert_eq!(
        s1.shell.as_deref(),
        Some("cutadapt -a AGATCGGAAG --clip-r1 5")
    );
    let s2 = config
        .rules
        .iter()
        .find(|r| r.name == "trim_control_S2")
        .expect("S2 instance");
    // Empty cell on the row renders empty.
    assert_eq!(s2.shell.as_deref(), Some("cutadapt -a CTGTCTCTTA "));

    // Column that no row defines renders empty on every instance.
    for name in ["qc_control_S1", "qc_control_S2"] {
        let rule = config.rules.iter().find(|r| r.name == name).expect(name);
        assert_eq!(rule.shell.as_deref(), Some("check "));
    }
}

#[test]
fn meta_namespace_resolves_input_paths_and_dag_edges() {
    // A rule-level `input` entry can be `{meta.<column>}` — the path is a
    // plan-time literal after expansion, so exact-match DAG edge inference
    // connects it to the producer.
    let (_dir, workflow_path) = metadata_workflow(
        "sample\tbam_path\nS1\taligned/S1.bam\nS2\taligned/S2.bam\n",
        r#"
        [[sample_groups]]
        name = "control"
        samples = ["S1", "S2"]
        "#,
        r#"
        [[rules]]
        name = "map"
        output = ["aligned/{sample}.bam"]
        shell = "echo hi > {output[0]}"

        [[rules]]
        name = "call"
        input = ["{meta.bam_path}"]
        output = ["calls/{sample}.vcf"]
        shell = "bcftools call {input[0]} -o {output[0]}"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();

    let call_s1 = config
        .rules
        .iter()
        .find(|r| r.name == "call_control_S1")
        .expect("call S1 instance");
    assert_eq!(call_s1.input.to_vec(), vec!["aligned/S1.bam"]);
    let call_s2 = config
        .rules
        .iter()
        .find(|r| r.name == "call_control_S2")
        .expect("call S2 instance");
    assert_eq!(call_s2.input.to_vec(), vec!["aligned/S2.bam"]);

    let dag = crate::dag::WorkflowDag::from_rules(&config.rules).unwrap();
    assert_eq!(
        dag.dependencies("call_control_S1").unwrap(),
        vec!["map_control_S1"]
    );
    assert_eq!(
        dag.dependencies("call_control_S2").unwrap(),
        vec!["map_control_S2"]
    );
}

#[test]
fn meta_namespace_in_when_renders_per_instance() {
    // methylseq-style endedness gate: the per-sample column is substituted
    // into `when` so `'SE' == 'SE'`-style predicates evaluate per instance;
    // a sample with no metadata row renders empty (gate closed).
    let (_dir, workflow_path) = metadata_workflow(
        "sample\tendedness\nSE1\tSE\nPE1\tPE\n",
        r#"
        [[sample_groups]]
        name = "control"
        samples = ["SE1", "PE1", "X1"]
        "#,
        r#"
        [[rules]]
        name = "trim"
        input = ["raw/{sample}_R1.fastq.gz"]
        output = ["trimmed/{sample}.fq"]
        when = "config.single_end_mode || {meta.endedness} == 'SE'"
        shell = "cp {input[0]} {output[0]}"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();

    let se1 = config
        .rules
        .iter()
        .find(|r| r.name == "trim_control_SE1")
        .expect("SE1 instance");
    assert_eq!(
        se1.when.as_deref(),
        Some("config.single_end_mode || 'SE' == 'SE'")
    );
    // PE1 and X1 are pruned at PLAN time (the baked gate evaluates false),
    // so the plan-time instance set matches the runtime verdict — no
    // phantom instances in dry-run/validate output.
    assert!(
        config
            .rules
            .iter()
            .all(|r| r.name != "trim_control_PE1" && r.name != "trim_control_X1"),
        "gated instances must not survive planning"
    );
}

#[test]
fn meta_namespace_lookup_falls_back_to_pair_id() {
    // Pair workflows: metadata rows keyed by pair_id (or experiment) still
    // resolve — the instance's sample-like bindings are tried in order.
    let (_dir, workflow_path) = metadata_workflow(
        "pair_id\tstudy\nP1\tcase\nP2\tcontrol\n",
        "",
        r#"
        [[pairs]]
        pair_id = "P1"
        experiment = "T1"
        control = "N1"

        [[pairs]]
        pair_id = "P2"
        experiment = "T2"
        control = "N2"

        [[rules]]
        name = "cmp"
        input = ["reads/{pair_id}/in.txt"]
        output = ["cmp/{pair_id}.txt"]
        shell = "echo {meta.study} {pair_id}"
        "#,
    );
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();

    let p1 = config
        .rules
        .iter()
        .find(|r| r.name == "cmp_P1")
        .expect("P1 instance");
    assert_eq!(p1.shell.as_deref(), Some("echo case P1"));
    let p2 = config
        .rules
        .iter()
        .find(|r| r.name == "cmp_P2")
        .expect("P2 instance");
    assert_eq!(p2.shell.as_deref(), Some("echo control P2"));
}

#[test]
fn input_groups_group_by_meta_fans_out_one_instance_per_value() {
    // chipseq multi-antibody (issue #227 item 4): group files by a metadata
    // column value instead of a pattern wildcard. 3 samples × 2 antibody
    // values → 2 instances; `{input}` is the group's files space-joined
    // (sorted), `{input_group.sample}` the sample names, and the group key
    // binds under the COLUMN name (`{antibody}`). Rows with an empty column
    // value are skipped.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("results/mapping")).unwrap();
    for sample in ["S1", "S2", "S3", "S4"] {
        std::fs::write(
            dir.path().join(format!("results/mapping/{sample}.bam")),
            "bam",
        )
        .unwrap();
    }
    let (_dir, workflow_path) = {
        std::fs::write(
            dir.path().join("samples.tsv"),
            "sample\tantibody\nS1\tH3K27ac\nS2\tH3K27ac\nS3\tInput\nS4\t\n",
        )
        .unwrap();
        let workflow_path = dir.path().join("chipseq.oxoflow");
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "chipseq"
            metadata_file = "samples.tsv"

            [[rules]]
            name = "peaks"
            input_groups = [
                { pattern = "results/mapping/{sample}.bam", group_by = "meta.antibody" }
            ]
            output = ["peaks/{antibody}.bed"]
            shell = "macs2 callpeak -t {input} -n {antibody} --outdir peaks && echo {input_group.sample}"
            "#,
        )
        .unwrap();
        (dir, workflow_path)
    };
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.expand_wildcards().unwrap();

    let h3 = config
        .rules
        .iter()
        .find(|r| r.name == "peaks_H3K27ac")
        .expect("H3K27ac instance");
    assert_eq!(
        h3.input.to_vec(),
        vec![
            "results/mapping/S1.bam".to_string(),
            "results/mapping/S2.bam".to_string(),
        ]
    );
    // `{input}` stays literal at expansion (execution-time rendering); the
    // group key binds under the COLUMN name (`{antibody}`) and
    // `{input_group.sample}` is the group's space-joined sample names.
    assert_eq!(
        h3.shell.as_deref(),
        Some("macs2 callpeak -t {input} -n H3K27ac --outdir peaks && echo S1 S2")
    );
    let input = config
        .rules
        .iter()
        .find(|r| r.name == "peaks_Input")
        .expect("Input instance");
    assert_eq!(input.input.to_vec(), vec!["results/mapping/S3.bam"]);
    assert_eq!(
        input.shell.as_deref(),
        Some("macs2 callpeak -t {input} -n Input --outdir peaks && echo S3")
    );
    // The empty-antibody row (S4) and its bam are skipped entirely.
    assert!(!config.rules.iter().any(|r| r.name == "peaks_S4"));
    assert!(!config.rules.iter().any(|r| {
        r.input
            .to_vec()
            .contains(&"results/mapping/S4.bam".to_string())
    }));
}

#[test]
fn input_groups_group_by_meta_rejects_sample_in_outputs() {
    // Metadata-grouped instances have no single {sample} binding — the
    // group key (column name) is the only binding. Outputs referencing a
    // pattern wildcard are a plan-time validation error.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("results/mapping")).unwrap();
    std::fs::write(dir.path().join("results/mapping/S1.bam"), "bam").unwrap();
    std::fs::write(
        dir.path().join("samples.tsv"),
        "sample\tantibody\nS1\tH3K27ac\n",
    )
    .unwrap();
    let workflow_path = dir.path().join("chipseq.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "chipseq"
        metadata_file = "samples.tsv"

        [[rules]]
        name = "peaks"
        input_groups = [
            { pattern = "results/mapping/{sample}.bam", group_by = "meta.antibody" }
        ]
        output = ["peaks/{sample}.bed"]
        shell = "echo {input} > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("{sample}"),
        "error should name the forbidden wildcard: {err}"
    );
}

#[test]
fn input_groups_group_by_meta_rejects_keep() {
    // `keep` selects pattern wildcards to bind into the instance map —
    // meaningless when the group key comes from metadata, where the spec
    // says the instance map binds only the column name.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("results/mapping")).unwrap();
    std::fs::write(dir.path().join("results/mapping/S1.bam"), "bam").unwrap();
    std::fs::write(
        dir.path().join("samples.tsv"),
        "sample\tantibody\nS1\tH3K27ac\n",
    )
    .unwrap();
    let workflow_path = dir.path().join("chipseq.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "chipseq"
        metadata_file = "samples.tsv"

        [[rules]]
        name = "peaks"
        input_groups = [
            { pattern = "results/mapping/{sample}.bam", group_by = "meta.antibody", keep = "sample" }
        ]
        output = ["peaks/{antibody}.bed"]
        shell = "echo {input} > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("keep"),
        "error should mention keep: {err}"
    );
}

#[test]
fn input_groups_group_by_meta_unknown_column_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("results/mapping")).unwrap();
    std::fs::write(dir.path().join("results/mapping/S1.bam"), "bam").unwrap();
    std::fs::write(
        dir.path().join("samples.tsv"),
        "sample\tantibody\nS1\tH3K27ac\n",
    )
    .unwrap();
    let workflow_path = dir.path().join("chipseq.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "chipseq"
        metadata_file = "samples.tsv"

        [[rules]]
        name = "peaks"
        input_groups = [
            { pattern = "results/mapping/{sample}.bam", group_by = "meta.notacolumn" }
        ]
        output = ["peaks/{antibody}.bed"]
        shell = "echo {input} > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("notacolumn"),
        "error should name the unknown column: {err}"
    );
}

// ---------------------------------------------------------------------------
// output_pattern (runtime-discovered fan-out, issue #227 item 5)
// ---------------------------------------------------------------------------

#[test]
fn output_pattern_config_parses() {
    // `output_pattern` declares files whose enumeration happens at RUNTIME
    // (filesystem scan after the producer instance completes). A fresh
    // wildcard `{part}` combines with an existing `{sample}` wildcard.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("parts.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "parts"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{sample}_{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/S1_a.txt"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{sample}_{part}.txt"]
        output = ["results/merged/{sample}_{part}.txt"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    let split = config
        .rules
        .iter()
        .find(|r| r.name == "split")
        .expect("split rule");
    assert_eq!(
        split.output_pattern.as_deref(),
        Some("results/chunks/{sample}_{part}.txt")
    );
    // The consumer references the fresh wildcard: not instantiated at plan
    // time (empty domain), but tracked as a pending consumer.
    assert!(
        config.rules.iter().all(|r| r.name != "collect"),
        "consumer of a fresh wildcard must not be instantiated at plan time"
    );
    assert_eq!(
        config.pending_output_pattern_consumers_of("split"),
        vec!["collect".to_string()]
    );
    // The producer records itself under the fresh wildcard name.
    assert_eq!(
        config.output_pattern_producer_of("collect").as_deref(),
        Some("split")
    );
}

#[test]
fn output_pattern_and_output_mutually_exclusive_errors() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("both.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "both"

        [[rules]]
        name = "split"
        output = ["results/chunks/a.txt"]
        output_pattern = "results/chunks/{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/a.txt"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("output_pattern")
            && err.to_string().contains("output")
            && err.to_string().contains("mutually exclusive"),
        "error should call out the mutual exclusion: {err}"
    );
}

#[test]
fn gpus_without_container_backend_errors() {
    // `gpus` only applies to container backends (docker `--gpus`); on a
    // system/conda rule the flag would be silently ignored, so it is a
    // validation error instead.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("gpus.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "gpus"

        [[rules]]
        name = "smi"
        output = ["out/smi.txt"]
        environment = { gpus = "all" }
        shell = "nvidia-smi > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("gpus") && err.to_string().contains("docker"),
        "error should point at the backend mismatch: {err}"
    );
}

#[test]
fn gpus_with_singularity_errors_not_implemented() {
    // GPU passthrough currently maps only to docker's --gpus; singularity
    // --nv is not implemented, and passing validation silently would be
    // the exact "silently ignored flag" pattern validation exists to kill.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("gpus-sif.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "gpus-sif"

        [[rules]]
        name = "smi"
        output = ["out/smi.txt"]
        environment = { singularity = "docker://biocontainers/bwa:0.7.17", gpus = "all" }
        shell = "nvidia-smi > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("gpus") && err.to_string().contains("docker"),
        "error should point at the docker-only contract: {err}"
    );
}

#[test]
fn gpus_with_docker_passes_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("gpus-ok.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "gpus-ok"

        [[rules]]
        name = "smi"
        output = ["out/smi.txt"]
        environment = { docker = "nvidia/cuda:12.6.3-base-ubuntu24.04", gpus = "all" }
        shell = "nvidia-smi > {output[0]}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    assert_eq!(config.rules[0].environment.gpus.as_deref(), Some("all"));
}

#[test]
fn output_pattern_rejects_transform_rules() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("transform.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "transform"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{part}.txt"
        transform = { split = { by = "part", values = ["a", "b"] }, map = "echo {part} > chunk", combine = { shell = "cat" } }
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("transform"),
        "transform rules cannot declare output_pattern: {err}"
    );
}

#[test]
fn output_pattern_fresh_consumer_declared_before_producer_warns() {
    // Declaration order matters: the fan-out pass can only attach a
    // consumer to a producer that is known at plan time. A consumer that
    // references the fresh wildcard BEFORE the producer is declared gets a
    // warning, not an error — the producer may still be found later.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("order.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "order"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{sample}_{part}.txt"]
        output = ["results/merged/{sample}_{part}.txt"]
        shell = "cat {input} > {output}"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{sample}_{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/S1_a.txt"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    // Warning, not error: expand succeeds and the consumer is still
    // attached to the producer once the producer's declaration appears.
    config.expand_wildcards().unwrap();
    assert_eq!(
        config.output_pattern_producer_of("collect").as_deref(),
        Some("split")
    );
    assert_eq!(
        config.pending_output_pattern_consumers_of("split"),
        vec!["collect".to_string()]
    );
}

#[test]
fn output_pattern_duplicate_fresh_wildcard_errors() {
    // One producer per fresh wildcard in v1: two rules declaring the same
    // `{interval}` vocabulary would both claim the consumer.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("dup.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "dup"

        [[rules]]
        name = "producer_a"
        output_pattern = "results/a/{interval}.txt"
        shell = "mkdir -p results/a && echo a > results/a/i1.txt"

        [[rules]]
        name = "producer_b"
        output_pattern = "results/b/{interval}.txt"
        shell = "mkdir -p results/b && echo b > results/b/i1.txt"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("interval"),
        "error should name the duplicate fresh wildcard: {err}"
    );
}

#[test]
fn output_pattern_without_wildcards_errors() {
    // A pattern that cannot enumerate anything is a configuration error.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("nowild.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "nowild"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/all.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/all.txt"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    let err = config.expand_wildcards().unwrap_err();
    assert!(
        err.to_string().contains("wildcard"),
        "error should explain the missing wildcard: {err}"
    );
}

#[test]
fn output_pattern_fanout_instantiates_consumers_from_discovered_domain() {
    // The runtime fan-out: after the producer instance completes, its
    // files are discovered from disk, contributed to the producer's
    // domain, and the deferred consumer is instantiated once per
    // discovered value with the bindings baked in.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("fanout.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "fanout"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/1.txt"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{part}.txt"]
        output = ["results/merged/{part}.txt"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    assert!(config.get_rule("split").is_some());
    assert!(
        config.get_rule("collect").is_none(),
        "consumer deferred at plan time"
    );

    // Simulate the producer completing: it wrote 3 files.
    for part in ["1", "2", "3"] {
        let path = dir.path().join(format!("results/chunks/{part}.txt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, part).unwrap();
    }
    let split = config.get_rule("split").cloned().unwrap();
    let combos = config
        .discover_output_pattern_files(&split, dir.path())
        .unwrap();
    assert_eq!(combos.len(), 3);
    let added = config.contribute_output_pattern_domain("split", combos);
    assert_eq!(added, 3, "all three parts are new");

    let new_names = config.expand_output_pattern_consumers().unwrap();
    assert_eq!(new_names, vec!["collect_1", "collect_2", "collect_3"]);

    let c1 = config.get_rule("collect_1").expect("collect_1 instance");
    assert_eq!(c1.input.to_vec(), vec!["results/chunks/1.txt"]);
    assert_eq!(c1.output.to_vec(), vec!["results/merged/1.txt"]);
    // Executor placeholders stay for execution-time rendering; only the
    // wildcard bindings are baked.
    assert_eq!(c1.shell.as_deref(), Some("cat {input} > {output}"));
    assert_eq!(
        config.expansion_templates.get("collect_1"),
        Some(&"collect".to_string())
    );
    // The consumer left the pending set.
    assert!(
        config
            .pending_output_pattern_consumers_of("split")
            .is_empty()
    );
    // Idempotency: re-running the pass creates nothing new.
    assert!(config.expand_output_pattern_consumers().unwrap().is_empty());
}

#[test]
fn output_pattern_domain_unions_across_producer_samples() {
    // Per-instance union semantics: a producer that fans out over
    // {sample} contributes each sample's slice of the domain; the
    // consumer is instantiated on the UNION, with the sample binding
    // reconstructed from the producer instance that discovered the file.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("union.oxoflow");
    for file in [
        "results/chunks/S1_p1.txt",
        "results/chunks/S1_p2.txt",
        "results/chunks/S2_p1.txt",
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file).unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "union"

        [[sample_groups]]
        name = "samples"
        samples = ["S1", "S2"]

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{sample}_{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/S1_p1.txt"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{sample}_{part}.txt"]
        output = ["results/merged/{sample}_{part}.txt"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    // The producer fanned out over {sample} at plan time (group-branch
    // instance naming: name_group_sample), each instance carrying its
    // baked slice of the pattern.
    let s1 = config
        .get_rule("split_samples_S1")
        .expect("split_samples_S1 instance");
    assert_eq!(
        s1.output_pattern.as_deref(),
        Some("results/chunks/S1_{part}.txt")
    );
    assert!(config.get_rule("collect").is_none());

    // S1 completes first: contributes {p1, p2} (with the S1 binding).
    let combos_s1 = config
        .discover_output_pattern_files(s1, dir.path())
        .unwrap();
    assert_eq!(combos_s1.len(), 2);
    config.contribute_output_pattern_domain("split", combos_s1.clone());

    // S2 completes: contributes {p1} with the S2 binding.
    let s2 = config
        .get_rule("split_samples_S2")
        .cloned()
        .expect("split_samples_S2 instance");
    let combos_s2 = config
        .discover_output_pattern_files(&s2, dir.path())
        .unwrap();
    assert_eq!(combos_s2.len(), 1);
    assert_eq!(combos_s2[0].get("sample").map(String::as_str), Some("S2"));
    let added = config.contribute_output_pattern_domain("split", combos_s2);
    assert_eq!(added, 1);
    // Re-contributing the same combos is a no-op (union semantics).
    assert_eq!(
        config.contribute_output_pattern_domain("split", combos_s1.clone()),
        0
    );

    // Failure attribution must survive the template being replaced by its
    // instances: `get_rule("split")` is gone, but the pending consumers
    // are still resolvable from the producer TEMPLATE (rule_templates).
    assert!(config.get_rule("split").is_none());
    assert_eq!(
        config.pending_output_pattern_consumers_of("split"),
        vec!["collect"]
    );

    let new_names = config.expand_output_pattern_consumers().unwrap();
    assert_eq!(
        new_names,
        vec!["collect_S1_p1", "collect_S1_p2", "collect_S2_p1"]
    );
    let c = config.get_rule("collect_S2_p1").unwrap();
    assert_eq!(c.input.to_vec(), vec!["results/chunks/S2_p1.txt"]);
    assert_eq!(c.shell.as_deref(), Some("cat {input} > {output}"));

    // The pending set is drained by the fan-out: no consumers left to
    // attribute after successful instantiation.
    assert!(
        config
            .pending_output_pattern_consumers_of("split")
            .is_empty()
    );
}

#[test]
fn output_pattern_zero_discoveries_leaves_consumers_pending() {
    // A producer that completed but produced no matching files contributes
    // nothing; its consumers stay pending (instantiated only if another
    // producer instance contributes the domain).
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("empty.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "empty"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{part}.txt"
        shell = "mkdir -p results/chunks"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{part}.txt"]
        output = ["results/merged/{part}.txt"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();

    let split = config.get_rule("split").cloned().unwrap();
    let combos = config
        .discover_output_pattern_files(&split, dir.path())
        .unwrap();
    assert!(combos.is_empty());
    assert_eq!(config.contribute_output_pattern_domain("split", combos), 0);
    assert!(config.expand_output_pattern_consumers().unwrap().is_empty());
    // Still pending, awaiting a future contribution.
    assert_eq!(
        config.pending_output_pattern_consumers_of("split"),
        vec!["collect".to_string()]
    );
}

#[test]
fn output_pattern_consumer_chain_bakes_output_pattern() {
    // A rule that consumes one fresh wildcard and produces another
    // (chain, e.g. interval scatter → per-region calls): deferred as a
    // consumer, instantiated with its OWN output_pattern baked per
    // discovery combo, ready to produce the next generation.
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("chain.oxoflow");
    for file in ["results/a/i1.txt", "results/a/i2.txt"] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file).unwrap();
    }
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "chain"

        [[rules]]
        name = "split"
        output_pattern = "results/a/{interval}.txt"
        shell = "mkdir -p results/a && echo x > results/a/i1.txt"

        [[rules]]
        name = "calls"
        input = ["results/a/{interval}.txt"]
        output_pattern = "results/b/{interval}_{region}.txt"
        shell = "mkdir -p results/b && cp {input} results/b/i1_r1.txt"
        "#,
    )
    .unwrap();

    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    assert!(
        config.get_rule("calls").is_none(),
        "chained rule deferred as a consumer"
    );

    let split = config.get_rule("split").cloned().unwrap();
    let combos = config
        .discover_output_pattern_files(&split, dir.path())
        .unwrap();
    assert_eq!(combos.len(), 2);
    config.contribute_output_pattern_domain("split", combos);

    let new_names = config.expand_output_pattern_consumers().unwrap();
    assert_eq!(new_names, vec!["calls_i1", "calls_i2"]);
    let c = config.get_rule("calls_i1").unwrap();
    assert_eq!(c.input.to_vec(), vec!["results/a/i1.txt"]);
    // The chained rule's own pattern carries the consumed binding baked
    // in; {region} stays fresh for the NEXT producer generation.
    assert_eq!(
        c.output_pattern.as_deref(),
        Some("results/b/i1_{region}.txt")
    );
    // Its instances are producers too.
    assert_eq!(
        config.output_pattern_template_of("calls_i1").as_deref(),
        Some("calls")
    );
}

#[test]
fn output_pattern_unrelated_wildcard_is_not_a_fresh_reference() {
    // `{nope}` is not the producer's vocabulary: the consumer must not be
    // deferred as an output_pattern consumer (the engine's permissive
    // stance — unknown wildcards stay literal for later expansion; the
    // execution-time residual guard covers engine-known names).
    let dir = tempfile::tempdir().unwrap();
    let workflow_path = dir.path().join("unknown.oxoflow");
    std::fs::write(
        &workflow_path,
        r#"
        [workflow]
        name = "unknown"

        [[rules]]
        name = "split"
        output_pattern = "results/chunks/{part}.txt"
        shell = "mkdir -p results/chunks && echo x > results/chunks/a.txt"

        [[rules]]
        name = "collect"
        input = ["results/chunks/{nope}.txt"]
        output = ["results/merged/{nope}.txt"]
        shell = "cat {input} > {output}"
        "#,
    )
    .unwrap();
    let mut config = WorkflowConfig::from_file(&workflow_path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    // Not deferred: the rule stays in the plan as a regular rule.
    assert!(config.get_rule("collect").is_some());
    assert!(config.output_pattern_producer_of("collect").is_none());
    assert!(
        config
            .pending_output_pattern_consumers_of("split")
            .is_empty()
    );
}
