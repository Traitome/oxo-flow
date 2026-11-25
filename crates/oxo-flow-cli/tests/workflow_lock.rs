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
    // Review M8: the checkpoint reuses the identity the gate verified
    // instead of recomputing it.
    assert_eq!(
        state["env_lock_identity"]["touch"], lock["rules"]["touch"]["environment"],
        "the checkpoint environment identity is the verified lock identity"
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

// 14. `lock --arg` pins the config context a `run --arg` will see, and a run
// that changes config under the lock is LOCK-003 (the documented way to run
// an overridden invocation under enforcement).
#[test]
fn lock_arg_pins_the_run_config_context() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_text = r#"
[workflow]
name = "lock-arg"
version = "1.0"

[config]
threshold = 10

[[rules]]
name = "touch"
shell = "echo ran > ran.txt; test {config.threshold} -eq 20"
"#;
    let workflow = write(dir.path(), "wf.oxoflow", workflow_text);

    // A lock generated for the DEFAULT config refuses the overridden run.
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--arg",
            "threshold=20",
            "--locked",
            "-j",
            "1",
        ],
    );
    assert_ne!(code, 0, "config drift under --locked must refuse: {stderr}");
    assert!(stderr.contains("LOCK-003"), "{stderr}");
    assert!(!dir.path().join("ran.txt").exists());

    // A lock generated WITH the override pins that context and the run
    // succeeds with it.
    let (code, _stdout, stderr) =
        run_in(dir.path(), &["lock", "--arg", "threshold=20", "wf.oxoflow"]);
    assert_eq!(code, 0, "{stderr}");
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "run",
            workflow.to_str().unwrap(),
            "--workdir",
            dir.path().to_str().unwrap(),
            "--arg",
            "threshold=20",
            "--locked",
            "-j",
            "1",
        ],
    );
    assert_eq!(
        code, 0,
        "a lock generated with the same --arg must verify: {stderr}"
    );
    assert!(dir.path().join("ran.txt").exists());
}

// 15. Review C1: the repair verification rerun must inherit `--locked`.
// The patch rewrites the workflow, so the child's gate fails `LOCK-002` and
// the run reports `REPAIR_RERUN_FAILED` instead of executing unverified
// bytes. Covers the flag-only form (the bypass) and the config-key form.
const FAILING_REPAIR_WORKFLOW: &str = r#"
[workflow]
name = "lock-repair"
version = "1.0"

[[rules]]
name = "boom"
shell = "echo original > ran.txt; exit 1"
"#;

const FIXING_PATCH: &str = r#"
[workflow]
name = "lock-repair"
version = "1.0"

[[rules]]
name = "boom"
shell = "echo patched > ran.txt; exit 0"
"#;

/// A whole-workflow patch that keeps the `[engine] locked = true` section
/// (the apply guard refuses a replacement that drops a declared section).
const FIXING_PATCH_WITH_ENGINE_LOCK: &str = r#"
[workflow]
name = "lock-repair"
version = "1.0"

[engine]
locked = true

[[rules]]
name = "boom"
shell = "echo patched > ran.txt; exit 0"
"#;

fn assert_repair_refused(dir: &Path, extra: &[&str], patch_text: &str) {
    let workflow = dir.join("wf.oxoflow");
    let patch = write(dir, "patch.oxoflow", patch_text);
    let mut args = vec![
        "run",
        workflow.to_str().unwrap(),
        "--workdir",
        dir.to_str().unwrap(),
        "--apply-patch",
        patch.to_str().unwrap(),
        "-j",
        "1",
    ];
    args.extend_from_slice(extra);
    let (code, _stdout, stderr) = run_in(dir, &args);
    assert_ne!(
        code, 0,
        "a repair under enforcement must not succeed (extra={extra:?}): {stderr}"
    );
    assert!(
        stderr.contains("REPAIR_APPLIED"),
        "the patch was applied: {stderr}"
    );
    assert!(
        stderr.contains("REPAIR_RERUN_FAILED"),
        "the verification rerun must fail closed: {stderr}"
    );
    assert!(stderr.contains("LOCK-002"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("ran.txt")).unwrap_or_default(),
        "original\n",
        "the patched rule must never execute under the lock"
    );
    // The workflow still no longer matches its lock: the refusal is real,
    // not a spurious error.
    let (check_code, _stdout, _stderr) = run_in(dir, &["lock", "--check", "wf.oxoflow"]);
    assert_eq!(check_code, 2);
}

