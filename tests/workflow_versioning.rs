//! Workflow versioning integration tests (issue #115 pillar 1).
//!
//! The engine records the workflow repository's HEAD SHA in the checkpoint
//! at run start; these tests exercise that end-to-end via the compiled
//! `oxo-flow` binary. Kept in a dedicated crate so parallel sessions can
//! own `cli_integration.rs` independently (each integration-test crate
//! compiles and links on its own).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Locate a workspace binary by name from the target directory.
///
/// This handles the case where binaries are defined in workspace sub-crates
/// rather than the root package, which means `CARGO_BIN_EXE_*` env vars
/// are not automatically set.
fn workspace_bin(name: &str) -> PathBuf {
    // Cargo sets OUT_DIR for build scripts and CARGO_MANIFEST_DIR for the package.
    // For integration tests, we can derive the target dir from the test binary location.
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();

    // Try the binary directly in the target/debug (or target/release) directory.
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    // On Windows, binaries have a .exe extension.
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }

    // Fall back to the deps subdirectory.
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

// ─── Workflow version provenance (issue #115 pillar 1) ───────────────

#[test]
fn cli_provenance_verify_displays_workflow_git_sha() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join("cp.json");
    fs::write(
        &cp,
        r#"{"completed_rules":["s"],"failed_rules":[],"benchmarks":{},"workflow_git_sha":"deadbeef","workflow_path":"/tmp/wf.oxoflow"}"#,
    )
    .unwrap();
    oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("workflow git HEAD: deadbeef"));
}

/// Pillar 1 of the versioning plan (issue #115): the engine records the
/// workflow repository's HEAD SHA in the checkpoint, so every result set is
/// auditable to the workflow version that produced it.
#[test]
fn cli_run_records_workflow_git_sha_in_git_repo() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    git(&["init", "-q"]);
    let wf = dir.path().join("versioned.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"versioned\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    git(&["add", "versioned.oxoflow"]);
    git(&[
        "-c",
        "user.name=oxo-test",
        "-c",
        "user.email=test@oxo-flow.local",
        "commit",
        "-q",
        "-m",
        "workflow v1",
    ]);
    let head = git(&["rev-parse", "HEAD"]);
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let cp = dir.path().join(".oxo-flow/checkpoint.json");
    let content = fs::read_to_string(&cp).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(v["workflow_git_sha"].as_str(), Some(head_sha.as_str()));
}

/// Outside a git repository the run must still succeed — the SHA is simply
/// absent instead of the run failing.
#[test]
fn cli_run_outside_git_repo_has_no_workflow_git_sha() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("plain.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"plain\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let cp = dir.path().join(".oxo-flow/checkpoint.json");
    let content = fs::read_to_string(&cp).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v.get("workflow_git_sha").is_none());
}
