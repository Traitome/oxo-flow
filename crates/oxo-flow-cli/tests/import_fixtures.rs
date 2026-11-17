//! Fixture-corpus gate for `oxo-flow import`.
//!
//! Every `*.ok.{nf,smk}` fixture must import AND survive the documented
//! static gates (`validate`, `lint`, `dry-run`). Every `*.err.{nf,smk}`
//! fixture must fail with a hard error whose text contains the marker on
//! its `# EXPECT-ERROR:` line, and must not leave an output file behind.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/import")
}

fn fixtures(dir: &str, suffix: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(fixtures_root().join(dir))
        .unwrap_or_else(|error| panic!("fixtures directory {dir}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(suffix))
        })
        .collect();
    entries.sort();
    entries
}

fn run(args: &[&str], dir: &Path) -> (bool, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn parse_json(stdout: &str, context: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|error| panic!("{context} must emit one JSON document ({error}): {stdout}"))
}

#[test]
fn supported_import_fixtures_import_and_pass_the_static_gates() {
    let work = tempfile::tempdir().unwrap();
    let mut checked = 0usize;
    for (dir, suffix) in [("nextflow", ".ok.nf"), ("snakemake", ".ok.smk")] {
        for source in fixtures(dir, suffix) {
            let stem = source.file_stem().unwrap().to_string_lossy();
            let output = work.path().join(format!("{dir}-{stem}.oxoflow"));
            let (ok, _stdout, stderr) = run(
                &[
                    "import",
                    "--quiet",
                    source.to_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                ],
                work.path(),
            );
            assert!(
                ok,
                "import must accept fixture {}: {stderr}",
                source.display()
            );

            let (ok, stdout, stderr) = run(
                &["validate", "--json", output.to_str().unwrap()],
                work.path(),
            );
            assert!(ok, "validate must pass for {}: {stderr}", source.display());
            let document = parse_json(&stdout, "validate");
            assert_eq!(
                document["valid"],
                Value::Bool(true),
                "validate report for {}: {document}",
                source.display()
            );

            let (ok, stdout, stderr) =
                run(&["lint", "--json", output.to_str().unwrap()], work.path());
            assert!(ok, "lint must pass for {}: {stderr}", source.display());
            let document = parse_json(&stdout, "lint");
            assert_eq!(
                document["passed"],
                Value::Bool(true),
                "lint report for {}: {document}",
                source.display()
            );

            let (ok, stdout, stderr) = run(
                &["dry-run", "--json", output.to_str().unwrap()],
                work.path(),
            );
            assert!(
                ok,
                "dry-run must succeed for {}: {stderr}",
                source.display()
            );
            parse_json(&stdout, "dry-run");
            checked += 1;
        }
    }
    assert!(checked >= 4, "expected at least 4 supported fixtures");
}

#[test]
fn unsupported_import_fixtures_fail_with_a_named_hard_error() {
    let work = tempfile::tempdir().unwrap();
    let mut checked = 0usize;
    for (dir, suffix) in [("nextflow", ".err.nf"), ("snakemake", ".err.smk")] {
        for source in fixtures(dir, suffix) {
            let content = std::fs::read_to_string(&source).unwrap();
            let marker = content
                .lines()
                .find_map(|line| {
                    line.strip_prefix("# EXPECT-ERROR:")
                        .or_else(|| line.strip_prefix("// EXPECT-ERROR:"))
                })
                .map(str::trim)
                .unwrap_or_else(|| panic!("{} needs an # EXPECT-ERROR: line", source.display()));

            let output = work.path().join(format!(
                "{dir}-{}.oxoflow",
                source.file_stem().unwrap().to_string_lossy()
            ));
            let (ok, _stdout, stderr) = run(
                &[
                    "import",
                    "--quiet",
                    source.to_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                ],
                work.path(),
            );
            assert!(
                !ok,
                "unsupported fixture {} must fail the import",
                source.display()
            );
            assert!(
                stderr.to_lowercase().contains(&marker.to_lowercase()),
                "error for {} must name `{marker}`: {stderr}",
                source.display()
            );
            assert!(
                !output.exists(),
                "a failed import must not write {}",
                output.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 6, "expected at least 6 unsupported fixtures");
}
