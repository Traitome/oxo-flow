//! Regression tests for the `test` command.
//!
//! `test --run -t <target>` must apply the target to the execution step:
//! PR #114 (module composition) swapped `run_command`'s `target`/`module`
//! positional arguments at this call site, which made `test --run -t` fail
//! with "unknown module" before executing anything.

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

/// `test --run -t gen` executes exactly the targeted rule. With the swapped
/// arguments the command errored with "unknown module 'gen'" and produced
/// no output file.
#[test]
fn cli_test_run_target_applies_to_execution() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("t.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    oxo_flow_cmd()
        .args(["test", "--run", "-t", "gen", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Execution"));
    assert!(
        dir.path().join("out.txt").exists(),
        "the targeted rule must have executed"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "hi\n"
    );
}
