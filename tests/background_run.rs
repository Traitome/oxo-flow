//! Background-run integration tests (issue #158 idea 1).
//!
//! `run --background` / `resume --background` must return immediately with
//! exit 0 while a DETACHED child runs the workflow to completion: the pid
//! file lands in the workdir, the run log carries the version header (the
//! child's tracing tee works), the checkpoint reflects the completed rules,
//! and the child process is gone once the run finishes.
//!
//! Kept in a dedicated crate so parallel sessions can own the other
//! integration-test files independently (each integration-test crate
//! compiles and links on its own).

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

// ─── helpers ───────────────────────────────────────────────────────────

/// Poll `cond` every 500ms until it returns true or `timeout` elapses.
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    cond()
}

/// Whether a process with the given pid currently exists (and is not a
/// zombie). `ps -p` on Unix; `tasklist` on Windows.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read and parse the background pid file.
fn read_pid_file(dir: &std::path::Path) -> u32 {
    let text = fs::read_to_string(dir.join(".oxo-flow/background.pid"))
        .expect("background.pid must be written by the foreground process");
    text.trim()
        .parse()
        .expect("background.pid must contain a numeric pid")
}

/// Whether the checkpoint records `rule` as completed.
fn checkpoint_completed(dir: &std::path::Path, rule: &str) -> bool {
    let Ok(text) = fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")) else {
        return false;
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    doc["completed_rules"]
        .as_array()
        .map(|rules| rules.iter().any(|r| r == rule))
        .unwrap_or(false)
}

/// A workflow whose single rule sleeps briefly so the detached child is
/// provably alive right after the foreground returns, then writes out.txt.
fn trivial_workflow(dir: &std::path::Path, name: &str) -> PathBuf {
    let wf = dir.join(format!("{name}.oxoflow"));
    fs::write(
        &wf,
        format!(
            "[workflow]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"sleep 1 && echo hi > {{output}}\"\n"
        ),
    )
    .unwrap();
    wf
}

// ─── run --background (issue #158 idea 1) ──────────────────────────────

/// The foreground invocation must exit 0 almost immediately (it only spawns
/// the detached child), the pid file must name a process that is alive right
/// after the spawn, and the workflow must complete in the background — the
/// checkpoint records the rule and out.txt exists. Once complete, the child
/// process is gone.
#[test]
fn cli_run_background_returns_immediately_and_completes() {
    let dir = tempfile::tempdir().unwrap();
    let wf = trivial_workflow(dir.path(), "bg");

    let started = Instant::now();
    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--background"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        output.status.success(),
        "foreground run --background must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "foreground must return immediately (took {elapsed:?}), not run the workflow"
    );
    assert!(
        output.stdout.is_empty(),
        "a background run must not emit the run's stdout from the foreground"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("started in background (pid"),
        "summary must announce the background pid, got: {stderr}"
    );
    assert!(
        stderr.contains("oxo-flow status"),
        "summary must name the monitoring command, got: {stderr}"
    );

    // The pid file names a live process right after the spawn (the rule
    // sleeps 1s, so the child cannot have finished yet).
    let pid = read_pid_file(dir.path());
    assert!(
        process_alive(pid),
        "child pid {pid} must be alive right after the foreground returns"
    );

    // … and the workflow completes in the background.
    assert!(
        wait_until(Duration::from_secs(60), || {
            dir.path().join("out.txt").exists() && checkpoint_completed(dir.path(), "gen")
        }),
        "workflow must complete in the background within 60s (child {pid} alive: {})",
        process_alive(pid)
    );

    // Once complete, the child process is gone.
    assert!(
        !process_alive(pid),
        "child pid {pid} must be gone after the background run completes"
    );
}

// ─── resume --background (issue #158 idea 1) ───────────────────────────

