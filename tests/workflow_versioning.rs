//! Workflow versioning integration tests (issue #115 pillar 1).
//!
//! The engine records the workflow repository's HEAD SHA in the checkpoint
//! at run start; these tests exercise that end-to-end via the compiled
//! `oxo-flow` binary. Kept in a dedicated crate so parallel sessions can
//! own `cli_integration.rs` independently (each integration-test crate
//! compiles and links on its own).

mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn oxo_flow_cmd() -> Command {
    // Log-assertion tests must see the engine's default (info) tracing
    // level — a developer shell exporting RUST_LOG=warn would suppress the
    // asserted log lines, so this variant always strips it.
    let mut cmd = common::oxo_flow_cmd();
    cmd.env_remove("RUST_LOG");
    cmd
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
        // The engine tees its tracing stream into the run log as JSON
        // events — `workflow_started` is the run-level one.
        content1.contains("\"event\":\"workflow_started\""),
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

// ─── info --json git provenance (issue #124 pillar 3) ────────────────────

/// `info --json` derives the workflow's git identity inside a repository
/// (contract with the community catalog: keys omitted outside a repo).
#[test]
fn cli_info_json_carries_git_provenance_in_repo() {
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
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/wf.git",
    ]);
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"wf\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    git(&["add", "wf.oxoflow"]);
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

    let out = oxo_flow_cmd()
        .args(["info", "--json", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(meta["git_sha"].as_str(), Some(head.as_str()));
    // Compare against git's own view of the remote: host-level insteadOf
    // rewrites legitimately change what `remote get-url` reports.
    let reported_remote = String::from_utf8_lossy(&git(&["remote", "get-url", "origin"]).stdout)
        .trim()
        .to_string();
    assert_eq!(
        meta["git_remote"].as_str(),
        Some(reported_remote.as_str()),
        "git_remote must match git remote get-url origin"
    );
    assert!(
        meta["git_describe"].as_str().is_some_and(|s| !s.is_empty()),
        "git_describe must fall back to an abbreviated SHA on untagged history"
    );
}

/// Outside a git repository the git keys are omitted entirely — never null.
#[test]
fn cli_info_json_omits_git_provenance_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"wf\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["info", "--json", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(meta.get("git_sha").is_none());
    assert!(meta.get("git_remote").is_none());
    assert!(meta.get("git_describe").is_none());
}

/// Issue #1324: the checkpoint records a content digest of the executed
/// workflow bytes, so two runs of the SAME uncommitted file stay
/// distinguishable, and a dirty tree is flagged. Deterministic,
/// provider-free.
#[test]
fn cli_run_records_workflow_content_hash_and_dirty_flag() {
    use sha2::{Digest, Sha256};

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
    let wf = dir.path().join("hashed.oxoflow");
    let v1 = "[workflow]\nname = \"hashed\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n";
    fs::write(&wf, v1).unwrap();
    git(&["add", "hashed.oxoflow"]);
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
    let head_before = git_head(dir.path());

    let sha = |s: &str| -> String {
        let digest = Sha256::digest(s.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256:{hex}")
    };

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let cp = dir.path().join(".oxo-flow/checkpoint.json");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cp).unwrap()).unwrap();
    assert_eq!(v["workflow_content_hash"].as_str(), Some(sha(v1).as_str()));
    assert_eq!(v["workflow_git_sha"].as_str(), Some(head_before.as_str()));
    assert_eq!(v["workflow_git_dirty"].as_bool(), Some(false));

    // Uncommitted edit between the two runs: HEAD never moves, but the
    // executed bytes differ, so the content hash must differ and the dirty
    // flag must now be set.
    let v2 = "[workflow]\nname = \"hashed\"\nversion = \"1.0.0\"\n\n# uncommitted edit\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n";
    fs::write(&wf, v2).unwrap();
    assert_ne!(sha(v1), sha(v2));

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cp).unwrap()).unwrap();
    assert_eq!(v["workflow_content_hash"].as_str(), Some(sha(v2).as_str()));
    assert_eq!(v["workflow_git_sha"].as_str(), Some(head_before.as_str()));
    assert_eq!(git_head(dir.path()), head_before);
    assert_eq!(v["workflow_git_dirty"].as_bool(), Some(true));
}

