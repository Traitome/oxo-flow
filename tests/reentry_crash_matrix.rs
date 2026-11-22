//! Adversarial crash / interruption matrix for checkpoint re-entry
//! (issue #1245, ROADMAP Stage 12.2).
//!
//! Each case drives the real `oxo-flow` binary through an abnormal stop and
//! then proves the RESUMED run **fails safe**: it either refuses with a clear
//! diagnostic, or re-runs the affected instance — it NEVER silently skips a
//! rule that did not actually complete.
//!
//! Covered (CI-exercisable, fast):
//!   * SIGTERM mid-run — graceful path records the in-flight rule as failed
//!     with signal evidence and flushes the checkpoint (exit 143).
//!   * SIGKILL mid-run — no flush; the checkpoint keeps the rule in `running`;
//!     the resume clears it and re-runs it (exit 137).
//!   * truncated / partially-written checkpoint — Execute refuses; the
//!     read-only preview degrades with a warning and plans everything.
//!   * unknown / older engine-version field — harmless (schema-additive):
//!     the checkpoint loads and completed rules still skip.
//!   * NEWER format version — a `checkpoint_format_version` past this
//!     engine's own: Execute refuses (`run`, `--resume-failed`, and the
//!     standalone `resume` command, the latter before its own trusting
//!     banner), the read-only preview degrades, and `status` warns
//!     (issue #1309; the one direction additive-serde cannot guard itself).
//!   * checkpoint missing a required field — treated as corrupt → refuses.
//!
//! See the `NOT_VALIDATED` list at the bottom for cases that cannot be
//! exercised in CI.

mod common;
use common::workspace_bin;

use std::path::Path;
use std::process::{Child, Command, Stdio};

fn run(dir: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn cp_text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap()
}

fn checkpoint(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&cp_text(dir)).unwrap()
}

/// Tolerant read for the polling loop: a missing/partial checkpoint file mid-run
/// is expected, so return `Null` instead of panicking.
fn checkpoint_opt(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Write a workflow: `slow` stays in flight long enough to interrupt, `quick`
/// finishes immediately. Both declare outputs so the engine validates them.
fn write_hang_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        "[workflow]\nname = \"crash\"\n\n\
         [[rules]]\nname = \"quick\"\noutput = [\"quick.done\"]\n\
         shell = \"echo q > quick.done\"\n\n\
         [[rules]]\nname = \"slow\"\noutput = [\"slow.done\"]\n\
         shell = \"sleep 5 && echo s > slow.done\"\n",
    )
    .unwrap();
}

/// A single fast rule — used to seed a valid checkpoint cheaply.
fn write_quick_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        "[workflow]\nname = \"ok\"\n\n\
         [[rules]]\nname = \"r1\"\noutput = [\"r1.done\"]\n\
         shell = \"echo x > r1.done\"\n",
    )
    .unwrap();
}

fn spawn(dir: &Path) -> Child {
    Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow", "-j", "2"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn signal(child: &Child, name: &str) {
    let ok = Command::new("kill")
        .args([name, &child.id().to_string()])
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "failed to deliver {name}");
}

/// Poll the checkpoint until `rule` shows as running (proves the CLI is
/// mid-flight), else panic after a deadline.
fn wait_running(dir: &Path, rule: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let ck = checkpoint_opt(dir);
        let running = has(&ck, "running", rule);
        if running {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "'{rule}' never reached the running state"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn has(list: &serde_json::Value, key: &str, needle: &str) -> bool {
    list[key]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(needle)))
}

#[test]
fn sigterm_records_inflight_rule_failed_and_resume_reruns_it() {
    let dir = tempfile::tempdir().unwrap();
    write_hang_workflow(dir.path());

    let mut child = spawn(dir.path());
    wait_running(dir.path(), "slow");
    signal(&child, "-TERM");
    let status = child.wait().unwrap();

    // 128 + SIGTERM = 143, and the death is NOT recorded as a success.
    assert_eq!(
        status.code(),
        Some(143),
        "SIGTERM must exit 143: {status:?}"
    );
    let ck = checkpoint(dir.path());
    assert!(
        has(&ck, "failed_rules", "slow"),
        "in-flight rule must be recorded failed: {ck}"
    );
    assert!(
        !has(&ck, "completed_rules", "slow"),
        "the interrupted rule must NOT be marked completed"
    );
    assert!(
        !dir.path().join("slow.done").exists(),
        "the interrupted rule must not leave a completed output"
    );

    // `slow` is now a failed rule — `--resume-failed` must re-run it.
    let out = run(dir.path(), &["--resume-failed"]);
    assert!(
        out.status.success(),
        "resume failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.path().join("slow.done").exists(),
        "the affected rule must actually re-run on resume"
    );
}

