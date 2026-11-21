//! Integration tests for `import --json` (issue #1332): the machine-readable
//! translation verdict must let CI assert what was accepted / what was lost
//! without regexing colored stderr. `import` previously emitted raw TOML on
//! stdout and colored warnings on stderr only.
//!
//! Falsifiability: the warning-construct case asserts the `warnings` array is
//! PRESENT and carries the translated message. Deleting the warnings array from
//! the emitted document (or flattening it away) fails these tests.

use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// A Snakemake workflow with no unsupported construct: translates cleanly.
const CLEAN_SMK: &str = r#"rule bwa_mem:
    input:
        "ref.fa",
        "reads.fq"
    output:
        "aligned.bam"
    threads: 4
    shell:
        "bwa mem -t {threads} {input} > {output}"
"#;

/// A Snakemake workflow with `benchmark:`, which the transpiler maps to a
/// declared path but cannot auto-populate — it emits a translation warning.
const WARN_SMK: &str = r#"rule bwa_mem:
    input:
        "ref.fa",
        "reads.fq"
    output:
        "aligned.bam"
    benchmark:
        "bench/bwa.tsv"
    threads: 4
    shell:
        "bwa mem -t {threads} {input} > {output}"
"#;

fn write_fixture(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn parse_stdout(out: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n{stdout}"))
}

#[test]
fn import_json_clean_fixture_reports_accepted() {
    let dir = std::env::temp_dir().join(format!("oxoflow_import_json_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "clean.smk", CLEAN_SMK);

    let out = oxo()
        .args(["import", "--json"])
        .arg(&src)
        .output()
        .expect("import runs");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "clean import exits 0: {:?}",
        out.status
    );
    assert_eq!(out.status.code(), Some(0));
    // --json keeps stderr clean (no colored human warnings).
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty in JSON mode: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed = parse_stdout(&out);
    // Full shape: a dropped key fails here.
    assert_eq!(parsed["tool"], "oxo-flow import");
    assert_eq!(parsed["format"], "snakemake");
    assert_eq!(parsed["accepted"], true);
    assert_eq!(parsed["warnings"], serde_json::json!([]));
    assert!(
        parsed["toml"]
            .as_str()
            .is_some_and(|t| t.contains("[workflow]")),
        "toml is embedded in the document: {parsed}"
    );
    assert!(
        parsed["source"]
            .as_str()
            .is_some_and(|s| s.ends_with("clean.smk")),
        "source path is reported: {parsed}"
    );
}

#[test]
fn import_json_unsupported_construct_surfaces_warning() {
    let dir = std::env::temp_dir().join(format!("oxoflow_import_json_warn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "warn.smk", WARN_SMK);

    let out = oxo()
        .args(["import", "--json"])
        .arg(&src)
        .output()
        .expect("import runs");
    let _ = std::fs::remove_dir_all(&dir);

    // A translation warning is a report, not a failure: exit stays 0.
    assert!(
        out.status.success(),
        "warning import exits 0: {:?}",
        out.status
    );
    assert_eq!(out.status.code(), Some(0));

    let parsed = parse_stdout(&out);
    assert_eq!(parsed["tool"], "oxo-flow import");
    assert_eq!(parsed["accepted"], false);

    // The warnings array MUST be present and carry the benchmark message.
    // Removing the array (or emitting an empty one) fails this assertion.
    let warnings = parsed["warnings"].as_array().expect("warnings is an array");
    assert_eq!(warnings.len(), 1, "exactly one warning: {warnings:?}");
    let message = warnings[0]["message"]
        .as_str()
        .expect("warning carries a message");
    assert!(
        message.contains("benchmark"),
        "warning names the unsupported construct: {message}"
    );
}

#[test]
fn import_without_json_keeps_human_output() {
    // Guard: the non-JSON path stays human — raw TOML on stdout, warnings on
    // stderr, stdout not valid JSON.
    let dir = std::env::temp_dir().join(format!("oxoflow_import_txt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "warn.smk", WARN_SMK);

    let out = oxo()
        .args(["import"])
        .arg(&src)
        .output()
        .expect("import runs");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[workflow]"),
        "raw TOML on stdout: {stdout}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&stdout).is_err(),
        "human stdout must not be valid JSON"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("benchmark"),
        "human warning on stderr: {stderr}"
    );
}
