//! Metrics adapters: parse real tool outputs into a normalized QC metric
//! protocol, plus a filesystem scanner that discovers tool output files and
//! attributes their metrics to samples (issue #83 P1-5).
//!
//! Every adapter returns metrics in a stable, adapter-defined order; every
//! metric carries an optional QC flag derived from fixed thresholds. The
//! scanner never panics: unreadable files and parse failures are counted and
//! reported through [`ScanStats`] instead.

use crate::error::{OxoFlowError, Result};
pub use crate::report::QcStatusLevel;
use std::path::Path;

/// A single normalized QC metric extracted from a tool output.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricValue {
    /// Stable metric identifier (e.g. `"q30_rate"`).
    pub name: String,
    /// Numeric value in the tool's native scale (fraction 0.0–1.0 for rates,
    /// percentage for STAR/kraken2, raw counts for totals).
    pub value: f64,
    /// QC interpretation of the value; `None` = informational only.
    pub flag: Option<QcStatusLevel>,
}

/// Metrics parsed from one tool output file, attributed to a sample.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMetrics {
    /// Tool that produced the file (e.g. `"fastp"`).
    pub tool: String,
    /// Sample derived from the filename; `None` when the filename has no
    /// sample prefix (e.g. a bare `fastp.json`).
    pub sample: Option<String>,
    /// Extracted metrics in adapter-defined stable order.
    pub metrics: Vec<MetricValue>,
}

/// Scanner result: what was parsed and — critically — what was not
/// (a scanner that cannot report its own gaps looks like it covered
/// everything, issue #83 P1-5).
#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    /// Successfully parsed metrics.
    pub parsed: Vec<ParsedMetrics>,
    /// Files that matched a known tool pattern but were skipped: parse
    /// failures, files larger than the size cap, non-UTF-8 content.
    pub skipped: usize,
    /// Files that matched a known tool pattern but could not be read at all
    /// (permissions, I/O errors) or whose metadata could not be obtained.
    pub unreadable: usize,
}

/// Recursive scanner for tool output files under a directory.
///
/// Walk is depth-limited, never follows symlinks, skips dot-directories
/// (including `.oxo-flow`), and caps every file read at `max_file_bytes`.
/// Directory entries are visited in sorted order so results are
/// deterministic across runs.
pub struct MetricsScanner {
    max_file_bytes: usize,
    max_depth: usize,
}

const DEFAULT_MAX_FILE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_DEPTH: usize = 8;

impl Default for MetricsScanner {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

impl MetricsScanner {
    /// Scanner with default limits (1 MiB per file, depth 8).
    pub fn new() -> Self {
        Self::default()
    }

    /// Scanner with explicit limits (used by tests and embedders).
    pub fn with_limits(max_file_bytes: usize, max_depth: usize) -> Self {
        Self {
            max_file_bytes,
            max_depth,
        }
    }

    /// Convenience: scan `dir` and return only the parsed metrics.
    pub fn scan(&self, dir: &Path) -> Vec<ParsedMetrics> {
        self.scan_with_stats(dir).parsed
    }

    /// Scan `dir`, reporting parsed metrics together with skip/unreadable
    /// counters. A missing or unreadable root directory yields empty stats,
    /// never an error.
    pub fn scan_with_stats(&self, dir: &Path) -> ScanStats {
        let mut stats = ScanStats::default();
        self.walk(dir, 0, &mut stats);
        stats
    }

    fn walk(&self, dir: &Path, depth: usize, stats: &mut ScanStats) {
        let mut entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
            Ok(read_dir) => read_dir.filter_map(|e| e.ok()).collect(),
            Err(_) => {
                // Sub-directories that cannot be listed are a data gap the
                // caller must see; the root itself just yields empty stats.
                if depth > 0 {
                    stats.unreadable += 1;
                }
                return;
            }
        };
        // Deterministic visit order — read_dir order is filesystem-defined.
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => {
                    stats.unreadable += 1;
                    continue;
                }
            };
            // Never follow symlinks; dot-dirs (.oxo-flow, .hidden) are
            // engine/workspace internals, not sample data.
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if name_str.starts_with('.') {
                    continue;
                }
                if depth < self.max_depth {
                    self.walk(&entry.path(), depth + 1, stats);
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            let Some((tool, sample)) = classify_filename(&name_str) else {
                continue;
            };

            // Size cap: skip oversize files entirely (a truncated parse
            // would only be reported as a confusing failure).
            match entry.metadata() {
                Ok(metadata) if metadata.len() > self.max_file_bytes as u64 => {
                    stats.skipped += 1;
                    continue;
                }
                Ok(_) => {}
                Err(_) => {
                    stats.unreadable += 1;
                    continue;
                }
            }

            let content = match std::fs::read(entry.path()) {
                Ok(content) => content,
                Err(_) => {
                    stats.unreadable += 1;
                    continue;
                }
            };
            let text = match String::from_utf8(content) {
                Ok(text) => text,
                Err(_) => {
                    stats.skipped += 1;
                    continue;
                }
            };

            match parse_for_tool(tool, &text) {
                Ok(metrics) if metrics.is_empty() => {
                    // Parsed cleanly but nothing extractable — not a
                    // failure, and not report-worthy noise.
                }
                Ok(metrics) => stats.parsed.push(ParsedMetrics {
                    tool: tool.to_string(),
                    sample,
                    metrics,
                }),
                Err(_) => stats.skipped += 1,
            }
        }
    }
}

