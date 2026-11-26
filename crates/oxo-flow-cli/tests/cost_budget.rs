//! ENFORCED user-declared-rate cost budget (issue #1400) — CLI contract.
//!
//! Falsifiable assertions the plan requires:
//!
//! 1. Hitting the cap with live work stops new dispatch and reports the
//!    terminal `cost_budget_exceeded` outcome: no rule scheduled after the
//!    gate fired runs, while in-flight work settled (its side effect exists).
//! 2. Unknown accounting fails closed by default: an executed rule that did
//!    not report a configured-rate input makes accrual unknown, and with
//!    `on_unknown = "refuse"` (the default) the run stops with `accrued: null`
//!    and the offending rule named.
//! 3. `on_unknown = "report"` keeps running on the same unknown input: the run
//!    completes and carries no `cost_budget` key (the gate never fired).
//! 4. A budget-free workflow is byte-compatible: its `run --json` carries no
//!    `cost_budget` key.
//! 5. `report --resource-ledger` attaches `budget` + `cost_accrual` from the
//!    same measured metrics, and works on a legacy checkpoint that predates the
//!    `resource_usage` map (unknown, never a crash).
//! 6. Plan-time surfaces (`dry-run` / `validate`) report the declared budget as
//!    UNESTIMABLE — never a fabricated `0`.
//! 7. `report --resource-ledger -f csv` carries the budget columns.
//! 8. Plan-time surfaces (`dry-run` / `validate`) carry no `budget` key when
//!    `max_cost` is undeclared — byte-identical to the pre-#1400 output.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {stdout}"))
}

fn checkpoint_path(dir: &Path) -> PathBuf {
    dir.join(".oxo-flow/checkpoint.json")
}

/// A rule that spins the CPU long enough to accrue measurable CPU-seconds, then
/// leaves a marker. The loop is a *fixture*, not a workload: the point is to
/// generate real `cpu_seconds` so a tiny `cpu_second_rate` cap can be tripped
/// deterministically. Guards against the unknown-metric case by keeping the
/// spin well past the 200 ms sampler tick.
const BURN_RULE: &str = r#"
[workflow]
name = "cost-budget-cap"
version = "1.0"

[resource_budget]
max_cost = { amount = 0.0001, currency = "USD", cpu_second_rate = 0.001 }

[[rules]]
name = "burn"
shell = "i=0; while [ $i -lt 600000 ]; do i=$((i+1)); done; echo done > burn.txt"

[[rules]]
name = "after"
depends_on = ["burn"]
shell = "echo after > after.txt"
"#;

/// Two sub-200 ms rules (they finish before the sampler's first tick, so they
/// report no CPU/RSS metrics — the natural "unknown accounting" fixture).
fn unknown_workflow(on_unknown: Option<&str>) -> String {
    let policy = on_unknown
        .map(|p| format!(", on_unknown = \"{p}\""))
        .unwrap_or_default();
    format!(
        r#"
[workflow]
name = "cost-budget-unknown"
version = "1.0"

[resource_budget]
max_cost = {{ amount = 1.0, currency = "USD", gb_second_rate = 0.00001{policy} }}

[[rules]]
name = "a"
shell = "echo a > a.txt"

[[rules]]
name = "b"
depends_on = ["a"]
shell = "echo b > b.txt"
"#
    )
}

// 1. Cap hit stops new dispatch; in-flight work settled.
#[test]
fn cap_hit_stops_dispatch_and_reports_outcome() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", BURN_RULE);
    let (code, stdout, stderr) = run_in(dir.path(), &["run", "wf.oxoflow", "--json"]);
    assert_eq!(code, 1, "a stopped run exits non-zero: {stderr}");
    let doc = parse_json(&stdout);
    assert_eq!(doc["status"], "cost_budget_exceeded", "{doc}");
    let cb = doc
        .get("cost_budget")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("cost_budget detail must be present: {doc}"));
    // The cap and currency are echoed back verbatim.
    assert_eq!(cb["currency"], "USD");
    assert_eq!(cb["limit"], serde_json::json!(0.0001));
    // The pending rule that must NOT have been dispatched is named.
    assert_eq!(
        cb["pending_ready"],
        serde_json::json!(["after"]),
        "the ready-but-unrun rule must be reported: {doc}"
    );
    // Behavioural proof of "no new dispatch, in-flight settled": the rule that
    // ran left its side effect; the rule gated off never did.
    assert!(
        dir.path().join("burn.txt").exists(),
        "the rule that executed keeps its side effect"
    );
    assert!(
        !dir.path().join("after.txt").exists(),
        "no rule scheduled after the gate fired may run"
    );
    // Consistency: if the accrual is known it must be a number; otherwise the
    // unknown rules must be the reason (never a silent partial).
    if cb["accrued"].is_null() {
        assert!(
            !cb["unknown_rules"].as_array().unwrap().is_empty(),
            "unknown accrual must name the offending rule: {doc}"
        );
    } else {
        assert!(cb["accrued"].as_f64().unwrap() >= 0.0001);
    }
}

