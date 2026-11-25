//! Validated cohort and metadata contracts (issue #1368).
//!
//! `[workflow.metadata_schema]` turns the free-form `metadata_file` lookup
//! table into a checked contract: declared columns, value types,
//! uniqueness, enumerated values, regular expressions, extra-column policy,
//! and coverage of the sample/pair domain. Findings are stable and
//! row-level — row is the 1-based data row in the metadata table — and are
//! sorted by `(code, row, column, id)` so the `validate --json` /
//! `dry-run --json` `metadata` arrays are byte-stable across runs.
//!
//! The canonical metadata digest ([`digest_for`]) pins the resolved schema
//! plus the effective rows: rows sorted by id, columns by name, values
//! trimmed and empty values omitted. It is byte-stable across TSV, CSV and
//! JSON sources carrying the same logical rows, and across input row order,
//! so the checkpoint provenance can prove which table a run actually used.
//!
//! All checks are order-independent by construction (index maps and
//! explicit sorts, never iteration order of a `HashMap`), so a shuffled
//! table produces the same digest and the same finding set.

mod digest;
mod evaluate;
#[cfg(test)]
mod perf_tests;
mod table;
#[cfg(test)]
mod tests;

pub use digest::{compute_digest, digest_for};
pub use evaluate::evaluate;
pub use table::{MetadataRow, MetadataTable};

use crate::config::WorkflowConfig;
use crate::error::{OxoFlowError, Result};
use serde::Serialize;
use std::collections::BTreeSet;

/// The metadata id column used when `[workflow.metadata_schema]` is absent
/// or omits `id_column`.
pub const DEFAULT_ID_COLUMN: &str = "sample";

/// How coverage gaps between the metadata table and the sample/pair domain
/// are reported (`[workflow.metadata_schema] coverage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataCoverage {
    /// Report nothing — domain coverage is the author's responsibility.
    Off,
    /// Report gaps as warnings (default): the workflow runs.
    #[default]
    Warn,
    /// Report gaps as errors: `validate` fails and `run` refuses to start.
    Error,
}

impl MetadataCoverage {
    /// The severity a coverage finding carries, or `None` when coverage is
    /// `off`.
    fn severity(self) -> Option<MetadataSeverity> {
        match self {
            Self::Off => None,
            Self::Warn => Some(MetadataSeverity::Warning),
            Self::Error => Some(MetadataSeverity::Error),
        }
    }
}

/// Declared value type of a metadata column.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum MetadataColumnType {
    /// Any value (default). Only presence constraints apply.
    #[default]
    String,
    /// A signed 64-bit integer (`12`, `-3`).
    Integer,
    /// A finite floating-point number (`3.5`, `-1e-4`).
    Float,
    /// `true` or `false` (case-insensitive).
    Boolean,
}

impl MetadataColumnType {
    fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Boolean => "boolean",
        }
    }

    /// Whether `value` (already trimmed, non-empty) satisfies this type.
    fn accepts(self, value: &str) -> bool {
        match self {
            Self::String => true,
            Self::Integer => value.parse::<i64>().is_ok(),
            Self::Float => value.parse::<f64>().is_ok_and(|number| number.is_finite()),
            Self::Boolean => {
                value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
            }
        }
    }
}

/// One declared metadata column: presence, uniqueness, type, pattern and
/// enumerated values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct MetadataColumn {
    /// Column name as written in the table header / JSON key.
    pub name: String,
    /// Every row must carry a non-empty value. Equivalent to listing the
    /// column in `required_columns`.
    #[serde(default)]
    pub required: bool,
    /// Values must be unique across rows (the id column's uniqueness is
    /// always enforced by `META-003`).
    #[serde(default)]
    pub unique: bool,
    /// Declared value type (default `string`).
    #[serde(default, rename = "type")]
    pub column_type: MetadataColumnType,
    /// Value must match this Rust regular expression (unanchored — use
    /// `^…$` for a whole-value match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// Value must be one of these strings (exact, case-sensitive match).
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

/// The optional `[workflow.metadata_schema]` contract for the
/// `metadata_file` table (issue #1368).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct MetadataSchema {
    /// Header name (TSV/CSV) / object key (JSON) holding the row id.
    /// Default `"sample"`; the id is always required and unique.
    #[serde(default = "default_id_column")]
    pub id_column: String,
    /// Columns every row must carry a non-empty value for.
    #[serde(default)]
    pub required_columns: Vec<String>,
    /// When `false`, a column outside the declared set is `META-012`.
    #[serde(default = "default_allow_extra_columns")]
    pub allow_extra_columns: bool,
    /// How domain/table coverage gaps are reported.
    #[serde(default)]
    pub coverage: MetadataCoverage,
    /// Per-column constraints.
    #[serde(default)]
    pub columns: Vec<MetadataColumn>,
}

impl Default for MetadataSchema {
    fn default() -> Self {
        Self {
            id_column: default_id_column(),
            required_columns: Vec::new(),
            allow_extra_columns: default_allow_extra_columns(),
            coverage: MetadataCoverage::default(),
            columns: Vec::new(),
        }
    }
}

fn default_id_column() -> String {
    DEFAULT_ID_COLUMN.to_string()
}

fn default_allow_extra_columns() -> bool {
    true
}

/// Severity of one metadata-contract finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataSeverity {
    /// The workflow is rejected (`validate` fails, `run` refuses to start).
    Error,
    /// Reported, but the workflow still runs.
    Warning,
}

impl std::fmt::Display for MetadataSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => f.write_str("error"),
            Self::Warning => f.write_str("warning"),
        }
    }
}

