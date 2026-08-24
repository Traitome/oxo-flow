//! Normalized QC metrics protocol and the filesystem scanner that discovers
//! tool output files and attributes their metrics to samples (issue #83
//! P1-5). The six tool-specific parsers live in `adapters`.
//!
//! Every adapter returns metrics in a stable, adapter-defined order; every
//! metric carries an optional QC flag derived from fixed thresholds. The
//! scanner never panics: unreadable files and parse failures are counted and
//! reported through [`ScanStats`] instead.

mod adapters;
pub use adapters::*;

use crate::report::QcStatusLevel;
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
    /// (permissions, I/O errors), whose metadata could not be obtained, or
    /// whose directory entries/listing failed.
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
        let mut entries: Vec<std::fs::DirEntry> = Vec::new();
        match std::fs::read_dir(dir) {
            Ok(read_dir) => {
                for entry in read_dir {
                    match entry {
                        Ok(entry) => entries.push(entry),
                        // An entry that cannot be read (e.g. vanished mid-
                        // listing) is a data gap the caller must see.
                        Err(_) => stats.unreadable += 1,
                    }
                }
            }
            Err(_) => {
                // Sub-directories that cannot be listed are a data gap the
                // caller must see; the root itself just yields empty stats.
                if depth > 0 {
                    stats.unreadable += 1;
                }
                return;
            }
        }
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

            match adapters::parse_for_tool(tool, &text) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "Uniquely mapped reads % |\t85.0%\n",
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