// 2. Unknown accounting fails closed by default.
#[test]
fn unknown_accounting_refuses_when_default() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", &unknown_workflow(None));
    let (code, stdout, stderr) = run_in(dir.path(), &["run", "wf.oxoflow", "--json"]);
    assert_eq!(code, 1, "refuse stops the run: {stderr}");
    let doc = parse_json(&stdout);
    assert_eq!(doc["status"], "cost_budget_exceeded", "{doc}");
    let cb = &doc["cost_budget"];
    assert!(cb["accrued"].is_null(), "unknown accrual is null: {doc}");
    assert_eq!(cb["on_unknown"], "refuse");
    assert_eq!(
        cb["unknown_rules"],
        serde_json::json!(["a"]),
        "the offending executed rule is named: {doc}"
    );
    // `a` executed (its effect exists); `b` was never dispatched.
    assert!(dir.path().join("a.txt").exists());
    assert!(
        !dir.path().join("b.txt").exists(),
        "refuse must stop before dispatching the next rule"
    );
}

// 3. `on_unknown = "report"` keeps running; no gate fired.
#[test]
fn unknown_accounting_reports_when_policy_report() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "wf.oxoflow", &unknown_workflow(Some("report")));
    let (code, stdout, stderr) = run_in(dir.path(), &["run", "wf.oxoflow", "--json"]);
    assert_eq!(code, 0, "report lets the run complete: {stderr}");
    let doc = parse_json(&stdout);
    assert_eq!(doc["status"], "completed", "{doc}");
    assert!(
        doc.get("cost_budget").is_none(),
        "the gate never fired, so no cost_budget key: {doc}"
    );
    assert!(dir.path().join("a.txt").exists());
    assert!(dir.path().join("b.txt").exists());
}

// 4. Budget-free workflow is unchanged.
#[test]
fn budget_free_workflow_carries_no_cost_budget_key() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        "[workflow]\nname = \"free\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"c\"\nshell = \"echo c > c.txt\"\n",
    );
    let (code, stdout, stderr) = run_in(dir.path(), &["run", "wf.oxoflow", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let doc = parse_json(&stdout);
    assert_eq!(doc["status"], "completed");
    assert!(
        doc.get("cost_budget").is_none(),
        "a budget-free run must not gain a cost_budget key (byte-compat): {doc}"
    );
    assert!(dir.path().join("c.txt").exists());
}

// 5. Report ledger attaches budget + cost_accrual; legacy checkpoint tolerated.
#[test]
fn report_ledger_carries_budget_and_tolerates_legacy_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "report-ledger"
version = "1.0"

[resource_budget]
max_cost = { amount = 1.0, currency = "USD", cpu_second_rate = 0.001 }

[[rules]]
name = "a"
shell = "sleep 1; echo a > a.txt"
"#,
    );
    let (code, _stdout, stderr) = run_in(dir.path(), &["run", "wf.oxoflow", "--json"]);
    assert_eq!(code, 0, "{stderr}");

    // Healthy checkpoint: budget echoed, accrual computed from measured metrics.
    let (code, stdout, stderr) = run_in(
        dir.path(),
        &["report", "--resource-ledger", "-f", "json", "wf.oxoflow"],
    );
    assert_eq!(code, 0, "{stderr}");
    let ledger = parse_json(&stdout);
    assert_eq!(ledger["budget"]["currency"], "USD");
    assert_eq!(ledger["cost_accrual"]["limit"], serde_json::json!(1.0));
    assert_eq!(ledger["cost_accrual"]["on_unknown"], "refuse");

    // Legacy checkpoint: strip the maps #1371 added. The ledger must still
    // render (unknown metrics, never a crash).
    let text = std::fs::read_to_string(checkpoint_path(dir.path())).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    let obj = doc.as_object_mut().unwrap();
    obj.remove("resource_usage");
    obj.remove("rule_runs");
    obj.remove("skipped_attempts");
    write(dir.path(), ".oxo-flow/checkpoint.json", &doc.to_string());

    let (code, stdout, stderr) = run_in(
        dir.path(),
        &["report", "--resource-ledger", "-f", "json", "wf.oxoflow"],
    );
    assert_eq!(
        code, 0,
        "legacy checkpoint must not crash the ledger: {stderr}"
    );
    let ledger = parse_json(&stdout);
    assert!(
        ledger.get("cost_accrual").is_some(),
        "budget still attaches on a legacy checkpoint: {ledger}"
    );
}

