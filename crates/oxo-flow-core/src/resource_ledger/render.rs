//! JSON and CSV rendering for the normalized resource ledger.

use super::*;

impl ResourceLedger {
    /// Render the canonical JSON document (pretty-printed, trailing newline).
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self).map_err(|e| OxoFlowError::Config {
            message: format!("failed to serialize the resource ledger: {e}"),
        })?;
        json.push('\n');
        Ok(json)
    }

    /// Render the CSV document: one row per rule plus a `TOTAL` row.
    ///
    /// Column order is [`CSV_HEADER`]; unknown values are empty cells (never
    /// `0`), and the `TOTAL` row lists any partially reported metrics in
    /// `incomplete_metrics`. The reserved metric columns mirror the JSON
    /// schema (empty until a backend reports them), and `source_raw_fields`
    /// carries the same key-ordered raw map as JSON, serialized compactly.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str(&CSV_HEADER.join(","));
        out.push('\n');
        for rule in &self.rules {
            let row = vec![
                "rule".to_string(),
                rule.rule.clone(),
                rule.status.clone(),
                opt_f64(rule.wall_time_secs),
                opt_f64(rule.cpu_seconds),
                opt_f64(rule.user_seconds),
                opt_f64(rule.system_seconds),
                opt_u64(rule.max_rss_bytes),
                opt_u64(rule.peak_memory_bytes),
                opt_u64(rule.io_read_bytes),
                opt_u64(rule.io_write_bytes),
                opt_u64(rule.scheduler_wait_secs),
                opt_u32(rule.nodes),
                opt_u32(rule.cores),
                opt_f64(rule.gpu_seconds),
                rule.cost
                    .as_ref()
                    .map(|c| format!("{:.3}", c.amount))
                    .unwrap_or_default(),
                rule.cost
                    .as_ref()
                    .map(|c| c.currency.clone())
                    .unwrap_or_default(),
                rule.cost
                    .as_ref()
                    .map(|c| c.method.clone())
                    .unwrap_or_default(),
                rule.source.backend.clone(),
                rule.source.tool.clone(),
                rule.source.version.clone().unwrap_or_default(),
                rule.source.note.clone().unwrap_or_default(),
                raw_fields_cell(&rule.source.raw_fields),
                rule.recorded_as.clone().unwrap_or_default(),
                // Cost-budget columns are a run-level view (issue #1400):
                // per-rule rows leave them empty, the TOTAL row carries them.
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ];
            out.push_str(&csv_row(&row));
            out.push('\n');
        }

        let totals = &self.run.totals;
        let total_row = vec![
            "total".to_string(),
            "TOTAL".to_string(),
            String::new(),
            opt_f64(totals.wall_time_secs),
            opt_f64(totals.cpu_seconds),
            String::new(),
            String::new(),
            opt_u64(totals.max_rss_bytes),
            String::new(),
            opt_u64(totals.io_read_bytes),
            opt_u64(totals.io_write_bytes),
            opt_u64(totals.scheduler_wait_secs),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            opt_f64(self.budget.as_ref().map(|b| b.amount)),
            self.budget
                .as_ref()
                .map(|b| b.currency.clone())
                .unwrap_or_default(),
            self.cost_accrual
                .as_ref()
                .and_then(|a| a.accrued)
                .map(|v| format!("{v:.3}"))
                .unwrap_or_default(),
            self.cost_accrual
                .as_ref()
                .map(|a| a.unknown_rules.join(";"))
                .unwrap_or_default(),
            incomplete_metrics(totals).join(";"),
        ];
        out.push_str(&csv_row(&total_row));
        out.push('\n');
        out
    }
}

/// Stable CSV column order. Documented in `docs/guide/src/commands/report.md`
/// — changing it is a breaking change for downstream spreadsheets. The three
/// reserved metric columns (`user_seconds`, `system_seconds`,
/// `peak_memory_bytes`) keep CSV and JSON shapes aligned even though no
/// backend reports them yet; `source_raw_fields` is the same key-ordered raw
/// map JSON carries, serialized as compact JSON (task-7c M-4).
///
/// The four cost-budget columns (`budget_amount`, `budget_currency`,
/// `accrued_cost`, `cost_unknown_rules`) are a run-level view (issue #1400):
/// per-rule rows leave them empty and the `TOTAL` row carries them. They are
/// empty unless the workflow declared `[resource_budget] max_cost`, and
/// `accrued_cost` stays empty (never `0`) when a rate's input metric is
/// missing — `cost_unknown_rules` names the executed rules it could not cost.
pub const CSV_HEADER: [&str; 29] = [
    "record_type",
    "rule",
    "status",
    "wall_time_secs",
    "cpu_seconds",
    "user_seconds",
    "system_seconds",
    "max_rss_bytes",
    "peak_memory_bytes",
    "io_read_bytes",
    "io_write_bytes",
    "scheduler_wait_secs",
    "nodes",
    "cores",
    "gpu_seconds",
    "cost_amount",
    "cost_currency",
    "cost_method",
    "source_backend",
    "source_tool",
    "source_version",
    "source_note",
    "source_raw_fields",
    "recorded_as",
    "budget_amount",
    "budget_currency",
    "accrued_cost",
    "cost_unknown_rules",
    "incomplete_metrics",
];

/// Raw scheduler fields as one CSV cell: compact JSON with `BTreeMap` key
/// order (the same bytes JSON rendering uses), empty when the source
/// recorded none. Serializing a `String`-keyed map cannot fail.
fn raw_fields_cell(fields: &BTreeMap<String, String>) -> String {
    if fields.is_empty() {
        String::new()
    } else {
        serde_json::to_string(fields).expect("a string map always serializes")
    }
}

/// Metrics whose run total is a partial sum (some executed rules reported
/// nothing), for the CSV `TOTAL` row.
fn incomplete_metrics(totals: &LedgerTotals) -> Vec<&'static str> {
    let mut out = Vec::new();
    if totals.wall_time_secs.is_some() && !totals.wall_time_secs_complete {
        out.push("wall_time_secs");
    }
    if totals.cpu_seconds.is_some() && !totals.cpu_seconds_complete {
        out.push("cpu_seconds");
    }
    if totals.max_rss_bytes.is_some() && !totals.max_rss_bytes_complete {
        out.push("max_rss_bytes");
    }
    if totals.io_read_bytes.is_some() && !totals.io_read_bytes_complete {
        out.push("io_read_bytes");
    }
    if totals.io_write_bytes.is_some() && !totals.io_write_bytes_complete {
        out.push("io_write_bytes");
    }
    if totals.scheduler_wait_secs.is_some() && !totals.scheduler_wait_secs_complete {
        out.push("scheduler_wait_secs");
    }
    out
}

fn opt_f64(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.3}")).unwrap_or_default()
}

fn opt_u64(value: Option<u64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

fn opt_u32(value: Option<u32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// Render one CSV row with standard quoting: a field containing a comma,
/// quote, CR, or LF is wrapped in double quotes, and embedded quotes double.
fn csv_row(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| {
            if field.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", field.replace('"', "\"\""))
            } else {
                field.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
