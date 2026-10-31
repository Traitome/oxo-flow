//! Unit and integration tests for BioTypes semantic biological typing.

use super::checker::check_workflow_types;
use super::*;
use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;

#[test]
fn test_bio_format_display_and_parse() {
    assert_eq!(BioFormat::Bam.as_str(), "bam");
    assert_eq!("BAM".parse::<BioFormat>().unwrap(), BioFormat::Bam);
    assert_eq!("fastq".parse::<BioFormat>().unwrap(), BioFormat::Fastq);
    assert_eq!("fq".parse::<BioFormat>().unwrap(), BioFormat::Fastq);
    assert_eq!("vcf".parse::<BioFormat>().unwrap(), BioFormat::Vcf);
    assert_eq!("cram".parse::<BioFormat>().unwrap(), BioFormat::Cram);
    assert_eq!("h5ad".parse::<BioFormat>().unwrap(), BioFormat::H5ad);
}

#[test]
fn test_sort_order_satisfaction() {
    assert!(SortOrder::Coordinate.satisfies(SortOrder::Coordinate));
    assert!(SortOrder::Coordinate.satisfies(SortOrder::Unsorted));
    assert!(SortOrder::Queryname.satisfies(SortOrder::Queryname));
    assert!(!SortOrder::Queryname.satisfies(SortOrder::Coordinate));
    assert!(!SortOrder::Unsorted.satisfies(SortOrder::Coordinate));
}

#[test]
fn test_compression_satisfaction() {
    assert!(CompressionKind::Bgzip.satisfies(CompressionKind::Gzip));
    assert!(CompressionKind::Bgzip.satisfies(CompressionKind::Bgzip));
    assert!(!CompressionKind::Gzip.satisfies(CompressionKind::Bgzip));
    assert!(!CompressionKind::None.satisfies(CompressionKind::Gzip));
}

#[test]
fn test_reference_build_normalization() {
    assert_eq!(normalize_reference_build("hg38"), "GRCh38");
    assert_eq!(normalize_reference_build("grch38"), "GRCh38");
    assert_eq!(normalize_reference_build("GRCh38"), "GRCh38");
    assert_eq!(normalize_reference_build("hg19"), "GRCh37");
    assert_eq!(normalize_reference_build("b37"), "GRCh37");
    assert_eq!(normalize_reference_build("t2t"), "T2T-CHM13");
}

#[test]
fn test_type_001_format_mismatch() {
    let producer = BioTypeSpec::new(BioFormat::Sam);
    let consumer = BioTypeSpec::new(BioFormat::Bam);

    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-001");
    assert!(
        err[0]
            .coercion_rule
            .as_ref()
            .unwrap()
            .contains("samtools view -b")
    );
}

#[test]
fn test_type_002_sort_order_mismatch() {
    let producer = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Queryname),
        ..Default::default()
    };
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Coordinate),
        ..Default::default()
    };

    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-002");
    assert!(
        err[0]
            .coercion_rule
            .as_ref()
            .unwrap()
            .contains("samtools sort")
    );
}

#[test]
fn test_type_003_missing_index() {
    let producer = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Coordinate),
        index: None,
        ..Default::default()
    };
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        sort_order: Some(SortOrder::Coordinate),
        index: Some(IndexKind::Bai),
        ..Default::default()
    };

    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-003");
    assert!(
        err[0]
            .coercion_rule
            .as_ref()
            .unwrap()
            .contains("samtools index")
    );
}

#[test]
fn test_type_004_reference_build_conflict() {
    let producer = BioTypeSpec {
        format: BioFormat::Bam,
        reference_build: Some("hg19".to_string()),
        ..Default::default()
    };
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        reference_build: Some("GRCh38".to_string()),
        ..Default::default()
    };
    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-004");
    assert!(err[0].message.contains("GRCh37"));
    assert!(err[0].message.contains("GRCh38"));

    // Aliases normalize to same build and do not conflict
    let producer_alias = BioTypeSpec {
        format: BioFormat::Bam,
        reference_build: Some("hg38".to_string()),
        ..Default::default()
    };
    assert!(producer_alias.is_subtype_of(&consumer).is_ok());
}

#[test]
fn test_type_005_read_groups_contract() {
    let producer = BioTypeSpec {
        format: BioFormat::Bam,
        read_groups: None,
        ..Default::default()
    };
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        read_groups: Some(ReadGroupContract {
            required: true,
            fields: vec!["ID".to_string(), "SM".to_string()],
        }),
        ..Default::default()
    };

    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-005");
    assert!(
        err[0]
            .coercion_rule
            .as_ref()
            .unwrap()
            .contains("AddOrReplaceReadGroups")
    );
}

#[test]
fn test_type_006_compression_mismatch() {
    let producer = BioTypeSpec {
        format: BioFormat::Vcf,
        compression: Some(CompressionKind::Gzip),
        ..Default::default()
    };
    let consumer = BioTypeSpec {
        format: BioFormat::Vcf,
        compression: Some(CompressionKind::Bgzip),
        ..Default::default()
    };

    let err = producer.is_subtype_of(&consumer).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "TYPE-006");
    assert!(err[0].coercion_rule.as_ref().unwrap().contains("bgzip"));
}

#[test]
fn test_dag_type_checking_pipeline() {
    let toml = r#"
        [workflow]
        name = "test_types"

        [types.raw_sam]
        format = "sam"
        sort_order = "unsorted"

        [types.sorted_bam]
        format = "bam"
        sort_order = "coordinate"
        index = "bai"

        [[rules]]
        name = "align"
        output = ["out.sam"]
        shell = "bwa mem ref.fa reads.fq > out.sam"
        [rules.types]
        outputs = { default = "raw_sam" }

        [[rules]]
        name = "call_variants"
        input = ["out.sam"]
        output = ["calls.vcf"]
        shell = "gatk HaplotypeCaller -I out.sam -O calls.vcf"
        [rules.types]
        inputs = { default = "sorted_bam" }
    "#;

    let config = WorkflowConfig::parse(toml).expect("valid workflow config");
    let dag = WorkflowDag::from_rules(&config.rules).expect("valid dag");
    let diagnostics = check_workflow_types(&config, &dag);

    assert!(!diagnostics.is_empty());
    let codes: Vec<&str> = diagnostics
        .iter()
        .map(|d| d.violation.code.as_str())
        .collect();
    assert!(
        codes.contains(&"TYPE-001"),
        "must detect SAM -> BAM format mismatch"
    );
    assert!(
        codes.contains(&"TYPE-002"),
        "must detect sort order mismatch"
    );
    assert!(codes.contains(&"TYPE-003"), "must detect missing BAI index");
}
