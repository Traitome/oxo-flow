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

// ─── Run-log persistence (issue #115 pillar 1 extension) ─────────────────

fn git_head(dir: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every run archives its own log under `.oxo-flow/logs/oxo-flow.log` with
/// numbered rotation: the second run moves the first run's log to `.1`.
/// The header names the exact workflow version (name, version, git HEAD),
/// and the engine's tracing stream is teed into the file.
#[test]
fn cli_run_log_rotates_and_names_workflow_version() {
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
    let wf = dir.path().join("logged.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"logged\"\nversion = \"2.3.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    git(&["add", "logged.oxoflow"]);
    git(&[
        "-c",
        "user.name=oxo-test",
        "-c",
        "user.email=test@oxo-flow.local",
        "commit",
        "-q",
        "-m",
        "v1",
    ]);
    let head = git_head(dir.path());

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let log1 = dir.path().join(".oxo-flow/logs/oxo-flow.log");
    let content1 = fs::read_to_string(&log1).unwrap();
    assert!(
        content1.starts_with("oxo-flow run log\n"),
        "log must start with the run-log header: {content1}"
    );
    assert!(content1.contains(&format!("git_sha: {head}")));
    assert!(content1.contains("workflow_name: logged"));
    assert!(content1.contains("workflow_version: 2.3.0"));
    assert!(
        content1.contains("workflow run started"),
        "the tracing stream must be teed into the log"
    );
    assert!(!dir.path().join(".oxo-flow/logs/oxo-flow.log.1").exists());

    // Second run: the previous log rotates to .1, the new run owns the base.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();
    let content2 = fs::read_to_string(&log1).unwrap();
    assert!(content2.starts_with("oxo-flow run log\n"));
    let rotated = dir.path().join(".oxo-flow/logs/oxo-flow.log.1");
    let content_rotated = fs::read_to_string(&rotated).unwrap();
    assert_eq!(
        content_rotated, content1,
        "the first run's log must survive in .1"
    );
}

/// Outside a git repository the run log still exists — the git_sha header
/// line says so explicitly and the run never fails.
#[test]
fn cli_run_log_outside_git_repo_still_writes() {
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
    let log = dir.path().join(".oxo-flow/logs/oxo-flow.log");
    let content = fs::read_to_string(&log).unwrap();
    assert!(content.contains("git_sha: (not inside a git repository)"));
}

/// `--log-file` overrides the default location; the default path is then
/// left untouched.
#[test]
fn cli_run_log_file_flag_uses_custom_path() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("custom.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"custom\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    let custom = dir.path().join("my-run.log");
    oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--log-file",
            custom.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .success();
    assert!(
        fs::read_to_string(&custom)
            .unwrap()
            .starts_with("oxo-flow run log\n")
    );
    assert!(!dir.path().join(".oxo-flow/logs/oxo-flow.log").exists());
}

/// The automatic report snapshot carries the workflow git SHA, closing the
/// audit chain: checkpoint → run log → report all name the version.
#[test]
fn cli_run_report_snapshot_carries_workflow_git_sha() {
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
    let wf = dir.path().join("reported.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"reported\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    git(&["add", "reported.oxoflow"]);
    git(&[
        "-c",
        "user.name=oxo-test",
        "-c",
        "user.email=test@oxo-flow.local",
        "commit",
        "-q",
        "-m",
        "v1",
    ]);
    let head = git_head(dir.path());

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let index: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".oxo-flow/reports/index.json")).unwrap(),
    )
    .unwrap();
    let report_path = index
        .as_array()
        .and_then(|a| a.last())
        .and_then(|e| e.get("report"))
        .and_then(|r| r.as_str())
        .expect("index.json must name the newest report");
    let report: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".oxo-flow/reports").join(report_path)).unwrap(),
    )
    .unwrap();
    assert_eq!(report["workflow_git_sha"].as_str(), Some(head.as_str()));
}
