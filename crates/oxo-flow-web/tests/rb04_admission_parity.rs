//! RB-04 (issue #1277): web run-creation admission and CLI preflight must
//! evaluate the IDENTICAL resolved workflow.
//!
//! The web create-run path stages sources, re-parses through the canonical
//! `from_file` loader, and runs the canonical BioType checker
//! (`validate_workflow_types`). The CLI's preflight applies `config_overrides`
//! BEFORE that same checker and the DAG build. Before #1277 the web applied
//! overrides only for the returned plan, so the admission graph was the
//! UN-overridden one: the same workflow + overrides were evaluated on two
//! different graphs. These tests pin the closed parity gap.
//!
//! This is a dedicated test binary (like `run_config_overrides.rs`) because
//! the run-creation rate limiter is a process-wide sliding window.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::process::Command;
use tower::ServiceExt;

mod common;

async fn ensure_db() {
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    common::ensure_db().await;
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("POST {uri}: body is not JSON ({e}): {bytes:?}"));
    (status, value)
}

/// Locate a workspace binary from the target directory. The CLI is not a
/// dependency of the web crate, so `CARGO_BIN_EXE_oxo-flow` is unset for
/// these tests; the sibling-path lookup mirrors `process_control_integration.rs`.
/// Requires `cargo build --workspace` first (CI does this).
fn workspace_bin(name: &str) -> std::path::PathBuf {
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; run `cargo build --workspace` first"
    );
}

/// A typed workflow whose DAG edge depends on `{config.input}`: rule `call`
/// consumes `{config.input}.sam` (typed `bam_sorted_indexed`) while rule
/// `align` produces `aligned.sam` (typed `sam_unsorted`). With the DEFAULT
/// (`aligned`) the edge align->call exists and the BioTypes mismatch
/// (TYPE-001/002/003); overriding `input=other` re-points `call` at an
/// unproduced file, so the edge vanishes and no contract is violated.
const CONFIG_ROUTED_TYPED_WORKFLOW: &str = r#"
[workflow]
name = "rb04-config-routed"

[config]
input = { default = "aligned" }

[types.sam_unsorted]
format = "sam"
sort_order = "unsorted"

[types.bam_sorted_indexed]
format = "bam"
sort_order = "coordinate"
index = "bai"

[[rules]]
name = "align"
output = ["aligned.sam"]
shell = "touch aligned.sam"

[rules.types]
outputs = { default = "sam_unsorted" }

[[rules]]
name = "call"
input = ["{config.input}.sam"]
output = ["calls.vcf"]
shell = "touch calls.vcf"

[rules.types]
inputs = { default = "bam_sorted_indexed" }
"#;

