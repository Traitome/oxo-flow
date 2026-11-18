//! Issue #1152 (audit ledger WF-01): the web load path (`WorkflowConfig::parse`
//! behind `POST /api/pipelines/parse` / `POST /api/pipelines/validate`, and
//! the staged-include path behind `POST /api/runs`) must resolve rule-level
//! `extends` — including multi-level chains — before it derives the plan it
//! reports.
//!
//! Commit-bound evidence: `WorkflowConfig::parse` never called
//! `resolve_rule_templates` before af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6
//! (#997) wired it in. These tests fail on that parent commit: the leaf rule
//! would report the default thread count and `system` environment instead of
//! the values it inherits through its parent.
//!
//! The run-creation path had a second, include-specific bypass: it derived
//! the plan from the host TOML string before `include_files` were staged, so
//! a host rule extending an included base rule was rejected with
//! "extends 'base' which does not exist", and a plain include was executed by
//! the CLI but omitted from the reported plan. `POST /api/runs` now builds
//! its plan from the same `from_file`-resolved config the admission gate
//! uses.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const MULTILEVEL_TOML: &str = r#"
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
"#;

async fn post_json(uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = server::build_router("personal").oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Run-creation setup: the dedicated run limiter (5/min) and the global
/// concurrent-run quota are process-wide, and this binary creates several
/// runs across independent cases — mirror the other run-contract suites.
async fn ensure_run_db() {
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    oxo_flow_web::infra::quota::global_quota_tracker().update_config(
        oxo_flow_web::infra::quota::QuotaConfig {
            max_concurrent_runs: 100,
            ..Default::default()
        },
    );
    common::ensure_db().await;
}

/// The parse handler returns the loaded plan (`rules[]` + DAG). The leaf row
/// must carry the values resolved transitively through its parent: threads
/// from mid's override, environment/memory from root.
#[tokio::test]
async fn web_parse_handler_resolves_multilevel_rule_extends() {
    common::ensure_db().await;

    let (status, body) = post_json(
        "/api/pipelines/parse",
        json!({ "toml_content": MULTILEVEL_TOML }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "parse failed: {body}");

    let rules = body["rules"].as_array().expect("rules array");
    assert_eq!(rules.len(), 3, "all three rules reported: {body}");
    let rule = |name: &str| {
        rules
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("rule '{name}' missing from the loaded plan: {body}"))
    };

    assert_eq!(rule("root_rule")["threads"], 7);
    assert_eq!(rule("root_rule")["environment"], "docker");
    // Mid overrides threads only.
    assert_eq!(rule("mid_rule")["threads"], 3);
    assert_eq!(rule("mid_rule")["environment"], "docker");
    // Leaf inherits the *resolved* mid values — multi-level proof.
    assert_eq!(
        rule("leaf_rule")["threads"],
        3,
        "leaf must inherit mid's override through the web load path: {body}"
    );
    assert_eq!(
        rule("leaf_rule")["environment"],
        "docker",
        "leaf must inherit root's environment via mid: {body}"
    );

    // The DAG is derived from the same resolved rules, so its nodes carry
    // the inherited environment too.
    let nodes = body["dag"]["nodes"].as_array().expect("dag nodes");
    assert_eq!(nodes.len(), 3, "DAG must contain every rule: {body}");
    let leaf_node = nodes
        .iter()
        .find(|n| n["id"] == "leaf_rule")
        .expect("leaf_rule DAG node");
    assert_eq!(leaf_node["environment"], "docker");
    assert_eq!(leaf_node["rule"]["threads"], 3);
}

/// The validate handler shares the same load path. Each inherited field
/// leaves a per-rule diagnostic once resolution has run: W025 (deprecated
/// rule-level `threads`/`memory`) on every rule, and W028 (unqualified
/// docker image) on every rule that inherited root's environment. Before the
/// load path resolved `extends`, only `root_rule` would carry these — the
/// child rows had no threads before resolution even though it is a warning,
/// not an error.
#[tokio::test]
async fn web_validate_handler_reports_inherited_fields_for_multilevel_chain() {
    common::ensure_db().await;

    let (status, body) = post_json(
        "/api/pipelines/validate",
        json!({ "toml_content": MULTILEVEL_TOML }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "validate failed: {body}");
    assert_eq!(body["valid"], true, "workflow must validate: {body}");
    assert_eq!(body["rules"], 3);

    // `errors` carries the validate_format pass (severity-marked, so W028
    // lands here), `warnings` the lint pass (W025). Both are per-rule.
    let diagnostics: Vec<&Value> = body["errors"]
        .as_array()
        .into_iter()
        .chain(body["warnings"].as_array())
        .flatten()
        .collect();
    let rules_with = |code: &str| -> Vec<&str> {
        let mut names: Vec<&str> = diagnostics
            .iter()
            .filter(|d| d["code"] == code)
            .filter_map(|d| d["rule"].as_str())
            .collect();
        names.sort_unstable();
        names
    };

    // Load-path errors are separate from the warning-severity diagnostics
    // reported alongside them; a resolvable chain yields none.
    assert!(
        !diagnostics
            .iter()
            .any(|d| d["code"].as_str().is_some_and(|c| c.starts_with('E'))),
        "a resolvable extends chain must not report errors: {body}"
    );
    // W025/W028 are per-rule and inherited: all three rules carry them only
    // when mid/leaf received root's values at load time.
    for code in ["W025", "W028"] {
        assert_eq!(
            rules_with(code),
            vec!["leaf_rule", "mid_rule", "root_rule"],
            "{code} must be reported for every rule that inherits the field: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Run creation: the response's `execution_plan` is the LOADED PLAN.
// ---------------------------------------------------------------------------

fn plan_names(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body["execution_plan"]["execution_order"]
        .as_array()
        .expect("execution_plan.execution_order")
        .iter()
        .filter_map(|n| n.as_str().map(String::from))
        .collect();
    names.sort_unstable();
    names
}

/// `POST /api/runs` with a multi-level `extends` chain reports every rule in
/// the plan it derives. The chain itself is proven field-by-field by the
/// parse/validate cases above; this pins the run-load path to the same
/// resolution: the plan must not stop at the host text.
#[tokio::test]
async fn web_create_run_plan_contains_multilevel_extends_chain() {
    ensure_run_db().await;

    let (status, body) = post_json(
        "/api/runs",
        json!({ "toml_content": MULTILEVEL_TOML, "dry_run": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create run failed: {body}");
    assert_eq!(body["execution_plan"]["total_rules"], 3, "{body}");
    assert_eq!(
        plan_names(&body),
        vec![
            "leaf_rule".to_string(),
            "mid_rule".to_string(),
            "root_rule".to_string()
        ],
        "the loaded plan must contain base, parent and child: {body}"
    );
}

/// Regression for ledger WF-01's documented exit evidence on the web path:
/// a base workflow's rules must appear in the loaded plan, not just in the
/// merged config the CLI later executes. Before the fix the response plan
/// carried only the host rule while the spawned CLI planned both.
#[tokio::test]
async fn web_create_run_plan_lists_rules_from_staged_include() {
    ensure_run_db().await;

    const BASE: &str = r#"
[workflow]
name = "base"
version = "1.0.0"

[[rules]]
name = "base_align"
threads = 5
memory = "6G"
output = ["base_align.txt"]
shell = "echo base > {output}"
"#;
    const PLAIN_CHILD: &str = r#"
[workflow]
name = "include-plain"
version = "1.0.0"

[[include]]
path = "base.oxoflow"

[[rules]]
name = "child_align"
output = ["child_align.txt"]
shell = "echo child > {output}"
"#;

    let (status, body) = post_json(
        "/api/runs",
        json!({
            "toml_content": PLAIN_CHILD,
            "dry_run": true,
            "include_files": { "base.oxoflow": BASE },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create run failed: {body}");
    assert_eq!(body["execution_plan"]["total_rules"], 2, "{body}");
    assert_eq!(
        plan_names(&body),
        vec!["base_align".to_string(), "child_align".to_string()],
        "the staged base workflow's rule must appear in the loaded plan: {body}"
    );
    // The resource estimate is derived from the same resolved rules: the
    // base rule's 6G is visible only once the include is merged.
    assert_eq!(
        body["estimated_resources"]["max_memory_mb"], 6144,
        "estimates must be derived after include resolution: {body}"
    );
}

/// The include-specific bypass: the host rule `extends` a rule that exists
/// only inside a staged `[[include]]`. Before the fix, create-run parsed the
/// host string before the include was staged and answered 400
/// "extends 'base_align' which does not exist" for a workflow the CLI runs
/// successfully.
#[tokio::test]
async fn web_create_run_resolves_extends_across_staged_include() {
    ensure_run_db().await;

    const BASE: &str = r#"
[workflow]
name = "base"
version = "1.0.0"

[[rules]]
name = "base_align"
threads = 5
memory = "6G"
output = ["base_align.txt"]
shell = "echo base > {output}"
"#;
    const CHILD: &str = r#"
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
"#;

    let (status, body) = post_json(
        "/api/runs",
        json!({
            "toml_content": CHILD,
            "dry_run": true,
            "include_files": { "base.oxoflow": BASE },
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "extends must resolve across a staged include before the plan is derived: {body}"
    );
    assert_eq!(body["execution_plan"]["total_rules"], 2, "{body}");
    assert_eq!(
        plan_names(&body),
        vec!["base_align".to_string(), "child_align".to_string()],
        "the include-resolved base rule must be planned: {body}"
    );
}
