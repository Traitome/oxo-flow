//! P3 acceptance tests for issue #78 — checkpoint re-entry.
//!
//! A `checkpoint = true` rule writes a re-entry manifest at runtime; the
//! engine merges the new samples, re-expands from templates, and executes the
//! round-2 instances in the same run. Resumes replay recorded re-entries
//! deterministically; invalidating the checkpoint rule revokes its samples.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate a workspace binary (mirrors the helper in cli_integration.rs).
fn workspace_bin(name: &str) -> PathBuf {
    let target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; run `cargo build --workspace` first"
    );
}

fn write_workflow(dir: &Path, manifest_body: &str, discover_input: &str) {
    let workflow = format!(
        r#"
[workflow]
name = "reentry-e2e"

[[sample_groups]]
name = "batch"
samples = ["S1"]

[[rules]]
name = "discover"
input = [{discover_input}]
output = ["discover.toml"]
shell = "printf '%s' '{manifest_body}' > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "analyze"
input = ["discover.toml"]
output = ["out/{{sample}}.txt"]
shell = "echo {{sample}} > out/{{sample}}.txt"
"#
    );
    std::fs::write(dir.join("wf.oxoflow"), workflow).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/S1.fq"), ">S1\nACGT\n").unwrap();
}

fn run(dir: &Path, extra: &[&str]) -> (bool, String) {
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

fn checkpoint(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap())
        .unwrap()
}

fn completed(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = checkpoint(dir)["completed_rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

const MANIFEST_TWO: &str = "[reentry]\\nsample = [\\\"S4\\\", \\\"S5\\\"]\\n";

#[test]
fn reentry_adds_round2_instances_and_records_rounds() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();

    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "run failed: {stderr}");

    // Round-2 instances executed in the same run.
    assert!(dir.path().join("out/S1.txt").exists());
    assert!(dir.path().join("out/S4.txt").exists(), "{stderr}");
    assert!(dir.path().join("out/S5.txt").exists(), "{stderr}");

    // The checkpoint records the re-entry.
    let ck = checkpoint(dir.path());
    let reentries = ck["reentries"].as_array().unwrap();
    assert_eq!(reentries.len(), 1);
    assert_eq!(reentries[0]["rule"].as_str().unwrap(), "discover");
    assert_eq!(reentries[0]["round"].as_u64().unwrap(), 1);
    let samples: Vec<&str> = reentries[0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(samples, vec!["S4", "S5"]);
}

#[test]
fn resume_replays_reentry_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();

    let (ok, _) = run(dir.path(), &[]);
    assert!(ok);
    let first = completed(dir.path());

    // Second run: nothing re-runs, the same plan reconstructs.
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "resume failed: {stderr}");
    assert_eq!(completed(dir.path()), first);
    assert!(
        stderr.contains("already completed"),
        "expected skips on resume: {stderr}"
    );
    assert_eq!(
        checkpoint(dir.path())["reentries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn invalidating_checkpoint_rule_revokes_its_samples() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();
    let (ok, _) = run(dir.path(), &[]);
    assert!(ok);
    assert!(dir.path().join("out/S4.txt").exists());

    // Change the checkpoint rule's input AND make it emit only S4 next time.
    std::fs::write(dir.path().join("catalog.txt"), "v2\n").unwrap();
    let manifest_one = "[reentry]\\nsample = [\\\"S4\\\"]\\n";
    let workflow = std::fs::read_to_string(dir.path().join("wf.oxoflow")).unwrap();
    std::fs::write(
        dir.path().join("wf.oxoflow"),
        workflow.replace(MANIFEST_TWO, manifest_one),
    )
    .unwrap();

    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "re-run failed: {stderr}");

    // discover re-ran and re-recorded; S5 revoked from the plan.
    let ck = checkpoint(dir.path());
    let reentries = ck["reentries"].as_array().unwrap();
    assert_eq!(reentries.len(), 1, "superseded, not appended");
    let samples: Vec<&str> = reentries[0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(samples, vec!["S4"]);
    assert!(dir.path().join("out/S4.txt").exists());
}

#[test]
fn missing_manifest_fails_the_checkpoint_rule() {
    let dir = tempfile::tempdir().unwrap();
    // discover writes nothing → checkpoint_manifest never appears.
    let workflow = r#"
[workflow]
name = "reentry-missing"

[[sample_groups]]
name = "batch"
samples = ["S1"]

[[rules]]
name = "discover"
output = ["discover.done"]
shell = "touch discover.done"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "analyze"
input = ["discover.done"]
output = ["out.txt"]
shell = "touch out.txt"
"#;
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(!ok, "run should fail");
    assert!(
        stderr.contains("manifest"),
        "expected manifest error, got: {stderr}"
    );
    assert!(
        !dir.path().join("out.txt").exists(),
        "dependent must not run"
    );
}

#[test]
fn empty_manifest_is_valid_noop() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), "[reentry]\\nsample = []\\n", r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "empty manifest run failed: {stderr}");
    assert!(dir.path().join("out/S1.txt").exists());
    // No new samples → no re-entry record (field omitted when empty).
    let reentries = checkpoint(dir.path())["reentries"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(reentries, 0);
}
