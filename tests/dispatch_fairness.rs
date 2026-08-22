//! Dispatch-fairness and run-loop accounting integration tests (issue #136).
//!
//! Guards the scheduler's submit-cap accounting end-to-end via the compiled
//! `oxo-flow` binary: `-j 0` must still execute rules, a failed rule must
//! not leak the running-count cap, and run-log surfaces (header masking,
//! `--log-file` resolution) must respect the masking/workdir contracts.
//! Kept in a dedicated crate so parallel sessions can own
//! `cli_integration.rs` independently (each integration-test crate
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

// ─── -j 0 (issue #136 fix 1) ────────────────────────────────────────

/// `-j 0` must clamp to one concurrent job like the semaphore does, not
/// silently run nothing: the submit-cap arithmetic used the raw `jobs`
/// value, so zero jobs meant zero submissions and a fake "0 succeeded" run.
#[test]
fn cli_jobs_zero_still_executes_rules() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("j0.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"j0\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "0"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Done: 1 succeeded"));

    assert!(
        dir.path().join("out.txt").exists(),
        "the rule must execute under -j 0"
    );
}

// ─── Submit-cap accounting after failures (issue #136 fix 2) ──────────

/// With `-j 1 --keep-going`, a failed rule must release its scheduler slot
/// so the remaining rules still run. Guards the running-count accounting:
/// if a failure path ever forgets `mark_completed`, the leaked slot shrinks
/// the submit cap to zero and every later rule silently never runs.
///
/// Note: the task-panic path in `run_command` (which previously leaked the
/// cap for real) cannot be triggered from the CLI — the executor returns
/// errors instead of panicking for reachable inputs — so this guard covers
/// the observable contract (cap accounting after a failed rule) end-to-end,
/// and the panic path itself is fixed + reasoned about in run.rs.
#[test]
fn cli_job1_keep_going_continues_after_rule_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("cap.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"cap\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"boom\"\noutput = [\"boom.txt\"]\nshell = \"exit 3\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    // Since #135, --keep-going still exits nonzero when any rule failed —
    // the exit code reflects the failure, the run does not die early.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "1", "--keep-going"])
        .current_dir(dir.path())
        .assert()
        .code(1);

    assert!(
        dir.path().join("out.txt").exists(),
        "with -j 1 the second rule must still run after the first fails — \
         a leaked running-count would permanently shrink the submit cap"
    );
}

// ─── Run-log header masking (issue #136 fix 3) ───────────────────────

/// The run-log header embeds the raw command line; sensitive values passed
/// via `--arg KEY=secret` must be masked there exactly like every other
/// captured surface (issue #99 B1), not written in plaintext.
#[test]
fn cli_run_log_header_masks_sensitive_arg_values() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("masklog.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"masklog\"\nversion = \"1.0.0\"\n\n[config]\nTOKEN = { default = \"not-used\", sensitive = true }\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--arg", "TOKEN=supersecret"])
        .current_dir(dir.path())
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".oxo-flow/logs/oxo-flow.log")).unwrap();
    assert!(
        !log.contains("supersecret"),
        "run log must not contain the raw sensitive value: {log}"
    );
    assert!(
        log.contains("TOKEN=***"),
        "run log must contain the masked command line: {log}"
    );
}

// ─── --log-file resolution (issue #136 fix 4) ─────────────────────────

/// A relative `--log-file` must resolve against the workdir (like the
/// default path), not against the current directory of the invocation.
#[test]
fn cli_log_file_relative_path_resolves_against_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let wf_dir = dir.path().join("wf");
    fs::create_dir(&wf_dir).unwrap();
    let wf = wf_dir.join("rellog.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"rellog\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    // Invoke from OUTSIDE the workdir so CWD-relative and workdir-relative
    // resolution are distinguishable.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--log-file", "logs/custom.log"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        wf_dir.join("logs/custom.log").exists(),
        "relative --log-file must resolve against the workdir (the workflow's directory)"
    );
    assert!(
        !dir.path().join("logs/custom.log").exists(),
        "relative --log-file must NOT resolve against the current directory"
    );
}
