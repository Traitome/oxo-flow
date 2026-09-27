//! Regression tests for the `test` command.
//!
//! `test --run -t <target>` must apply the target to the execution step:
//! PR #114 (module composition) swapped `run_command`'s `target`/`module`
//! positional arguments at this call site, which made `test --run -t` fail
//! with "unknown module" before executing anything.

mod common;
use common::workspace_bin;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

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
