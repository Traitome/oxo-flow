//! Issue #1152 (audit ledger WF-01): the workflow load path must resolve
//! rule-level `extends` — including multi-level chains — before any derived
//! form (DAG, expanded plan) is built. These tests drive the real CLI
//! (`dry-run --json`, i.e. the loaded plan) against a temporary workflow.
//!
//! Commit-bound evidence: the resolution was wired into both load paths
//! (`WorkflowConfig::parse` and `WorkflowConfig::from_file`) by
//! af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6 (#997) after the audit found the
//! load path never called `resolve_rule_templates`. The namespaced-include
//! case was still broken until the `extends` reference was rewritten with the
//! module namespace in this PR.

use serde_json::Value;

mod common;

fn write_workflow(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn dry_run_json(wf: &std::path::Path, cwd: &std::path::Path) -> Value {
    let out = common::oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap(), "--json"])
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("dry-run --json must emit one JSON document")
}

fn plan_entry<'a>(json: &'a Value, name: &str) -> &'a Value {
    json["plan"]
        .as_array()
        .expect("plan array")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("rule '{name}' absent from the loaded plan: {json}"))
}

/// A -> B -> C inheritance chain: the leaf must receive values its direct
/// base only holds through *its* own resolution (threads from B's override,
/// memory/environment from A). This is the multi-level case the pre-#997
/// snapshot-based resolver got wrong.
#[test]
fn cli_load_path_resolves_multilevel_rule_extends() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(
        dir.path(),
        "multi.oxoflow",
        r#"
[workflow]
name = "extends-multilevel"
version = "1.0.0"

[[rules]]
name = "root_rule"
threads = 7
memory = "4G"
environment = { docker = "ubuntu:22.04" }
output = ["root_rule.txt"]
shell = "echo root > {output}"

[[rules]]
name = "mid_rule"
extends = "root_rule"
threads = 3
output = ["mid_rule.txt"]
shell = "echo mid > {output}"

[[rules]]
name = "leaf_rule"
extends = "mid_rule"
output = ["leaf_rule.txt"]
shell = "echo leaf > {output}"
"#,
    );

    let json = dry_run_json(&wf, dir.path());
    assert_eq!(
        json["total_rules"], 3,
        "all three rules in the plan: {json}"
    );

    // Root declares everything.
    let root = plan_entry(&json, "root_rule");
    assert_eq!(root["threads"], 7);
    assert_eq!(root["memory"], "4G");
    assert_eq!(root["environment"], "docker");

    // Mid overrides threads, inherits memory/environment.
    let mid = plan_entry(&json, "mid_rule");
    assert_eq!(mid["threads"], 3);
    assert_eq!(mid["memory"], "4G");
    assert_eq!(mid["environment"], "docker");

    // Leaf inherits the *resolved* mid values — the multi-level proof.
    let leaf = plan_entry(&json, "leaf_rule");
    assert_eq!(
        leaf["threads"], 3,
        "leaf must inherit mid's override: {json}"
    );
    assert_eq!(leaf["memory"], "4G", "leaf must inherit via mid: {json}");
    assert_eq!(
        leaf["environment"], "docker",
        "leaf must inherit via mid: {json}"
    );
}

/// The base workflow's rule is merged into the child's loaded plan before
/// `extends` resolution, so the child (which declares one rule) plans and
/// would execute two. This is the documented exit evidence: "a regression
/// test where a base workflow's rules appear in the loaded plan".
#[test]
fn cli_load_path_merges_included_base_rules_before_extends_resolution() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(
        dir.path(),
        "base.oxoflow",
        r#"
[workflow]
name = "base"
version = "1.0.0"

[[rules]]
name = "base_align"
threads = 5
memory = "6G"
environment = { docker = "ubuntu:22.04" }
output = ["base_align.txt"]
shell = "echo base > {output}"
"#,
    );
    let child = write_workflow(
        dir.path(),
        "child.oxoflow",
        r#"
[workflow]
name = "extends-include"
version = "1.0.0"

[[include]]
path = "base.oxoflow"

[[rules]]
name = "child_align"
extends = "base_align"
output = ["child_align.txt"]
shell = "echo child > {output}"
"#,
    );

    let json = dry_run_json(&child, dir.path());
    assert_eq!(
        json["total_rules"], 2,
        "the base workflow's rule must appear in the loaded plan: {json}"
    );
    let order = json["execution_order"].as_array().expect("execution order");
    assert!(
        order.iter().any(|name| name == "base_align"),
        "base_align must be planned: {json}"
    );

    // The child extends a rule that only exists after include resolution.
    let child = plan_entry(&json, "child_align");
    assert_eq!(
        child["threads"], 5,
        "extends resolves across includes: {json}"
    );
    assert_eq!(child["memory"], "6G");
    assert_eq!(child["environment"], "docker");
}

/// Namespaced includes rewrite rule names to `ns::rule`; `extends` must be
/// rewritten with them. Regression for the load-path failure this PR fixes:
/// the module declared `child_mod extends base_mod`, but only the rule name
/// was namespaced, so resolution reported "extends 'base_mod' which does not
/// exist" and the workflow could not load at all.
#[test]
fn cli_load_path_resolves_extends_inside_namespaced_include() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(
        dir.path(),
        "mod.oxoflow",
        r#"
[workflow]
name = "module"

[[rules]]
name = "base_mod"
threads = 6
output = ["base_mod.txt"]
shell = "echo base > {output}"

[[rules]]
name = "mid_mod"
extends = "base_mod"
output = ["mid_mod.txt"]
shell = "echo mid > {output}"

[[rules]]
name = "child_mod"
extends = "mid_mod"
output = ["child_mod.txt"]
shell = "echo child > {output}"
"#,
    );
    let host = write_workflow(
        dir.path(),
        "ns_host.oxoflow",
        r#"
[workflow]
name = "ns-host"
version = "1.0.0"

[[include]]
path = "mod.oxoflow"
namespace = "m"

[[rules]]
name = "host_rule"
output = ["host_rule.txt"]
shell = "echo host > {output}"
"#,
    );

    let json = dry_run_json(&host, dir.path());
    assert_eq!(json["total_rules"], 4, "1 host + 3 module rules: {json}");
    let child = plan_entry(&json, "m::child_mod");
    assert_eq!(
        child["threads"], 6,
        "namespace-qualified extends must resolve transitively: {json}"
    );
}
