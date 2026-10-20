//! Issue #882/#883 regression tests: when-aware closure must hold for FULL
//! runs and `--module` scoping, not only for `--target` runs.
//!
//! A pair-level `when` gate prunes the failing instance during expansion,
//! but the consumer's `expand_inputs` still bakes every pair's concrete
//! path into `rule.input` — the run then dies on the dead literal
//! (`cat: ... No such file or directory`). The fix drops when-skipped
//! outputs from the baked list (with a warning) and prunes unreachable
//! consumers from the full-run plan.

mod common;
use common::workspace_bin;

use assert_cmd::Command;
use std::fs;

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

const PRODUCER_MODULE: &str = r#"[workflow]
name = "repro"
version = "1.0.0"

[[pairs]]
pair_id = "P1"
experiment = "Pt01_T"
control = "Pt01_N"

[[pairs]]
pair_id = "P2"
experiment = "Pt02_T"
# tumor-only: no control -> {control} expands empty

[[rules]]
name = "fit_purity"
input = ["raw/{pair_id}.txt"]
output = ["ascat/{pair_id}/purity_ploidy.tsv"]
shell = """
mkdir -p ascat/{pair_id}
cp raw/{pair_id}.txt {output}
"""
when = "wildcard.control != ''"

[[rules]]
name = "ascat_cohort"
input = []
expand_inputs = [
  { pattern = "ascat/{pair_id}/purity_ploidy.tsv", variables = { pair_id = "config.pairs_list" } },
]
output = ["cohort/purity_ploidy_summary.tsv"]
shell = """
mkdir -p cohort
cat {input} > {output}
"""
"#;

/// Rule-only module: `[[pairs]]` live in the HOST file because `[[include]]`
/// merges rules/params/config but not pair tables — the module is validated
/// against the host's pairs at expansion time.
const RULES_MODULE: &str = r#"[workflow]
name = "cohort"
version = "1.0.0"

[[rules]]
name = "fit_purity"
input = ["raw/{pair_id}.txt"]
output = ["ascat/{pair_id}/purity_ploidy.tsv"]
shell = """
mkdir -p ascat/{pair_id}
cp raw/{pair_id}.txt {output}
"""
when = "wildcard.control != ''"

[[rules]]
name = "ascat_cohort"
input = []
expand_inputs = [
  { pattern = "ascat/{pair_id}/purity_ploidy.tsv", variables = { pair_id = "config.pairs_list" } },
]
output = ["cohort/purity_ploidy_summary.tsv"]
shell = """
mkdir -p cohort
cat {input} > {output}
"""
"#;

/// Full run WITHOUT `--target` must succeed when a pair instance is pruned
/// by its pair-level `when` gate and a cohort consumer bakes `expand_inputs`
/// over the pairs list (the exact #883 repro shape).
#[test]
fn cli_full_run_prunes_when_gated_pair_consumer() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("repro.oxoflow");
    fs::write(&wf, PRODUCER_MODULE).unwrap();
    fs::create_dir_all(dir.path().join("raw")).unwrap();
    fs::write(dir.path().join("raw/P1.txt"), b"x").unwrap();
    fs::write(dir.path().join("raw/P2.txt"), b"x").unwrap();

    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "full run must succeed: {stderr}");
    assert!(
        dir.path().join("ascat/P1/purity_ploidy.tsv").exists(),
        "when-true instance must run: {stderr}"
    );
    assert!(
        !dir.path().join("ascat/P2/purity_ploidy.tsv").exists(),
        "when-false instance must not run"
    );
    assert!(
        dir.path().join("cohort/purity_ploidy_summary.tsv").exists(),
        "cohort consumer must run on surviving producers: {stderr}"
    );
    let summary = fs::read_to_string(dir.path().join("cohort/purity_ploidy_summary.tsv")).unwrap();
    assert_eq!(
        summary.trim(),
        "x",
        "baked input must contain ONLY the surviving producer path"
    );
    assert!(
        stderr.contains("expand_inputs entries dropped"),
        "dropped-entry warning must be emitted: {stderr}"
    );
}

/// Same defect class via `--module` scoping (issue #882): module producer
/// resolution must honor pair-level when-verdicts so a module containing a
/// when-gated pair still runs to completion. The host declares the pairs;
/// the included module contributes the rules.
#[test]
fn cli_module_run_prunes_when_gated_pair_consumer() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("cohort.oxoflow"), RULES_MODULE).unwrap();
    let wf = dir.path().join("main.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "composed"
version = "1.0.0"

[[pairs]]
pair_id = "P1"
experiment = "Pt01_T"
control = "Pt01_N"

[[pairs]]
pair_id = "P2"
experiment = "Pt02_T"
# tumor-only: no control -> {control} expands empty

[[include]]
path = "cohort.oxoflow"
inputs = []
outputs = ["cohort/purity_ploidy_summary.tsv"]

[[rules]]
name = "stage_data"
output = ["raw/P1.txt", "raw/P2.txt"]
shell = "mkdir -p raw && echo x > raw/P1.txt && echo x > raw/P2.txt"
"#,
    )
    .unwrap();

    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--module", "cohort"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "module run must succeed with when-gated pair inside: {stderr}"
    );
    assert!(
        dir.path().join("cohort/purity_ploidy_summary.tsv").exists(),
        "module consumer must run: {stderr}"
    );
    assert!(
        !dir.path().join("ascat/P2/purity_ploidy.tsv").exists(),
        "when-false instance must not run"
    );
    assert!(
        dir.path().join("ascat/P1/purity_ploidy.tsv").exists(),
        "when-true instance must run: {stderr}"
    );
}
