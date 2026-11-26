//! End-to-end strict-resume verification of recorded output manifests
//! (issue #1426).
//!
//! A rule that writes 1024 bytes is truncated to 500 after a successful run.
//! `resume` must re-execute it and restore the file (the recorded manifest's
//! size no longer matches) instead of reporting `completed` / `remaining 0`
//! from a size+mtime verdict. Intact outputs still skip; legacy checkpoints
//! (no manifest) re-run unless `--trust-mtime` accepts mtime evidence.
use std::path::{Path, PathBuf};
use std::process::Command;

const WRITER: &str = r#"[workflow]
name = "manifest-demo"

[[rules]]
name = "write"
output = ["out.bin"]
shell = "echo run >> runs.log && head -c 1024 /dev/zero > out.bin"
"#;

fn oxo() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    // Ambient overrides must never decide a test's verdict.
    cmd.env_remove("OXO_FLOW_MANIFEST_HASH_MAX_BYTES");
    cmd
}

fn run_cli(dir: &Path, args: &[&str], hash_cap: Option<&str>) -> (bool, String, String) {
    let mut cmd = oxo();
    cmd.args(args).current_dir(dir);
    if let Some(cap) = hash_cap {
        cmd.env("OXO_FLOW_MANIFEST_HASH_MAX_BYTES", cap);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write_workflow(dir: &Path) -> PathBuf {
    let wf = dir.join("w.oxoflow");
    std::fs::write(&wf, WRITER).unwrap();
    wf
}

fn checkpoint(dir: &Path) -> PathBuf {
    dir.join(".oxo-flow/checkpoint.json")
}

fn run_count(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs.log"))
        .map(|log| log.lines().count())
        .unwrap_or(0)
}

fn output_len(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("out.bin")).unwrap().len()
}

/// Remove the `output_manifests` map, simulating a pre-#1426 checkpoint
/// written before output manifests existed.
fn strip_output_manifests(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    doc.as_object_mut().unwrap().remove("output_manifests");
    std::fs::write(path, serde_json::to_string(&doc).unwrap()).unwrap();
}

fn first_run(dir: &Path, hash_cap: Option<&str>) {
    let wf = write_workflow(dir);
    let (ok, _stdout, stderr) = run_cli(dir, &["run", wf.to_str().unwrap(), "-j", "1"], hash_cap);
    assert!(ok, "initial run failed:\n{stderr}");
    assert_eq!(run_count(dir), 1, "the rule executed once");
    assert_eq!(output_len(dir), 1024);
}

/// The defect: a 1024-byte output truncated to 500 bytes was still reported
/// up to date. Strict `resume` re-executes and restores the artifact.
#[test]
fn resume_reruns_a_truncated_output_and_restores_it() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);

    // Truncate to 500 bytes. The rewrite also bumps the mtime, so the
    // historical size+mtime verdict would be "fresh" — only the recorded
    // manifest catches the damage.
    std::fs::write(dir.path().join("out.bin"), vec![0u8; 500]).unwrap();
    assert_eq!(output_len(dir.path()), 500);

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        output_len(dir.path()),
        1024,
        "the truncated output must be regenerated"
    );
    assert_eq!(run_count(dir.path()), 2, "the rule re-executed:\n{stderr}");
    assert!(
        stderr.contains("strict output verification failed"),
        "the forced re-run must be warned about:\n{stderr}"
    );
    assert!(
        stderr.contains("out.bin"),
        "the warning must name the output:\n{stderr}"
    );
}

/// An intact output verifies against the manifest: resume skips it.
#[test]
fn resume_skips_a_verified_output() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 1, "no re-execution:\n{stderr}");
    assert!(
        stderr.contains("1 completed, 0 failed, 0 remaining"),
        "the verified rule must leave nothing remaining:\n{stderr}"
    );
    assert!(
        stderr.contains("1 skipped"),
        "the verified rule must be skipped:\n{stderr}"
    );
}

/// A legacy checkpoint carries no output manifest. The safe default is to
/// re-execute; `--trust-mtime` restores the historical size+mtime verdict.
#[test]
fn resume_reruns_a_legacy_checkpoint_unless_trust_mtime() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);
    let cp = checkpoint(dir.path());

    strip_output_manifests(&cp);
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["resume", cp.to_str().unwrap()], None);
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "a manifest-less completion must re-execute:\n{stderr}"
    );
    assert!(
        stderr.contains("no output manifest is recorded"),
        "the reason must be named:\n{stderr}"
    );

    // Strip the fresh manifest again, then accept mtime evidence explicitly.
    strip_output_manifests(&cp);
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--trust-mtime"],
        None,
    );
    assert!(ok, "trust-mtime resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "--trust-mtime must accept the size+mtime verdict:\n{stderr}"
    );
    assert!(
        stderr.contains("1 skipped"),
        "the rule must be skipped:\n{stderr}"
    );
}

/// Above the recording cap the manifest stores size only. Strict resume
/// treats that entry as unverified (re-run); `--trust-mtime` skips it.
#[test]
fn resume_reruns_an_unhashed_large_output_unless_trust_mtime() {
    let dir = tempfile::tempdir().unwrap();
    // 1024-byte output, 16-byte cap → size-only manifest entry.
    first_run(dir.path(), Some("16"));
    let cp = checkpoint(dir.path());

    let entry = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&cp).unwrap())
        .unwrap()
        .get("output_manifests")
        .and_then(|m| m.get("write"))
        .and_then(|m| m.get(0))
        .cloned()
        .expect("manifest entry recorded");
    assert_eq!(entry["size"], 1024);
    assert!(
        entry.get("sha256").is_none(),
        "over-cap output must be size-only: {entry}"
    );

    let (ok, _stdout, stderr) = run_cli(dir.path(), &["resume", cp.to_str().unwrap()], Some("16"));
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "unverified entry re-runs:\n{stderr}"
    );
    assert!(
        stderr.contains("no recorded content hash"),
        "the unverified reason must be named:\n{stderr}"
    );

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--trust-mtime"],
        Some("16"),
    );
    assert!(ok, "trust-mtime resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 2, "trust-mtime skips:\n{stderr}");
    assert!(stderr.contains("1 skipped"), "{stderr}");
}

/// With a cap at/above the output size the hash is recorded, so an intact
/// output skips — the strict default is usable, not merely a re-run hammer.
#[test]
fn resume_skips_a_hashed_large_output() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), Some("1024"));

    let cp = checkpoint(dir.path());
    let entry = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&cp).unwrap())
        .unwrap()
        .get("output_manifests")
        .and_then(|m| m.get("write"))
        .and_then(|m| m.get(0))
        .cloned()
        .expect("manifest entry recorded");
    assert!(
        entry["sha256"]
            .as_str()
            .is_some_and(|h| h.starts_with("sha256:")),
        "at-cap output must record a hash: {entry}"
    );

    let (ok, _stdout, stderr) =
        run_cli(dir.path(), &["resume", cp.to_str().unwrap()], Some("1024"));
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 1, "hashed output skips:\n{stderr}");
}

/// A deleted declared output re-executes (the manifest has no file to
/// verify).
#[test]
fn resume_reruns_when_a_declared_output_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);
    std::fs::remove_file(dir.path().join("out.bin")).unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(output_len(dir.path()), 1024, "the output was regenerated");
    assert_eq!(run_count(dir.path()), 2, "the rule re-executed:\n{stderr}");
}