// 6. Report ledger CSV carries the budget columns.
#[test]
fn report_ledger_csv_has_budget_columns() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "csv-budget"
version = "1.0"

[resource_budget]
max_cost = { amount = 1.0, currency = "USD", cpu_second_rate = 0.001 }

[[rules]]
name = "a"
shell = "sleep 1; echo a > a.txt"
"#,
    );
    assert_eq!(run_in(dir.path(), &["run", "wf.oxoflow", "--json"]).0, 0);
    let (code, stdout, stderr) = run_in(
        dir.path(),
        &["report", "--resource-ledger", "-f", "csv", "wf.oxoflow"],
    );
    assert_eq!(code, 0, "{stderr}");
    let header = stdout.lines().next().unwrap_or_default();
    assert!(header.contains("budget_amount"), "header: {header}");
    assert!(header.contains("budget_currency"), "header: {header}");
    assert!(header.contains("accrued_cost"), "header: {header}");
}

// 7. Plan-time surfaces report the budget as unestimable.
#[test]
fn plan_time_surfaces_declare_budget_unestimable() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        r#"
[workflow]
name = "plan-budget"
version = "1.0"

[resource_budget]
max_cost = { amount = 2.5, currency = "USD", cpu_second_rate = 0.001, gb_second_rate = 0.00002 }

[[rules]]
name = "a"
shell = "echo a > a.txt"
"#,
    );
    for cmd in ["dry-run", "validate"] {
        let (code, stdout, stderr) = run_in(dir.path(), &[cmd, "wf.oxoflow", "--json"]);
        assert_eq!(code, 0, "{cmd}: {stderr}");
        let doc = parse_json(&stdout);
        let budget = doc
            .get("budget")
            .unwrap_or_else(|| panic!("{cmd} must carry a budget object: {doc}"));
        assert_eq!(budget["amount"], serde_json::json!(2.5), "{cmd}");
        assert_eq!(budget["currency"], "USD", "{cmd}");
        assert!(
            budget["accrued"].is_null(),
            "{cmd}: plan-time cost is never a fabricated number: {doc}"
        );
        assert_eq!(
            budget["estimate_available"],
            serde_json::json!(false),
            "{cmd}: plan-time estimate is unestimable: {doc}"
        );
    }
}

// 8. Plan-time surfaces stay byte-identical when no budget is declared: the
// `budget` key must be ABSENT, not `null`. This is the regression guard for the
// four unconditional-`"budget"` sites that serialized `null` for every workflow.
#[test]
fn plan_time_surfaces_have_no_budget_key_when_undeclared() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "wf.oxoflow",
        "[workflow]\nname = \"plan-free\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"a\"\nshell = \"echo a > a.txt\"\n",
    );
    for cmd in ["dry-run", "validate"] {
        let (code, stdout, stderr) = run_in(dir.path(), &[cmd, "wf.oxoflow", "--json"]);
        assert_eq!(code, 0, "{cmd}: {stderr}");
        let doc = parse_json(&stdout);
        assert!(
            doc.get("budget").is_none(),
            "{cmd}: no `budget` key when `max_cost` is undeclared (not null): {doc}"
        );
    }
}
