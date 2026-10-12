//! Integration tests for `batch --dry-run --json-output` (issue #864):
//! the previewed commands must be consumable as JSON on stdout — before the
//! fix the dry-run exited 0 with zero bytes on stdout (human preview on
//! stderr only). `-n` alone keeps stdout empty (no behavior change).

use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

#[test]
fn dry_run_json_emits_previewed_commands() {
    let out = oxo()
        .args(["batch", "-n", "--json-output", "echo item={item}"])
        .arg("a.txt")
        .arg("b.txt")
        .output()
        .expect("batch binary runs");
    assert!(out.status.success(), "exit: {:?}", out.status);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "dry-run --json-output must write the preview to stdout"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n{stdout}"));
    assert_eq!(parsed["tool"], "oxo-flow batch");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["total"], 2);
    assert_eq!(parsed["failed"], 0);
    assert_eq!(parsed["success"], true);

    let results = parsed["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["item"], "a.txt");
    assert_eq!(results[0]["command"], "echo item=a.txt");
    assert_eq!(results[1]["item"], "b.txt");
    assert_eq!(results[1]["command"], "echo item=b.txt");
    // All pre-execution fields are null/true — nothing ran.
    for r in results {
        assert!(r["exit_code"].is_null());
        assert_eq!(r["success"], true);
        assert!(r["error"].is_null());
    }

    // The human-readable preview stays on stderr so stdout stays pure JSON.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Batch (dry-run):"),
        "human preview expected on stderr: {stderr}"
    );
    assert!(stderr.contains("[1] echo item=a.txt"));
}

#[test]
fn dry_run_without_json_keeps_stdout_silent() {
    let out = oxo()
        .args(["batch", "-n", "echo item={item}", "a.txt"])
        .output()
        .expect("batch binary runs");
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "-n alone must keep stdout empty (no behavior change)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[1] echo item=a.txt"), "preview on stderr");
}

#[test]
fn dry_run_json_via_global_json_flag() {
    // The global --json drives the same output (alignment audit 2026-08-14).
    let out = oxo()
        .args(["--json", "batch", "-n", "echo {item}", "x"])
        .output()
        .expect("batch binary runs");
    assert!(out.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("global --json also yields JSON in dry-run");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["results"][0]["command"], "echo x");
}
