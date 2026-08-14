//! P2 acceptance tests for issue #78 — unified storage invalidation.
//!
//! Remote inputs degrade gracefully when no cloud backend is registered:
//! the run completes, the remote entry is skipped from the manifest with a
//! warning, and local manifest entries still work as before.

use std::path::PathBuf;
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

#[test]
fn remote_input_without_backend_degrades_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = r#"
[workflow]
name = "remote-degrade"

[[rules]]
name = "mix"
input = ["s3://bucket/key.bam", "data/local.fq"]
output = ["out.txt"]
shell = "wc -l data/local.fq > out.txt"
"#;
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/local.fq"), ">S1\nACGT\n").unwrap();

    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The remote entry was skipped with a warning; the local one recorded.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no storage backend registered"),
        "expected degradation warning, got: {stderr}"
    );
    let checkpoint = std::fs::read_to_string(dir.path().join(".oxo-flow/checkpoint.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    let manifests = json["input_manifests"].as_object().unwrap();
    let mix = manifests["mix"].as_array().expect("mix manifest recorded");
    assert_eq!(mix.len(), 1, "remote entry must be skipped: {mix:?}");
    assert_eq!(mix[0]["path"].as_str().unwrap(), "data/local.fq");
    assert!(
        mix[0]["remote"].is_null(),
        "local entries have no remote field"
    );

    // A second run is a no-op: the local entry still matches.
    let out2 = Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("already completed"),
        "second run should skip: {stderr2}"
    );
}