#[test]
fn sigkill_leaves_running_mark_and_resume_reruns_it() {
    use std::os::unix::process::ExitStatusExt;
    let dir = tempfile::tempdir().unwrap();
    write_hang_workflow(dir.path());

    let mut child = spawn(dir.path());
    wait_running(dir.path(), "slow");
    signal(&child, "-KILL");
    let status = child.wait().unwrap();

    // Killed by SIGKILL (no exit code), no graceful flush.
    assert_eq!(status.code(), None, "SIGKILL must not yield an exit code");
    assert_eq!(status.signal(), Some(9), "must die by SIGKILL: {status:?}");

    // The rule the engine was mid-spawn on survives as `running`.
    let ck = checkpoint(dir.path());
    assert!(
        has(&ck, "running", "slow"),
        "the in-flight rule must persist in `running` (no flush): {ck}"
    );

    // Resume: the stale `running` entry is cleared and the rule re-runs —
    // it is NEVER silently treated as completed.
    let out = run(dir.path(), &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("died with it and will re-run"),
        "resume must narrate the stale in-flight rule: {stderr}"
    );
    assert!(dir.path().join("slow.done").exists(), "slow must re-run");
}

#[test]
fn truncated_checkpoint_refuses_execute_and_degrades_preview() {
    let dir = tempfile::tempdir().unwrap();
    write_quick_workflow(dir.path());
    let out = run(dir.path(), &[]);
    assert!(out.status.success());
    // Truncate the checkpoint mid-file — the crash class the atomic rename
    // guards against, but a merged/edited file can still hit.
    let good = cp_text(dir.path());
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        &good[..good.len() / 3],
    )
    .unwrap();

    // Execute: fail safe — refuse, never silently re-run everything.
    let out = run(dir.path(), &[]);
    assert!(
        !out.status.success(),
        "Execute must refuse a corrupt checkpoint"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("corrupt") && stderr.contains("refusing to start"),
        "must name the corruption and refuse: {stderr}"
    );

    // Read-only preview: degrade with a warning, keep dry-run usable.
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["dry-run", "wf.oxoflow"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "dry-run must still succeed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("corrupt") && stderr.contains("never completed"),
        "preview must warn and plan everything: {stderr}"
    );
}

#[test]
fn unknown_and_missing_fields_are_handled_fail_safe() {
    let dir = tempfile::tempdir().unwrap();
    write_quick_workflow(dir.path());
    let out = run(dir.path(), &[]);
    assert!(out.status.success());

    // Unknown / older engine-version fields: schema-additive → harmless.
    let mut ck: serde_json::Value = checkpoint(dir.path());
    ck["engine_version"] = serde_json::json!("0.0.1-ancient");
    ck["schema_version"] = serde_json::json!(1);
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        serde_json::to_string_pretty(&ck).unwrap(),
    )
    .unwrap();
    let out = run(dir.path(), &[]);
    assert!(
        out.status.success(),
        "unknown fields must not break loading"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already completed"),
        "a genuinely-completed rule must still skip"
    );

    // Missing a required field → corrupt → refuse (never silently re-run).
    ck.as_object_mut().unwrap().remove("benchmarks");
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        serde_json::to_string_pretty(&ck).unwrap(),
    )
    .unwrap();
    let out = run(dir.path(), &[]);
    assert!(
        !out.status.success(),
        "a missing required field must refuse"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("refusing to start"),
        "must refuse, not silently re-run"
    );
}

