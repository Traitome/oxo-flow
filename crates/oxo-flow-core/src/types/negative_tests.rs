//! Negative truth set for the BioType compatibility lattice (issue #1107).
//!
//! A table-driven set of `(workflow, expected code, should-accept)` rows that
//! drives BOTH admission surfaces — the full finder [`check_workflow_types`]
//! and the filtered gate [`validate_workflow_types`] — so the two can never
//! disagree about what a workflow declares. Covers TYPE-001..TYPE-007 in both
//! directions plus the advisory TYPE-008/TYPE-009/TYPE-010 unverifiable-edge
//! codes, and pins the deliberate compatibility bypasses as explicit
//! assertions.

use super::checker::{check_workflow_types, validate_workflow_types};
use super::inspect::IntrospectedBioType;
use super::*;
use crate::config::WorkflowConfig;
use crate::dag::WorkflowDag;

/// One negative-truth-set row.
///
/// `expect_code` is the diagnostic that [`check_workflow_types`] must surface
/// (`None` = the workflow is fully typed and must yield zero diagnostics).
/// `accept` is what [`validate_workflow_types`] must return: `false` for a
/// proven violation, `true` for a fully-satisfied workflow *and* for advisory
/// unverifiable edges (TYPE-008/TYPE-009/TYPE-010), which are reported but
/// non-fatal.
struct Case {
    name: &'static str,
    types_block: &'static str,
    producer_ref: Option<&'static str>,
    consumer_ref: Option<&'static str>,
    expect_code: Option<&'static str>,
    accept: bool,
}

/// Read-group contract fixtures shared by the TYPE-005 rows. Kept here (not
/// inline) because `read_groups` is a nested `[types.<name>.read_groups]`
/// table that the inline `FORMAT[key=value]` syntax cannot express.
const RG_TYPES: &str = "\
[types.rg_required]
format = \"bam\"

