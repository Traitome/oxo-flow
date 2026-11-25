//! Canonical metadata digest (issue #1368): byte-stable across TSV/CSV/JSON
//! sources carrying the same logical rows and across row order.

use super::{
    MetadataColumnType, MetadataCoverage, MetadataRow, MetadataSchema, effective_rows,
    resolved_schema,
};
use crate::config::WorkflowConfig;
use serde::Serialize;
use std::collections::BTreeMap;

/// Canonical metadata digest for one workflow: `sha256:<hex>` over
/// `{format, schema, rows}`, or `None` when the workflow declares neither a
/// metadata table nor a `[workflow.metadata_schema]`.
///
/// Canonical form (byte-stable across TSV/CSV/JSON sources carrying the
/// same logical rows and across row order):
///
/// - schema: every field materialized with its default, `required_columns`
///   sorted/deduped, `columns` sorted by name, enum values sorted;
/// - rows: sorted by id, columns sorted by name, values trimmed, empty
///   values omitted (an empty TSV cell and an absent JSON key are the same
///   logical value);
/// - row numbers are NOT part of the digest — they depend on source order.
#[must_use]
pub fn digest_for(config: &WorkflowConfig) -> Option<String> {
    let schema = resolved_schema(config);
    if config.workflow.metadata_schema.is_none() && config.metadata.is_empty() {
        return None;
    }
    let rows = effective_rows(config);
    Some(compute_digest(&schema, &rows))
}

#[derive(Serialize)]
struct DigestIdentity<'a> {
    format: &'static str,
    schema: DigestSchema<'a>,
    rows: Vec<DigestRow<'a>>,
}

#[derive(Serialize)]
struct DigestSchema<'a> {
    id_column: &'a str,
    required_columns: Vec<&'a str>,
    allow_extra_columns: bool,
    coverage: MetadataCoverage,
    columns: Vec<DigestColumn<'a>>,
}

#[derive(Serialize)]
struct DigestColumn<'a> {
    name: &'a str,
    required: bool,
    unique: bool,
    #[serde(rename = "type")]
    column_type: MetadataColumnType,
    #[serde(skip_serializing_if = "Option::is_none")]
    regex: Option<&'a str>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<&'a str>>,
}

#[derive(Serialize)]
struct DigestRow<'a> {
    id: &'a str,
    values: BTreeMap<&'a str, &'a str>,
}

/// Hash a resolved schema plus effective rows in canonical form. Public so
/// the determinism/perf tests can pin the exact bytes without loading a
/// file.
#[must_use]
pub fn compute_digest(schema: &MetadataSchema, rows: &[MetadataRow]) -> String {
    let mut required_columns: Vec<&str> =
        schema.required_columns.iter().map(String::as_str).collect();
    required_columns.sort_unstable();
    required_columns.dedup();

    let mut columns: Vec<DigestColumn<'_>> = schema
        .columns
        .iter()
        .map(|column| {
            let mut enum_values: Option<Vec<&str>> = column
                .enum_values
                .as_ref()
                .map(|values| values.iter().map(String::as_str).collect());
            if let Some(values) = &mut enum_values {
                values.sort_unstable();
                values.dedup();
            }
            DigestColumn {
                name: &column.name,
                required: column.required,
                unique: column.unique,
                column_type: column.column_type,
                regex: column.regex.as_deref(),
                enum_values,
            }
        })
        .collect();
    columns.sort_by_key(|column| column.name);

    let mut canonical_rows: Vec<DigestRow<'_>> = rows
        .iter()
        .map(|row| {
            let values: BTreeMap<&str, &str> = row
                .values
                .iter()
                .filter_map(|(column, value)| {
                    let value = value.trim();
                    (!value.is_empty()).then_some((column.as_str(), value))
                })
                .collect();
            DigestRow {
                id: row.id.as_str(),
                values,
            }
        })
        .collect();
    canonical_rows.sort_by(|a, b| a.id.cmp(b.id));

    let identity = DigestIdentity {
        format: "oxo-flow.metadata-digest/v1",
        schema: DigestSchema {
            id_column: &schema.id_column,
            required_columns,
            allow_extra_columns: schema.allow_extra_columns,
            coverage: schema.coverage,
            columns,
        },
        rows: canonical_rows,
    };
    let canonical = serde_json::to_vec(&identity)
        .expect("serializing the metadata digest identity cannot fail");
    use sha2::Digest;
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(&canonical)))
}
