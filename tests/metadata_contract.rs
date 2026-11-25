//! Issue #1368: validated cohort and metadata contracts through the REAL
//! `validate --json` / `dry-run --json` surfaces.
//!
//! Every case in `tests/fixtures/metadata/` is an independently-authored
//! workflow + table pair (the truth set recorded in
//! `docs/superpowers/audits/metadata-contracts-2026-11-25.md`). These tests
//! spawn the compiled CLI — not a unit helper — so the exit codes, the
//! stable `metadata` array, the digest, and the human one-line-per-finding
//! output are all pinned end to end.

mod common;

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/metadata")
        .join(name)
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(common::workspace_bin("oxo-flow"))
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .expect("CLI runs")
}

fn validate_json(name: &str) -> (bool, Value) {
    let path = fixture(name);
    let output = run_cli(&["validate", "--json", path.to_str().unwrap()]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "validate --json {name} emitted non-JSON stdout ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output.status.success(), document)
}

/// `(code, severity, row, column, id)` for one finding.
type FindingTuple = (String, String, Option<u64>, Option<String>, Option<String>);

/// [`FindingTuple`] with borrowed strings, for the expected-case tables.
type ExpectedFinding<'a> = (
    &'a str,
    &'a str,
    Option<u64>,
    Option<&'a str>,
    Option<&'a str>,
);

