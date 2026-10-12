//! Integration tests for `debug --rule` name resolution (issue #862):
//! the template rule name — the name every other surface (the workflow
//! file, `lint`, `validate`, docs) uses — must resolve after wildcard
//! expansion to all instances of that template, instance names keep
//! working, and a miss suggests the nearest rule names instead of a bare
//! "rule not found".

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Run `oxo-flow <sub> [args..]` in `dir`; return (status, stdout, stderr).
fn run_cli(dir: &Path, sub: &str, args: &[&str]) -> (bool, String, String) {
    let out = oxo()
        .args([sub])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// One pair-fanned rule and one plain rule: after expansion the pair rule
/// gains the `_<pair_id>` instance suffix (`align` → `align_P1`).
const WORKFLOW: &str = r#"
[workflow]
name = "debug-rule-demo"
version = "1.0.0"

[[pairs]]
pair_id = "P1"
experiment = "E1"
control = "C1"

[[rules]]
name = "align"
output = ["out/{pair_id}/aln.bam"]
shell = "echo aligning {pair_id}"

[[rules]]
name = "plain"
output = ["plain.txt"]
shell = "echo plain"
"#;

#[test]
fn template_rule_name_resolves_to_all_instances() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), WORKFLOW).unwrap();

    let (ok, stdout, stderr) = run_cli(dir.path(), "debug", &["w.oxoflow", "-r", "align"]);
    assert!(
        ok,
        "template name must resolve after wildcard expansion:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("template 'align' expanded to 1 instance(s)"),
        "the template-note line is missing:\n{stderr}"
    );
    assert!(
        stderr.contains("── Rule: align_P1 ──"),
        "the expanded instance section is missing:\n{stderr}"
    );
    assert!(
        !stdout.contains("Rule:"),
        "the report must stay on stderr (docs say so):\n{stdout}"
    );
}

#[test]
fn instance_rule_name_still_resolves() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), WORKFLOW).unwrap();

    let (ok, _stdout, stderr) = run_cli(dir.path(), "debug", &["w.oxoflow", "-r", "align_P1"]);
    assert!(ok, "instance names must keep working:\n{stderr}");
    assert!(
        stderr.contains("── Rule: align_P1 ──") && !stderr.contains("template '"),
        "instance resolution must not print the template note:\n{stderr}"
    );
}

#[test]
fn plain_rule_name_resolves_without_fan_out() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), WORKFLOW).unwrap();

    let (ok, _stdout, stderr) = run_cli(dir.path(), "debug", &["w.oxoflow", "-r", "plain"]);
    assert!(ok, "non-fanned rule must resolve directly:\n{stderr}");
    assert!(
        stderr.contains("── Rule: plain ──"),
        "missing section:\n{stderr}"
    );
}

#[test]
fn typo_suggests_nearest_rule_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), WORKFLOW).unwrap();

    let (ok, _stdout, stderr) = run_cli(dir.path(), "debug", &["w.oxoflow", "-r", "algn"]);
    assert!(!ok, "a typo must still exit non-zero");
    assert!(
        stderr.contains("did you mean"),
        "the miss must suggest candidates:\n{stderr}"
    );
    assert!(
        stderr.contains("align"),
        "the nearest name 'align' (one deletion from 'algn') must be offered:\n{stderr}"
    );
}

#[test]
fn total_miss_has_no_suggestion_suffix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.oxoflow"), WORKFLOW).unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        "debug",
        &["w.oxoflow", "-r", "zzzcompletely_unrelated"],
    );
    assert!(!ok);
    assert!(
        stderr.contains("rule 'zzzcompletely_unrelated' not found"),
        "the bare miss message must survive:\n{stderr}"
    );
    assert!(
        !stderr.contains("did you mean"),
        "no candidates fit — the suggestion clause must be absent:\n{stderr}"
    );
}
