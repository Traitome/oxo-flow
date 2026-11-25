//! Unit tests for the metadata contract (issue #1368).

use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// `(source row, id, column/value pairs)` for a hand-built table.
type RowSpec<'a> = (Option<usize>, &'a str, &'a [(&'a str, &'a str)]);

fn rows_from(id_column: &str, data: &[RowSpec<'_>]) -> MetadataTable {
    let mut columns: BTreeSet<String> = BTreeSet::new();
    let rows: Vec<MetadataRow> = data
        .iter()
        .map(|(row, id, values)| {
            let values: BTreeMap<String, String> = values
                .iter()
                .map(|(column, value)| (column.to_string(), value.to_string()))
                .collect();
            columns.extend(values.keys().cloned());
            MetadataRow {
                row: *row,
                id: id.to_string(),
                values,
            }
        })
        .collect();
    MetadataTable {
        id_column: id_column.to_string(),
        columns: columns.into_iter().collect(),
        rows,
    }
}

fn config_with(table: MetadataTable, schema: Option<MetadataSchema>) -> WorkflowConfig {
    let mut config = WorkflowConfig::parse(
        r#"
            [workflow]
            name = "meta"
            "#,
    )
    .unwrap();
    for row in &table.rows {
        config
            .metadata
            .insert(row.id.clone(), row.values.clone().into_iter().collect());
    }
    config.workflow.metadata_schema = schema;
    config.metadata_table = Some(table);
    config.refresh_metadata_contract();
    config
}

fn codes(report: &MetadataReport) -> Vec<&str> {
    report.findings.iter().map(|f| f.code.as_str()).collect()
}

#[test]
fn declared_types_are_checked_and_empty_cells_are_absent_values() {
    let table = rows_from(
        "sample",
        &[
            (
                Some(1),
                "S1",
                &[("age", "42"), ("value", "1.5"), ("qc", "TRUE")],
            ),
            (
                Some(2),
                "S2",
                &[("age", "x"), ("value", "NaN"), ("qc", "maybe")],
            ),
            (Some(3), "S3", &[("age", ""), ("value", ""), ("qc", "")]),
        ],
    );
    let schema = MetadataSchema {
        columns: vec![
            crate::config::MetadataColumn {
                name: "age".into(),
                column_type: MetadataColumnType::Integer,
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "value".into(),
                column_type: MetadataColumnType::Float,
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "qc".into(),
                column_type: MetadataColumnType::Boolean,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let report = evaluate(&config_with(table, Some(schema)));
    // S2 has three type violations; S3's empty cells are skipped.
    let types: Vec<&MetadataFinding> = report
        .findings
        .iter()
        .filter(|f| f.code == "META-007")
        .collect();
    assert_eq!(types.len(), 3, "{:?}", report.findings);
    assert!(
        types
            .iter()
            .any(|f| f.column.as_deref() == Some("age") && f.row == Some(2))
    );
    assert!(types.iter().any(|f| f.column.as_deref() == Some("value")));
    assert!(types.iter().any(|f| f.column.as_deref() == Some("qc")));
}

#[test]
fn findings_are_sorted_by_code_row_column_id() {
    let table = rows_from(
        "sample",
        &[
            (Some(1), "S2", &[("batch", "x")]),
            (Some(2), "S1", &[("batch", "y")]),
            (Some(3), "S3", &[("batch", "x")]),
        ],
    );
    let schema = MetadataSchema {
        required_columns: vec!["missing".into()],
        columns: vec![crate::config::MetadataColumn {
            name: "batch".into(),
            unique: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let report = evaluate(&config_with(table, Some(schema)));
    assert_eq!(codes(&report), ["META-001", "META-004"]);
    assert_eq!(report.findings[0].row, None);
    assert_eq!(report.findings[1].row, Some(3));
    assert_eq!(report.findings[1].id.as_deref(), Some("S3"));
}

#[test]
fn duplicate_ids_do_not_repeat_other_checks_and_name_the_first_row() {
    let table = rows_from(
        "sample",
        &[
            (Some(1), "S1", &[("v", "a")]),
            (Some(2), "S1", &[("v", "b")]),
        ],
    );
    let report = evaluate(&config_with(table, None));
    assert_eq!(codes(&report), ["META-003"]);
    assert!(report.findings[0].message.contains("first at row 1"));
    assert_eq!(report.findings[0].row, Some(2));
}

#[test]
fn coverage_respects_the_declared_level_and_skips_without_a_domain() {
    let table = rows_from(
        "sample",
        &[
            (Some(1), "S1", &[("v", "a")]),
            (Some(2), "EXTRA", &[("v", "b")]),
        ],
    );
    let mut config = config_with(table.clone(), None);
    config.sample_groups.push(crate::config::SampleGroup {
        name: "g".into(),
        samples: vec!["S1".into(), "S2".into()],
        metadata: Default::default(),
    });
    let report = evaluate(&config);
    // coverage defaults to warn: S2 missing a row and EXTRA outside the domain.
    assert_eq!(codes(&report), ["META-010", "META-011"]);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.severity == MetadataSeverity::Warning)
    );

    let off = MetadataSchema {
        coverage: MetadataCoverage::Off,
        ..Default::default()
    };
    config.workflow.metadata_schema = Some(off);
    assert!(evaluate(&config).findings.is_empty());

    let strict = MetadataSchema {
        coverage: MetadataCoverage::Error,
        ..Default::default()
    };
    config.workflow.metadata_schema = Some(strict);
    let report = evaluate(&config);
    assert!(report.has_errors());
    assert_eq!(report.error_count, 2);

    // With no declared domain, a metadata lookup table is not a coverage
    // violation.
    config.sample_groups.clear();
    config.workflow.metadata_schema = Some(MetadataSchema {
        coverage: MetadataCoverage::Error,
        ..Default::default()
    });
    assert!(evaluate(&config).findings.is_empty());
}

#[test]
fn pair_conflicts_and_pair_member_coverage() {
    let table = rows_from("sample", &[(Some(1), "T1", &[("v", "a")])]);
    let mut config = config_with(table, None);
    config.pairs = vec![
        crate::config::ExperimentControlPair {
            pair_id: "P1".into(),
            experiment: "T1".into(),
            control: Some("T1".into()),
            experiment_type: None,
            metadata: Default::default(),
            when: None,
        },
        crate::config::ExperimentControlPair {
            pair_id: "P1".into(),
            experiment: "T2".into(),
            control: Some("N2".into()),
            experiment_type: None,
            metadata: Default::default(),
            when: None,
        },
    ];
    let report = evaluate(&config);
    // experiment==control, duplicate pair_id, and T2/N2 without rows
    // (coverage warn) — T1 has a row.
    assert_eq!(
        codes(&report),
        ["META-008", "META-008", "META-009", "META-009"],
        "{:?}",
        report.findings
    );
    let pair_findings: Vec<&MetadataFinding> = report
        .findings
        .iter()
        .filter(|f| f.code == "META-008")
        .collect();
    assert!(pair_findings[0].message.contains("experiment == control"));
    assert!(pair_findings[1].message.contains("duplicate pair_id"));
}

#[test]
fn enum_regex_and_extra_columns_report_the_offending_key() {
    let table = rows_from(
        "sample",
        &[
            (
                Some(1),
                "S1",
                &[("strand", "forward"), ("bam", "reads/a.bam"), ("junk", "1")],
            ),
            (
                Some(2),
                "S2",
                &[("strand", "sideways"), ("bam", "x.txt"), ("junk", "2")],
            ),
        ],
    );
    let schema = MetadataSchema {
        allow_extra_columns: false,
        columns: vec![
            crate::config::MetadataColumn {
                name: "strand".into(),
                enum_values: Some(vec!["forward".into(), "reverse".into()]),
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "bam".into(),
                regex: Some(r"^reads/.*\.bam$".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let report = evaluate(&config_with(table, Some(schema)));
    assert_eq!(
        codes(&report),
        ["META-005", "META-006", "META-012"],
        "{:?}",
        report.findings
    );
    assert!(report.findings[0].message.contains("sideways"));
    assert!(report.findings[0].message.contains("forward, reverse"));
    assert!(report.findings[1].message.contains("x.txt"));
    assert_eq!(report.findings[2].column.as_deref(), Some("junk"));
}

#[test]
fn missing_required_column_is_table_level_and_not_repeated_per_row() {
    let table = rows_from("sample", &[(Some(1), "S1", &[("v", "")])]);
    let schema = MetadataSchema {
        required_columns: vec!["absent".into(), "v".into()],
        ..Default::default()
    };
    let report = evaluate(&config_with(table, Some(schema)));
    assert_eq!(codes(&report), ["META-001", "META-002"]);
    assert_eq!(report.findings[0].row, None);
    assert_eq!(report.findings[0].column.as_deref(), Some("absent"));
    assert_eq!(report.findings[1].row, Some(1));
    assert_eq!(report.findings[1].column.as_deref(), Some("v"));
}

#[test]
fn schema_declaration_errors_name_the_offending_key() {
    let dup = MetadataSchema {
        columns: vec![
            crate::config::MetadataColumn {
                name: "x".into(),
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "x".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(dup.validate().unwrap_err().to_string().contains("'x'"));

    let bad_regex = MetadataSchema {
        columns: vec![crate::config::MetadataColumn {
            name: "x".into(),
            regex: Some("[z-a]".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let message = bad_regex.validate().unwrap_err().to_string();
    assert!(
        message.contains("'x'") && message.contains("[z-a]"),
        "{message}"
    );

    let empty_enum = MetadataSchema {
        columns: vec![crate::config::MetadataColumn {
            name: "x".into(),
            enum_values: Some(Vec::new()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        empty_enum
            .validate()
            .unwrap_err()
            .to_string()
            .contains("empty enum")
    );

    assert!(
        MetadataSchema {
            id_column: "  ".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

// ── Digest determinism ─────────────────────────────────────────────

fn digest_config(schema: Option<MetadataSchema>, rows: &[(&str, &[(&str, &str)])]) -> String {
    let table = rows_from(
        "sample",
        &rows
            .iter()
            .map(|(id, values)| (None, *id, *values))
            .collect::<Vec<_>>(),
    );
    digest_for(&config_with(table, schema)).expect("digest")
}

#[test]
fn digest_is_order_independent_and_ignores_empty_cells() {
    let forward = digest_config(
        None,
        &[
            ("S1", &[("a", "1"), ("b", "")]),
            ("S2", &[("a", "2"), ("b", "x")]),
        ],
    );
    let reversed = digest_config(
        None,
        &[
            ("S2", &[("b", "x"), ("a", "2")]),
            ("S1", &[("b", ""), ("a", "1")]),
        ],
    );
    assert_eq!(forward, reversed);
    // An empty cell and an absent key are the same logical value.
    let absent = digest_config(
        None,
        &[("S1", &[("a", "1")]), ("S2", &[("a", "2"), ("b", "x")])],
    );
    assert_eq!(forward, absent);
    // A real value change moves the digest.
    let changed = digest_config(
        None,
        &[("S1", &[("a", "9")]), ("S2", &[("a", "2"), ("b", "x")])],
    );
    assert_ne!(forward, changed);
    // Whitespace around values is trimmed before hashing.
    let padded = digest_config(
        None,
        &[("S1", &[("a", " 1 ")]), ("S2", &[("a", "2"), ("b", " x ")])],
    );
    assert_eq!(forward, padded);
}

#[test]
fn digest_materializes_schema_defaults_and_sorts_declarations() {
    let explicit_default = digest_config(Some(MetadataSchema::default()), &[("S1", &[("a", "1")])]);
    assert_eq!(
        explicit_default,
        digest_config(None, &[("S1", &[("a", "1")])])
    );

    let enum_forward = MetadataSchema {
        columns: vec![crate::config::MetadataColumn {
            name: "a".into(),
            enum_values: Some(vec!["x".into(), "y".into()]),
            ..Default::default()
        }],
        ..Default::default()
    };
    let enum_reversed = MetadataSchema {
        columns: vec![crate::config::MetadataColumn {
            name: "a".into(),
            enum_values: Some(vec!["y".into(), "x".into()]),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        digest_config(Some(enum_forward), &[("S1", &[("a", "x")])]),
        digest_config(Some(enum_reversed), &[("S1", &[("a", "x")])]),
        "declaration order is not semantic"
    );
}

#[test]
fn digest_is_none_without_any_contract_or_rows() {
    let config = WorkflowConfig::parse(
        r#"
            [workflow]
            name = "plain"
            "#,
    )
    .unwrap();
    assert_eq!(digest_for(&config), None);
    let schema_only = MetadataSchema {
        required_columns: vec!["x".into()],
        ..Default::default()
    };
    let mut config = config;
    config.workflow.metadata_schema = Some(schema_only);
    assert!(digest_for(&config).is_some());
}

#[test]
fn loader_honors_id_column_and_rejects_empty_ids() {
    let dir = tempfile::tempdir().unwrap();
    let tsv = dir.path().join("custom.tsv");
    std::fs::write(&tsv, "sample_id\tbatch\nS1\tb1\nS2\tb2\n").unwrap();
    let table = crate::config::SampleMetadata::load_table_from_file(&tsv, "sample_id").unwrap();
    assert_eq!(table.rows[0].id, "S1");
    assert_eq!(table.rows[0].row, Some(1));
    assert_eq!(table.columns, vec!["batch".to_string()]);

    // A non-default id column that the header does not declare is a named
    // error listing the headers the file actually has.
    let err = crate::config::SampleMetadata::load_table_from_file(&tsv, "subject").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("subject") && message.contains("sample_id, batch"),
        "{message}"
    );

    // The default id keeps the legacy first-column layout.
    let legacy = dir.path().join("legacy.tsv");
    std::fs::write(&legacy, "id\tbatch\nS1\tb1\n").unwrap();
    let table =
        crate::config::SampleMetadata::load_table_from_file(&legacy, DEFAULT_ID_COLUMN).unwrap();
    assert_eq!(table.rows[0].id, "S1");

    // JSON keys are honored the same way; a missing key names the row.
    let json = dir.path().join("custom.json");
    std::fs::write(&json, r#"[{"sample_id": "S1", "batch": "b1"}]"#).unwrap();
    let table = crate::config::SampleMetadata::load_table_from_file(&json, "sample_id").unwrap();
    assert_eq!(table.rows[0].id, "S1");
    let err = crate::config::SampleMetadata::load_table_from_file(&json, "sample").unwrap_err();
    assert!(err.to_string().contains("row 1"), "{err}");

    // An empty id is a parse error naming the data row, never a silent skip.
    let empty = dir.path().join("empty.tsv");
    std::fs::write(&empty, "sample\tbatch\n\tb1\n").unwrap();
    let err =
        crate::config::SampleMetadata::load_table_from_file(&empty, DEFAULT_ID_COLUMN).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("data row 1") && message.contains("empty id"),
        "{message}"
    );
}
