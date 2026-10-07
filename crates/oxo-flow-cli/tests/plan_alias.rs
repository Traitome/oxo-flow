//! Integration tests for the `plan` subcommand alias of `dry-run`
//! (issue #831): the alias dispatches to the identical preview with the
//! same flag surface, and the equivalence is discoverable from `--help`.

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// A two-rule chain `generate_data → transform` (the docs' canonical demo).
const TWO_STEP: &str = r#"
[workflow]
name = "plan-alias-demo"

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
fn plan_dispatches_to_the_dry_run_preview() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();
    let (ok, stdout, stderr) = run_cli(dir.path(), "plan", &["w.oxoflow"]);
    assert!(ok, "plan must succeed:\n{stderr}");
    // The human plan goes to stderr (stdout stays machine-reserved).
    assert!(
        stderr.contains("Plan: would run: 2"),
        "plan headline missing:\n{stderr}"
    );
    assert!(
        stderr.contains("generate_data") && stderr.contains("transform"),
        "both rules must appear:\n{stderr}"
    );
    assert!(
        stderr.contains("[run: never completed]"),
        "checkpoint-preview markers missing:\n{stderr}"
    );
    assert!(
        stderr.contains("To execute:"),
        "dry-run's closing hint missing:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "stdout stays empty without --json:\n{stdout}"
    );
}

#[test]
fn plan_output_is_byte_identical_to_dry_run() {
    // dry-run is read-only (the checkpoint is loaded, never written), so
    // both invocations against the same fresh directory must agree
    // byte-for-byte on both streams.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();
    let (ok_plan, stdout_plan, stderr_plan) = run_cli(dir.path(), "plan", &["w.oxoflow"]);
    let (ok_dry, stdout_dry, stderr_dry) = run_cli(dir.path(), "dry-run", &["w.oxoflow"]);
    assert!(ok_plan && ok_dry);
    assert_eq!(stdout_plan, stdout_dry, "stdout must be identical");
    assert_eq!(stderr_plan, stderr_dry, "stderr must be identical");
}

#[test]
fn plan_accepts_the_dry_run_flag_surface() {
    // 1:1 flag transcription: --json and a --target filter behave exactly
    // as under dry-run.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), TWO_STEP).unwrap();
    let (ok, stdout, stderr) = run_cli(
        dir.path(),
        "plan",
        &["--json", "-t", "transform", "w.oxoflow"],
    );
    assert!(ok, "plan --json -t must succeed:\n{stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("--json must emit valid JSON: {e}\n{stdout}"));
    let summary = &json["summary"];
    assert_eq!(
        summary["would_execute"].as_u64(),
        Some(2),
        "--target transform still runs its producer:\n{stdout}"
    );
    let names: Vec<&str> = json["plan"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        names.contains(&"transform"),
        "target rule present:\n{stdout}"
    );
}

#[test]
fn plan_help_advertises_the_equivalence() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, stdout, _) = run_cli(dir.path(), "plan", &["--help"]);
    assert!(ok);
    // clap prints an explicitly-requested --help to stdout.
    assert!(
        stdout.contains("oxo-flow dry-run"),
        "usage line names the canonical command:\n{stdout}"
    );
    assert!(
        stdout.contains("alias: plan"),
        "the about text must advertise the alias:\n{stdout}"
    );
}