#[test]
fn flag_only_locked_repair_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", FAILING_REPAIR_WORKFLOW);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    assert_repair_refused(dir.path(), &["--locked"], FIXING_PATCH);
}

#[test]
fn engine_locked_repair_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_text = FAILING_REPAIR_WORKFLOW.replace(
        "[workflow]\nname = \"lock-repair\"\nversion = \"1.0\"\n",
        "[workflow]\nname = \"lock-repair\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n",
    );
    write(dir.path(), "wf.oxoflow", &workflow_text);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    assert_repair_refused(dir.path(), &[], FIXING_PATCH_WITH_ENGINE_LOCK);
}

// A patch that flips `[engine] locked` to false must not be able to turn
// enforcement off for the rerun: the parent captures the effective
// enforcement at gate time and forwards `--locked`.
#[test]
fn repair_cannot_disable_engine_lock_and_continue() {
    let dir = tempfile::tempdir().unwrap();
    let workflow_text = FAILING_REPAIR_WORKFLOW.replace(
        "[workflow]\nname = \"lock-repair\"\nversion = \"1.0\"\n",
        "[workflow]\nname = \"lock-repair\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n",
    );
    write(dir.path(), "wf.oxoflow", &workflow_text);
    assert_eq!(run_in(dir.path(), &["lock", "wf.oxoflow"]).0, 0);
    assert_repair_refused(
        dir.path(),
        &[],
        &FIXING_PATCH_WITH_ENGINE_LOCK.replace("locked = true", "locked = false"),
    );
}

// 16. Review I2: the web/queued/remote contract stages `workflow.oxoflow`
// (plus client-supplied include_files) into a per-run workdir; a
// `[engine] locked` workflow without `oxo-flow.lock` staged beside it fails
// closed LOCK-001, and staging the lock (as an include_files entry) makes
// it run. This exercises the spawned-CLI contract the web server and remote
// wrapper use.
#[test]
fn web_style_staged_workflow_without_lock_fails_closed() {
    let source = tempfile::tempdir().unwrap();
    write(
        source.path(),
        "wf.oxoflow",
        &MARKER_WORKFLOW.replace(
            "[workflow]\nname = \"lock-cli\"\nversion = \"1.0\"\n",
            "[workflow]\nname = \"lock-cli\"\nversion = \"1.0\"\n\n[engine]\nlocked = true\n",
        ),
    );
    assert_eq!(run_in(source.path(), &["lock", "wf.oxoflow"]).0, 0);

    // Stage-only-the-workflow workdir (what a web create-run writes when the
    // client does not upload the lock).
    let staged = tempfile::tempdir().unwrap();
    std::fs::copy(
        source.path().join("wf.oxoflow"),
        staged.path().join("workflow.oxoflow"),
    )
    .unwrap();
    let (code, _stdout, stderr) = run_in(
        staged.path(),
        &[
            "run",
            staged.path().join("workflow.oxoflow").to_str().unwrap(),
            "--workdir",
            staged.path().to_str().unwrap(),
            "-j",
            "1",
        ],
    );
    assert_ne!(code, 0);
    assert!(stderr.contains("LOCK-001"), "{stderr}");
    assert!(!staged.path().join("ran.txt").exists());

    // Staging the lock beside the workflow (the documented `include_files`
    // entry) satisfies enforcement.
    std::fs::copy(
        source.path().join("oxo-flow.lock"),
        staged.path().join("oxo-flow.lock"),
    )
    .unwrap();
    let (code, _stdout, stderr) = run_in(
        staged.path(),
        &[
            "run",
            staged.path().join("workflow.oxoflow").to_str().unwrap(),
            "--workdir",
            staged.path().to_str().unwrap(),
            "-j",
            "1",
        ],
    );
    assert_eq!(code, 0, "a staged lock verifies: {stderr}");
    assert!(staged.path().join("ran.txt").exists());
}

