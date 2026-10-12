//! Integration tests for `status` CHECKPOINT-argument handling when the
//! argument is a workflow (`.oxoflow`) file (issue #859): the sibling
//! checkpoint `<workflow-dir>/.oxo-flow/checkpoint.json` is auto-resolved,
//! and a genuinely missing checkpoint keeps an error that names the path
//! that was checked instead of a blanket "run first" hint.

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// A two-rule chain `generate_data → transform` (the docs' canonical demo).
const TWO_STEP: &str = r#"
[workflow]
name = "status-resolve-demo"

[[rules]]
name = "generate_data"
output = ["data/greeting.txt"]
shell = "mkdir -p data && echo hello > data/greeting.txt"

[[rules]]
name = "transform"
input = ["data/greeting.txt"]
output = ["results/uppercase.txt"]
shell = "mkdir -p results && tr a-z A-Z < data/greeting.txt > results/uppercase.txt"
"#;

/// A minimal valid checkpoint: `benchmarks` is the only field without a
/// serde default; everything else is skipped when empty.
const MINIMAL_CHECKPOINT: &str =
    r#"{"completed_rules": ["generate_data"], "failed_rules": [], "benchmarks": {}}"#;

/// Run `oxo-flow <sub> [args..]` in `dir`; return (status, stdout, stderr).
fn run_cli(dir: &Path, sub: &str, args: &[&str]) -> (bool, String, String) {
    let out = oxo()
        .args([sub])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn status_resolves_a_workflow_file_to_its_sibling_checkpoint() {
    // The live-run scenario from the issue: `status workflow.oxoflow` while
    // `<dir>/.oxo-flow/checkpoint.json` exists must succeed against it.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();
    let dot = dir.path().join(".oxo-flow");
    std::fs::create_dir(&dot).unwrap();
    std::fs::write(dot.join("checkpoint.json"), MINIMAL_CHECKPOINT).unwrap();

    let (ok, stdout, stderr) = run_cli(dir.path(), "status", &["w.oxoflow"]);
    assert!(
        ok,
        "sibling checkpoint must be auto-resolved, not rejected:\n{stderr}"
    );
    assert!(
        stderr.contains("sibling checkpoint"),
        "the note naming the resolved checkpoint is missing:\n{stderr}"
    );
    assert!(
        stderr.contains(".oxo-flow/checkpoint.json"),
        "the resolved path must be printed:\n{stderr}"
    );
    assert!(
        stderr.contains("generate_data"),
        "checkpoint contents must be reported:\n{stderr}"
    );
    assert!(
        !stderr.contains("Run 'oxo-flow run"),
        "the stale 'run first' hint must not appear on success:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "stdout stays machine-reserved without --json:\n{stdout}"
    );
}

#[test]
fn status_workflow_file_without_checkpoint_names_the_checked_path() {
    // No sibling checkpoint exists: the error must distinguish "no
    // checkpoint found at <path>" from a generic rejection, so the operator
    // can see exactly where the engine looked.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();

    let (ok, _stdout, stderr) = run_cli(dir.path(), "status", &["w.oxoflow"]);
    assert!(!ok, "missing checkpoint must still fail");
    assert!(
        stderr.contains("No checkpoint found at"),
        "reworded error missing:\n{stderr}"
    );
    assert!(
        stderr.contains(".oxo-flow/checkpoint.json"),
        "the checked path must be named:\n{stderr}"
    );
    assert!(
        stderr.contains("Run 'oxo-flow run w.oxoflow' first"),
        "the run-first hint must persist when nothing exists:\n{stderr}"
    );
}

#[test]
fn status_explicit_checkpoint_path_still_wins_untouched() {
    // An explicit non-workflow CHECKPOINT argument keeps working exactly as
    // before — the auto-resolve only fires for .oxoflow args.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();
    std::fs::write(dir.path().join("cp.json"), MINIMAL_CHECKPOINT).unwrap();

    let (ok, _stdout, stderr) = run_cli(dir.path(), "status", &["cp.json"]);
    assert!(ok, "explicit checkpoint path must load:\n{stderr}");
    assert!(
        stderr.contains("cp.json"),
        "the explicit path must be echoed as-is:\n{stderr}"
    );
    assert!(
        !stderr.contains("sibling checkpoint"),
        "no auto-resolve note for an explicit checkpoint:\n{stderr}"
    );
}
