//! The metadata-contract validator (issue #1368): stable, row-level
//! `META-001` … `META-012` findings, sorted by `(code, row, column, id)`.

use super::{
    MetadataFinding, MetadataReport, MetadataRow, MetadataSchema, MetadataSeverity, effective_rows,
    resolved_schema,
};
use crate::config::WorkflowConfig;
use std::collections::{BTreeMap, BTreeSet};

/// Every non-id column any effective source declares.
fn declared_table_columns(config: &WorkflowConfig, rows: &[MetadataRow]) -> BTreeSet<String> {
    let mut columns: BTreeSet<String> = config
        .metadata_table
        .as_ref()
        .map(|table| table.columns.iter().cloned().collect())
        .unwrap_or_default();
    for row in rows {
        columns.extend(row.values.keys().cloned());
    }
    columns
}

/// Where a column's required-ness was declared — named back in the finding
/// suggestion so the operator edits the declaration that actually holds it
/// (m7): `required_columns` vs `columns[].required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredSite {
    RequiredColumns,
    ColumnFlag,
    Both,
}

/// The names that must carry a non-empty value in every row, with the
/// declaration site. The id column is implicitly required (the loader
/// rejects an empty id) and never needs a presence check.
fn required_columns(schema: &MetadataSchema) -> BTreeMap<String, RequiredSite> {
    let mut required: BTreeMap<String, RequiredSite> = schema
        .required_columns
        .iter()
        .map(|name| (name.clone(), RequiredSite::RequiredColumns))
        .collect();
    for column in &schema.columns {
        if column.required {
            required
                .entry(column.name.clone())
                .and_modify(|site| *site = RequiredSite::Both)
                .or_insert(RequiredSite::ColumnFlag);
        }
    }
    required.remove(&schema.id_column);
    required
}

/// The "or remove the requirement" half of a META-001/META-002 suggestion,
/// naming every declaration that actually holds the requirement.
fn drop_required_hint(name: &str, site: RequiredSite) -> String {
    match site {
        RequiredSite::RequiredColumns => {
            "drop it from [workflow.metadata_schema] required_columns".to_string()
        }
        RequiredSite::ColumnFlag => format!(
            "drop required = true from the '{name}' entry under              [[workflow.metadata_schema.columns]]"
        ),
        RequiredSite::Both => format!(
            "drop it from [workflow.metadata_schema] required_columns and from required = true              on the '{name}' column entry"
        ),
    }
}

/// The workflow's declared sample/pair domain: every sample-group member,
/// every pair experiment/control, and every pair id (a metadata table may
/// legitimately be keyed by any of these — expansion resolves `{meta.*}`
/// from sample, pair id, experiment, then control).
fn sample_domain(config: &WorkflowConfig) -> BTreeSet<String> {
    let mut domain = BTreeSet::new();
    for group in &config.sample_groups {
        domain.extend(group.samples.iter().cloned());
    }
    for pair in &config.pairs {
        domain.insert(pair.pair_id.clone());
        domain.insert(pair.experiment.clone());
        if let Some(control) = &pair.control
            && !control.is_empty()
        {
            domain.insert(control.clone());
        }
    }
    domain
}