/// Issue #1350: `workflow_content_hash` digests the workflow's RAW bytes,
/// so two runs of the same file with different `--arg KEY=VALUE` overrides
/// would collide on it. The override-aware `workflow_resolved_hash`
/// separates them while the raw digest (kept for compatibility) stays put;
/// identical invocations stay byte-stable on both.
#[test]
fn cli_run_records_override_aware_resolved_hash() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("override.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"override\"\nversion = \"1.0.0\"\n\n[config]\nthreshold = 1\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo {config.threshold} > {output}\"\n",
    )
    .unwrap();
    let cp = dir.path().join(".oxo-flow/checkpoint.json");
    let read_checkpoint = || -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(&cp).unwrap()).unwrap()
    };

    oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--arg",
            "threshold=1",
            "--provenance",
        ])
        .current_dir(dir.path())
        .assert()
        .success();
    let first = read_checkpoint();
    let raw_hash = first["workflow_content_hash"].as_str().unwrap().to_string();
    let resolved_1 = first["workflow_resolved_hash"]
        .as_str()
        .unwrap()
        .to_string();

    // Same file, different override: the raw digest MUST NOT move (the
    // #1325 finding), the resolved fingerprint MUST.
    oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--arg",
            "threshold=2",
            "--provenance",
        ])
        .current_dir(dir.path())
        .assert()
        .success();
    let second = read_checkpoint();
    assert_eq!(
        second["workflow_content_hash"].as_str().unwrap(),
        raw_hash,
        "raw-bytes digest is override-blind by construction and must stay put"
    );
    let resolved_2 = second["workflow_resolved_hash"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(
        resolved_1, resolved_2,
        "two override-driven runs of the same file must not share the resolved fingerprint"
    );

    // Back to the first invocation: the fingerprint is a deterministic
    // function of (raw digest, resolved config) — byte-stable JSON
    // identity, no timestamps in the derivation.
    oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--arg",
            "threshold=1",
            "--provenance",
        ])
        .current_dir(dir.path())
        .assert()
        .success();
    let third = read_checkpoint();
    assert_eq!(
        third["workflow_resolved_hash"].as_str().unwrap(),
        resolved_1,
        "same file + same overrides must reproduce the identical fingerprint"
    );
}

/// Issue #1350 finding 2: `provenance verify` re-hashes the workflow file
/// named in the checkpoint and compares it with the recorded
/// `workflow_content_hash`. A workflow edited after the run is a
/// verification finding (exit 1, JSON `workflow_content_check.status =
/// "changed"`); a relocated checkpoint whose workflow file is gone is
/// reported as unverifiable WITHOUT failing (the output checksums stay the
/// audit's subject).
#[test]
fn cli_provenance_verify_catches_workflow_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("mutated.oxoflow");
    let v1 = "[workflow]\nname = \"mutated\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n";
    fs::write(&wf, v1).unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--provenance"])
        .current_dir(dir.path())
        .assert()
        .success();
    let cp = dir.path().join(".oxo-flow/checkpoint.json");

    // Clean state: the file still hashes to the recorded digest.
    let out = oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = &v["verify"]["workflow_content_check"];
    assert_eq!(check["status"].as_str(), Some("matched"), "{v}");
    assert_eq!(check["recorded"], v["verify"]["workflow_content_hash"]);
    assert!(check["actual"].is_string(), "{v}");
    assert!(v["verify"]["workflow_resolved_hash"].is_string(), "{v}");

    // Post-run mutation: the recorded bytes no longer match the file —
    // verify must say so and exit 1.
    fs::write(&wf, format!("{v1}\n# post-run edit\n")).unwrap();
    let out = oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "a changed workflow is a verification finding: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = &v["verify"]["workflow_content_check"];
    assert_eq!(check["status"].as_str(), Some("changed"), "{v}");
    assert_ne!(
        check["actual"].as_str().unwrap(),
        check["recorded"].as_str().unwrap()
    );

    // Relocated checkpoint (workflow file gone): unverifiable — reported,
    // not failed, so the output-checksum audit stays usable.
    fs::remove_file(&wf).unwrap();
    let out = oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = &v["verify"]["workflow_content_check"];
    assert_eq!(check["status"].as_str(), Some("unverifiable"), "{v}");
    assert!(
        check.get("actual").is_none() || check["actual"].is_null(),
        "{v}"
    );
    // The output checksums were unaffected by the workflow mutation and the
    // removal — they still verify.
    assert_eq!(v["verify"]["matched"].as_u64(), Some(1), "{v}");
    assert_eq!(v["verify"]["mismatched"].as_u64(), Some(0), "{v}");
}