/// A failed run leaves the checkpoint; after the failure cause is fixed,
/// `resume <checkpoint> --background` must complete the remaining rules in
/// the background: foreground exits 0 immediately, both rules complete, and
/// the child process is gone afterwards.
#[test]
fn cli_resume_background_completes_remaining_rule() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("bgres.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"bgres\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"step1\"\noutput = [\"out1.txt\"]\nshell = \"test -f go.txt && echo ok > {output} || { echo 'step1 needs go.txt'; exit 1; }\"\n\n[[rules]]\nname = \"step2\"\noutput = [\"out2.txt\"]\nshell = \"echo done > {output}\"\ndepends_on = [\"step1\"]\n",
    )
    .unwrap();

    // First run fails at step1 (go.txt absent); step2 never runs.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .code(1);
    assert!(
        !dir.path().join("out2.txt").exists(),
        "step2 must not run while step1 failed"
    );

    // Fix the condition, then resume in the background.
    fs::write(dir.path().join("go.txt"), "go\n").unwrap();
    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    let started = Instant::now();
    let output = oxo_flow_cmd()
        .args(["resume", checkpoint.to_str().unwrap(), "--background"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        output.status.success(),
        "foreground resume --background must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "foreground resume must return immediately (took {elapsed:?})"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("started in background (pid"),
        "resume summary must announce the background pid"
    );

    let pid = read_pid_file(dir.path());
    assert!(
        process_alive(pid),
        "child pid {pid} must be alive right after the foreground returns"
    );

    assert!(
        wait_until(Duration::from_secs(60), || {
            dir.path().join("out2.txt").exists()
                && checkpoint_completed(dir.path(), "step1")
                && checkpoint_completed(dir.path(), "step2")
        }),
        "resume must complete both rules in the background within 60s (child {pid} alive: {})",
        process_alive(pid)
    );
    assert!(
        !process_alive(pid),
        "child pid {pid} must be gone after the resumed background run completes"
    );
}

// ─── pid file + run log (child tee) ────────────────────────────────────

/// The pid file lives at `<workdir>/.oxo-flow/background.pid` and the run
/// log — written by the CHILD's tracing tee, not the foreground — carries
/// the version header naming the workflow.
#[test]
fn cli_run_background_pid_file_and_log() {
    let dir = tempfile::tempdir().unwrap();
    let wf = trivial_workflow(dir.path(), "bgl");

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--background"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        wait_until(Duration::from_secs(60), || checkpoint_completed(
            dir.path(),
            "gen"
        )),
        "workflow must complete in the background within 60s"
    );

    // Pid file: exists with a numeric pid naming a now-gone process.
    let pid_text = fs::read_to_string(dir.path().join(".oxo-flow/background.pid"))
        .expect("background.pid must exist");
    let pid: u32 = pid_text
        .trim()
        .parse()
        .expect("background.pid must contain a numeric pid");
    assert!(
        !process_alive(pid),
        "child pid {pid} must be gone after the background run completes"
    );

    // Run log: the child's tracing tee wrote the version header.
    let log = fs::read_to_string(dir.path().join(".oxo-flow/logs/oxo-flow.log"))
        .expect("the child's run log must exist");
    assert!(
        log.contains("oxo-flow run log"),
        "run log must carry the header, got: {log}"
    );
    assert!(
        log.contains("oxo-flow: v"),
        "run log must carry the version header, got: {log}"
    );
    assert!(
        log.contains("workflow_name: bgl"),
        "run log must name the workflow, got: {log}"
    );
}

// ─── --background combined with --json ─────────────────────────────────

/// `--background` + `--json`: the foreground prints its summary to stderr
/// and exits 0 — stdout stays empty (the JSON run summary belongs to the
/// actual run, which happens in the child; documented in run.md).
#[test]
fn cli_run_background_with_json_exits_zero_and_keeps_stdout_empty() {
    let dir = tempfile::tempdir().unwrap();
    let wf = trivial_workflow(dir.path(), "bgj");

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--background", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "run --background --json must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "--background must not emit the run's JSON summary from the foreground"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("started in background (pid"),
        "the background summary must be on stderr"
    );

    // The workflow still completes in the background.
    assert!(
        wait_until(Duration::from_secs(60), || checkpoint_completed(
            dir.path(),
            "gen"
        )),
        "workflow must complete in the background within 60s"
    );
}
