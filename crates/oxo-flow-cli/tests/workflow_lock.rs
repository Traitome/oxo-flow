//! `oxo-flow lock` and `run --locked` / `[engine] locked = true` — the
//! workflow-level lockfile contract (issue #1399).
//!
//! Falsifiable assertions:
//!
//! 1. `lock` writes one canonical, deterministic document; a second
//!    generation is byte-identical and `lock --check` exits 0.
//! 2. `lock --check` never mutates the lock; drift exits 2 with the stable
//!    `LOCK-*` code and, in `--json` mode, one machine-readable document.
//! 3. A workflow that cannot be parsed is a usage error (exit 4).
//! 4. `run --locked` verifies before ANY rule executes: a missing or stale
//!    lock exits non-zero and leaves no rule side effect on disk.
//! 5. Without `--locked` / `[engine] locked` the lock is never read: a
//!    workflow with a stale lock still runs, and its checkpoint has no
//!    `lock_digest` field (byte-compatible with pre-#1399 checkpoints).
//! 6. A lock-enforced run records `lock_digest` in the checkpoint, reports
//!    it from `provenance verify --json`, and binds it into the attestation
//!    document as an additive `workflow.lock_digest` field.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Run a command in `dir`, returning `(exit_code, stdout, stderr)`.
fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {stdout}"))
}

/// A workflow that leaves an observable side effect (`ran.txt`) so a
/// refused run can be proven to have executed nothing.
const MARKER_WORKFLOW: &str = r#"
[workflow]
name = "lock-cli"
version = "1.0"

[[rules]]
name = "touch"
shell = "echo ran > ran.txt"
"#;

fn checkpoint(dir: &Path) -> Value {
    let path = dir.join(".oxo-flow/checkpoint.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("checkpoint must exist at {}: {e}", path.display()));
    serde_json::from_str(&text).expect("checkpoint is valid JSON")
}

// 1. Generate → clean check → identical regeneration.
#[test]
fn lock_write_and_check_are_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);

    let (code, _stdout, stderr) = run_in(dir.path(), &["lock", "wf.oxoflow"]);
    assert_eq!(code, 0, "lock writes: {stderr}");
    let lock_path = dir.path().join("oxo-flow.lock");
    let first = std::fs::read_to_string(&lock_path).unwrap();
    assert!(first.contains("oxo-flow.lock/v1"), "{first}");
    assert!(first.contains("lock_digest"), "{first}");

    let (code, _stdout, stderr) = run_in(dir.path(), &["lock", "--check", "wf.oxoflow"]);
    assert_eq!(code, 0, "a fresh lock checks clean: {stderr}");

    let (code, _stdout, _stderr) = run_in(dir.path(), &["lock", "wf.oxoflow"]);
    assert_eq!(code, 0);
    assert_eq!(
        first,
        std::fs::read_to_string(&lock_path).unwrap(),
        "regeneration must be byte-identical"
    );
}

// 2. `--check` is read-only and reports structured drift.
#[test]
fn lock_check_reports_workflow_drift_and_never_mutates() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    let lock_path = dir.path().join("oxo-flow.lock");
    let before = std::fs::read_to_string(&lock_path).unwrap();

    write(
        dir.path(),
        "wf.oxoflow",
        &MARKER_WORKFLOW.replace("echo ran", "echo changed"),
    );
    let (code, stdout, stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2, "drift exits 2: {stderr}");
    let document = parse_json(&stdout);
    assert_eq!(document["ok"], false);
    let findings = document["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["code"] == "LOCK-002"),
        "workflow drift must be LOCK-002: {document}"
    );
    assert!(
        stderr.contains("LOCK-002"),
        "the human report names the code: {stderr}"
    );
    assert_eq!(
        before,
        std::fs::read_to_string(&lock_path).unwrap(),
        "`--check` must never mutate the lock file"
    );
}

// 3. Missing lock under `--check` is LOCK-001 (exit 2), not a crash.
#[test]
fn lock_check_missing_lock_is_lock_001() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    let (code, stdout, _stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2);
    let document = parse_json(&stdout);
    assert_eq!(document["ok"], false);
    assert_eq!(document["findings"][0]["code"], "LOCK-001");
}