// ── Filename classification & sample attribution ──────────────────────────

/// Suffix → tool. Order matters: checked top-down, first match wins.
/// Matching is case-insensitive; sample attribution strips the matched
/// suffix (in original case) from the filename.
const TOOL_SUFFIXES: &[(&str, &str)] = &[
    ("fastp.json", "fastp"),
    (".flagstat", "flagstat"),
    ("flagstat.txt", "flagstat"),
    ("log.final.out", "star"),
    (".summary", "featurecounts"),
    (".bcftools.stats", "bcftools"),
    (".kraken2.report", "kraken2"),
    (".kraken.report", "kraken2"),
];

/// Classify a filename into `(tool, sample)`; `None` when no known tool
/// pattern matches. Sample attribution follows the issue #83 P1-5 rulings:
/// strip the tool suffix together with its separator dot —
/// `S1.fastp.json` → `S1`, `S1.Log.final.out` → `S1`, `S1.flagstat` → `S1`,
/// `S1.bcftools.stats` → `S1`, `S1.kraken2.report` → `S1`, and any
/// `*.summary` → the part before `.summary`. A filename that is exactly the
/// suffix (`fastp.json`) leaves an empty remainder → sample = `None`.
fn classify_filename(name: &str) -> Option<(&'static str, Option<String>)> {
    let lower = name.to_lowercase();
    for (suffix, tool) in TOOL_SUFFIXES {
        if lower.ends_with(suffix) {
            // Include the separator dot in the strip (S1.fastp.json minus
            // ".fastp.json" = "S1"); a bare `fastp.json` has none.
            let mut strip_len = suffix.len();
            let before = lower.len().checked_sub(suffix.len() + 1);
            if before.is_some_and(|i| lower.as_bytes()[i] == b'.') {
                strip_len += 1;
            }
            let stripped = &name[..name.len() - strip_len];
            let sample = if stripped.is_empty() {
                None
            } else {
                Some(stripped.to_string())
            };
            return Some((tool, sample));
        }
    }
    None
}

// ── Adapters ──────────────────────────────────────────────────────────────

fn metric(name: &str, value: f64, flag: Option<QcStatusLevel>) -> MetricValue {
    MetricValue {
        name: name.to_string(),
        value,
        flag,
    }
}

fn parse_for_tool(tool: &str, text: &str) -> Result<Vec<MetricValue>> {
    match tool {
        "fastp" => parse_fastp_json(text),
        "flagstat" => parse_flagstat(text),
        "star" => parse_star_final_out(text),
        "featurecounts" => parse_featurecounts_summary(text),
        "bcftools" => parse_bcftools_stats(text),
        "kraken2" => parse_kraken2_report(text),
        // classify_filename only returns known tools.
        _ => Err(OxoFlowError::Report {
            message: format!("unknown metrics tool: {tool}"),
        }),
    }
}

fn report_parse_error(tool: &str, message: impl Into<String>) -> OxoFlowError {
    OxoFlowError::Report {
        message: format!("{tool} output parse failed: {}", message.into()),
    }
}

