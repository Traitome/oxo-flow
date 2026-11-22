//! Integration tests for `resume --json` (issue #1334).
use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

const TWO_STEP: &str = r#"
[workflow]
name = "resume-json-demo"

[[rules]]
name = "generate_data"
output = ["data/greeting.txt"]
shell = "mkdir -p data && echo hello > data/greeting.txt"

[[rules]]
name = "transform"
input = ["data/greeting.txt"]
output = ["results/uppercase.txt"]
shell = "mkdir -p results && tr a-z A-Z < data/greeting.txt > results/uppercase.txt"
"#;

/// One completed rule (`generate_data`), no failures: work remains.
fn checkpoint_with_work(workflow_abs: &str) -> String {
    let cp = serde_json::json!({
        "completed_rules": ["generate_data"],
        "failed_rules": [],
        "benchmarks": {},
        "workflow_path": workflow_abs,
    });
    serde_json::to_string(&cp).unwrap()
}

/// Nothing executed yet: the nothing-to-resume early return.
fn checkpoint_empty(workflow_abs: &str) -> String {
    let cp = serde_json::json!({
        "completed_rules": [],
        "failed_rules": [],
        "benchmarks": {},
        "workflow_path": workflow_abs,
    });
    serde_json::to_string(&cp).unwrap()
}

/// Run `oxo-flow <args..>` in `dir`; return (ok, stdout, stderr).
fn run_cli(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Nothing executed yet: `--json` still emits a complete document with a
/// status a monitor can act on, and stdout stays pure JSON.
#[test]
fn resume_json_nothing_to_resume_emits_document() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_STEP).unwrap();
    let cp = dir.path().join("cp.json");
    std::fs::write(&cp, checkpoint_empty(&wf.display().to_string())).unwrap();

    let (ok, stdout, stderr) = run_cli(dir.path(), &["resume", "cp.json", "--json"]);
    assert!(ok, "nothing-to-resume exits 0; stderr:\n{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"));
    // Every field the document promises must be present, else this fails.
    assert_eq!(doc["command"], "resume");
    assert_eq!(doc["status"], "nothing_to_resume");
    assert_eq!(doc["completed"], 0);
    assert_eq!(doc["failed"], 0);
    assert_eq!(doc["remaining"], 2, "both rules remain unexecuted");
    assert!(doc["workflow"].as_str().unwrap().ends_with("w.oxoflow"));
    assert!(doc["checkpoint"].as_str().unwrap().ends_with("cp.json"));
    // Human narration stays on stderr; stdout is the document alone.
    assert!(
        stderr.contains("No rules have been executed"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Note:"),
        "prose leaked into stdout:\n{stdout}"
    );
}

/// Work to resume: the document reports the checkpoint's own counts and the
/// delegate run's outcome; exit code is today's (0 on success).
#[test]
fn resume_json_with_work_emits_document() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_STEP).unwrap();
    // transform consumes data/greeting.txt; pre-seed it so the one resumed
    // rule succeeds deterministically.
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/greeting.txt"), "hello\n").unwrap();
    let cp = dir.path().join("cp.json");
    std::fs::write(&cp, checkpoint_with_work(&wf.display().to_string())).unwrap();

    let (ok, stdout, stderr) = run_cli(dir.path(), &["resume", "cp.json", "--json"]);
    assert!(ok, "successful resume exits 0; stderr:\n{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"));
    assert_eq!(doc["command"], "resume");
    assert_eq!(doc["status"], "completed");
    assert_eq!(doc["completed"], 1, "the one already-complete rule");
    assert_eq!(doc["failed"], 0);
    assert_eq!(doc["remaining"], 1, "the one rule this resume runs");
    assert!(
        dir.path().join("results/uppercase.txt").exists(),
        "transform ran"
    );
}

/// The non-`--json` path is unchanged: stdout stays empty (human logs live
/// on stderr), so scripts that redirected stdout see exactly what they did
/// before.
#[test]
fn resume_without_json_stdout_empty() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_STEP).unwrap();
    let cp = dir.path().join("cp.json");
    std::fs::write(&cp, checkpoint_empty(&wf.display().to_string())).unwrap();

    let (ok, stdout, stderr) = run_cli(dir.path(), &["resume", "cp.json"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(
        stdout.is_empty(),
        "stdout must stay empty without --json:\n{stdout}"
    );
    assert!(
        stderr.contains("No rules have been executed"),
        "stderr:\n{stderr}"
    );
}

/// A malformed checkpoint under `--json` fails non-zero and still emits the
/// shared structured error envelope rather than empty stdout.
#[test]
fn resume_json_malformed_checkpoint_error_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join("cp.json");
    std::fs::write(&cp, "{ this is not valid json").unwrap();

    let (ok, stdout, _stderr) = run_cli(dir.path(), &["resume", "cp.json", "--json"]);
    assert!(
        !ok,
        "a malformed checkpoint must exit non-zero; stdout:\n{stdout}"
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"));
    assert_eq!(doc["command"], "resume");
    assert!(
        doc["errors"].is_array(),
        "errors envelope missing:\n{stdout}"
    );
    assert!(
        doc["errors"][0]["code"].is_string(),
        "errors[0].code missing:\n{stdout}"
    );
    assert!(
        doc["errors"][0]["message"].is_string(),
        "errors[0].message missing:\n{stdout}"
    );
}