// 4. A malformed/unreadable/unsupported lock is LOCK-007 (exit 2).
#[test]
fn lock_check_malformed_lock_is_lock_007() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    write(dir.path(), "oxo-flow.lock", "{not json");
    let (code, stdout, _stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2);
    let document = parse_json(&stdout);
    assert_eq!(document["findings"][0]["code"], "LOCK-007");

    // A syntactically valid but unsupported format tag is also LOCK-007.
    write(
        dir.path(),
        "oxo-flow.lock",
        r#"{"format":"oxo-flow.lock/v99","generated_by":{"tool":"x","engine_version":"0"},"workflow":{"name":"x","version":"1","content_sha256":"sha256:0","resolved_hash":"sha256:0"},"lock_digest":"sha256:0"}"#,
    );
    let (code, stdout, _stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2);
    let document = parse_json(&stdout);
    assert_eq!(document["findings"][0]["code"], "LOCK-007");
}

// 5. Environment spec drift is LOCK-004 (offline: a file hash, no solver).
#[test]
fn lock_check_environment_spec_drift_is_lock_004() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "env-lock"
version = "1.0"

[[rules]]
name = "align"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
"#,
    );
    write(dir.path(), "envs/env.yaml", "bwa=0.7.17\n");
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    assert_eq!(
        run_in(dir.path(), &["lock", "--check", "wf.oxoflow"]).0,
        0,
        "an untouched spec checks clean"
    );
    write(dir.path(), "envs/env.yaml", "bwa=0.7.18\n");
    let (code, stdout, _stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2);
    let document = parse_json(&stdout);
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "LOCK-004"),
        "{document}"
    );
}

// 6. Include drift is LOCK-005.
#[test]
fn lock_check_include_drift_is_lock_005() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "inc-lock"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "main"
shell = "echo main"
"#,
    );
    write(
        dir.path(),
        "rules/extra.oxoflow",
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
    );
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    write(
        dir.path(),
        "rules/extra.oxoflow",
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo changed\"\n",
    );
    let (code, stdout, _stderr) = run_in(dir.path(), &["lock", "--check", "--json", "wf.oxoflow"]);
    assert_eq!(code, 2);
    let document = parse_json(&stdout);
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "LOCK-005"),
        "{document}"
    );
}

// 7. An unparsable workflow is a usage error (exit 4), not drift.
#[test]
fn lock_check_unparsable_workflow_exits_4() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", "this is not TOML [");
    let (code, _stdout, stderr) = run_in(dir.path(), &["lock", "--check", "wf.oxoflow"]);
    assert_eq!(code, 4, "usage/parse errors exit 4: {stderr}");
}

// 8. `run --locked` with a valid lock executes and records the digest.
#[test]
fn run_locked_with_valid_lock_executes_and_records_digest() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    let lock: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("oxo-flow.lock")).unwrap())
            .unwrap();

    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--locked",
            "-j",
            "1",
        ],
    );
    assert_eq!(code, 0, "a locked run with a fresh lock succeeds: {stderr}");
    assert!(dir.path().join("ran.txt").exists(), "the rule executed");
    let state = checkpoint(dir.path());
    assert_eq!(
        state["lock_digest"], lock["lock_digest"],
        "the checkpoint records the verified lock digest"
    );
    assert!(
        state["workflow_resolved_hash"].is_string(),
        "the folded resolved hash is recorded"
    );
}

// 9. `run --locked` fails closed BEFORE any rule executes.
#[test]
fn run_locked_missing_lock_fails_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--locked",
            "-j",
            "1",
        ],
    );
    assert_ne!(code, 0, "a missing lock refuses the run");
    assert!(stderr.contains("LOCK-001"), "{stderr}");
    assert!(
        !dir.path().join("ran.txt").exists(),
        "no rule may execute before the lock gate"
    );
}

// 10. A stale lock also fails closed before execution.
#[test]
fn run_locked_stale_lock_fails_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    write(
        dir.path(),
        "wf.oxoflow",
        &MARKER_WORKFLOW.replace("echo ran", "echo changed"),
    );
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--locked",
            "-j",
            "1",
        ],
    );
    assert_ne!(code, 0);
    assert!(stderr.contains("LOCK-002"), "{stderr}");
    assert!(
        !dir.path().join("ran.txt").exists(),
        "no rule may execute on drift"
    );
}