/// Parse a fastp `report.json`.
///
/// Extracts `summary.before_filtering.total_reads`,
/// `summary.after_filtering.q30_rate`, `summary.after_filtering.gc_content`
/// and `summary.duplication.rate`. Missing keys yield absent metrics (not
/// an error); only invalid JSON fails.
pub fn parse_fastp_json(text: &str) -> Result<Vec<MetricValue>> {
    let json: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| report_parse_error("fastp", format!("report.json is not valid JSON: {e}")))?;
    let summary = json.get("summary");
    let as_f64 = |v: &serde_json::Value| v.as_f64();

    let mut metrics = Vec::new();
    if let Some(before) = summary.and_then(|s| s.get("before_filtering"))
        && let Some(total_reads) = before.get("total_reads").and_then(as_f64)
    {
        metrics.push(metric("total_reads", total_reads, None));
    }
    if let Some(after) = summary.and_then(|s| s.get("after_filtering")) {
        if let Some(q30_rate) = after.get("q30_rate").and_then(as_f64) {
            let flag = if q30_rate >= 0.85 {
                QcStatusLevel::Pass
            } else if q30_rate >= 0.75 {
                QcStatusLevel::Warn
            } else {
                QcStatusLevel::Fail
            };
            metrics.push(metric("q30_rate", q30_rate, Some(flag)));
        }
        if let Some(gc_content) = after.get("gc_content").and_then(as_f64) {
            metrics.push(metric("gc_content", gc_content, None));
        }
    }
    if let Some(dup_rate) = summary
        .and_then(|s| s.get("duplication"))
        .and_then(|d| d.get("rate"))
        .and_then(as_f64)
    {
        let flag = if dup_rate >= 0.5 {
            QcStatusLevel::Warn
        } else {
            QcStatusLevel::Info
        };
        metrics.push(metric("duplication_rate", dup_rate, Some(flag)));
    }
    Ok(metrics)
}

/// Parse the leading unsigned integer of a line (flagstat counts).
fn parse_leading_u64(line: &str) -> Option<u64> {
    line.split_whitespace().next()?.parse().ok()
}

/// Parse `samtools flagstat` output.
///
/// Line 1 `N + 0 in total (...)` → `total_reads`; the `N + 0 mapped (...)` /
/// `N + 0 properly paired (...)` lines become rates against the total. A
/// zero total guards the divisions (rate metrics are skipped, not an
/// error); a missing first line or non-numeric first line is malformed.
pub fn parse_flagstat(text: &str) -> Result<Vec<MetricValue>> {
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| report_parse_error("flagstat", "empty output — missing 'in total' line"))?;
    let total = parse_leading_u64(first).ok_or_else(|| {
        report_parse_error(
            "flagstat",
            format!("first line does not start with a read count: '{first}'"),
        )
    })?;
    let mut metrics = vec![metric("total_reads", total as f64, None)];

    if total == 0 {
        // Guard division by zero: no rates can be computed.
        return Ok(metrics);
    }

    let mut mapped: Option<u64> = None;
    let mut properly_paired: Option<u64> = None;
    for line in lines {
        if line.contains(" mapped (") && mapped.is_none() {
            mapped = parse_leading_u64(line);
        } else if line.contains(" properly paired (") && properly_paired.is_none() {
            properly_paired = parse_leading_u64(line);
        }
    }

    if let Some(mapped) = mapped {
        let mapped_rate = mapped as f64 / total as f64;
        let flag = if mapped_rate >= 0.90 {
            QcStatusLevel::Pass
        } else if mapped_rate >= 0.80 {
            QcStatusLevel::Warn
        } else {
            QcStatusLevel::Fail
        };
        metrics.push(metric("mapped_rate", mapped_rate, Some(flag)));
    }
    if let Some(properly_paired) = properly_paired {
        metrics.push(metric(
            "properly_paired_rate",
            properly_paired as f64 / total as f64,
            None,
        ));
    }
    Ok(metrics)
}

/// Parse a STAR `Log.final.out` (`key | value` lines).
///
/// Extracts `Uniquely mapped reads %` and `% of reads mapped to multiple
/// loci`. Lines without a `|` separator are ignored (headers, timestamps);
/// a file with no `key | value` lines at all is not a STAR log.
pub fn parse_star_final_out(text: &str) -> Result<Vec<MetricValue>> {
    let mut uniquely_mapped_pct: Option<f64> = None;
    let mut multimapping_pct: Option<f64> = None;
    let mut saw_separator = false;

    for line in text.lines() {
        let Some((key, value)) = line.split_once('|') else {
            continue;
        };
        saw_separator = true;
        let parsed = value.trim().parse::<f64>().ok();
        match key.trim() {
            "Uniquely mapped reads %" => uniquely_mapped_pct = parsed,
            "% of reads mapped to multiple loci" => multimapping_pct = parsed,
            _ => {}
        }
    }
    if !saw_separator {
        return Err(report_parse_error(
            "STAR",
            "not a Log.final.out: no 'key | value' lines",
        ));
    }

    let mut metrics = Vec::new();
    if let Some(pct) = uniquely_mapped_pct {
        let flag = if pct >= 70.0 {
            QcStatusLevel::Pass
        } else if pct >= 60.0 {
            QcStatusLevel::Warn
        } else {
            QcStatusLevel::Fail
        };
        metrics.push(metric("uniquely_mapped_pct", pct, Some(flag)));
    }
    if let Some(pct) = multimapping_pct {
        metrics.push(metric("multimapping_pct", pct, None));
    }
    Ok(metrics)
}