/// Resolved routing of one rule on the CLI side: its expanded inputs.
fn cli_plan_inputs(value: &Value, rule: &str) -> Vec<String> {
    value["plan"]
        .as_array()
        .expect("plan array")
        .iter()
        .find(|p| p["name"] == rule)
        .map(|p| {
            p["inputs_expanded"]
                .as_array()
                .expect("inputs_expanded array")
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Run the CLI `dry-run --json` on `toml` with the given overrides.
/// Returns `(success, resolved_node_names_sorted, total_rules, call_inputs,
/// stderr)`. `call_inputs` is `call`'s expanded inputs — the concrete evidence
/// that `{config.input}` was resolved through the override.
fn cli_dry_run(toml: &str, overrides: &[&str]) -> (bool, Vec<String>, u64, Vec<String>, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("w.oxoflow");
    std::fs::write(&path, toml).expect("write workflow");
    let mut cmd = Command::new(workspace_bin("oxo-flow"));
    cmd.arg("dry-run").arg(&path).arg("--json");
    for o in overrides {
        cmd.arg("--arg").arg(o);
    }
    let out = cmd.output().expect("spawn oxo-flow dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return (false, Vec::new(), 0, Vec::new(), stderr);
    }
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("CLI stdout not JSON ({e}): {stdout}"));
    let mut order: Vec<String> = value["execution_order"]
        .as_array()
        .expect("execution_order array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    order.sort();
    let total = value["total_rules"].as_u64().unwrap_or_default();
    let call_inputs = cli_plan_inputs(&value, "call");
    (true, order, total, call_inputs, stderr)
}

// ---------------------------------------------------------------------------
// (b) A typed workflow whose routing depends on an overridden {config.*}
//     value is admitted on the OVERRIDDEN graph, matching the CLI.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overridden_config_routing_decides_admission_exactly_like_the_cli() {
    ensure_db().await;
    let app = server::build_router("personal");

    // Override re-points `call`'s input away from `align`'s output, so the
    // typed edge disintegrates and admission succeeds on the OVERRIDDEN
    // graph. (The DEFAULT would have been rejected — proven below.)
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": CONFIG_ROUTED_TYPED_WORKFLOW,
            "dry_run": true,
            "config_overrides": ["input=other"]
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "override must be applied BEFORE admission so the re-routed graph is admitted: {body}"
    );

    // The CLI sees the same override as a reroute, not a contract violation.
    let (cli_ok, _, _, _, cli_err) = cli_dry_run(CONFIG_ROUTED_TYPED_WORKFLOW, &["input=other"]);
    assert!(
        cli_ok,
        "CLI must also admit the overridden graph: {cli_err}"
    );
}

#[tokio::test]
async fn default_routing_rejects_the_same_typed_violation_in_web_and_cli() {
    ensure_db().await;
    let app = server::build_router("personal");

    // No override (and the explicit default) keeps the align->call edge and
    // its contract violation on BOTH paths.
    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": CONFIG_ROUTED_TYPED_WORKFLOW,
            "dry_run": true,
            "config_overrides": ["input=aligned"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    for code in ["TYPE-001", "TYPE-002", "TYPE-003"] {
        assert!(message.contains(code), "web finding {code} missing: {body}");
    }

    let (cli_ok, _, _, _, cli_err) = cli_dry_run(CONFIG_ROUTED_TYPED_WORKFLOW, &["input=aligned"]);
    assert!(!cli_ok, "CLI must reject the same violation");
    for code in ["TYPE-001", "TYPE-002", "TYPE-003"] {
        assert!(
            cli_err.contains(code),
            "CLI finding {code} missing: {cli_err}"
        );
    }
}

// ---------------------------------------------------------------------------
// (a) Differential test: the same typed workflow + the same config_overrides
//     produce the IDENTICAL resolved DAG through web admission and CLI
//     preflight.
//
//     The resolved-DAG signal compared here is the sorted RULE SET plus the
//     resolved routing (`call`'s expanded inputs) and the DAG size. Raw
//     topological ORDER is deliberately NOT compared: for the two INDEPENDENT
//     rules left after the override removes the align->call edge, the web's
//     `WorkflowDag::execution_order()` (petgraph) and the CLI's
//     `parallel_display_order` break the tie differently. A tie-break between
//     independent nodes is a presentation choice, not a difference in the
//     resolved graph — both are valid topological orders of the same DAG.
//     The edge set itself is asserted directly via `parallel_groups` below.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn web_admission_and_cli_preflight_agree_on_the_resolved_dag() {
    ensure_db().await;
    let app = server::build_router("personal");

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": CONFIG_ROUTED_TYPED_WORKFLOW,
            "dry_run": true,
            "config_overrides": ["input=other"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut web_order: Vec<String> = body["execution_plan"]["execution_order"]
        .as_array()
        .expect("web execution_order")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    web_order.sort();
    let web_total = body["execution_plan"]["total_rules"]
        .as_u64()
        .unwrap_or_default();

    let (cli_ok, cli_order, cli_total, cli_call_inputs, cli_err) =
        cli_dry_run(CONFIG_ROUTED_TYPED_WORKFLOW, &["input=other"]);
    assert!(cli_ok, "CLI dry-run must succeed: {cli_err}");

    // Same resolved rule set and size on both paths.
    assert_eq!(
        web_order, cli_order,
        "web admission and CLI preflight must resolve the identical rule set"
    );
    assert_eq!(
        web_total, cli_total,
        "web admission and CLI preflight must agree on the resolved DAG size"
    );

    // The override reached BOTH graphs identically: `call`'s `{config.input}`
    // resolved to `other.sam` (the unproduced file), which is why there is no
    // align->call edge and the typed contract passes.
    assert_eq!(
        cli_call_inputs,
        vec!["other.sam".to_string()],
        "CLI must resolve {{config.input}} through the override: {cli_err}"
    );

    // Edge-set parity: with the align->call edge removed by the override,
    // `align` and `call` are independent and live in ONE parallel group on
    // the web side. (The DEFAULT keeps the edge — see the sibling test — and
    // would split them into two groups.)
    let groups = body["execution_plan"]["parallel_groups"]
        .as_array()
        .expect("web parallel_groups");
    let flat: Vec<String> = groups
        .iter()
        .flat_map(|g| g.as_array().expect("group array"))
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let mut flat_sorted = flat.clone();
    flat_sorted.sort();
    assert_eq!(
        flat_sorted, web_order,
        "every resolved rule appears exactly once across the parallel groups"
    );
    assert_eq!(
        groups.len(),
        1,
        "the override removed the align->call edge, so both rules are independent: {groups:?}"
    );
}

#[tokio::test]
async fn included_workflow_override_is_identical_in_plan_snapshot_and_argv() {
    ensure_db().await;
    let app = server::build_router("personal");

    let host = r#"
[workflow]
name = "rb04-included-snapshot"

[[include]]
path = "modules/routed.oxoflow"
"#;
    let included = r#"
[workflow]
name = "rb04-included-snapshot-module"

[config]
input = { default = "aligned" }

[[rules]]
name = "align"
output = ["aligned.sam"]
shell = "touch aligned.sam"

[[rules]]
name = "call"
input = ["{config.input}.sam"]
output = ["calls.vcf"]
shell = "touch calls.vcf"
"#;

    let (status, body) = post(
        &app,
        "/api/runs",
        json!({
            "toml_content": host,
            "dry_run": true,
            "include_files": {"modules/routed.oxoflow": included},
            "config_overrides": ["input=other"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        body["execution_plan"]["total_rules"].as_u64(),
        Some(2),
        "the include-resolved plan must contain both rules: {body}"
    );

    let run_id = body["run_id"].as_str().expect("run id");
    let pool = oxo_flow_web::infra::db::sqlite::try_pool().expect("SQLite pool");
    let (pipeline_snapshot, workdir, config_overrides): (String, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT pipeline_snapshot, workdir, config_overrides FROM runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("persisted run row");

    assert_eq!(
        config_overrides.as_deref(),
        Some(r#"["input=other"]"#),
        "the same override must forward to the spawned argv"
    );

    // RB-02: the row keeps the EXACT submitted host document (run identity,
    // retry + override replay). The effective workflow is materialized beside
    // the staged sources and is the value every derivation must have used.
    assert!(
        pipeline_snapshot.contains(r#"input = { default = "aligned" }"#)
            || pipeline_snapshot.contains("path = \"modules/routed.oxoflow\""),
        "the row snapshot must stay the submitted host document: {pipeline_snapshot}"
    );

    let workdir = workdir.expect("run workdir");
    let snapshot_path = std::path::Path::new(&workdir)
        .join(".oxo-flow")
        .join("resolved-workflow.oxoflow");
    let resolved_snapshot =
        std::fs::read_to_string(&snapshot_path).expect("resolved snapshot beside staged sources");
    let parsed =
        oxo_flow_core::config::WorkflowConfig::parse(&resolved_snapshot).expect("effective TOML");
    assert_eq!(
        parsed
            .config_placeholder_values()
            .get("config.input")
            .map(String::as_str),
        Some("other"),
        "the materialized effective snapshot must contain the override: {resolved_snapshot}"
    );
    assert!(
        resolved_snapshot.contains("{config.input}.sam"),
        "the snapshot keeps the engine template as workflow semantics"
    );
}
