//! Transpiler round-trip truth table (issue #1319).
//!
//! The exporter is defined over the same documented subset the importer
//! accepts. This harness MEASURES the round trip over every fixture in
//! `tests/fixtures/import/**` and asserts the truth table with real
//! counts — updated deliberately, never silently:
//!
//! - fixtures the importer REJECTS have no parsed model to export (the
//!   exporter is undefined for them);
//! - accepted fixtures EXPORT, or are refused naming the out-of-subset
//!   construct (never silent loss);
//! - exported fixtures must RE-IMPORT byte-identically (the strongest
//!   declared-artifact equivalence: rules, DAG edges, declared I/O,
//!   resources, environments all agree because the whole document does).
//!
//! Current truth table (27 fixtures):
//! - 19 rejected by the importer (out-of-subset constructs, each named)
//! - 5 accepted fixtures -> exported, re-imported BYTE-IDENTICAL
//!   (4 Nextflow + the Snakemake fixture whose imported model carries no
//!   wildcard_constraints/benchmark/log)
//! - 3 accepted Snakemake fixtures -> export refused, construct named
//!   (`[wildcard_constraints]`, rule `benchmark`, rule `log` — Snakemake
//!   import emits fields the Nextflow DSL cannot express back)

mod common;

use assert_cmd::Command;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::transpile::{SourceFormat, transpile_with_warnings};

#[derive(Debug, PartialEq, Eq)]
enum RoundTripVerdict {
    /// The importer rejects the fixture (no parsed model exists to export).
    RejectedByImporter,
    /// The importer accepted it but the exporter refuses, naming the
    /// out-of-subset construct.
    ExportRefused { construct: String },
    /// Exported and re-imported byte-identically.
    RoundTripped,
}

fn format_of(path: &std::path::Path) -> SourceFormat {
    if path.extension().and_then(|e| e.to_str()) == Some("nf") {
        SourceFormat::Nextflow
    } else {
        SourceFormat::Snakemake
    }
}

fn round_trip(path: &std::path::Path) -> RoundTripVerdict {
    let source = std::fs::read_to_string(path).expect("fixture reads");
    let format = format_of(path);
    let Ok(imported) = transpile_with_warnings(&source, format) else {
        return RoundTripVerdict::RejectedByImporter;
    };
    let config = WorkflowConfig::parse(&imported.toml).expect("imported TOML parses");
    let dag = WorkflowDag::from_rules(&config.rules).expect("imported DAG builds");
    match oxo_flow_core::transpile::export_nextflow(&config, &dag) {
        Err(error) => RoundTripVerdict::ExportRefused {
            construct: error.to_string(),
        },
        Ok((dsl, _warnings)) => {
            let reimported = transpile_with_warnings(&dsl, SourceFormat::Nextflow)
                .expect("the exported DSL must re-import");
            assert!(
                reimported.warnings.is_empty(),
                "the exported DSL must re-import warning-free: {:?}",
                reimported.warnings
            );
            assert_eq!(
                reimported.toml,
                imported.toml,
                "fixture {} must round-trip byte-identically",
                path.display()
            );
            RoundTripVerdict::RoundTripped
        }
    }
}