[types.rg_required.read_groups]
required = true
fields = [\"ID\", \"SM\"]

[types.rg_full]
format = \"bam\"

[types.rg_full.read_groups]
required = true
fields = [\"ID\", \"SM\", \"PL\"]
";

const CASES: &[Case] = &[
    // --- TYPE-001 format, both directions + the accept direction --------------
    Case {
        name: "type_001_format_reject_sam_to_bam",
        types_block: "",
        producer_ref: Some("SAM"),
        consumer_ref: Some("BAM"),
        expect_code: Some("TYPE-001"),
        accept: false,
    },
    Case {
        name: "type_001_format_reject_bam_to_sam",
        types_block: "",
        producer_ref: Some("BAM"),
        consumer_ref: Some("SAM"),
        expect_code: Some("TYPE-001"),
        accept: false,
    },
    Case {
        name: "type_001_format_accept_same",
        types_block: "",
        producer_ref: Some("BAM"),
        consumer_ref: Some("BAM"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-002 sort order --------------------------------------------------
    Case {
        name: "type_002_sort_reject_queryname_to_coordinate",
        types_block: "",
        producer_ref: Some("BAM[sort_order=queryname]"),
        consumer_ref: Some("BAM[sort_order=coordinate]"),
        expect_code: Some("TYPE-002"),
        accept: false,
    },
    Case {
        name: "type_002_sort_accept_coordinate",
        types_block: "",
        producer_ref: Some("BAM[sort_order=coordinate]"),
        consumer_ref: Some("BAM[sort_order=coordinate]"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-003 companion index --------------------------------------------
    Case {
        name: "type_003_index_reject",
        types_block: "",
        producer_ref: Some("BAM[sort_order=coordinate]"),
        consumer_ref: Some("BAM[sort_order=coordinate,index=bai]"),
        expect_code: Some("TYPE-003"),
        accept: false,
    },
    Case {
        name: "type_003_index_accept",
        types_block: "",
        producer_ref: Some("BAM[sort_order=coordinate,index=bai]"),
        consumer_ref: Some("BAM[sort_order=coordinate,index=bai]"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-004 reference build --------------------------------------------
    Case {
        name: "type_004_build_reject_hg19_to_grch38",
        types_block: "",
        producer_ref: Some("BAM[reference_build=hg19]"),
        consumer_ref: Some("BAM[reference_build=GRCh38]"),
        expect_code: Some("TYPE-004"),
        accept: false,
    },
    Case {
        name: "type_004_build_accept_alias",
        types_block: "",
        producer_ref: Some("BAM[reference_build=hg38]"),
        consumer_ref: Some("BAM[reference_build=GRCh38]"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-005 read groups -------------------------------------------------
    Case {
        name: "type_005_read_group_reject",
        types_block: RG_TYPES,
        producer_ref: Some("BAM"),
        consumer_ref: Some("rg_required"),
        expect_code: Some("TYPE-005"),
        accept: false,
    },
    Case {
        name: "type_005_read_group_accept_superset",
        types_block: RG_TYPES,
        producer_ref: Some("rg_full"),
        consumer_ref: Some("rg_required"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-006 compression, both directions -------------------------------
    Case {
        name: "type_006_compression_reject_gzip_to_bgzip",
        types_block: "",
        producer_ref: Some("VCF[compression=gzip]"),
        consumer_ref: Some("VCF[compression=bgzip]"),
        expect_code: Some("TYPE-006"),
        accept: false,
    },
    Case {
        name: "type_006_compression_accept_bgzip_to_gzip",
        types_block: "",
        producer_ref: Some("VCF[compression=bgzip]"),
        consumer_ref: Some("VCF[compression=gzip]"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-007 attributes --------------------------------------------------
    Case {
        name: "type_007_attribute_reject_somatic_to_germline",
        types_block: "",
        producer_ref: Some("VCF[variant_type=somatic]"),
        consumer_ref: Some("VCF[variant_type=germline]"),
        expect_code: Some("TYPE-007"),
        accept: false,
    },
    Case {
        name: "type_007_attribute_accept",
        types_block: "",
        producer_ref: Some("VCF[variant_type=germline]"),
        consumer_ref: Some("VCF[variant_type=germline]"),
        expect_code: None,
        accept: true,
    },
    // --- TYPE-008 typed consumer, untyped producer (advisory) -----------------
    Case {
        name: "type_008_typed_consumer_untyped_producer",
        types_block: "",
        producer_ref: None,
        consumer_ref: Some("BAM"),
        expect_code: Some("TYPE-008"),
        accept: true,
    },
    // --- TYPE-009 build-incomplete pair (advisory) ---------------------------
    Case {
        name: "type_009_consumer_names_build_producer_omits",
        types_block: "",
        producer_ref: Some("BAM"),
        consumer_ref: Some("BAM[reference_build=GRCh38]"),
        expect_code: Some("TYPE-009"),
        accept: true,
    },
];

/// Render a two-rule workflow: `producer` emits `stage.bam`, `consumer` reads
/// it. Each side's `[rules.types]` block is emitted only when a type reference
/// is supplied, so an untyped producer is expressible (TYPE-008).
fn wf(types_block: &str, producer_ref: Option<&str>, consumer_ref: Option<&str>) -> String {
    let mut s = String::from("[workflow]\nname = \"negative\"\n");
    if !types_block.is_empty() {
        s.push('\n');
        s.push_str(types_block);
        if !types_block.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push_str("\n[[rules]]\nname = \"producer\"\noutput = [\"stage.bam\"]\nshell = \"echo\"\n");
    if let Some(t) = producer_ref {
        s.push_str("[rules.types]\n");
        s.push_str(&format!("outputs = {{ default = \"{t}\" }}\n"));
    }
    s.push_str(
        "\n[[rules]]\nname = \"consumer\"\ninput = [\"stage.bam\"]\noutput = [\"final.vcf\"]\nshell = \"echo\"\n",
    );
    if let Some(t) = consumer_ref {
        s.push_str("[rules.types]\n");
        s.push_str(&format!("inputs = {{ default = \"{t}\" }}\n"));
    }
    s
}

/// Parse a row's TOML into a config + DAG, failing the test on any parse error.
fn compile(case: &Case) -> (WorkflowConfig, WorkflowDag) {
    let toml = wf(case.types_block, case.producer_ref, case.consumer_ref);
    let config = WorkflowConfig::parse(&toml)
        .unwrap_or_else(|e| panic!("case '{}' must parse: {e}\n{toml}", case.name));
    let dag = WorkflowDag::from_rules(&config.rules)
        .unwrap_or_else(|e| panic!("case '{}' must build a DAG: {e}", case.name));
    (config, dag)
}

/// Parse raw TOML into a config + DAG, failing the test on any parse error.
fn compile_toml(toml: &str) -> (WorkflowConfig, WorkflowDag) {
    let config = WorkflowConfig::parse(toml).unwrap_or_else(|e| panic!("must parse: {e}\n{toml}"));
    let dag =
        WorkflowDag::from_rules(&config.rules).unwrap_or_else(|e| panic!("must build a DAG: {e}"));
    (config, dag)
}

/// Render a two-rule workflow whose typed contracts may carry MULTIPLE channels
/// per side — the single-channel `wf()` helper above cannot express the
/// multi-channel / `default`-keyed edges of truth-set cases a08/a09/a11.
///
/// `producer_outputs` / `consumer_inputs` are the literal `[rules.types]` map
/// bodies (e.g. `"a.bam" = "BAM", "b.vcf" = "VCF"`); the path lists are the raw
/// `output` / `input` file globs the DAG wires on.
fn multichannel_wf(
    producer_paths: &[&str],
    producer_outputs: &str,
    consumer_paths: &[&str],
    consumer_inputs: &str,
) -> String {
    let quote = |paths: &[&str]| {
        paths
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "[workflow]\nname = \"multichannel\"\n\n\
         [[rules]]\nname = \"make\"\noutput = [{prod_paths}]\nshell = \"echo\"\n\
         [rules.types]\noutputs = {{ {producer_outputs} }}\n\n\
         [[rules]]\nname = \"use\"\ninput = [{cons_paths}]\noutput = [\"result.txt\"]\nshell = \"echo\"\n\
         [rules.types]\ninputs = {{ {consumer_inputs} }}\n",
        prod_paths = quote(producer_paths),
        cons_paths = quote(consumer_paths),
    )
}

// --- Issue #1254: multi-channel pairing silent-skip -------------------------

/// Truth-set a08: a multi-output producer typed by RAW PATH (`a.bam`→BAM,
/// `b.vcf`→VCF) feeding a consumer whose single input is keyed `default`.
/// Neither channel name pairs, so before #1254 the typed edge produced NO
/// diagnostic at all — a silent skip (false-accept). The fix must surface
/// exactly one advisory instead of silence.
#[test]
fn issue_1254_unpaired_default_consumer_reports_advisory() {
    let toml = multichannel_wf(
        &["a.bam", "b.vcf"],
        "\"a.bam\" = \"BAM\", \"b.vcf\" = \"VCF\"",
        &["a.bam"],
        "default = \"BAM[sort_order=coordinate]\"",
    );
    let (config, dag) = compile_toml(&toml);
    let diagnostics = check_workflow_types(&config, &dag);
    let advisories: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.violation.code == "TYPE-010")
        .collect();
    assert_eq!(
        advisories.len(),
        1,
        "an unpaired typed edge must surface exactly one TYPE-010 advisory, not silence: {diagnostics:?}"
    );
    let adv = advisories[0];
    assert!(
        adv.producer_channel.contains("a.bam") && adv.producer_channel.contains("b.vcf"),
        "advisory must name the producer's unpaired channels: {adv:?}"
    );
    assert!(
        adv.consumer_channel.contains("default"),
        "advisory must name the consumer's unpaired channel: {adv:?}"
    );
    // Advisory codes are non-fatal: the admission gate still accepts the edge.
    assert!(
        validate_workflow_types(&config, &dag).is_ok(),
        "TYPE-010 is advisory and must not fail the gate"
    );
}

/// Truth-set a11: same root cause, different surface — the producer types its
/// two channels by NAME (`aligned`, `calls`) and the consumer's single input is
/// keyed `reads`. No pair forms → advisory, not silence.
#[test]
fn issue_1254_unpaired_named_consumer_reports_advisory() {
    let toml = multichannel_wf(
        &["a.bam", "b.vcf"],
        "aligned = \"BAM\", calls = \"VCF\"",
        &["a.bam"],
        "reads = \"BAM[sort_order=coordinate]\"",
    );
    let (config, dag) = compile_toml(&toml);
    let diagnostics = check_workflow_types(&config, &dag);
    assert!(
        diagnostics.iter().any(|d| d.violation.code == "TYPE-010"),
        "unpaired named edge must surface the TYPE-010 advisory: {diagnostics:?}"
    );
    assert!(validate_workflow_types(&config, &dag).is_ok());
}

/// Truth-set a09 CONTROL: identical producer to a08, but the consumer keys its
/// input by the shared raw path `a.bam`, so `out_ch == in_ch` pairs and the
/// concrete violation fires. Pins that the #1254 fallback adds NO advisory
/// noise (nor a spurious extra TYPE-002) when a pair DOES form.
#[test]
fn issue_1254_paired_edge_yields_one_type_002_and_no_advisory() {
    let toml = multichannel_wf(
        &["a.bam", "b.vcf"],
        "\"a.bam\" = \"BAM\", \"b.vcf\" = \"VCF\"",
        &["a.bam"],
        "\"a.bam\" = \"BAM[sort_order=coordinate]\"",
    );
    let (config, dag) = compile_toml(&toml);
    let diagnostics = check_workflow_types(&config, &dag);
    let type_002: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.violation.code == "TYPE-002")
        .collect();
    assert_eq!(
        type_002.len(),
        1,
        "exactly one TYPE-002 for the single paired edge: {diagnostics:?}"
    );
    assert!(
        !diagnostics.iter().any(|d| d.violation.code == "TYPE-010"),
        "no advisory may fire when a channel pair forms: {diagnostics:?}"
    );
    assert!(validate_workflow_types(&config, &dag).is_err());
}

// --- Issue #1255: format-implied compression --------------------------------

/// Truth-set a12: BAM is BY SPECIFICATION a BGZF stream, so a plain `BAM`
/// producer already satisfies a consumer's `compression=bgzip` requirement.
/// Before #1255 this was a false-reject (TYPE-006).
#[test]
fn issue_1255_plain_bam_satisfies_bgzip_requirement() {
    let producer = BioTypeSpec::new(BioFormat::Bam);
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        compression: Some(CompressionKind::Bgzip),
        ..Default::default()
    };
    assert!(
        producer.is_subtype_of(&consumer).is_ok(),
        "a plain BAM already IS bgzip; requiring bgzip must not reject"
    );
}

/// Format-invariance guard: FASTQ and VCF carry NO implied stream compression,
/// so they must still require the explicit expectation (t04 stays a reject).
#[test]
fn issue_1255_fastq_and_vcf_get_no_implied_compression() {
    for format in [BioFormat::Fastq, BioFormat::Vcf] {
        let producer = BioTypeSpec::new(format);
        let consumer = BioTypeSpec {
            format,
            compression: Some(CompressionKind::Bgzip),
            ..Default::default()
        };
        let err = producer
            .is_subtype_of(&consumer)
            .expect_err("only BAM/CRAM imply bgzip");
        assert!(
            err.iter().any(|v| v.code == "TYPE-006"),
            "{format} must reject a bgzip requirement: {err:?}"
        );
    }
}

/// Genuine-mismatch guard: the #1255 short-circuit must not weaken a real
/// compression conflict — BAM against an explicit `compression=zstd` still
/// rejects.
#[test]
fn issue_1255_bam_versus_zstd_still_rejects() {
    let producer = BioTypeSpec::new(BioFormat::Bam);
    let consumer = BioTypeSpec {
        format: BioFormat::Bam,
        compression: Some(CompressionKind::Zstd),
        ..Default::default()
    };
    let err = producer
        .is_subtype_of(&consumer)
        .expect_err("BAM is bgzip, not zstd");
    assert!(
        err.iter().any(|v| v.code == "TYPE-006"),
        "BAM vs zstd must still be TYPE-006: {err:?}"
    );
}

/// Truth-set a12 end-to-end through the DAG checker (not just the spec lattice).
#[test]
fn issue_1255_truthset_a12_edge_accepts_end_to_end() {
    let toml = "\
[workflow]\nname = \"a12\"\n\n\
[[rules]]\nname = \"align\"\noutput = [\"a.bam\"]\nshell = \"samtools sort -o a.bam in.sam\"\n\
[rules.types]\noutputs = { default = \"BAM\" }\n\n\
[[rules]]\nname = \"index\"\ninput = [\"a.bam\"]\noutput = [\"out.txt\"]\nshell = \"samtools index a.bam\"\n\
[rules.types]\ninputs = { default = \"BAM[compression=bgzip]\" }\n";
    let (config, dag) = compile_toml(toml);
    let diagnostics = check_workflow_types(&config, &dag);
    assert!(diagnostics.is_empty(), "a12 must accept: {diagnostics:?}");
    assert!(validate_workflow_types(&config, &dag).is_ok());
}

#[test]
fn negative_truth_set_drives_both_admission_surfaces() {
    for case in CASES {
        let (config, dag) = compile(case);
        let diagnostics = check_workflow_types(&config, &dag);
        let codes: Vec<&str> = diagnostics
            .iter()
            .map(|d| d.violation.code.as_str())
            .collect();

        match case.expect_code {
            Some(code) => assert!(
                codes.contains(&code),
                "case '{}' must surface {code}: {diagnostics:?}",
                case.name
            ),
            None => assert!(
                diagnostics.is_empty(),
                "case '{}' is fully typed and must yield no diagnostics: {diagnostics:?}",
                case.name
            ),
        }

        let accepted = validate_workflow_types(&config, &dag).is_ok();
        assert_eq!(
            accepted, case.accept,
            "case '{}' validate_workflow_types acceptance mismatch: {diagnostics:?}",
            case.name
        );
    }
}

#[test]
fn negative_truth_set_covers_every_fatal_code() {
    let rejected: Vec<&str> = CASES
        .iter()
        .filter(|c| !c.accept)
        .filter_map(|c| c.expect_code)
        .collect();
    for code in [
        "TYPE-001", "TYPE-002", "TYPE-003", "TYPE-004", "TYPE-005", "TYPE-006", "TYPE-007",
    ] {
        assert!(
            rejected.contains(&code),
            "negative truth set must exercise a rejection for {code}"
        );
    }
    // Advisory unverifiable-edge codes are still *emitted* (they appear as the
    // expected code) even though `validate_workflow_types` accepts them.
    let reported: Vec<&str> = CASES.iter().filter_map(|c| c.expect_code).collect();
    assert!(reported.contains(&"TYPE-008"));
    assert!(reported.contains(&"TYPE-009"));
    // And every fatal code has a matching accept direction.
    let accepted = CASES.iter().filter(|c| c.accept).count();
    assert!(
        accepted >= 7,
        "expected an accept-direction row for each contract dimension, found {accepted}"
    );
}

#[test]
fn fully_typed_workflow_yields_zero_diagnostics() {
    // Over-flagging guard: a workflow where every edge is fully typed and
    // mutually satisfiable must produce no TypeDiagnostic at all (the new
    // TYPE-008/009 findings must never fire on a complete contract).
    let toml = r#"
        [workflow]
        name = "fully_typed"

        [types.raw_bam]
        format = "bam"
        sort_order = "coordinate"
        reference_build = "GRCh38"
        compression = "bgzip"

        [types.calls_vcf]
        format = "vcf"
        compression = "bgzip"

        [[rules]]
        name = "align"
        output = ["aligned.bam"]
        shell = "echo"
        [rules.types]
        outputs = { default = "raw_bam" }

        [[rules]]
        name = "call"
        input = ["aligned.bam"]
        output = ["calls.vcf.gz"]
        shell = "echo"
        [rules.types]
        inputs = { default = "raw_bam" }
        outputs = { default = "calls_vcf" }
    "#;
    let config = WorkflowConfig::parse(toml).expect("valid fully typed workflow");
    let dag = WorkflowDag::from_rules(&config.rules).expect("valid dag");
    let diagnostics = check_workflow_types(&config, &dag);
    assert!(
        diagnostics.is_empty(),
        "fully typed workflow must not be flagged: {diagnostics:?}"
    );
    assert!(validate_workflow_types(&config, &dag).is_ok());
}

#[test]
fn pin_compatibility_bypasses() {
    // BioFormat::Generic on the *expected* side short-circuits TYPE-001: any
    // producer format satisfies a Generic consumer requirement.
    assert!(
        BioTypeSpec::new(BioFormat::Bam)
            .is_subtype_of(&BioTypeSpec::new(BioFormat::Generic))
            .is_ok()
    );
    assert!(
        BioTypeSpec::new(BioFormat::Vcf)
            .is_subtype_of(&BioTypeSpec::new(BioFormat::Generic))
            .is_ok()
    );
    // ...but a concrete expected format still rejects a Generic producer.
    assert!(
        BioTypeSpec::new(BioFormat::Generic)
            .is_subtype_of(&BioTypeSpec::new(BioFormat::Bam))
            .is_err()
    );

    // An expected sort order of Unknown (or Unsorted) is a don't-care: it never
    // fires TYPE-002 regardless of the producer's sort order.
    for expected_order in [SortOrder::Unknown, SortOrder::Unsorted] {
        let expected = BioTypeSpec {
            format: BioFormat::Bam,
            sort_order: Some(expected_order),
            ..Default::default()
        };
        assert!(
            BioTypeSpec::new(BioFormat::Bam)
                .is_subtype_of(&expected)
                .is_ok(),
            "{expected_order} must be a don't-care expectation"
        );
    }
    // SortOrder::Unknown as an *observed* order satisfies an Unknown/Unsorted
    // expectation, but NOT a concrete Coordinate/Queryname requirement.
    assert!(SortOrder::Unknown.satisfies(SortOrder::Unsorted));
    assert!(SortOrder::Unknown.satisfies(SortOrder::Unknown));
    assert!(!SortOrder::Unknown.satisfies(SortOrder::Coordinate));
    assert!(!SortOrder::Unknown.satisfies(SortOrder::Queryname));

    // BGZF is valid GZIP; standard GZIP does not satisfy a BGZF requirement.
    assert!(CompressionKind::Bgzip.satisfies(CompressionKind::Gzip));
    assert!(!CompressionKind::Gzip.satisfies(CompressionKind::Bgzip));
    assert!(CompressionKind::Gzip.satisfies(CompressionKind::Gzip));
    assert!(!CompressionKind::None.satisfies(CompressionKind::Gzip));
}

/// Intended read-modify projection of a header observation onto the declared
/// [`BioTypeSpec`] domain. Restricted to producers with no observed `@RG`
/// records, where the projection is exact and independently expressible; the
/// read-group collapse is exercised by the TYPE-005 rows and `inspect_tests`.
fn observed_spec(observed: &IntrospectedBioType, expected: &BioTypeSpec) -> BioTypeSpec {
    BioTypeSpec {
        format: observed.format,
        sort_order: observed.sort_order,
        index: expected.index,
        reference_build: observed.reference_build.clone(),
        compression: Some(observed.compression),
        read_groups: None,
        attributes: expected.attributes.clone(),
    }
}

/// Expected specs spanning every compatibility dimension (format, sort,
/// index, build, compression, read groups, attributes) plus the Generic
/// short-circuit, so the matrix stresses each branch of the lattice.
fn expected_specs() -> Vec<BioTypeSpec> {
    vec![
        BioTypeSpec::new(BioFormat::Bam),
        BioTypeSpec::new(BioFormat::Generic),
        BioTypeSpec {
            format: BioFormat::Bam,
            sort_order: Some(SortOrder::Coordinate),
            ..Default::default()
        },
        BioTypeSpec {
            format: BioFormat::Bam,
            index: Some(IndexKind::Bai),
            ..Default::default()
        },
        BioTypeSpec {
            format: BioFormat::Bam,
            reference_build: Some("GRCh38".to_string()),
            ..Default::default()
        },
        BioTypeSpec {
            format: BioFormat::Vcf,
            compression: Some(CompressionKind::Bgzip),
            ..Default::default()
        },
        BioTypeSpec {
            format: BioFormat::Bam,
            read_groups: Some(ReadGroupContract {
                required: true,
                fields: vec!["ID".to_string(), "SM".to_string()],
            }),
            ..Default::default()
        },
        BioTypeSpec {
            format: BioFormat::Vcf,
            attributes: [("variant_type".to_string(), "germline".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    ]
}

#[test]
fn satisfies_spec_and_is_subtype_of_agree_on_every_pair() {
    // Issue #1107 item 3: a single compatibility lattice must govern both the
    // intra-DAG checker and the header introspector. `satisfies_spec` now
    // delegates to `is_subtype_of`, and this matrix pins that they accept and
    // reject identically across every observable dimension.
    let formats = [
        BioFormat::Bam,
        BioFormat::Sam,
        BioFormat::Vcf,
        BioFormat::Generic,
    ];
    let sorts = [
        None,
        Some(SortOrder::Unsorted),
        Some(SortOrder::Coordinate),
        Some(SortOrder::Queryname),
        Some(SortOrder::Unknown),
    ];
    let compressions = [
        CompressionKind::None,
        CompressionKind::Gzip,
        CompressionKind::Bgzip,
    ];
    let builds = [None, Some("hg19".to_string()), Some("GRCh38".to_string())];
    let expected = expected_specs();

    let mut pairs = 0usize;
    for &format in &formats {
        for &sort_order in &sorts {
            for &compression in &compressions {
                for build in &builds {
                    let observed = IntrospectedBioType {
                        format,
                        compression,
                        sort_order,
                        reference_build: build.clone(),
                        ..Default::default()
                    };
                    for spec in &expected {
                        let via_introspection = observed.satisfies_spec(spec).is_ok();
                        let via_spec = observed_spec(&observed, spec).is_subtype_of(spec).is_ok();
                        assert_eq!(
                            via_introspection, via_spec,
                            "satisfies_spec/is_subtype_of disagree for observed={observed:?} expected={spec:?}"
                        );
                        pairs += 1;
                    }
                }
            }
        }
    }
    assert_eq!(pairs, 4 * 5 * 3 * 3 * expected_specs().len());
}