/// Parse a featureCounts `*.summary` (tab-separated `Status\tcount`).
///
/// Sums all counts as `total_count`; `Assigned / total` → `assigned_rate`.
/// Non-numeric rows (headers) are skipped; a file with no numeric row at
/// all is malformed; a zero total skips the rate.
pub fn parse_featurecounts_summary(text: &str) -> Result<Vec<MetricValue>> {
    let mut total: u64 = 0;
    let mut assigned: u64 = 0;
    let mut saw_numeric = false;

    for line in text.lines() {
        let mut fields = line.split('\t');
        let status = fields.next().unwrap_or("").trim();
        let Some(count) = fields.next().and_then(|c| c.trim().parse::<u64>().ok()) else {
            continue;
        };
        saw_numeric = true;
        total += count;
        if status == "Assigned" {
            assigned = count;
        }
    }
    if !saw_numeric {
        return Err(report_parse_error(
            "featureCounts",
            "not a .summary file: no 'Status<TAB>count' data rows",
        ));
    }

    let mut metrics = vec![metric("total_count", total as f64, None)];
    if total > 0 {
        let assigned_rate = assigned as f64 / total as f64;
        let flag = if assigned_rate >= 0.60 {
            QcStatusLevel::Pass
        } else if assigned_rate >= 0.40 {
            QcStatusLevel::Warn
        } else {
            QcStatusLevel::Fail
        };
        metrics.push(metric("assigned_rate", assigned_rate, Some(flag)));
    }
    Ok(metrics)
}

/// Parse `bcftools stats` output (tab-separated `SN\t0\t<stat>:\t<value>`).
///
/// Extracts the number of SNPs, the number of indels and the ts/tv ratio.
/// ts/tv values may be a plain float (`1.24`) or an `x/y` division (guarded
/// against `y == 0`). All metrics are informational — no thresholds.
pub fn parse_bcftools_stats(text: &str) -> Result<Vec<MetricValue>> {
    let mut snps: Option<f64> = None;
    let mut indels: Option<f64> = None;
    let mut ts_tv_ratio: Option<f64> = None;
    let mut saw_sn_line = false;

    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 4 || fields[0] != "SN" {
            continue;
        }
        saw_sn_line = true;
        let value = fields[3].trim();
        match fields[2] {
            "number of SNPs:" => snps = value.parse::<f64>().ok(),
            "number of indels:" => indels = value.parse::<f64>().ok(),
            "ts/tv:" => {
                ts_tv_ratio = if let Some((x, y)) = value.split_once('/') {
                    let (x, y) = (x.parse::<f64>().ok(), y.parse::<f64>().ok());
                    match (x, y) {
                        // Guard division by zero: skip the metric.
                        (Some(x), Some(y)) if y != 0.0 => Some(x / y),
                        _ => None,
                    }
                } else {
                    value.parse::<f64>().ok()
                };
            }
            _ => {}
        }
    }
    if !saw_sn_line {
        return Err(report_parse_error(
            "bcftools",
            "not a stats output: no 'SN' summary lines",
        ));
    }

    let mut metrics = Vec::new();
    if let Some(snps) = snps {
        metrics.push(metric("snps", snps, Some(QcStatusLevel::Info)));
    }
    if let Some(indels) = indels {
        metrics.push(metric("indels", indels, Some(QcStatusLevel::Info)));
    }
    if let Some(ratio) = ts_tv_ratio {
        metrics.push(metric("ts_tv_ratio", ratio, Some(QcStatusLevel::Info)));
    }
    Ok(metrics)
}