/// One stable, row-level metadata-contract finding.
///
/// `row` is the 1-based data row in the metadata table (`null` for
/// table-level findings and for inline `[metadata]` rows, which have no
/// source row number); `column` is the metadata column the finding is
/// about; `id` is the metadata id of the offending row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct MetadataFinding {
    /// Stable diagnostic code (`META-001` … `META-012`).
    pub code: String,
    pub severity: MetadataSeverity,
    pub row: Option<usize>,
    pub column: Option<String>,
    pub id: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

impl MetadataFinding {
    fn new(
        code: &str,
        severity: MetadataSeverity,
        message: String,
        suggestion: Option<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity,
            row: None,
            column: None,
            id: None,
            message,
            suggestion,
        }
    }

    fn at_row(mut self, row: Option<usize>) -> Self {
        self.row = row;
        self
    }

    fn in_column(mut self, column: &str) -> Self {
        self.column = Some(column.to_string());
        self
    }

    fn for_id(mut self, id: &str) -> Self {
        self.id = Some(id.to_string());
        self
    }
}

/// The complete metadata-contract verdict for one workflow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataReport {
    /// Findings sorted by `(code, row, column, id)`.
    pub findings: Vec<MetadataFinding>,
    pub error_count: usize,
    pub warning_count: usize,
}

impl MetadataReport {
    /// Whether any finding is an error (the workflow must be rejected).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    fn push(&mut self, finding: MetadataFinding) {
        match finding.severity {
            MetadataSeverity::Error => self.error_count += 1,
            MetadataSeverity::Warning => self.warning_count += 1,
        }
        self.findings.push(finding);
    }
}

/// Every effective metadata row the workflow can look up, in a stable
/// order: the loaded table's rows, then any id present only in
/// [`WorkflowConfig::metadata`] (programmatically built configs / defensive
/// completeness), sorted by id.
pub(super) fn effective_rows(config: &WorkflowConfig) -> Vec<MetadataRow> {
    let Some(table) = &config.metadata_table else {
        return MetadataTable::from_inline(&resolved_schema(config).id_column, &config.metadata)
            .rows;
    };
    let mut rows = table.rows.clone();
    let known: BTreeSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let mut extra: Vec<&String> = config
        .metadata
        .keys()
        .filter(|id| !known.contains(id.as_str()))
        .collect();
    extra.sort();
    for id in extra {
        rows.push(MetadataRow {
            row: None,
            id: id.clone(),
            values: config.metadata[id]
                .iter()
                .map(|(column, value)| (column.clone(), value.clone()))
                .collect(),
        });
    }
    rows
}

/// The declared contract, materialized with defaults when
/// `[workflow.metadata_schema]` is absent — the same resolved schema feeds
/// validation and the digest.
pub(super) fn resolved_schema(config: &WorkflowConfig) -> MetadataSchema {
    config.workflow.metadata_schema.clone().unwrap_or_default()
}

impl WorkflowConfig {
    /// Stable, row-level findings for this workflow's metadata contract.
    #[must_use]
    pub fn metadata_contract_report(&self) -> MetadataReport {
        evaluate(self)
    }

    /// Recompute the engine-internal metadata table (inline rows only when
    /// no file was loaded) and the canonical digest. Called once at the end
    /// of `parse` / `from_file`; nothing mutates metadata rows afterwards.
    pub(crate) fn refresh_metadata_contract(&mut self) {
        let id_column = resolved_schema(self).id_column;
        if self.metadata_table.is_none() && !self.metadata.is_empty() {
            self.metadata_table = Some(MetadataTable::from_inline(&id_column, &self.metadata));
        }
        self.metadata_digest = digest_for(self);
    }
}

impl MetadataSchema {
    /// Validate the contract declaration itself (not the data): a usable id
    /// column, unique non-empty column names, non-empty enum lists, and
    /// compilable regexes. Data findings are produced by [`evaluate`].
    pub fn validate(&self) -> Result<()> {
        if self.id_column.trim().is_empty() {
            return Err(OxoFlowError::Config {
                message: "[workflow.metadata_schema] id_column must not be empty — it names \
                          the metadata column that identifies a row"
                    .to_string(),
            });
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for column in &self.columns {
            if column.name.trim().is_empty() {
                return Err(OxoFlowError::Config {
                    message: "[workflow.metadata_schema] columns[] entry has an empty name"
                        .to_string(),
                });
            }
            if !seen.insert(column.name.as_str()) {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "[workflow.metadata_schema] column '{}' is declared more than once",
                        column.name
                    ),
                });
            }
            if let Some(pattern) = &column.regex {
                regex::Regex::new(pattern).map_err(|error| OxoFlowError::Config {
                    message: format!(
                        "[workflow.metadata_schema] column '{}' declares an invalid regex \
                         '{pattern}': {error}",
                        column.name
                    ),
                })?;
            }
            if let Some(values) = &column.enum_values
                && values.is_empty()
            {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "[workflow.metadata_schema] column '{}' has an empty enum list — \
                         every value would be rejected",
                        column.name
                    ),
                });
            }
        }
        let mut required: BTreeSet<&str> = BTreeSet::new();
        for name in &self.required_columns {
            if name.trim().is_empty() {
                return Err(OxoFlowError::Config {
                    message: "[workflow.metadata_schema] required_columns contains an empty name"
                        .to_string(),
                });
            }
            if !required.insert(name.as_str()) {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "[workflow.metadata_schema] required column '{name}' is listed more \
                         than once"
                    ),
                });
            }
        }
        Ok(())
    }
}
