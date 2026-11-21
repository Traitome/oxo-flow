//! `oxo-flow env list --json` / `env check --json` (issue #1336).
//!
//! Falsifiable contract for the machine-readable documents:
//!
//! 1. both subcommands emit exactly one JSON document on stdout and nothing
//!    on stderr's machine path (the human banner/text goes to stderr only);
//! 2. the document carries the documented fields and nested arrays — deleting
//!    a field or an array element in `infra.rs` makes these assertions fail;
//! 3. the non-`--json` path keeps stdout empty (byte-identical to the
//!    stderr-only behavior it had before this change);
//! 4. `env check`'s failure exit code (1) survives the JSON mode.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Run a command, returning (exit_code, stdout, stderr).
fn run(args: &[&str], dir: &Path) -> (i32, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {stdout}"))
}

const WORKFLOW: &str = r#"
[workflow]
name = "env-json"

[defaults.environment]
conda = "envs/default.yaml"

[[rules]]
name = "step_a"
shell = "echo hi"

[[rules]]
name = "step_b"
shell = "echo hi"
[rules.environment]
venv = ".venv"
"#;

/// A spec the resolver rejects (modules combined with a real backend), so
/// `env check` reliably reports `ok: false` and exits 1.
const BAD_WORKFLOW: &str = r#"
[workflow]
name = "env-json-bad"

[[rules]]
name = "bad"
shell = "echo hi"
[rules.environment]
conda = "envs/default.yaml"
modules = ["gcc/11"]
"#;

fn write_workflow(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn env_list_json_no_workflow_lists_backends() {
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout, _stderr) = run(&["env", "list", "--json"], dir.path());

    assert_eq!(code, 0, "env list --json exits 0");
    let doc = parse_json(&stdout);
    assert_eq!(doc["command"], "env list");
    // No workflow -> no per-rule rows, but the resolver always reports at
    // least the `system` backend, and every backend row must carry the
    // documented fields (dropping either field fails here).
    assert!(doc["workflow"].is_null(), "workflow omitted without a path");
    let backends = doc["backends"]
        .as_array()
        .expect("env list --json carries a `backends` array");
    assert!(!backends.is_empty(), "at least `system` is available");
    for backend in backends {
        assert!(backend["name"].is_string(), "each backend has a name");
        assert!(
            backend["available"].is_boolean(),
            "each backend has an availability flag"
        );
    }
    assert_eq!(
        doc["rules"].as_array().map(Vec::len),
        Some(0),
        "no workflow -> empty `rules` array"
    );
}

#[test]
fn env_list_json_with_workflow_lists_resolved_rules() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "wf.oxoflow", WORKFLOW);
    let (code, stdout, _stderr) = run(&["env", "list", &wf, "--json"], dir.path());

    assert_eq!(code, 0, "env list --json exits 0");
    let doc = parse_json(&stdout);
    assert_eq!(doc["command"], "env list");
    assert_eq!(doc["workflow"], wf);
    let rules = doc["rules"]
        .as_array()
        .expect("env list --json carries a `rules` array");
    // Both rules are distinct (name+backend) and each row must carry `name`
    // and `backend` — dropping either fails the assertions below.
    assert_eq!(rules.len(), 2, "one row per rule");
    let names: Vec<&str> = rules
        .iter()
        .map(|r| r["name"].as_str().expect("rule row has a name"))
        .collect();
    assert!(names.contains(&"step_a") && names.contains(&"step_b"));
    for rule in rules {
        assert!(
            rule["backend"].as_str().is_some(),
            "each rule row has a resolved backend"
        );
    }
    // `list` documents never carry a check verdict.
    assert!(rules.iter().all(|r| r.get("ok").is_none()));
}

#[test]
fn env_check_json_reports_all_ok() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "wf.oxoflow", WORKFLOW);
    let (code, stdout, _stderr) = run(&["env", "check", &wf, "--json"], dir.path());

    assert_eq!(code, 0, "a valid workflow checks clean");
    let doc = parse_json(&stdout);
    assert_eq!(doc["command"], "env check");
    assert_eq!(doc["all_ok"], Value::Bool(true));
    let rules = doc["rules"].as_array().expect("`rules` array present");
    assert_eq!(rules.len(), 2);
    for rule in rules {
        assert_eq!(rule["ok"], Value::Bool(true), "each rule validated");
        assert!(rule["name"].is_string());
        assert!(rule["backend"].is_string());
    }
}

#[test]
fn env_check_json_failure_keeps_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "bad.oxoflow", BAD_WORKFLOW);
    let (code, stdout, _stderr) = run(&["env", "check", &wf, "--json"], dir.path());

    // The exit code is preserved under --json (the only machine signal today).
    assert_eq!(code, 1, "check failure still exits 1");
    let doc = parse_json(&stdout);
    assert_eq!(doc["command"], "env check");
    assert_eq!(doc["all_ok"], Value::Bool(false));
    let rules = doc["rules"].as_array().expect("`rules` array present");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["ok"], Value::Bool(false));
    // The failure reason is carried in-document, not only on stderr.
    let err = rules[0]["error"]
        .as_str()
        .expect("failed rule carries an `error` string");
    assert!(
        err.contains("modules"),
        "the error names the offending backend: {err}"
    );
}

#[test]
fn env_check_json_backend_availability_globally() {
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout, _stderr) = run(&["env", "check", "--json"], dir.path());

    assert_eq!(code, 0, "availability report exits 0");
    let doc = parse_json(&stdout);
    assert_eq!(doc["command"], "env check");
    // Availability is informational: the text path exits 0 even when a
    // backend is missing, so `all_ok` stays true here.
    assert_eq!(doc["all_ok"], Value::Bool(true));
    let backends = doc["backends"]
        .as_array()
        .expect("`backends` array present");
    assert!(!backends.is_empty());
    for backend in backends {
        assert!(backend["name"].is_string());
        assert!(backend["available"].is_boolean());
    }
}

#[test]
fn env_non_json_path_leaves_stdout_empty() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "wf.oxoflow", WORKFLOW);

    // The human output lives entirely on stderr; stdout stays pristine so a
    // `--json`-less invocation can never be mistaken for a document.
    let (code, stdout, stderr) = run(&["env", "list", &wf], dir.path());
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "no JSON on the non-json path: {stdout}");
    assert!(
        stderr.contains("step_a"),
        "human text is still on stderr: {stderr}"
    );

    let (code, stdout, _stderr) = run(&["env", "check", &wf], dir.path());
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "no JSON on the non-json path: {stdout}");
}

#[test]
fn env_gc_still_rejects_json() {
    // `env gc` has no document; the --json gate must keep rejecting it so the
    // newly-whitelisted `env` family does not silently swallow the flag.
    let dir = tempfile::tempdir().unwrap();
    let (_code, stdout, stderr) = run(&["--json", "env", "gc"], dir.path());
    assert!(stdout.is_empty(), "no document is produced for env gc");
    assert!(
        stderr.contains("does not support --json"),
        "the gate still rejects env gc: {stderr}"
    );
}
