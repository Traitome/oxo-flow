//! Editor-canvas environment parity with the run path (issue #1220).
//!
//! The editor preview reads the environment straight out of
//! `parse_pipeline` / `build_dag` (domains/workflow/service.rs), while the
//! run, dry-run and monitor surfaces resolve it through the documented
//! precedence `env_group` → the rule's inline environment → `[defaults]
//! environment` (issue #1210, folded by `apply_defaults`). Before #1220 the
//! editor read the raw authored inline value, so a rule with
//! `env_group = "docker-group"` + a conflicting inline `venv` painted
//! "venv" on the canvas while the run actually wrapped it in docker.
//!
//! These tests pin all three surfaces to the SAME resolved backend for the
//! `env_group` + conflicting-inline case, so the editor can never drift from
//! what the engine executes again.

use oxo_flow_core::WorkflowConfig;
use oxo_flow_web::domains::workflow::service::{build_dag, parse_pipeline};

/// `env_group` wins over the conflicting inline `venv`; an inline-only rule
/// keeps its inline backend. `with_group` is the parity-critical rule.
const ENV_GROUP_TOML: &str = r#"
[workflow]
name = "editor-env-group"
version = "1.0"

[env_groups.docker_group]
docker = "biocontainers/bwa:0.7.17"

[[rules]]
name = "with_group"
env_group = "docker_group"
environment = { venv = "envs/inline" }
output = ["a.txt"]
shell = "true > {output}"

[[rules]]
name = "inline_only"
environment = { venv = "envs/inline" }
output = ["b.txt"]
shell = "true > {output}"
"#;

/// The backend the run/dry-run path resolves for `rule` — the same
/// `resolve_environment` the engine and the DAG-status monitor call.
fn run_path_env(toml: &str, rule: &str) -> Option<String> {
    let config = WorkflowConfig::parse(toml).expect("parse");
    let r = config.rules.iter().find(|r| r.name == rule).expect("rule");
    config.resolve_environment(r).map(|e| e.kind().to_string())
}

/// Editor rule-summary environment (`parse_pipeline`) must equal the value
/// the run path resolves — for the env_group rule AND the inline rule.
#[test]
fn parse_pipeline_environment_matches_the_run_path() {
    let resp = parse_pipeline(ENV_GROUP_TOML, None).expect("parse_pipeline");

    let with_group = resp
        .rules
        .iter()
        .find(|r| r.name == "with_group")
        .expect("with_group summary");
    assert_eq!(
        with_group.environment.as_deref(),
        Some("docker"),
        "editor must paint the env_group backend the run executes, not the \
         conflicting inline venv"
    );
    assert_eq!(
        with_group.environment.as_deref(),
        run_path_env(ENV_GROUP_TOML, "with_group").as_deref(),
        "editor parse_pipeline and the run path disagree"
    );

    let inline_only = resp
        .rules
        .iter()
        .find(|r| r.name == "inline_only")
        .expect("inline_only summary");
    assert_eq!(inline_only.environment.as_deref(), Some("venv"));
    assert_eq!(
        inline_only.environment.as_deref(),
        run_path_env(ENV_GROUP_TOML, "inline_only").as_deref(),
    );
}

/// Editor DAG-canvas node environment (`build_dag`) must equal the run path
/// too — the surface the editor actually draws.
#[test]
fn build_dag_node_environment_matches_the_run_path() {
    let dag = build_dag(ENV_GROUP_TOML).expect("build_dag");

    let node = dag
        .nodes
        .iter()
        .find(|n| n.id == "with_group")
        .expect("with_group node");
    assert_eq!(
        node.environment, "docker",
        "editor DAG node must paint the env_group backend the run executes"
    );
    assert_eq!(
        node.environment,
        run_path_env(ENV_GROUP_TOML, "with_group").expect("run env"),
        "editor build_dag and the run path disagree"
    );

    let inline = dag
        .nodes
        .iter()
        .find(|n| n.id == "inline_only")
        .expect("inline_only node");
    assert_eq!(inline.environment, "venv");
}

/// All three editor surfaces plus the run path agree in one shot — the exact
/// audit assertion: the editor's environment string for a rule is identical
/// whichever surface produced it, and identical to what the engine wraps.
#[test]
fn editor_and_run_paths_agree_on_the_env_group_backend() {
    let run_env = run_path_env(ENV_GROUP_TOML, "with_group");
    let parsed = parse_pipeline(ENV_GROUP_TOML, None).expect("parse_pipeline");
    let dag = build_dag(ENV_GROUP_TOML).expect("build_dag");

    let parsed_env = parsed
        .rules
        .iter()
        .find(|r| r.name == "with_group")
        .and_then(|r| r.environment.clone());
    let dag_env = dag
        .nodes
        .iter()
        .find(|n| n.id == "with_group")
        .map(|n| n.environment.clone());

    assert_eq!(run_env.as_deref(), Some("docker"));
    assert_eq!(parsed_env, run_env);
    assert_eq!(dag_env, run_env);
}