/// Parse a kraken2 `*.report` (tab-separated
/// `pct\tclade_count\ttax_rank\ttaxid\tname`; some lines may carry extra
/// columns).
///
/// The unclassified row (`tax_rank == "U"`) yields `unclassified_rate` (its
/// percentage). Per the issue #83 P1-5 ruling, the top species is a string
/// and has no place in the numeric metric protocol, so it is not extracted.
pub fn parse_kraken2_report(text: &str) -> Result<Vec<MetricValue>> {
    let mut unclassified_rate: Option<f64> = None;
    let mut saw_row = false;

    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 5 {
            continue;
        }
        let Some(pct) = fields[0].trim().parse::<f64>().ok() else {
            continue;
        };
        saw_row = true;
        if fields[2] == "U" {
            unclassified_rate = Some(pct);
        }
    }
    if !saw_row {
        return Err(report_parse_error(
            "kraken2",
            "not a report: no tab-separated taxonomy rows",
        ));
    }

    let mut metrics = Vec::new();
    if let Some(rate) = unclassified_rate {
        let flag = if rate <= 20.0 {
            QcStatusLevel::Pass
        } else if rate <= 40.0 {
            QcStatusLevel::Warn
        } else {
            QcStatusLevel::Fail
        };
        metrics.push(metric("unclassified_rate", rate, Some(flag)));
    }
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── fastp ────────────────────────────────────────────────────────────

    const FASTP_HAPPY: &str = r#"{
        "summary": {
            "before_filtering": { "total_reads": 1000000, "total_bases": 150000000 },
            "after_filtering": { "total_reads": 990000, "q30_rate": 0.92, "gc_content": 0.451 },
            "duplication": { "rate": 0.11 }
        }
    }"#;

    #[test]
    fn fastp_happy_path() {
        let metrics = parse_fastp_json(FASTP_HAPPY).unwrap();
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        // Adapter-defined stable order.
        assert_eq!(
            names,
            ["total_reads", "q30_rate", "gc_content", "duplication_rate"]
        );
        assert_eq!(metrics[0].value, 1_000_000.0);
        assert_eq!(metrics[0].flag, None);
        assert_eq!(metrics[1].value, 0.92);
        assert_eq!(metrics[1].flag, Some(QcStatusLevel::Pass));
        assert_eq!(metrics[2].value, 0.451);
        assert_eq!(metrics[3].value, 0.11);
        assert_eq!(metrics[3].flag, Some(QcStatusLevel::Info));
    }

    #[test]
    fn fastp_q30_thresholds() {
        let json = |q30: f64| {
            format!(
                r#"{{"summary": {{"before_filtering": {{"total_reads": 1}}, "after_filtering": {{"q30_rate": {q30}}}, "duplication": {{"rate": 0.1}}}}}}"#
            )
        };
        let q30_flag = |text: &str| {
            parse_fastp_json(text)
                .unwrap()
                .iter()
                .find(|m| m.name == "q30_rate")
                .unwrap()
                .flag
                .clone()
        };
        assert_eq!(q30_flag(&json(0.85)), Some(QcStatusLevel::Pass));
        assert_eq!(q30_flag(&json(0.80)), Some(QcStatusLevel::Warn));
        assert_eq!(q30_flag(&json(0.74)), Some(QcStatusLevel::Fail));
        // duplication.rate >= 0.5 is a warning.
        let dup = parse_fastp_json(
            r#"{"summary": {"after_filtering": {}, "duplication": {"rate": 0.6}}}"#,
        )
        .unwrap();
        assert_eq!(dup[0].flag, Some(QcStatusLevel::Warn));
    }

    #[test]
    fn fastp_missing_keys_are_absent_not_errors() {
        let metrics = parse_fastp_json("{}").unwrap();
        assert!(metrics.is_empty());
        let metrics =
            parse_fastp_json(r#"{"summary": {"before_filtering": {"total_reads": 42}}}"#).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "total_reads");
    }

    #[test]
    fn fastp_malformed_json_errors() {
        assert!(parse_fastp_json("not json at all").is_err());
        assert!(parse_fastp_json("").is_err());
    }

    // ── flagstat ─────────────────────────────────────────────────────────

    const FLAGSTAT_HAPPY: &str = "100000 + 0 in total (QC-passed reads + QC-failed reads)\n\
        95000 + 0 mapped (95.00% : N/A)\n\
        90000 + 0 properly paired (90.00% : N/A)\n\
        89000 + 0 with itself and mate mapped\n";

    #[test]
    fn flagstat_happy_path() {
        let metrics = parse_flagstat(FLAGSTAT_HAPPY).unwrap();
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["total_reads", "mapped_rate", "properly_paired_rate"]
        );
        assert_eq!(metrics[0].value, 100_000.0);
        assert_eq!(metrics[1].value, 0.95);
        assert_eq!(metrics[1].flag, Some(QcStatusLevel::Pass));
        assert_eq!(metrics[2].value, 0.90);
    }

    #[test]
    fn flagstat_mapping_thresholds() {
        let flagstat = |mapped: u64| {
            format!(
                "100000 + 0 in total (QC-passed reads + QC-failed reads)\n{mapped} + 0 mapped (x% : N/A)\n"
            )
        };
        let flag = |text: &str| parse_flagstat(text).unwrap()[1].flag.clone();
        assert_eq!(flag(&flagstat(90_000)), Some(QcStatusLevel::Pass));
        assert_eq!(flag(&flagstat(80_000)), Some(QcStatusLevel::Warn));
        assert_eq!(flag(&flagstat(79_000)), Some(QcStatusLevel::Fail));
    }

    #[test]
    fn flagstat_zero_total_skips_rates() {
        let metrics =
            parse_flagstat("0 + 0 in total (QC-passed reads + QC-failed reads)\n").unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "total_reads");
    }

    #[test]
    fn flagstat_missing_lines_are_absent_not_errors() {
        let metrics =
            parse_flagstat("42 + 0 in total (QC-passed reads + QC-failed reads)\n").unwrap();
        assert_eq!(metrics.len(), 1);
    }

    #[test]
    fn flagstat_malformed_errors() {
        assert!(parse_flagstat("").is_err());
        assert!(parse_flagstat("not a count + 0 in total").is_err());
    }

    // ── STAR ─────────────────────────────────────────────────────────────

    const STAR_HAPPY: &str = "                                 Started job on |\tJun 06 12:00:00\n\
                              Number of input reads |\t1000000\n\
                              Uniquely mapped reads % |\t87.34\n\
                              Average mapped length |\t149.9\n\
                              % of reads mapped to multiple loci |\t1.23\n";

    #[test]
    fn star_happy_path() {
        let metrics = parse_star_final_out(STAR_HAPPY).unwrap();
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["uniquely_mapped_pct", "multimapping_pct"]);
        assert_eq!(metrics[0].value, 87.34);
        assert_eq!(metrics[0].flag, Some(QcStatusLevel::Pass));
        assert_eq!(metrics[1].value, 1.23);
        assert_eq!(metrics[1].flag, None);
    }

    #[test]
    fn star_mapping_thresholds() {
        let star = |pct: f64| format!("Uniquely mapped reads % |\t{pct}\n");
        let flag = |text: &str| parse_star_final_out(text).unwrap()[0].flag.clone();
        assert_eq!(flag(&star(70.0)), Some(QcStatusLevel::Pass));
        assert_eq!(flag(&star(65.0)), Some(QcStatusLevel::Warn));
        assert_eq!(flag(&star(59.9)), Some(QcStatusLevel::Fail));
    }

    #[test]
    fn star_missing_keys_are_absent_not_errors() {
        let metrics = parse_star_final_out("Started job on |\tJun 06 12:00:00\n").unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn star_malformed_errors() {
        assert!(parse_star_final_out("no separator anywhere").is_err());
        assert!(parse_star_final_out("").is_err());
    }

    // ── featureCounts ─────────────────────────────────────────────────────

    const FEATURECOUNTS_HAPPY: &str = "Status\t/workspace/ref.gtf\n\
        Assigned\t90\n\
        Unassigned_NoFeatures\t8\n\
        Unassigned_Ambiguity\t2\n";

    #[test]
    fn featurecounts_happy_path() {
        let metrics = parse_featurecounts_summary(FEATURECOUNTS_HAPPY).unwrap();
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["total_count", "assigned_rate"]);
        assert_eq!(metrics[0].value, 100.0);
        assert_eq!(metrics[1].value, 0.90);
        assert_eq!(metrics[1].flag, Some(QcStatusLevel::Pass));
    }

    #[test]
    fn featurecounts_assignment_thresholds() {
        let summary = |assigned: u64| {
            format!(
                "Status\tref.gtf\nAssigned\t{assigned}\nUnassigned_NoFeatures\t{}\n",
                100 - assigned
            )
        };
        let flag = |text: &str| parse_featurecounts_summary(text).unwrap()[1].flag.clone();
        assert_eq!(flag(&summary(60)), Some(QcStatusLevel::Pass));
        assert_eq!(flag(&summary(50)), Some(QcStatusLevel::Warn));
        assert_eq!(flag(&summary(39)), Some(QcStatusLevel::Fail));
    }

    #[test]
    fn featurecounts_non_numeric_rows_are_skipped() {
        // A junk row between valid data rows must not break parsing.
        let metrics =
            parse_featurecounts_summary("Status\tref.gtf\nAssigned\t10\njunk\tnot-a-number\n")
                .unwrap();
        assert_eq!(metrics[0].value, 10.0);
    }

    #[test]
    fn featurecounts_malformed_errors() {
        assert!(parse_featurecounts_summary("").is_err());
        assert!(parse_featurecounts_summary("Status\tref.gtf\n").is_err());
    }

    // ── bcftools ──────────────────────────────────────────────────────────

    const BCFTOOLS_HAPPY: &str = "SN\t0\tnumber of samples:\t1\n\
        SN\t0\tnumber of records:\t100\n\
        SN\t0\tnumber of SNPs:\t50\n\
        SN\t0\tnumber of indels:\t10\n\
        SN\t0\tts/tv:\t1.24\n";

    #[test]
    fn bcftools_happy_path() {
        let metrics = parse_bcftools_stats(BCFTOOLS_HAPPY).unwrap();
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["snps", "indels", "ts_tv_ratio"]);
        assert_eq!(metrics[0].value, 50.0);
        assert_eq!(metrics[0].flag, Some(QcStatusLevel::Info));
        assert_eq!(metrics[1].value, 10.0);
        assert_eq!(metrics[2].value, 1.24);
        assert_eq!(metrics[2].flag, Some(QcStatusLevel::Info));
    }

    #[test]
    fn bcftools_ts_tv_division_and_zero_guard() {
        // x/y form divides; y == 0 skips the metric entirely.
        let metrics =
            parse_bcftools_stats("SN\t0\tnumber of SNPs:\t2\nSN\t0\tts/tv:\t2.5/0.5\n").unwrap();
        assert_eq!(metrics[1].name, "ts_tv_ratio");
        assert!((metrics[1].value - 5.0).abs() < 1e-9);
        let metrics =
            parse_bcftools_stats("SN\t0\tnumber of SNPs:\t2\nSN\t0\tts/tv:\t2.5/0\n").unwrap();
        assert_eq!(metrics.len(), 1);
    }

    #[test]
    fn bcftools_missing_keys_are_absent_not_errors() {
        let metrics = parse_bcftools_stats("SN\t0\tnumber of samples:\t1\n").unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn bcftools_malformed_errors() {
        assert!(parse_bcftools_stats("").is_err());
        assert!(parse_bcftools_stats("hello\n").is_err());
    }

    // ── kraken2 ───────────────────────────────────────────────────────────

    const KRAKEN2_HAPPY: &str = "100.00\t1000\tU\t0\tunclassified\n\
        50.00\t500\tR\t1\troot\n\
        45.00\t450\tS\t562\tEscherichia coli\n";

    #[test]
    fn kraken2_happy_path() {
        let metrics = parse_kraken2_report(KRAKEN2_HAPPY).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "unclassified_rate");
        // Percentages — thresholds are on the percentage scale.
        assert_eq!(metrics[0].value, 100.0);
        assert_eq!(metrics[0].flag, Some(QcStatusLevel::Fail));
    }

    #[test]
    fn kraken2_six_column_rows_parse() {
        // A 6th column (name continuation) must not break row parsing.
        let metrics = parse_kraken2_report(
            "3.50\t35\tU\t0\tunclassified\n\
             90.00\t900\tS\t1280\tStaphylococcus\tstuff\n",
        )
        .unwrap();
        assert_eq!(metrics[0].value, 3.50);
        assert_eq!(metrics[0].flag, Some(QcStatusLevel::Pass));
    }

    #[test]
    fn kraken2_unclassified_thresholds() {
        let report = |pct: f64| format!("{pct}\t10\tU\t0\tunclassified\n");
        let flag = |text: &str| parse_kraken2_report(text).unwrap()[0].flag.clone();
        assert_eq!(flag(&report(20.0)), Some(QcStatusLevel::Pass));
        assert_eq!(flag(&report(30.0)), Some(QcStatusLevel::Warn));
        assert_eq!(flag(&report(41.0)), Some(QcStatusLevel::Fail));
    }

    #[test]
    fn kraken2_missing_unclassified_row_is_absent_not_error() {
        let metrics = parse_kraken2_report("50.00\t500\tR\t1\troot\n").unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn kraken2_malformed_errors() {
        assert!(parse_kraken2_report("").is_err());
        assert!(parse_kraken2_report("no tabs here\n").is_err());
    }

    // ── Scanner ───────────────────────────────────────────────────────────

    /// A valid minimal fastp report.json for scanner fixtures.
    fn fastp_json(total_reads: u64) -> String {
        format!(
            r#"{{"summary": {{"before_filtering": {{"total_reads": {total_reads}}}, "after_filtering": {{"q30_rate": 0.9}}, "duplication": {{"rate": 0.1}}}}}}"#
        )
    }

    #[test]
    fn scanner_parses_matching_files_and_skips_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = fastp_json(111);
        std::fs::write(dir.path().join("S1.fastp.json"), &s1).unwrap();
        // Matches the pattern but is not valid JSON → skipped.
        std::fs::write(dir.path().join("S2.fastp.json"), "garbage").unwrap();
        // Valid JSON but larger than the cap → skipped by the size limit.
        let s3 = format!(
            r#"{{"summary": {{"before_filtering": {{"total_reads": 333}}, "pad": "{}"}}}}"#,
            "x".repeat(300)
        );
        std::fs::write(dir.path().join("S3.fastp.json"), &s3).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "irrelevant").unwrap();
        std::fs::write(dir.path().join("S4.bam"), "binary").unwrap();

        let scanner = MetricsScanner::with_limits(s1.len() + 10, 8);
        let stats = scanner.scan_with_stats(dir.path());
        assert_eq!(stats.parsed.len(), 1, "only S1 fits within the cap");
        assert_eq!(stats.parsed[0].sample.as_deref(), Some("S1"));
        assert_eq!(stats.parsed[0].tool, "fastp");
        assert_eq!(stats.parsed[0].metrics[0].value, 111.0);
        assert_eq!(stats.skipped, 2, "invalid JSON + oversize file");
        assert_eq!(stats.unreadable, 0);
    }

    #[test]
    fn scanner_skips_dot_dirs_and_oxo_flow_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".oxo-flow")).unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".oxo-flow/S1.fastp.json"), fastp_json(1)).unwrap();
        std::fs::write(dir.path().join(".hidden/S2.fastp.json"), fastp_json(2)).unwrap();
        std::fs::write(dir.path().join("S3.fastp.json"), fastp_json(3)).unwrap();

        let parsed = MetricsScanner::new().scan(dir.path());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sample.as_deref(), Some("S3"));
    }

    #[test]
    fn scanner_respects_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/S1.fastp.json"), fastp_json(1)).unwrap();
        std::fs::write(dir.path().join("a/b/S2.fastp.json"), fastp_json(2)).unwrap();

        // depth 1: root files + one level of subdirectories.
        let parsed = MetricsScanner::with_limits(1 << 20, 1).scan(dir.path());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sample.as_deref(), Some("S1"));

        let parsed = MetricsScanner::with_limits(1 << 20, 2).scan(dir.path());
        assert_eq!(parsed.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn scanner_never_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, fastp_json(7)).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("S1.fastp.json")).unwrap();

        let parsed = MetricsScanner::new().scan(dir.path());
        assert!(parsed.is_empty(), "symlinked fastp.json must not be parsed");
    }

    #[test]
    fn scanner_sample_attribution() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("S1.fastp.json"), fastp_json(1)).unwrap();
        std::fs::write(
            dir.path().join("S2.Log.final.out"),
            "Uniquely mapped reads % |\t85.0\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("S3.flagstat"),
            "10 + 0 in total (QC-passed reads + QC-failed reads)\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("S4.flagstat.txt"),
            "10 + 0 in total (QC-passed reads + QC-failed reads)\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("S5.featureCounts.summary"),
            "Status\tref\nAssigned\t1\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("S6.bcftools.stats"),
            "SN\t0\tnumber of SNPs:\t5\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("S7.kraken2.report"),
            "3.5\t35\tU\t0\tunclassified\n",
        )
        .unwrap();
        // No sample prefix — sample attribution yields None.
        std::fs::write(dir.path().join("fastp.json"), fastp_json(9)).unwrap();

        // Entries are visited in sorted filename order → deterministic.
        let parsed = MetricsScanner::new().scan(dir.path());
        let samples: Vec<(String, Option<String>)> = parsed
            .iter()
            .map(|p| (p.tool.clone(), p.sample.clone()))
            .collect();
        assert_eq!(
            samples,
            vec![
                ("fastp".into(), Some("S1".into())),
                ("star".into(), Some("S2".into())),
                ("flagstat".into(), Some("S3".into())),
                ("flagstat".into(), Some("S4".into())),
                ("featurecounts".into(), Some("S5.featureCounts".into())),
                ("bcftools".into(), Some("S6".into())),
                ("kraken2".into(), Some("S7".into())),
                ("fastp".into(), None),
            ]
        );
    }

    #[test]
    fn scanner_matches_filenames_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("S1.FASTP.JSON"), fastp_json(21)).unwrap();
        let parsed = MetricsScanner::new().scan(dir.path());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sample.as_deref(), Some("S1"));
        assert_eq!(parsed[0].metrics[0].value, 21.0);
    }

    #[cfg(unix)]
    #[test]
    fn scanner_counts_unreadable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("S1.fastp.json");
        std::fs::write(&path, fastp_json(1)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let stats = MetricsScanner::new().scan_with_stats(dir.path());
        match (stats.unreadable, stats.parsed.len()) {
            // Non-root: the file cannot be opened → unreadable.
            (1, 0) => {}
            // Root (CI): the file is readable after all → parsed.
            (0, 1) => {}
            other => panic!("unexpected scanner outcome: {other:?}"),
        }
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    }
}
