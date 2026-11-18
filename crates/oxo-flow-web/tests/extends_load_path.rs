//! Issue #1152 (audit ledger WF-01): the web load path (`WorkflowConfig::parse`
//! behind `POST /api/pipelines/parse` / `POST /api/pipelines/validate`) must
//! resolve rule-level `extends` — including multi-level chains — before it
//! derives the plan it reports.
//!
//! Commit-bound evidence: `WorkflowConfig::parse` never called
//! `resolve_rule_templates` before af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6
//! (#997) wired it in. These tests fail on that parent commit: the leaf rule
//! would report the default thread count and `system` environment instead of
//! the values it inherits through its parent.

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
