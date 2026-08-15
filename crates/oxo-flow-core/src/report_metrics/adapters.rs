//! Tool-specific metric adapters (issue #83 P1-5): parse the six supported
//! tool outputs into normalized [`MetricValue`]s.
//!
//! Every adapter returns metrics in a stable, adapter-defined order; every
//! metric carries an optional QC flag derived from fixed thresholds. Adapters
//! never panic on malformed input: structural problems (invalid JSON, no
//! separator, no data rows) return [`crate::error::OxoFlowError::Report`];
//! missing fields yield absent metrics instead of errors.

use super::MetricValue;
use crate::error::{OxoFlowError, Result};
use crate::report::QcStatusLevel;

/// Build a [`MetricValue`] with an owned name.
fn metric(name: &str, value: f64, flag: Option<QcStatusLevel>) -> MetricValue {
    MetricValue {
        name: name.to_string(),
        value,
        flag,
    }
}

/// Dispatch to the adapter for `tool` (tools come from
/// [`super::classify_filename`], which only returns known names).
pub(super) fn parse_for_tool(tool: &str, text: &str) -> Result<Vec<MetricValue>> {
    match tool {
        "fastp" => parse_fastp_json(text),
        "flagstat" => parse_flagstat(text),
        "star" => parse_star_final_out(text),
        "featurecounts" => parse_featurecounts_summary(text),
        "bcftools" => parse_bcftools_stats(text),
        "kraken2" => parse_kraken2_report(text),
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
/// loci`. Real logs print percentages with a trailing `%` (e.g. `87.34%`),
/// which is stripped before parsing. Lines without a `|` separator are
/// ignored (headers, timestamps); a file with no `key | value` lines at all
/// is not a STAR log.
pub fn parse_star_final_out(text: &str) -> Result<Vec<MetricValue>> {
    let mut uniquely_mapped_pct: Option<f64> = None;
    let mut multimapping_pct: Option<f64> = None;
    let mut saw_separator = false;

    for line in text.lines() {
        let Some((key, value)) = line.split_once('|') else {
            continue;
        };
        saw_separator = true;
        let parsed = value
            .trim()
            .trim_end_matches('%')
            .trim()
            .parse::<f64>()
            .ok();
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

    /// Real STAR logs print percentages with a trailing `%` — the adapter
    /// must strip it (issue #83 P1-5 review).
    const STAR_HAPPY: &str = "                                 Started job on |\tJun 06 12:00:00\n\
                              Number of input reads |\t1000000\n\
                              Uniquely mapped reads % |\t87.34%\n\
                              Average mapped length |\t149.9\n\
                              % of reads mapped to multiple loci |\t1.23%\n";

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
        let star = |pct: f64| format!("Uniquely mapped reads % |\t{pct}%\n");
        let flag = |text: &str| parse_star_final_out(text).unwrap()[0].flag.clone();
        assert_eq!(flag(&star(70.0)), Some(QcStatusLevel::Pass));
        assert_eq!(flag(&star(65.0)), Some(QcStatusLevel::Warn));
        assert_eq!(flag(&star(59.9)), Some(QcStatusLevel::Fail));
    }

    #[test]
    fn star_values_without_percent_signs_still_parse() {
        let metrics = parse_star_final_out("Uniquely mapped reads % |\t85.0\n").unwrap();
        assert_eq!(metrics[0].value, 85.0);
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
}
