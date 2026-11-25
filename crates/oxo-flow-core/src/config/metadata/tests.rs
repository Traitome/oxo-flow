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

#[test]
fn schema_less_tables_report_warnings_and_a_schema_opts_into_strict_errors() {
    // Issue #1368 review I1: a free-form (schema-less) metadata table must
    // never fail validation/run for the built-in table findings; declaring
    // `[workflow.metadata_schema]` opts into strict errors.
    let table = rows_from(
        "sample",
        &[
            (Some(1), "S1", &[("batch", "b1")]),
            (Some(2), "S1", &[("batch", "b2")]),
        ],
    );
    let freeform = config_with(table.clone(), None);
    let report = evaluate(&freeform);
    assert_eq!(codes(&report), ["META-003"]);
    assert_eq!(report.findings[0].severity, MetadataSeverity::Warning);
    assert!(!report.has_errors(), "schema-less findings must not fail");

    let strict = config_with(table, Some(MetadataSchema::default()));
    let report = evaluate(&strict);
    assert_eq!(codes(&report), ["META-003"]);
    assert_eq!(report.findings[0].severity, MetadataSeverity::Error);
    assert!(report.has_errors());

    // A pair conflict follows the same gate.
    let mut config = config_with(MetadataTable::default(), None);
    config.pairs.push(crate::config::ExperimentControlPair {
        pair_id: "P1".into(),
        experiment: "T1".into(),
        control: Some("T1".into()),
        experiment_type: None,
        metadata: Default::default(),
        when: None,
    });
    let report = evaluate(&config);
    assert_eq!(codes(&report), ["META-008"]);
    assert_eq!(report.findings[0].severity, MetadataSeverity::Warning);
    config.workflow.metadata_schema = Some(MetadataSchema::default());
    let report = evaluate(&config);
    assert_eq!(report.findings[0].severity, MetadataSeverity::Error);
}

#[test]
fn digest_canonicalizes_declared_types() {
    // Issue #1368 review I2: the same logical values written with different
    // textual forms under a declared type must hash identically.
    let schema = || MetadataSchema {
        columns: vec![
            crate::config::MetadataColumn {
                name: "n".into(),
                column_type: MetadataColumnType::Integer,
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "v".into(),
                column_type: MetadataColumnType::Float,
                ..Default::default()
            },
            crate::config::MetadataColumn {
                name: "flag".into(),
                column_type: MetadataColumnType::Boolean,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let plain = digest_config(
        Some(schema()),
        &[("S1", &[("n", "7"), ("v", "1.5"), ("flag", "true")])],
    );
    let padded = digest_config(
        Some(schema()),
        &[("S1", &[("n", "007"), ("v", "1.50"), ("flag", "TRUE")])],
    );
    assert_eq!(plain, padded, "declared types must canonicalize the digest");
    // String columns keep case (only trimming is applied).
    let string_upper = digest_config(None, &[("S1", &[("x", "ABC")])]);
    let string_lower = digest_config(None, &[("S1", &[("x", "abc")])]);
    assert_ne!(string_upper, string_lower);
}

#[test]
fn digest_is_order_independent_for_duplicate_ids() {
    // Issue #1368 review I2: duplicate ids (a strict-error shape, but one
    // `validate --json` still digests) must not depend on source order.
    let row = |values: &[(&str, &str)]| MetadataRow {
        row: Some(1),
        id: "S1".into(),
        values: values
            .iter()
            .map(|(column, value)| (column.to_string(), value.to_string()))
            .collect(),
    };
    let schema = MetadataSchema::default();
    let first = compute_digest(&schema, &[row(&[("v", "a")]), row(&[("v", "b")])]);
    let second = compute_digest(&schema, &[row(&[("v", "b")]), row(&[("v", "a")])]);
    assert_eq!(first, second);
}

#[test]
fn pair_id_keyed_rows_satisfy_pair_coverage() {
    // Issue #1368 review m2: `{meta.*}` lookup falls back to pair_id, so a
    // metadata row keyed by the pair id satisfies its experiment/control.
    let table = rows_from("sample", &[(Some(1), "P1", &[("v", "a")])]);
    let mut config = config_with(
        table,
        Some(MetadataSchema {
            coverage: MetadataCoverage::Error,
            ..Default::default()
        }),
    );
    config.pairs.push(crate::config::ExperimentControlPair {
        pair_id: "P1".into(),
        experiment: "T1".into(),
        control: Some("N1".into()),
        experiment_type: None,
        metadata: Default::default(),
        when: None,
    });
    // P1 is in the domain (pair id), and the pair's members resolve from its
    // row — no coverage finding at all.
    assert!(evaluate(&config).findings.is_empty());
}

#[test]
fn header_only_metadata_file_is_a_contract() {
    // Issue #1368 review m6: a declared metadata_file with a header and zero
    // rows is a contract — every domain member has no row, and the digest is
    // still recorded.
    let mut config = config_with(MetadataTable::default(), None);
    config.workflow.metadata_file = Some("samples.tsv".into());
    config.sample_groups.push(crate::config::SampleGroup {
        name: "cohort".into(),
        samples: vec!["S1".into()],
        metadata: Default::default(),
    });
    let report = evaluate(&config);
    assert_eq!(codes(&report), ["META-010"]);
    assert_eq!(report.findings[0].severity, MetadataSeverity::Warning);
    assert!(
        digest_for(&config).is_some(),
        "a declared header-only table must still be digested"
    );
}

#[test]
fn suggestions_name_the_actual_required_declaration_site() {
    // Issue #1368 review m7: required_columns and columns[].required are
    // different edit sites.
    let table = rows_from("sample", &[(Some(1), "S1", &[("batch", ""), ("qc", "")])]);
    let schema = MetadataSchema {
        required_columns: vec!["batch".into()],
        columns: vec![crate::config::MetadataColumn {
            name: "qc".into(),
            required: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let report = evaluate(&config_with(table, Some(schema)));
    let hint_for = |column: &str| {
        report
            .findings
            .iter()
            .find(|finding| finding.column.as_deref() == Some(column))
            .and_then(|finding| finding.suggestion.clone())
            .unwrap_or_default()
    };
    assert!(hint_for("batch").contains("required_columns"));
    assert!(!hint_for("batch").contains("required = true"));
    assert!(hint_for("qc").contains("required = true"));
    // Review N2: no runs of literal spaces from string-continuation
    // formatting in any suggestion.
    for column in ["batch", "qc"] {
        assert!(
            !hint_for(column).contains("  "),
            "suggestion for '{column}' has a space run: {:?}",
            hint_for(column)
        );
    }
}

#[test]
fn strict_loader_requires_the_declared_id_header() {
    // Issue #1368 review m1: once a schema is declared, an explicit
    // `id_column = "sample"` must not fall back to the first column.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subject.tsv");
    std::fs::write(&path, "subject\tbatch\nS1\tb1\n").unwrap();
    let strict =
        crate::config::SampleMetadata::load_table_from_file_strict(&path, DEFAULT_ID_COLUMN);
    let err = strict.unwrap_err().to_string();
    assert!(err.contains("sample") && err.contains("subject"), "{err}");
    // The legacy (schema-less) path still binds the first column.
    let legacy =
        crate::config::SampleMetadata::load_table_from_file(&path, DEFAULT_ID_COLUMN).unwrap();
    assert_eq!(legacy.rows[0].id, "S1");
}
