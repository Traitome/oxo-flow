//! Deterministic performance and stability checks (issue #1368).

use super::*;

// ── Deterministic performance (issue #1368) ────────────────────────
//
// A fixed-seed generator produces the same tables on every run; no
// randomness, no network, no external files. The bound is generous
// (debug builds are slow) — the point is to catch accidental quadratic
// behavior, not to benchmark.

const PERF_SCHEMA_TSV_HEADER: &str = "sample\tbatch\tage\tvalue\tqc\n";

fn generated_tsv(rows: usize) -> String {
    let mut out = String::with_capacity(PERF_SCHEMA_TSV_HEADER.len() + rows * 32);
    out.push_str(PERF_SCHEMA_TSV_HEADER);
    for index in 0..rows {
        // Deterministic, collision-free for `value` (the unique column).
        let age = index % 97;
        let value = index as f64 * 0.5;
        let qc = if index % 3 == 0 { "false" } else { "true" };
        out.push_str(&format!(
            "S{index:06}\tb{}\t{age}\t{value:.1}\t{qc}\n",
            index % 13
        ));
    }
    out
}

const PERF_WORKFLOW_TEMPLATE: &str = r#"
            [workflow]
            name = "perf"
            metadata_file = "samples.tsv"

            [workflow.metadata_schema]
            required_columns = ["batch"]

            [[workflow.metadata_schema.columns]]
            name = "batch"
            regex = "^b\\d+$"

            [[workflow.metadata_schema.columns]]
            name = "age"
            type = "integer"

            [[workflow.metadata_schema.columns]]
            name = "value"
            type = "float"
            unique = true

            [[workflow.metadata_schema.columns]]
            name = "qc"
            type = "boolean"
            enum = ["true", "false"]

            [[rules]]
            name = "step"
            output = ["out.txt"]
            shell = "touch out.txt"
            "#;

/// Write the perf workflow plus its table into `dir` and load it through
/// the real `from_file` path (loader + schema + digest all exercised).
fn load_perf_config(dir: &std::path::Path, table: &str) -> WorkflowConfig {
    std::fs::write(dir.join("samples.tsv"), table).unwrap();
    let workflow = dir.join("wf.oxoflow");
    std::fs::write(&workflow, PERF_WORKFLOW_TEMPLATE).unwrap();
    WorkflowConfig::from_file(&workflow).unwrap()
}

fn generated_config(rows: usize) -> (tempfile::TempDir, WorkflowConfig) {
    let dir = tempfile::tempdir().unwrap();
    let config = load_perf_config(dir.path(), &generated_tsv(rows));
    (dir, config)
}

/// Same table as [`generated_tsv`], with a deterministic regex
/// violation every 1,000 rows so the shuffle test compares a NON-empty
/// finding set.
fn generated_tsv_with_violations(rows: usize) -> String {
    generated_tsv(rows)
        .lines()
        .enumerate()
        .map(|(line, text)| {
            if line > 0 && (line - 1) % 1_000 == 0 {
                let mut fields: Vec<&str> = text.split('\t').collect();
                fields[1] = "not-a-batch";
                fields.join("\t")
            } else {
                text.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Deterministic Fisher-Yates shuffle over the row LINES of `tsv`
/// (header kept first), seeded with a fixed LCG.
fn shuffled_tsv(tsv: &str, seed: u64) -> String {
    let mut lines: Vec<&str> = tsv.lines().collect();
    let header = lines.remove(0);
    let mut state = seed;
    for index in (1..lines.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let swap_with = (state >> 33) as usize % (index + 1);
        lines.swap(index, swap_with);
    }
    let mut out = String::with_capacity(tsv.len());
    out.push_str(header);
    out.push('\n');
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn ten_thousand_and_one_hundred_thousand_rows_are_linear_and_stable() {
    use std::time::Instant;

    let (_small_dir, small) = generated_config(10_000);
    let started = Instant::now();
    let small_report = evaluate(&small);
    let small_elapsed = started.elapsed();
    eprintln!("metadata contract perf: 10k evaluate = {small_elapsed:?}");
    assert!(
        small_report.findings.is_empty(),
        "{:?}",
        small_report.findings
    );
    assert!(small.metadata_digest.is_some());

    let (_large_dir, large) = generated_config(100_000);
    let started = Instant::now();
    let large_report = evaluate(&large);
    let large_elapsed = started.elapsed();
    eprintln!("metadata contract perf: 100k evaluate = {large_elapsed:?}");
    assert!(
        large_report.findings.is_empty(),
        "{:?}",
        large_report.findings
    );

    // Generous debug-build bound (the measured cost is well under a
    // second); a regression to quadratic behavior blows through it.
    assert!(
        large_elapsed.as_secs_f64() < 10.0,
        "100k-row metadata validation took {large_elapsed:?}"
    );
    let small_seconds = small_elapsed.as_secs_f64().max(0.001);
    assert!(
        large_elapsed.as_secs_f64() < small_seconds * 60.0 + 1.0,
        "100k rows took {large_elapsed:?} vs 10k {small_elapsed:?} — looks super-linear"
    );

    // Repeated runs: identical digest and findings.
    assert_eq!(evaluate(&large), large_report);
    assert_eq!(digest_for(&large), digest_for(&large));
}

#[test]
fn digest_and_findings_survive_row_shuffles() {
    let rows = 10_000;
    let dir = tempfile::tempdir().unwrap();
    let mut digests = Vec::new();
    let mut finding_sets = Vec::new();
    let base = generated_tsv_with_violations(rows);
    for (case, seed) in [(0usize, 1u64), (1, 42), (2, 9_001)] {
        let tsv = if case == 0 {
            base.clone()
        } else {
            shuffled_tsv(&base, seed)
        };
        let config = load_perf_config(dir.path(), &tsv);
        digests.push(config.metadata_digest.clone().expect("digest"));
        let mut findings: Vec<(String, Option<String>, Option<String>)> = evaluate(&config)
            .findings
            .into_iter()
            .map(|finding| (finding.code, finding.column, finding.id))
            .collect();
        findings.sort();
        finding_sets.push(findings);
    }
    assert!(
        !finding_sets[0].is_empty(),
        "the fixture must have findings"
    );
    assert_eq!(digests[0], digests[1], "digest must survive a row shuffle");
    assert_eq!(digests[0], digests[2], "digest must survive a row shuffle");
    assert_eq!(finding_sets[0], finding_sets[1]);
    assert_eq!(finding_sets[0], finding_sets[2]);
}