/// Evaluate the metadata contract for one workflow.
///
/// Pure and deterministic: the same config always produces the same
/// findings in the same order. Called by `validate`, `dry-run`, and `run`
/// after the workflow has loaded — the metadata table, sample groups, pairs,
/// and `sample_pattern` discovery are final at that point. The web layer
/// surfaces a contract failure through the spawned CLI run, not at
/// admission.
#[must_use]
pub fn evaluate(config: &WorkflowConfig) -> MetadataReport {
    let schema = resolved_schema(config);
    let rows = effective_rows(config);
    // Declaring `[workflow.metadata_schema]` opts the workflow into STRICT
    // metadata validation. Without it the table stays free-form (the
    // documented pre-#1368 behavior): every non-coverage finding is
    // reported as a WARNING so a pre-upgrade table with, say, a duplicated
    // row never starts failing, and there is no schema to relax in the
    // failure hint. Domain coverage keeps its own `coverage` severity.
    let strict = config.workflow.metadata_schema.is_some();
    let severity = if strict {
        MetadataSeverity::Error
    } else {
        MetadataSeverity::Warning
    };
    let mut report = MetadataReport::default();

    // ── Column-level checks ────────────────────────────────────────────
    let present_columns = declared_table_columns(config, &rows);
    let required = required_columns(&schema);
    for (column, site) in &required {
        if !present_columns.contains(column) {
            report.push(
                MetadataFinding::new(
                    "META-001",
                    severity,
                    format!("required column '{column}' is missing from the metadata table"),
                    Some(format!(
                        "add a '{column}' column to the metadata table, or {}",
                        drop_required_hint(column, *site)
                    )),
                )
                .in_column(column),
            );
        }
    }

    let extra_columns: Vec<&String> = if schema.allow_extra_columns {
        Vec::new()
    } else {
        let declared: BTreeSet<&str> = schema
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .chain(schema.required_columns.iter().map(String::as_str))
            .collect();
        present_columns
            .iter()
            .filter(|column| {
                column.as_str() != schema.id_column && !declared.contains(column.as_str())
            })
            .collect()
    };
    for column in extra_columns {
        report.push(
            MetadataFinding::new(
                "META-012",
                severity,
                format!(
                    "column '{column}' is not declared by [workflow.metadata_schema] and \
                     allow_extra_columns = false"
                ),
                Some(format!(
                    "declare '{column}' under [[workflow.metadata_schema.columns]], add it to \
                     required_columns, or set allow_extra_columns = true"
                )),
            )
            .in_column(column),
        );
    }

    // Compile regexes once per column; schema validation rejects invalid
    // patterns at load, and a programmatically built config gets an explicit
    // table-level finding instead of a silent skip.
    let mut compiled: BTreeMap<&str, regex::Regex> = BTreeMap::new();
    for column in &schema.columns {
        let Some(pattern) = column.regex.as_deref() else {
            continue;
        };
        match regex::Regex::new(pattern) {
            Ok(regex) => {
                compiled.insert(column.name.as_str(), regex);
            }
            Err(error) => report.push(
                MetadataFinding::new(
                    "META-006",
                    severity,
                    format!(
                        "column '{}' declares an invalid regex '{pattern}': {error}",
                        column.name
                    ),
                    Some("fix the regex or remove it from the column declaration".to_string()),
                )
                .in_column(&column.name),
            ),
        }
    }

    // ── Row-level checks ───────────────────────────────────────────────
    let mut first_row_by_id: BTreeMap<&str, Option<usize>> = BTreeMap::new();
    let mut first_row_of_unique: BTreeMap<(&str, String), (Option<usize>, String)> =
        BTreeMap::new();
    for row in &rows {
        // META-003: duplicate id (the later row is the finding; the first
        // occurrence is named so the operator can find both).
        if let Some(first_row) = first_row_by_id.get(row.id.as_str()) {
            let first = match first_row {
                Some(line) => format!(" (first at row {line})"),
                None => String::new(),
            };
            report.push(
                MetadataFinding::new(
                    "META-003",
                    severity,
                    format!(
                        "duplicate metadata id '{}' in column '{}'{}",
                        row.id, schema.id_column, first
                    ),
                    Some(
                        "merge or remove the duplicate row — the last one silently wins"
                            .to_string(),
                    ),
                )
                .at_row(row.row)
                .in_column(&schema.id_column)
                .for_id(&row.id),
            );
        } else {
            first_row_by_id.insert(row.id.as_str(), row.row);
        }

        for (name, site) in &required {
            if !present_columns.contains(name) {
                // Already reported once as META-001; do not repeat it per row.
                continue;
            }
            let missing = row
                .values
                .get(name)
                .is_none_or(|value| value.trim().is_empty());
            if missing {
                report.push(
                    MetadataFinding::new(
                        "META-002",
                        severity,
                        format!("required column '{name}' has no value for id '{}'", row.id),
                        Some(format!(
                            "fill in '{name}' for this row, or {}",
                            drop_required_hint(name, *site)
                        )),
                    )
                    .at_row(row.row)
                    .in_column(name)
                    .for_id(&row.id),
                );
            }
        }

        for column in &schema.columns {
            let value = row
                .values
                .get(&column.name)
                .map(|value| value.trim())
                .unwrap_or("");
            if value.is_empty() {
                // Empty optional cells are absent values, never type errors.
                continue;
            }

            if column.unique && column.name != schema.id_column {
                let key = (column.name.as_str(), value.to_string());
                match first_row_of_unique.get(&key) {
                    Some((first_row, first_id)) => {
                        let first = match first_row {
                            Some(line) => format!("row {line} (id '{first_id}')"),
                            None => format!("id '{first_id}'"),
                        };
                        report.push(
                            MetadataFinding::new(
                                "META-004",
                                severity,
                                format!(
                                    "column '{}' is declared unique but value '{value}' repeats \
                                     (first seen at {first})",
                                    column.name
                                ),
                                Some(format!(
                                    "make '{}' values distinct, or drop unique = true",
                                    column.name
                                )),
                            )
                            .at_row(row.row)
                            .in_column(&column.name)
                            .for_id(&row.id),
                        );
                    }
                    None => {
                        first_row_of_unique.insert(key, (row.row, row.id.clone()));
                    }
                }
            }

            if let Some(values) = &column.enum_values
                && !values.iter().any(|allowed| allowed == value)
            {
                report.push(
                    MetadataFinding::new(
                        "META-005",
                        severity,
                        format!(
                            "column '{}' value '{value}' is not one of the declared enum values: {}",
                            column.name,
                            values.join(", ")
                        ),
                        Some(format!(
                            "use one of the declared values, or extend the enum for '{}'",
                            column.name
                        )),
                    )
                    .at_row(row.row)
                    .in_column(&column.name)
                    .for_id(&row.id),
                );
            }

            if let Some(regex) = compiled.get(column.name.as_str())
                && !regex.is_match(value)
            {
                report.push(
                    MetadataFinding::new(
                        "META-006",
                        severity,
                        format!(
                            "column '{}' value '{value}' does not match the declared regex '{}'",
                            column.name,
                            column.regex.as_deref().unwrap_or("")
                        ),
                        Some(format!(
                            "correct the value, or adjust the regex for '{}'",
                            column.name
                        )),
                    )
                    .at_row(row.row)
                    .in_column(&column.name)
                    .for_id(&row.id),
                );
            }

            if !column.column_type.accepts(value) {
                report.push(
                    MetadataFinding::new(
                        "META-007",
                        severity,
                        format!(
                            "column '{}' value '{value}' is not a valid {}",
                            column.name,
                            column.column_type.name()
                        ),
                        Some(format!(
                            "write a valid {} value, or change the declared type for '{}'",
                            column.column_type.name(),
                            column.name
                        )),
                    )
                    .at_row(row.row)
                    .in_column(&column.name)
                    .for_id(&row.id),
                );
            }
        }
    }

    // ── Pair conflicts (META-008) ──────────────────────────────────────
    let mut pair_ids: BTreeMap<&str, ()> = BTreeMap::new();
    for pair in &config.pairs {
        if let Some(control) = &pair.control
            && !control.trim().is_empty()
            && pair.experiment.trim() == control.trim()
        {
            report.push(
                MetadataFinding::new(
                    "META-008",
                    severity,
                    format!(
                        "pair '{}' declares experiment == control ('{}') — the comparison has \
                         no contrast",
                        pair.pair_id, pair.experiment
                    ),
                    Some(
                        "use a distinct control sample, or omit control for a tumor-only pair"
                            .to_string(),
                    ),
                )
                .in_column("control")
                .for_id(&pair.pair_id),
            );
        }
        if pair_ids.insert(pair.pair_id.as_str(), ()).is_some() {
            report.push(
                MetadataFinding::new(
                    "META-008",
                    severity,
                    format!("duplicate pair_id '{}'", pair.pair_id),
                    Some("make every pair_id unique — instances and outputs key on it".to_string()),
                )
                .in_column("pair_id")
                .for_id(&pair.pair_id),
            );
        }
    }

    // ── Domain coverage (META-009 … META-011) ─────────────────────────
    // Meaningful only when a contract exists: with no metadata table, no
    // declared `metadata_file`, and no schema there is nothing to cover,
    // and with no declared sample/pair domain there is nothing to cover it
    // against (a metadata table may be a pure lookup). A header-only
    // `metadata_file` IS a contract — the common "forgot to populate"
    // mistake must surface as every domain member missing a row.
    let contract_exists = config.workflow.metadata_file.is_some()
        || !rows.is_empty()
        || config.workflow.metadata_schema.is_some();
    let domain = sample_domain(config);
    if let Some(severity) = schema.coverage.severity()
        && contract_exists
        && !domain.is_empty()
    {
        for pair in &config.pairs {
            // Lookup order is sample -> pair_id -> experiment -> control: a
            // row keyed by the pair id resolves every `{meta.*}` read for
            // the pair, so neither member is missing coverage.
            if config.metadata.contains_key(&pair.pair_id) {
                continue;
            }
            for (column, sample) in [
                ("experiment", Some(pair.experiment.as_str())),
                ("control", pair.control.as_deref()),
            ] {
                let Some(sample) = sample.filter(|sample| !sample.trim().is_empty()) else {
                    continue;
                };
                if !config.metadata.contains_key(sample) {
                    report.push(
                        MetadataFinding::new(
                            "META-009",
                            severity,
                            format!(
                                "pair '{}' references sample '{sample}' with no metadata row",
                                pair.pair_id
                            ),
                            Some(format!(
                                "add a metadata row for '{sample}' or remove the sample from the pair"
                            )),
                        )
                        .in_column(column)
                        .for_id(sample),
                    );
                }
            }
        }

        for group in &config.sample_groups {
            for sample in &group.samples {
                if !config.metadata.contains_key(sample) {
                    report.push(
                        MetadataFinding::new(
                            "META-010",
                            severity,
                            format!(
                                "sample '{sample}' in group '{}' has no metadata row",
                                group.name
                            ),
                            Some(if config.workflow.metadata_schema.is_some() {
                                format!(
                                    "add a metadata row for '{sample}', or set \
                                     [workflow.metadata_schema] coverage = \"off\""
                                )
                            } else {
                                format!("add a metadata row for '{sample}'")
                            }),
                        )
                        .in_column(&schema.id_column)
                        .for_id(sample),
                    );
                }
            }
        }

        let mut reported: BTreeSet<&str> = BTreeSet::new();
        for row in &rows {
            if domain.contains(&row.id) || !reported.insert(row.id.as_str()) {
                continue;
            }
            report.push(
                MetadataFinding::new(
                    "META-011",
                    severity,
                    format!(
                        "metadata id '{}' is outside every sample group and pair domain",
                        row.id
                    ),
                    Some(
                        "remove the row, or add the id to a [[sample_groups]] / [[pairs]] entry"
                            .to_string(),
                    ),
                )
                .at_row(row.row)
                .in_column(&schema.id_column)
                .for_id(&row.id),
            );
        }
    }

    // Stable order: (code, row, column, id). `Option` sorts `None` before
    // `Some`, so table-level findings of one code come first — deterministic
    // and documented.
    report.findings.sort_by(|a, b| {
        (&a.code, a.row, &a.column, &a.id).cmp(&(&b.code, b.row, &b.column, &b.id))
    });
    report
}
