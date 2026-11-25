//! Canonical metadata digest (issue #1368).
//!
//! Byte-stable across TSV/CSV/JSON sources carrying the same logical rows
//! and across row order, because values are canonicalized per declared
//! column type before hashing:
//!
//! - `integer` values are parsed and re-formatted (`007` → `7`);
//! - `float` values are parsed as finite `f64` and re-formatted (`1.50` →
//!   `1.5`, `1.0` → `1`);
//! - `boolean` values are lowercased (`TRUE` → `true`);
//! - `string` values (and columns without a declared type) are trimmed with
//!   case preserved.
//!
//! Values that do not satisfy their declared type are hashed as trimmed
//! text — validation reports them separately (`META-007`), and the digest
//! must stay computable. Empty values are omitted, so an empty TSV cell and
//! an absent JSON key are the same logical value. Rows are sorted by
//! `(id, canonical values)`, making duplicate-id tables order-independent
//! too; row numbers are not part of the digest.

use super::{
    MetadataColumn, MetadataColumnType, MetadataCoverage, MetadataRow, MetadataSchema,
    effective_rows, resolved_schema,
};
use crate::config::WorkflowConfig;
use serde::Serialize;
use std::collections::BTreeMap;

/// Canonical metadata digest for one workflow: `sha256:<hex>` over
/// `{format, schema, rows}`, or `None` when the workflow declares no
/// metadata table, no `metadata_file`, and no
/// `[workflow.metadata_schema]`.
#[must_use]
pub fn digest_for(config: &WorkflowConfig) -> Option<String> {
    let schema = resolved_schema(config);
    let no_contract = config.workflow.metadata_schema.is_none()
        && config.workflow.metadata_file.is_none()
        && config.metadata.is_empty();
    if no_contract {
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
    values: BTreeMap<&'a str, String>,
}

/// Canonical form of one non-empty value under its declared column type.
/// Returns `None` for an empty (absent) value. A value that does not parse
/// as its declared type is kept as trimmed text — `META-007` is the
/// validation surface for that, and the digest must not silently drop it.
fn canonical_value(column: Option<&MetadataColumn>, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let Some(column) = column else {
        return Some(trimmed.to_string());
    };
    Some(match column.column_type {
        MetadataColumnType::String => trimmed.to_string(),
        MetadataColumnType::Integer => trimmed
            .parse::<i64>()
            .map(|value| value.to_string())
            .unwrap_or_else(|_| trimmed.to_string()),
        MetadataColumnType::Float => trimmed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| value.to_string())
            .unwrap_or_else(|| trimmed.to_string()),
        MetadataColumnType::Boolean => {
            if trimmed.eq_ignore_ascii_case("true") {
                "true".to_string()
            } else if trimmed.eq_ignore_ascii_case("false") {
                "false".to_string()
            } else {
                trimmed.to_string()
            }
        }
    })
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

    let columns_by_name: BTreeMap<&str, &MetadataColumn> = schema
        .columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect();

    let mut canonical_rows: Vec<DigestRow<'_>> = rows
        .iter()
        .map(|row| {
            let values: BTreeMap<&str, String> = row
                .values
                .iter()
                .filter_map(|(column, value)| {
                    canonical_value(columns_by_name.get(column.as_str()).copied(), value)
                        .map(|canonical| (column.as_str(), canonical))
                })
                .collect();
            DigestRow {
                id: row.id.as_str(),
                values,
            }
        })
        .collect();
    // Sort by (id, canonical values): duplicate-id tables (which strict
    // validation rejects, but `validate --json` still reports a digest for)
    // are order-independent as well.
    canonical_rows.sort_by(|a, b| {
        a.id.cmp(b.id).then_with(|| {
            a.values
                .iter()
                .collect::<Vec<_>>()
                .cmp(&b.values.iter().collect::<Vec<_>>())
        })
    });

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
