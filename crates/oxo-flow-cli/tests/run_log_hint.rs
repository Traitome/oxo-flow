//! Integration tests for the empty-declared-log Info hint (issue #832):
//! a succeeded rule whose declared `log` path stayed empty (0-byte file or
//! empty dir) while both captured stdout/stderr tails are also empty gets
//! ONE Info hint that the tool likely self-logs elsewhere. Any content in
//! any of the three keeps the rule silent.

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Four rules covering the hint's gate matrix:
/// - `silent_tool`: `2>` creates a 0-byte log; nothing captured → hint fires
/// - `dir_log_tool`: declared log is an empty directory → hint fires
/// - `chatty_tool`: 0-byte log but stdout was captured → silent
/// - `stderr_tool`: content written into the declared log → silent
/// - `no_log_tool`: no `log` key at all → silent (nothing declared, nothing
///   to compare)
const HINT_MATRIX: &str = r#"
[workflow]
name = "log-hint-demo"

[[rules]]
name = "silent_tool"
output = ["out/silent.txt"]
log = "logs/silent.log"
shell = "mkdir -p logs out && true 2> logs/silent.log && echo done > out/silent.txt"

[[rules]]
name = "dir_log_tool"
output = ["out/dir.txt"]
log = "logs/dirrun"
shell = "mkdir -p logs/dirrun out && echo done > out/dir.txt"

[[rules]]
name = "chatty_tool"
output = ["out/chatty.txt"]
log = "logs/chatty.log"
shell = "mkdir -p logs out && echo captured-to-stdout 2> logs/chatty.log && echo done > out/chatty.txt"

[[rules]]
name = "stderr_tool"
output = ["out/stderr.txt"]
log = "logs/stderr.log"
shell = "mkdir -p logs out && echo boom > logs/stderr.log && echo done > out/stderr.txt"

[[rules]]
name = "no_log_tool"
output = ["out/no.txt"]
shell = "mkdir -p out && echo done > out/no.txt"
"#;

/// Run `oxo-flow run <workflow>` in `dir`; return (status, stdout, stderr).
fn run_cli(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn hint_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("likely writes its own log elsewhere"))
        .collect()
}

#[test]
fn empty_declared_log_with_fully_silent_tool_emits_one_hint_per_rule() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), HINT_MATRIX).unwrap();
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(ok, "all five rules must succeed:\n{stderr}");

    let hints = hint_lines(&stderr);
    assert_eq!(
        hints.len(),
        2,
        "exactly silent_tool and dir_log_tool fire; got:\n{stderr}"
    );
    assert!(
        hints.iter().any(|l| l.contains("silent_tool")),
        "0-byte declared log + empty tails must hint:\n{stderr}"
    );
    assert!(
        hints.iter().any(|l| l.contains("logs/dirrun")),
        "empty-dir declared log must hint at the path:\n{stderr}"
    );
    // The hint points at where the captured tails actually live.
    assert!(
        hints.iter().all(|l| l.contains("checkpoint job records")),
        "hint must point at the persisted tails:\n{stderr}"
    );
    // The hint is informational: the rules stay successful.
    for f in [
        "silent.txt",
        "dir.txt",
        "chatty.txt",
        "stderr.txt",
        "no.txt",
    ] {
        assert!(
            dir.path().join("out").join(f).exists(),
            "out/{f} must exist"
        );
    }
}

#[test]
fn content_in_any_channel_stays_silent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), HINT_MATRIX).unwrap();
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(ok, "run must succeed:\n{stderr}");

    // chatty_tool: declared log is 0 bytes but stdout was captured.
    let chatty = stderr
        .lines()
        .filter(|l| l.contains("chatty_tool") && l.contains("likely writes"))
        .count();
    assert_eq!(
        chatty, 0,
        "captured stdout must keep the rule silent:\n{stderr}"
    );

    // stderr_tool: the declared log itself has content.
    let stderr_tool = stderr
        .lines()
        .filter(|l| l.contains("stderr_tool") && l.contains("likely writes"))
        .count();
    assert_eq!(
        stderr_tool, 0,
        "log content must keep the rule silent:\n{stderr}"
    );

    // no_log_tool: nothing declared → nothing to compare → no hint.
    let no_log = stderr
        .lines()
        .filter(|l| l.contains("no_log_tool") && l.contains("likely writes"))
        .count();
    assert_eq!(
        no_log, 0,
        "rule without a log key must stay silent:\n{stderr}"
    );
}

#[test]
fn hint_repeats_per_run_not_per_rule_execution_channel() {
    // A second run re-executes nothing (up to date) — the hint is bound to
    // an actual execution, so the up-to-date skip must not re-hint.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), HINT_MATRIX).unwrap();
    let (ok1, _, err1) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(ok1, "first run must succeed:\n{err1}");
    assert_eq!(hint_lines(&err1).len(), 2, "first run hints twice:\n{err1}");

    let (ok2, _, err2) = run_cli(dir.path(), &["run", "w.oxoflow"]);
    assert!(ok2, "second run must succeed:\n{err2}");
    assert!(
        hint_lines(&err2).is_empty(),
        "up-to-date skips must not re-hint:\n{err2}"
    );
}