// 17. Review M3: the lock output may never be the workflow itself.
#[test]
fn lock_refuses_to_overwrite_the_workflow() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", MARKER_WORKFLOW);
    let before = std::fs::read_to_string(dir.path().join("wf.oxoflow")).unwrap();
    let (code, _stdout, stderr) = run_in(dir.path(), &["lock", "wf.oxoflow", "-o", "wf.oxoflow"]);
    assert_eq!(code, 4, "the guard is a usage error: {stderr}");
    assert_eq!(
        before,
        std::fs::read_to_string(dir.path().join("wf.oxoflow")).unwrap(),
        "the workflow must be untouched"
    );
}

// 18. Review M7: workflow discovery failures are usage errors (4) in both
// modes, not a bare exit 1.
#[test]
fn lock_with_no_workflow_exits_4() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _stdout, stderr) = run_in(dir.path(), &["lock", "--check"]);
    assert_eq!(code, 4, "{stderr}");
}

// 19. Review I3: credential-shaped config keys are pinned as digests, while
// a plaintext undeclared override is warned about before commit.
#[test]
fn lock_hashes_credential_keys_and_warns_on_plaintext_overrides() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "lock-secrets"
version = "1.0"

[config]
token = { default = "declared-default", sensitive = true }
apiKey = "camel-declared"

[[rules]]
name = "touch"
shell = "echo hi"
"#,
    );
    let (code, _stdout, stderr) = run_in(
        dir.path(),
        &[
            "lock",
            "--arg",
            "API_TOKEN=supersecret-value",
            "--arg",
            "PLAIN_NOTE=hello",
            "wf.oxoflow",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let lock_text = std::fs::read_to_string(dir.path().join("oxo-flow.lock")).unwrap();
    assert!(
        !lock_text.contains("supersecret-value"),
        "credential-shaped keys must be hashed: {lock_text}"
    );
    assert!(lock_text.contains("sha256:"));
    assert!(
        !lock_text.contains("camel-declared"),
        "a declared camel-case apiKey must be hashed, not plaintext (review R2-1): {lock_text}"
    );
    assert!(
        lock_text.contains("\"apiKey\""),
        "the key is still pinned (as a digest): {lock_text}"
    );
    assert!(
        lock_text.contains("PLAIN_NOTE") && lock_text.contains("hello"),
        "a non-credential undeclared override is pinned verbatim: {lock_text}"
    );
    assert!(
        stderr.contains("pinned in plaintext") && stderr.contains("PLAIN_NOTE"),
        "the writer must warn about plaintext pins: {stderr}"
    );

    // The same token verifies; a rotated one is LOCK-003 without leaking it.
    let (code, _stdout, _stderr) = run_in(
        dir.path(),
        &[
            "lock",
            "--check",
            "--arg",
            "API_TOKEN=supersecret-value",
            "--arg",
            "PLAIN_NOTE=hello",
            "wf.oxoflow",
        ],
    );
    assert_eq!(code, 0);
    let (code, stdout, _stderr) = run_in(
        dir.path(),
        &[
            "lock",
            "--check",
            "--json",
            "--arg",
            "API_TOKEN=rotated-value",
            "--arg",
            "PLAIN_NOTE=hello",
            "wf.oxoflow",
        ],
    );
    assert_eq!(code, 2);
    assert!(
        !stdout.contains("rotated-value"),
        "drift output must not leak the value: {stdout}"
    );
}