#[test]
fn newer_format_version_checkpoint_refuses_execute_and_degrades_preview() {
    // Cross-version compatibility (issue #1309): a checkpoint written by a
    // NEWER engine records a `checkpoint_format_version` this build cannot
    // prove compatible. The re-entry contract forbids silently trusting its
    // completed-rule set, so Execute must refuse and the read-only preview
    // must degrade (plan everything) with a warning.
    let dir = tempfile::tempdir().unwrap();
    write_quick_workflow(dir.path());
    let out = run(dir.path(), &[]);
    assert!(out.status.success());

    // Simulate a future build: bump the marker well past this engine's.
    let mut ck = checkpoint(dir.path());
    ck["checkpoint_format_version"] = serde_json::json!(9999);
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        serde_json::to_string_pretty(&ck).unwrap(),
    )
    .unwrap();

    // Execute: fail safe — refuse, never silently skip on untrusted state.
    let out = run(dir.path(), &[]);
    assert!(
        !out.status.success(),
        "Execute must refuse a newer-format checkpoint"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NEWER oxo-flow build") && stderr.contains("refusing to start"),
        "must name the newer format and refuse: {stderr}"
    );

    // Read-only preview: degrade with a warning, keep dry-run usable.
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["dry-run", "wf.oxoflow"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "dry-run must still succeed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NEWER oxo-flow build") && stderr.contains("never"),
        "preview must warn and plan everything: {stderr}"
    );

    // Read-only viewer: `status` still shows the recorded state (inspection
    // is its purpose) but must not present untrusted completion state
    // silently.
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["status"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "status must stay usable");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NEWER oxo-flow build"),
        "status must warn that the format is untrusted: {stderr}"
    );
}

#[test]
fn standalone_resume_refuses_newer_format_checkpoint_before_its_banner() {
    // The standalone `resume` command loads the checkpoint itself for its
    // banner and JSON counts — that load must carry the same fail-safe
    // format gate (issue #1309): refuse BEFORE narrating a trusting
    // "Resuming workflow / N completed" banner from state this engine
    // cannot prove compatible.
    let dir = tempfile::tempdir().unwrap();
    write_quick_workflow(dir.path());
    let out = run(dir.path(), &[]);
    assert!(out.status.success());

    let mut ck = checkpoint(dir.path());
    ck["checkpoint_format_version"] = serde_json::json!(9999);
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        serde_json::to_string_pretty(&ck).unwrap(),
    )
    .unwrap();

    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["resume", ".oxo-flow/checkpoint.json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "standalone resume must refuse a newer-format checkpoint"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NEWER oxo-flow build") && stderr.contains("refusing to start"),
        "must name the newer format and refuse: {stderr}"
    );
    assert!(
        !stderr.contains("Resuming workflow"),
        "the trusting banner must not narrate before the refusal: {stderr}"
    );
}

#[test]
fn resume_with_absent_version_marker_still_skips_completed_work() {
    // A checkpoint WITHOUT the marker (written by a build that predates it)
    // must not hard-break resume: the legacy format is the one every
    // existing checkpoint uses, so absence is compatible by contract —
    // and the genuinely-completed work is still skipped, nothing re-runs.
    let dir = tempfile::tempdir().unwrap();
    write_quick_workflow(dir.path());
    let out = run(dir.path(), &[]);
    assert!(out.status.success());

    // Simulate the legacy writer: strip the version marker entirely.
    let mut ck = checkpoint(dir.path());
    ck.as_object_mut()
        .unwrap()
        .remove("checkpoint_format_version");
    std::fs::write(
        dir.path().join(".oxo-flow/checkpoint.json"),
        serde_json::to_string_pretty(&ck).unwrap(),
    )
    .unwrap();

    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["resume", ".oxo-flow/checkpoint.json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "a marker-less (legacy) checkpoint must still resume"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already completed"),
        "the genuinely-completed rule must still skip on a legacy checkpoint: {stderr}"
    );
}

// NOT_VALIDATED — cases the brief lists that this CI test does not exercise,
// with the reason:
//
//   * Real power loss / kernel panic between the staged `*.tmp` write and the
//     atomic rename. The atomic-rename design (staged temp + fsync + rename +
//     parent fsync) is exercised by code inspection and the truncated-file case
//     above, but a true mid-rename power cut needs hardware fault injection, not
//     a CI-runnable test.
//   * A checkpoint written by a genuine OLDER oxo-flow whose Rust struct lacks
//     a field this build requires. Simulated here by removing `benchmarks`
//     (which proves the fail-closed path); a real cross-version binary matrix
//     is out of scope for this campaign.
//   * SIGTERM/SIGKILL arriving in the narrow window before signal handlers are
//     installed (between process exec and `run`'s select). Covered in spirit
//     by the SIGKILL case (which needs no handler) but not separately pinned.