// 11. Legacy behavior: without the flag/key the lock is never read, and the
// checkpoint keeps its pre-#1399 shape.
#[test]
fn run_without_enforcement_ignores_a_stale_lock() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    write(
        dir.path(),
        "wf.oxoflow",
        &MARKER_WORKFLOW.replace("echo ran", "echo legacy"),
    );
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "-j",
            "1",
        ],
    );
    assert_eq!(
        code, 0,
        "without --locked the stale lock has no effect: {stderr}"
    );
    assert!(dir.path().join("ran.txt").exists());
    let state = checkpoint(dir.path());
    assert!(
        state.get("lock_digest").is_none(),
        "a legacy run must not grow a lock_digest field: {state}"
    );
}

// 12. `[engine] locked = true` enforces through the config key (the form
// web-spawned/queued runs inherit).
#[test]
fn engine_locked_key_enforces() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_text = MARKER_WORKFLOW.replace(
        "[workflow]\nname = \"lock-cli\"\nversion = \"1.0\"\n",
        "[workflow]\nname = \"lock-cli\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n",
    );
    let workflow = write(dir.path(), "wf.oxoflow", &workflow_text);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "-j",
            "1",
        ],
    );
    assert_eq!(code, 0, "[engine] locked with a fresh lock runs: {stderr}");
    assert!(dir.path().join("ran.txt").exists());

    // A drift under the config key refuses the next run.
    write(
        dir.path(),
        "wf.oxoflow",
        &workflow_text.replace("echo ran", "echo changed"),
    );
    std::fs::remove_file(dir.path().join("ran.txt")).unwrap();
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "-j",
            "1",
        ],
    );
    assert_ne!(code, 0);
    assert!(stderr.contains("LOCK-002"), "{stderr}");
    assert!(!dir.path().join("ran.txt").exists());
}

// 13. The lock digest reaches provenance verify and the attestation; a
// legacy run's documents do not grow the key.
#[test]
fn lock_digest_reaches_provenance_and_attestation() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--locked",
            "--provenance",
            "-j",
            "1",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let cp = dir.path().join(".oxo-flow/checkpoint.json");
    let lock: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("oxo-flow.lock")).unwrap())
            .unwrap();
    let digest = lock["lock_digest"].as_str().unwrap();

    let (code, stdout, stderr) = run_in(
        dir.path(),
        &["provenance", "verify", cp.to_str().unwrap(), "--json"],
    );
    assert_eq!(code, 0, "{stderr}");
    let document = parse_json(&stdout);
    assert_eq!(document["verify"]["lock_digest"], digest);

    let (code, stdout, stderr) =
        run_in(dir.path(), &["provenance", "attest", cp.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    let attestation = parse_json(&stdout);
    assert_eq!(attestation["workflow"]["lock_digest"], digest);
    assert_eq!(attestation["format"], "oxo-flow.attestation/v1");

    // Legacy arm: a second workdir without enforcement keeps the documents
    // unchanged (no lock_digest key anywhere).
    let legacy = tempfile::tempdir().unwrap();
    let legacy_workflow = write(legacy.path(), "wf.oxoflow", MARKER_WORKFLOW);
    let (code, _stdout, stderr) = run_in(
        legacy.path(),
        &[
            "run",
            legacy_workflow.to_str().unwrap(),
            "--workdir",
            legacy.path().to_str().unwrap(),
            "--provenance",
            "-j",
            "1",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let legacy_cp = legacy.path().join(".oxo-flow/checkpoint.json");
    let (code, stdout, _stderr) = run_in(
        legacy.path(),
        &[
            "provenance",
            "verify",
            legacy_cp.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let document = parse_json(&stdout);
    assert!(
        document["verify"].get("lock_digest").is_none(),
        "a legacy verify document must not grow the key: {document}"
    );
    let (code, stdout, _stderr) = run_in(
        legacy.path(),
        &["provenance", "attest", legacy_cp.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    let attestation = parse_json(&stdout);
    assert!(
        attestation["workflow"].get("lock_digest").is_none(),
        "a legacy attestation must not grow the key: {attestation}"
    );
}