/// `(code, severity, row, column, id)` for each finding, in the document's
/// (already sorted) order.
fn findings(document: &Value) -> Vec<FindingTuple> {
    document
        .get("metadata")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("validate document has no `metadata` array: {document}"))
        .iter()
        .map(|finding| {
            (
                finding["code"].as_str().unwrap().to_string(),
                finding["severity"].as_str().unwrap().to_string(),
                finding["row"].as_u64(),
                finding["column"].as_str().map(str::to_string),
                finding["id"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn digest(document: &Value) -> String {
    document["metadata_digest"]
        .as_str()
        .unwrap_or_else(|| panic!("document has no metadata_digest: {document}"))
        .to_string()
}

#[test]
fn clean_tsv_csv_and_json_pass_with_identical_digests() {
    for name in ["clean.oxoflow", "clean_csv.oxoflow", "clean_json.oxoflow"] {
        let (success, document) = validate_json(name);
        assert!(success, "{name} must validate: {document}");
        assert_eq!(document["valid"], true, "{name}: {document}");
        assert!(findings(&document).is_empty(), "{name}: {document}");
        assert!(digest(&document).starts_with("sha256:"));
    }
    // Same logical rows in three formats → one canonical digest.
    let tsv = digest(&validate_json("clean.oxoflow").1);
    let csv = digest(&validate_json("clean_csv.oxoflow").1);
    let json = digest(&validate_json("clean_json.oxoflow").1);
    assert_eq!(tsv, csv, "TSV and CSV must produce the same digest");
    assert_eq!(tsv, json, "TSV and JSON must produce the same digest");
}

#[test]
fn metadata_digest_tracks_content_changes() {
    // Same workflow/schema, one changed value → different digest (the
    // checkpoint records the digest, and the resolved-workflow hash folds
    // it in — pinned by the core checkpoint test).
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::copy(fixture("clean.oxoflow"), &workflow).unwrap();
    let data = dir.path().join("clean.tsv");
    std::fs::copy(fixture("clean.tsv"), &data).unwrap();
    let first = validate_json_at(&workflow);

    std::fs::write(
        &data,
        "sample\tbatch\tage\tvalue\tqc\nS1\tb1\t31\t1.5\ttrue\nS2\tb2\t40\t2.5\tfalse\n",
    )
    .unwrap();
    let second = validate_json_at(&workflow);
    assert_ne!(digest(&first), digest(&second));
}

fn validate_json_at(workflow: &Path) -> Value {
    let output = run_cli(&["validate", "--json", workflow.to_str().unwrap()]);
    serde_json::from_slice(&output.stdout).expect("validate JSON")
}

#[test]
fn custom_id_column_is_honored() {
    let (success, document) = validate_json("custom_id.oxoflow");
    assert!(
        success,
        "id_column = sample_id must load cleanly: {document}"
    );
    assert!(findings(&document).is_empty(), "{document}");
}

#[test]
fn row_level_codes_are_exact_and_sorted() {
    let cases: &[(&str, &[ExpectedFinding<'_>])] = &[
        (
            "missing_required_column.oxoflow",
            &[("META-001", "error", None, Some("sample_prep"), None)],
        ),
        (
            "missing_value.oxoflow",
            &[("META-002", "error", Some(2), Some("batch"), Some("S2"))],
        ),
        (
            "duplicate_id.oxoflow",
            &[("META-003", "error", Some(2), Some("sample"), Some("S1"))],
        ),
        (
            "duplicate_unique.oxoflow",
            &[("META-004", "error", Some(2), Some("value"), Some("S2"))],
        ),
        (
            "enum_violation.oxoflow",
            &[("META-005", "error", Some(2), Some("qc"), Some("S2"))],
        ),
        (
            "regex_violation.oxoflow",
            &[("META-006", "error", Some(2), Some("batch"), Some("S2"))],
        ),
        (
            "type_violation.oxoflow",
            &[("META-007", "error", Some(1), Some("age"), Some("S1"))],
        ),
        (
            "extra_column.oxoflow",
            &[("META-012", "error", None, Some("notes"), None)],
        ),
    ];
    for (name, expected) in cases {
        let (success, document) = validate_json(name);
        assert!(!success, "{name} must fail validation: {document}");
        assert_eq!(document["valid"], false, "{name}: {document}");
        // Findings live in the `metadata` array only — never duplicated in
        // the existing `errors` / `warnings` arrays.
        assert_eq!(
            document["errors"].as_array().unwrap().len(),
            0,
            "{name}: metadata findings must not be duplicated in errors: {document}"
        );
        let actual: Vec<FindingTuple> = findings(&document);
        let expected: Vec<FindingTuple> = expected
            .iter()
            .map(|(code, severity, row, column, id)| {
                (
                    code.to_string(),
                    severity.to_string(),
                    *row,
                    column.map(str::to_string),
                    id.map(str::to_string),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{name}: {document}");
    }
}

#[test]
fn coverage_warn_reports_without_failing_and_error_fails() {
    let (success, warn) = validate_json("coverage_warn.oxoflow");
    assert!(success, "coverage = warn must keep exit 0: {warn}");
    assert_eq!(warn["valid"], true);
    let warn_findings = findings(&warn);
    let codes: Vec<&str> = warn_findings.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(codes, ["META-010", "META-010", "META-011"], "{warn}");
    assert!(
        warn_findings.iter().all(|f| f.1 == "warning"),
        "coverage = warn must emit warnings: {warn}"
    );

    let (success, strict) = validate_json("coverage_error.oxoflow");
    assert!(!success, "coverage = error must fail validation: {strict}");
    let strict_findings = findings(&strict);
    assert_eq!(
        strict_findings
            .iter()
            .map(|f| f.0.as_str())
            .collect::<Vec<_>>(),
        ["META-010", "META-010", "META-011"]
    );
    assert!(
        strict_findings.iter().all(|f| f.1 == "error"),
        "coverage = error must emit errors: {strict}"
    );
}

#[test]
fn pair_conflict_is_reported() {
    let (success, document) = validate_json("pair_conflict.oxoflow");
    assert!(!success, "{document}");
    assert_eq!(findings(&document)[0].0, "META-008", "{document}");
    assert!(
        document["metadata"][0]["message"]
            .as_str()
            .unwrap()
            .contains("experiment == control"),
        "{document}"
    );
}

#[test]
fn empty_id_is_a_named_parse_error() {
    let (success, document) = validate_json("empty_id.oxoflow");
    assert!(!success);
    assert_eq!(document["valid"], false);
    assert_eq!(document["errors"][0]["code"], "PARSE", "{document}");
    let message = document["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("data row 2") && message.contains("empty id"),
        "{message}"
    );
}

#[test]
fn human_output_prints_one_line_per_finding() {
    let path = fixture("coverage_error.oxoflow");
    let output = run_cli(&["validate", path.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("[META-"))
        .collect();
    assert_eq!(lines.len(), 3, "one line per finding: {stderr}");
    for line in &lines {
        assert!(line.trim_start().starts_with("error [META-"), "{line}");
    }
}

#[test]
fn dry_run_json_exposes_the_same_findings_and_digest() {
    let path = fixture("coverage_error.oxoflow");
    let output = run_cli(&["dry-run", "--json", path.to_str().unwrap()]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "dry-run --json emitted non-JSON stdout ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert!(
        !output.status.success(),
        "a metadata contract error must fail dry-run: {document}"
    );
    let dry_run_findings: Vec<String> = findings(&document).into_iter().map(|f| f.0).collect();
    let validate_findings: Vec<String> = findings(&validate_json("coverage_error.oxoflow").1)
        .into_iter()
        .map(|f| f.0)
        .collect();
    assert_eq!(dry_run_findings, validate_findings, "{document}");
    assert!(digest(&document).starts_with("sha256:"));

    // Warnings do not fail the preview.
    let warn_path = fixture("coverage_warn.oxoflow");
    let output = run_cli(&["dry-run", "--json", warn_path.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "coverage = warn must not fail dry-run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(findings(&document).len(), 3);
}

/// Run a fixture workflow for real (one `touch` rule) and return its
/// checkpoint document.
fn run_and_read_checkpoint(workflow: &Path, workdir: &Path) -> Value {
    let output = run_cli(&[
        "run",
        workflow.to_str().unwrap(),
        "--workdir",
        workdir.to_str().unwrap(),
        "--provenance",
        "--quiet",
    ]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let checkpoint = workdir.join(".oxo-flow/checkpoint.json");
    serde_json::from_slice(&std::fs::read(&checkpoint).expect("checkpoint written"))
        .expect("checkpoint JSON")
}

#[test]
fn run_records_the_metadata_digest_and_verify_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::copy(fixture("clean.oxoflow"), &workflow).unwrap();
    let data = dir.path().join("clean.tsv");
    std::fs::copy(fixture("clean.tsv"), &data).unwrap();

    // The digest validate reports and the digest the run records are the
    // same canonical value.
    let validated = validate_json_at(&workflow);
    let expected = digest(&validated);
    let checkpoint = run_and_read_checkpoint(&workflow, &dir.path().join("work-a"));
    assert_eq!(
        checkpoint["metadata_digest"].as_str(),
        Some(expected.as_str())
    );

    let verify = run_cli(&[
        "provenance",
        "verify",
        "--json",
        dir.path()
            .join("work-a/.oxo-flow/checkpoint.json")
            .to_str()
            .unwrap(),
    ]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_doc: Value = serde_json::from_slice(&verify.stdout).expect("verify JSON");
    assert_eq!(
        verify_doc["verify"]["metadata_digest"].as_str(),
        Some(expected.as_str())
    );

    // A changed table changes the recorded digest AND the resolved-workflow
    // identity (the digest is folded into that fingerprint).
    std::fs::write(
        &data,
        "sample\tbatch\tage\tvalue\tqc\nS1\tb1\t31\t1.5\ttrue\nS2\tb2\t40\t2.5\tfalse\n",
    )
    .unwrap();
    let changed = run_and_read_checkpoint(&workflow, &dir.path().join("work-b"));
    assert_ne!(
        changed["metadata_digest"], checkpoint["metadata_digest"],
        "a changed metadata value must change the recorded digest"
    );
    assert_ne!(
        changed["workflow_resolved_hash"], checkpoint["workflow_resolved_hash"],
        "the resolved-workflow identity must change with the metadata table"
    );
}
