//! Effective metadata rows and the table a workflow loaded (issue #1368).

use std::collections::{BTreeMap, BTreeSet};

/// One effective metadata row: source row number (when known), id, and the
/// non-id columns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataRow {
    /// 1-based data row in the metadata file; `None` for inline rows.
    pub row: Option<usize>,
    pub id: String,
    /// Column → raw value, preserving every declared column (empty cells
    /// stay empty strings, exactly as the source declared them).
    pub values: BTreeMap<String, String>,
}

/// The effective metadata table for a loaded workflow: every row a lookup
/// can reach, in source order (file rows first, then inline-only rows
/// sorted by id), plus every non-id column any source declared.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataTable {
    pub id_column: String,
    /// Non-id columns declared by the file header or any row, sorted.
    pub columns: Vec<String>,
    pub rows: Vec<MetadataRow>,
}

impl MetadataTable {
    /// Build the table from inline `[metadata]` rows (no file): rows are
    /// sorted by id for determinism and carry `row = None`.
    pub(crate) fn from_inline(
        id_column: &str,
        inline: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    ) -> Self {
        let mut ids: Vec<&String> = inline.keys().collect();
        ids.sort();
        let mut columns: BTreeSet<String> = BTreeSet::new();
        let rows: Vec<MetadataRow> = ids
            .into_iter()
            .map(|id| {
                let values: BTreeMap<String, String> = inline[id]
                    .iter()
                    .map(|(column, value)| (column.clone(), value.clone()))
                    .collect();
                columns.extend(values.keys().cloned());
                MetadataRow {
                    row: None,
                    id: id.clone(),
                    values,
                }
            })
            .collect();
        Self {
            id_column: id_column.to_string(),
            columns: columns.into_iter().collect(),
            rows,
        }
    }

    /// Append inline rows whose id the file does not define. A file row
    /// overwrites an inline row with the same id at load time, so the
    /// shadowed inline row is not an effective source row.
    pub(crate) fn extend_with_inline(
        &mut self,
        inline: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    ) {
        let from_file: BTreeSet<String> = self.rows.iter().map(|row| row.id.clone()).collect();
        let mut extra: Vec<&String> = inline
            .keys()
            .filter(|id| !from_file.contains(*id))
            .collect();
        extra.sort();
        for id in extra {
            let values: BTreeMap<String, String> = inline[id]
                .iter()
                .map(|(column, value)| (column.clone(), value.clone()))
                .collect();
            self.columns.extend(values.keys().cloned());
            self.rows.push(MetadataRow {
                row: None,
                id: id.clone(),
                values,
            });
        }
        self.columns.sort();
        self.columns.dedup();
    }
}
