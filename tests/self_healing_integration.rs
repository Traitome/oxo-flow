//! Integration tests for the four-layer causal fault isolation and self-healing runtime.

use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::diagnostics::{
    DiagnosticContext, DiagnosticsEngine, ErrorCategory, RepairAction,
};
use oxo_flow_core::types::checker::check_workflow_types;

#[test]
fn test_causal_fault_oom_scale_resources() {
    let engine = DiagnosticsEngine::global();
    let log = "STAR exiting: FATAL error, OUT OF MEMORY\nEXITING because of FATAL ERROR: 137";
    let ctx = DiagnosticContext::new("star_align", log, Some(137))
        .with_signal(9)
        .with_telemetry(Some(32768), Some(120.5), Some(125000));

    let results = engine.analyze_context(&ctx);
    assert!(!results.is_empty(), "OOM context must produce a diagnostic");
    let diag = &results[0];
    assert_eq!(diag.category, ErrorCategory::Resource);
    assert_eq!(diag.error_pattern.as_deref(), Some("oom_killed"));
    assert!(diag.auto_fixable);
    assert_eq!(
        diag.repair_action,
        Some(RepairAction::ScaleResources {
            memory_multiplier: 2.0,
            time_multiplier: 1.0,
        })
    );
}

#[test]
fn test_causal_fault_timeout_scale_resources() {
    let engine = DiagnosticsEngine::global();
    let log = "Command timed out after 7200 seconds";
    let ctx = DiagnosticContext::new("bwa_mem", log, Some(124)).with_signal(24);

    let results = engine.analyze_context(&ctx);
    assert!(
        !results.is_empty(),
        "Timeout context must produce a diagnostic"
    );
    let diag = &results[0];
    assert_eq!(diag.category, ErrorCategory::Resource);
    assert_eq!(diag.error_pattern.as_deref(), Some("timeout"));
    assert_eq!(
        diag.repair_action,
        Some(RepairAction::ScaleResources {
            memory_multiplier: 1.0,
            time_multiplier: 2.0,
        })
    );
}

#[test]
fn test_causal_fault_disk_pressure_reroute_scratch() {
    let engine = DiagnosticsEngine::global();
    let log = "OSError: [Errno 28] No space left on device while writing intermediate matrix";
    let ctx = DiagnosticContext::new("feature_counts", log, Some(1));

    let results = engine.analyze_context(&ctx);
    assert!(!results.is_empty());
    let diag = &results[0];
    assert_eq!(diag.category, ErrorCategory::Resource);
    assert_eq!(diag.error_pattern.as_deref(), Some("disk_full"));
    assert!(matches!(
        diag.repair_action,
        Some(RepairAction::RerouteScratch { .. })
    ));
}

#[test]
fn test_causal_fault_bio_contract_mismatch_bridge_repair() {
    let engine = DiagnosticsEngine::global();
    let ctx = DiagnosticContext::new("call_variants", "reading bam...", Some(1))
        .with_header_mismatch("BAM sort order is queryname, required coordinate");

    let results = engine.analyze_context(&ctx);
    assert!(!results.is_empty());
    let diag = &results[0];
    assert_eq!(diag.category, ErrorCategory::Data);
    assert_eq!(
        diag.error_pattern.as_deref(),
        Some("bio_contract_violation")
    );

    match &diag.repair_action {
        Some(RepairAction::InsertBridgeRule {
            rule_name,
            command,
            input,
            output,
        }) => {
            assert!(rule_name.contains("sort_bridge"));
            assert!(command.contains("samtools sort"));
            assert_eq!(input[0], "{input}");
            assert_eq!(output[0], "{output}.sorted.bam");
        }
        other => panic!("expected InsertBridgeRule, got {other:?}"),
    }
}

#[test]
fn test_repair_action_dag_integration_and_type_check() {
    let initial_workflow = r#"
[workflow]
name = "variant_pipeline"
version = "0.1.0"

[types.bam_coord]
format = "bam"
sort_order = "coordinate"
index = "bai"

[[rules]]
name = "align"
input = ["data/{sample}.fastq.gz"]
output = ["aligned/{sample}.bam"]
shell = "bwa mem -t 4 ref.fa {input} > {output}"
types = { outputs = { "aligned/{sample}.bam" = "bam_coord" } }

[[rules]]
name = "call"
input = ["aligned/{sample}.bam"]
output = ["variants/{sample}.vcf.gz"]
shell = "freebayes -f ref.fa {input} > {output}"
types = { inputs = { "aligned/{sample}.bam" = "bam_coord" } }
    "#;

    let config = WorkflowConfig::parse(initial_workflow).expect("initial workflow must parse");
    let dag = WorkflowDag::from_rules(&config.rules).expect("dag constructs");
    let type_diags = check_workflow_types(&config, &dag);
    assert!(type_diags.is_empty(), "initial workflow has valid types");
    assert_eq!(dag.node_count(), 2);
    assert_eq!(dag.edge_count(), 1);
}
