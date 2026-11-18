//! Integration tests for the verified-repair loop's rerun layer (issue
//! #1164). The repair decision → apply path is unit-tested inside the crate
//! (`commands/run/repair.rs`, no live provider needed); these tests drive
//! the exact subprocess the repair loop spawns —
//! `oxo-flow run <workflow> --resume-failed --no-ai-recover` — against tiny
//! real workflows and assert the resume semantics (succeeded rules stay
//! skipped, failed rules re-run), the `--no-ai-recover` loop-guard flag, and
//! the checkpoint `repair_record` surviving the rerun.

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// The poisoned workflow: `boom` always fails after `prepare` succeeded.
const POISONED: &str = r#"
[workflow]
name = "repair-loop"

[[rules]]
name = "prepare"
output = ["out/prep.txt"]
shell = "mkdir -p out && echo ready > {output[0]}"

[[rules]]
name = "boom"
input = ["out/prep.txt"]
output = ["out/final.txt"]
shell = "exit 1"
"#;

/// The fixed workflow: exactly what `apply_fix` would write — same rules,
/// one changed shell.
const FIXED: &str = r#"
[workflow]
name = "repair-loop"

[[rules]]
name = "prepare"
output = ["out/prep.txt"]
shell = "mkdir -p out && echo ready > {output[0]}"

[[rules]]
name = "boom"
input = ["out/prep.txt"]
output = ["out/final.txt"]
shell = "echo fixed > {output[0]}"
"#;

/// Run `oxo-flow` in `dir`; return (success, stdout, stderr).
fn run_cli(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn checkpoint_json(dir: &Path) -> serde_json::Value {
    let path = dir.join(".oxo-flow/checkpoint.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The rerun subprocess (issue #1164 R1/R5b): after a failed run, applying
/// the fix and spawning `run --resume-failed --no-ai-recover` completes the
/// workflow — `prepare` stays skipped (succeeded), `boom` re-runs to
/// success — and the parent's repair_record survives the child's checkpoint
/// rewrite.
#[test]
fn verification_rerun_completes_the_fixed_workflow() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), POISONED).unwrap();

    // 1. The poisoned run fails and records `boom` as failed.
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(!ok, "the poisoned run must fail:\n{stderr}");
    let ck = checkpoint_json(dir.path());
    assert!(
        ck["failed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "boom"),
        "boom must be recorded as failed: {}",
        ck["failed_rules"]
    );
    assert!(
        ck["completed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "prepare"),
        "prepare must stay completed: {}",
        ck["completed_rules"]
    );

    // 2. The repair loop's apply step rewrites the workflow (the exact
    //    mechanical effect of `apply_fix`) and the applied repair_record is
    //    persisted BEFORE the rerun spawns.
    std::fs::write(dir.path().join("w.oxoflow"), FIXED).unwrap();
    let ck_path = dir.path().join(".oxo-flow/checkpoint.json");
    let mut ck: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ck_path).unwrap()).unwrap();
    ck["repair_record"] = serde_json::json!({
        "applied": true,
        "session_id": "sess-1164-test",
        "rerun_attempted": false,
        "rerun_succeeded": false,
        "detail": "AI fix applied; verification rerun pending",
        "timestamp": "2026-11-18T00:00:00+00:00",
    });
    std::fs::write(&ck_path, serde_json::to_string_pretty(&ck).unwrap()).unwrap();

    // 3. The verification rerun: the exact subprocess the repair loop
    //    spawns (see commands/run/repair.rs).
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", "w.oxoflow", "--resume-failed", "--no-ai-recover"],
    );
    assert!(ok, "the verification rerun must succeed:\n{stderr}");
    assert!(
        stderr.contains("re-running 1 failed rules"),
        "the rerun must report resume semantics:\n{stderr}"
    );

    // 4. Final checkpoint: nothing failed anymore, both rules completed, and
    //    the repair record survived the child's save.
    let ck = checkpoint_json(dir.path());
    assert!(
        ck["failed_rules"].as_array().unwrap().is_empty(),
        "the fixed rerun must clear the failure: {}",
        ck["failed_rules"]
    );
    assert!(
        ck["completed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "boom"),
        "boom must be completed after the rerun: {}",
        ck["completed_rules"]
    );
    let record = &ck["repair_record"];
    assert_eq!(record["applied"], serde_json::json!(true));
    assert_eq!(record["session_id"], "sess-1164-test");
}

/// A rerun whose "fix" does not actually fix the failure keeps the failure
/// path: the child exits non-zero (issue #1164 R3).
#[test]
fn verification_rerun_that_still_fails_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), POISONED).unwrap();
    let (ok, _stdout, _stderr) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(!ok);

    // The "fix" changes the workflow but `boom` still fails.
    let still_broken = FIXED.replace("echo fixed > {output[0]}", "exit 7");
    std::fs::write(dir.path().join("w.oxoflow"), still_broken).unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", "w.oxoflow", "--resume-failed", "--no-ai-recover"],
    );
    assert!(!ok, "a rerun of an unfixed workflow must fail:\n{stderr}");
    let ck = checkpoint_json(dir.path());
    assert!(
        ck["failed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "boom"),
        "boom must be failed again after the rerun: {}",
        ck["failed_rules"]
    );
}

/// The `--no-ai-recover` loop-guard flag (issue #1164 R1): a run with it can
/// never enter the repair loop, even with `[ai] enabled = true` +
/// `auto_fix = "always"` in the workflow — no repair narration, plain
/// failure exit.
#[test]
fn no_ai_recover_disables_recovery_even_for_ai_enabled_workflows() {
    let dir = tempfile::tempdir().unwrap();
    let wf = r#"
[workflow]
name = "repair-loop"

[ai]
enabled = true
auto_fix = "always"

[[rules]]
name = "boom"
shell = "exit 1"
"#;
    std::fs::write(dir.path().join("w.oxoflow"), wf).unwrap();

    let (ok, stdout, stderr) = run_cli(
        dir.path(),
        &[
            "run",
            "w.oxoflow",
            "--no-ai-recover",
            "--no-report-snapshot",
        ],
    );
    assert!(!ok, "the failing rule must fail the run:\n{stderr}");
    assert!(
        !stdout.contains("REPAIR_APPLIED") && !stderr.contains("REPAIR_APPLIED"),
        "the loop guard must keep the repair loop out of a guarded run:\n{stdout}\n{stderr}"
    );
    assert!(
        !stdout.contains("REPAIR_RERUN") && !stderr.contains("REPAIR_RERUN"),
        "no rerun may be attempted under --no-ai-recover:\n{stdout}\n{stderr}"
    );
}