fn fixture_files() -> Vec<std::path::PathBuf> {
    let mut root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("tests/fixtures/import");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("nextflow"))
        .expect("nextflow fixture dir")
        .chain(std::fs::read_dir(root.join("snakemake")).expect("snakemake fixture dir"))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("nf") | Some("smk")
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn the_fixture_corpus_yields_the_documented_truth_table() {
    let files = fixture_files();
    let mut rejected = Vec::new();
    let mut refused = Vec::new();
    let mut round_tripped = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match round_trip(path) {
            RoundTripVerdict::RejectedByImporter => rejected.push(name),
            RoundTripVerdict::ExportRefused { construct } => {
                assert!(
                    construct.contains("no Nextflow DSL2 equivalent")
                        || construct.contains("cannot round-trip")
                        || construct.contains("wildcards other than"),
                    "the refusal must name the construct: {construct}"
                );
                refused.push((name, construct));
            }
            RoundTripVerdict::RoundTripped => round_tripped.push(name),
        }
    }

    // The truth table, asserted with real counts. Adding a fixture or
    // changing the subset MUST update these numbers deliberately — the
    // counts are the measured claim.
    assert_eq!(files.len(), 27, "fixture corpus size changed");
    assert_eq!(
        rejected.len(),
        19,
        "importer-rejected fixtures: {:?}",
        rejected
    );
    assert_eq!(
        round_tripped.len(),
        5,
        "byte-identical round trips: {:?}",
        round_tripped
    );
    let round_trip_names: Vec<&str> = round_tripped
        .iter()
        .map(|name| name.as_str())
        .collect::<Vec<_>>();
    for name in [
        "nfcore_faidx.ok.nf",
        "nfcore_module.ok.nf",
        "rnaseq.ok.nf",
        "samtools_sort.ok.nf",
        "training_faidx.ok.smk",
    ] {
        assert!(
            round_trip_names.contains(&name),
            "the fixture {name} must round-trip byte-identically"
        );
    }
    assert_eq!(refused.len(), 3, "export-refused fixtures: {refused:?}");
    // Each refusal names ITS construct (never a generic "unsupported").
    let refused_names: Vec<&str> = refused
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    for (fixture, construct) in &refused {
        let expected = match fixture.as_str() {
            "expand.ok.smk" => "[wildcard_constraints]",
            "rnaseq.ok.smk" => "'benchmark'",
            "rseqc_stat.ok.smk" => "'log'",
            other => panic!("unexpected refused fixture {other}"),
        };
        assert!(
            construct.contains(expected),
            "refusal for {fixture} must name {expected}: {construct}"
        );
    }
    assert_eq!(refused_names.len(), 3);
}

/// The CLI document (`export-dsl --json`) is deterministic: two runs emit
/// byte-identical stdout, and the shape mirrors `import --json`.
#[test]
fn cli_export_dsl_json_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("wf.oxoflow");
    std::fs::write(
        &workflow,
        r#"
[workflow]
name = "demo_pipeline"
version = "0.1.0"
sample_pattern = "data/{sample}.txt"

[[rules]]
name = "demo"
input = ["data/{sample}.txt"]
output = ["{sample}.out"]
shell = "run {input} {sample}.out"
"#,
    )
    .unwrap();

    let oxo_flow = || {
        let mut cmd = Command::new(common::workspace_bin("oxo-flow"));
        cmd.env_remove("RUST_LOG");
        cmd
    };
    let run = || {
        let out = oxo_flow()
            .args(["export-dsl", workflow.to_str().unwrap(), "--json"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout.clone()
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "the --json document must be byte-stable");

    let doc: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(doc["tool"].as_str(), Some("oxo-flow export-dsl"));
    assert_eq!(doc["format"].as_str(), Some("nextflow"));
    assert_eq!(doc["accepted"].as_bool(), Some(true));
    assert!(doc["dsl"].as_str().unwrap().contains("process demo {"));
    assert!(doc["warnings"].as_array().unwrap().is_empty());
}

/// Unsupported formats are refused by name, mirroring the importer's
/// hard-error policy; out-of-subset workflows fail loudly.
#[test]
fn cli_export_dsl_refuses_unsupported_formats_and_constructs() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("wf.oxoflow");
    std::fs::write(
        &workflow,
        r#"
[workflow]
name = "demo_pipeline"
version = "0.1.0"

[[rules]]
name = "demo"
output = ["a.txt"]
shell = "touch a.txt"
"#,
    )
    .unwrap();

    let oxo_flow = || {
        let mut cmd = Command::new(common::workspace_bin("oxo-flow"));
        cmd.env_remove("RUST_LOG");
        cmd
    };
    oxo_flow()
        .args([
            "export-dsl",
            workflow.to_str().unwrap(),
            "--format",
            "snakemake",
        ])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "export format 'snakemake' is not implemented",
        ));

    // A rule field outside the subset: hard error naming the construct.
    let with_log = dir.path().join("log.oxoflow");
    std::fs::write(
        &with_log,
        r#"
[workflow]
name = "demo_pipeline"
version = "0.1.0"

[[rules]]
name = "demo"
log = "logs/demo.log"
output = ["a.txt"]
shell = "touch a.txt"
"#,
    )
    .unwrap();
    oxo_flow()
        .args(["export-dsl", with_log.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("'log'"));
}
