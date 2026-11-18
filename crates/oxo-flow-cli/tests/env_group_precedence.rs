//! Issue #1210: `env_group` is the documented highest-precedence
//! environment selector (`env_group` → the rule's inline `environment` →
//! `[defaults] environment`, workflow-format reference). `env list` honored
//! it, but the run path resolved only `rule.environment` after
//! `apply_defaults`, so `dry-run --json` previewed — and `run` executed —
//! the conflicting inline spec. One fixture, both surfaces, one answer.

use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// A group spec (docker) conflicting with the same rule's inline spec (venv),
/// plus an inline-only control rule proving the fold is not over-applied.
const FIXTURE: &str = r#"
[workflow]
name = "env-group-precedence"
version = "1.0"

[env_groups.group_env]
docker = "biocontainers/bwa:0.7.17"

[[rules]]
name = "with_group"
env_group = "group_env"
environment = { venv = "envs/inline" }
output = ["out/group.txt"]
shell = "echo group > {output}"

[[rules]]
name = "inline_only"
environment = { venv = "envs/inline" }
output = ["out/inline.txt"]
shell = "echo inline > {output}"
"#;

/// `env list` (the reporting surface) and `dry-run --json` (the execution
/// surface) must agree: `env_group` wins over a conflicting inline spec.
/// Before #1210 the second surface said `venv` here while the first said
/// `docker`, so this assertion failed on the pre-fix tree.
#[test]
fn env_list_and_dry_run_json_agree_on_env_group_precedence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), FIXTURE).unwrap();

    // Surface 1: `env list` reports the resolved backend on stderr.
    let out = oxo()
        .args(["env", "list", "w.oxoflow"])
        .current_dir(dir.path())
        .output()
        .expect("env list runs");
    assert!(out.status.success(), "env list failed: {:?}", out.status);
    let listed = String::from_utf8_lossy(&out.stderr);
    assert!(
        listed.contains("with_group [docker]"),
        "env list must report the env_group backend: {listed}"
    );
    assert!(
        listed.contains("inline_only [venv]"),
        "inline-only rule must keep its inline backend: {listed}"
    );

    // Surface 2: `dry-run --json` reports the plan the run executes.
    let out = oxo()
        .args(["dry-run", "w.oxoflow", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("dry-run runs");
    assert!(out.status.success(), "dry-run failed: {:?}", out.status);
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("dry-run stdout is not JSON: {e}"));
    let plan = parsed["plan"].as_array().expect("plan array");
    let previewed_env = |name: &str| -> String {
        plan.iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("rule '{name}' missing from plan: {parsed}"))["environment"]
            .as_str()
            .unwrap_or_else(|| panic!("rule '{name}' has no environment: {parsed}"))
            .to_string()
    };
    assert_eq!(
        previewed_env("with_group"),
        "docker",
        "dry-run must preview the documented winner (env_group), not the inline venv"
    );
    assert_eq!(previewed_env("inline_only"), "venv");
}
