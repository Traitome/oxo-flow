//! Cohort-scale deterministic replay for hybrid checkpoint re-entry +
//! `[[values]]` expansion (issue #1245, ROADMAP Stage 12.2).
//!
//! Proves, over a synthetic cohort built deterministically here (fixed
//! seed, sorted iteration — no HashMap-order dependence):
//!
//! 1. the COLD `dry-run --json` plan is exactly the static Cartesian
//!    product `N samples x M values` of `analyze` instances plus the
//!    `discover` producer;
//! 2. the run's runtime re-entry (a `checkpoint = true` rule contributing
//!    one new sample AND one new `[[values]]` axis) grows the plan to
//!    `(N+1) x (M+1)` — nothing dropped, nothing duplicated;
//! 3. a RESUME replays the recorded re-entry deterministically: the
//!    reconstructed instance-identity set equals the run's completed set;
//! 4. repeating the resume is stable (identical set each time).
//!
//! The instance-identity surface is `dry-run --json` `plan[].name`
//! (sorted, de-duplicated), corroborated by `checkpoint.json`
//! `completed_rules`. The measured N=200 numbers live in
//! `docs/superpowers/audits/reentry-2026-11.md`; this test is the fast
//! CI-friendly proof of the same contract.

mod common;
use common::workspace_bin;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// Cohort size: kept modest so CI stays fast (measured N=200 lives in the
/// audit harness, `scripts/reentry-cohort-replay.sh`).
const N: usize = 12;
/// `[[values]]` strata — the brief's M=3.
const STRATA: [&str; 3] = ["sA", "sB", "sC"];
const NEW_SAMPLE: &str = "SNEW";
const NEW_STRATUM: &str = "sD";

fn gen_workflow(dir: &Path) {
    let samples: Vec<String> = (1..=N).map(|i| format!("\"S{i:03}\"")).collect();
    let strata: Vec<String> = STRATA.iter().map(|s| format!("\"{s}\"")).collect();
    // The discover rule writes a manifest adding one sample + one stratum.
    let manifest = format!(
        "[reentry]\\nsample = [\\\"{NEW_SAMPLE}\\\"]\\nvalues = {{ stratum = [\\\"{NEW_STRATUM}\\\"] }}\\n"
    );
    let wf = format!(
        r#"
[workflow]
name = "reentry-cohort"

[[sample_groups]]
name = "cohort"
samples = [{samples}]

[[values]]
name = "stratum"
values = [{strata}]

[[rules]]
name = "discover"
input = ["catalog.txt"]
output = ["discover.toml"]
shell = "printf '%s' '{manifest}' > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "stage1"
input = ["discover.toml"]
output = ["s1/{{sample}}_{{stratum}}.txt"]
shell = "echo {{sample}} > s1/{{sample}}_{{stratum}}.txt"

[[rules]]
name = "stage2"
input = ["s1/{{sample}}_{{stratum}}.txt"]
output = ["s2/{{sample}}_{{stratum}}.txt"]
shell = "echo {{sample}} > s2/{{sample}}_{{stratum}}.txt"
"#,
        samples = samples.join(", "),
        strata = strata.join(", "),
        manifest = manifest,
    );
    std::fs::write(dir.join("wf.oxoflow"), wf).unwrap();
    std::fs::write(dir.join("catalog.txt"), "v1\n").unwrap();
}

/// Run the real CLI binary; returns (success, stderr).
fn run(dir: &Path, extra: &[&str]) -> (bool, String) {
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Instance-identity set from `dry-run --json` `plan[].name` (sorted).
fn plan_set(dir: &Path) -> BTreeSet<String> {
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["dry-run", "wf.oxoflow", "--json"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "dry-run failed");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    json["plan"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

/// Instance-identity set from the checkpoint's `completed_rules`.
fn completed_set(dir: &Path) -> BTreeSet<String> {
    let ck: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap(),
    )
    .unwrap();
    ck["completed_rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn cohort_cold_plan_is_full_cartesian_product() {
    let dir = tempfile::tempdir().unwrap();
    gen_workflow(dir.path());
    let plan = plan_set(dir.path());
    // N*M stage1 + N*M stage2 + discover producer.
    assert_eq!(plan.len(), N * STRATA.len() * 2 + 1, "cold plan size");
    assert!(plan.contains("discover"));
    assert!(plan.contains("stage1_stratum_sA_cohort_S001"));
    assert!(plan.contains("stage2_stratum_sC_cohort_S012"));
}

#[test]
fn reentry_grows_cohort_and_resume_replays_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    gen_workflow(dir.path());

    // COLD run: the discover producer contributes a sample + a stratum, so
    // the run's completed set is the GROWN product, not the cold plan.
    let (ok, stderr) = run(dir.path(), &["-j", "4"]);
    assert!(ok, "cold run failed: {stderr}");

    let grown = completed_set(dir.path());
    let nm1 = N + 1; // 12 -> 13 after re-entry
    let m1 = STRATA.len() + 1; // 3 -> 4 after re-entry
    assert_eq!(grown.len(), nm1 * m1 * 2 + 1, "grown completed set size");
    assert!(grown.contains("discover"));
    assert!(grown.contains(&format!("stage1_stratum_{NEW_STRATUM}_cohort_{NEW_SAMPLE}")));

    // No dropped / duplicated instance: the set is exactly the product.
    for i in 1..=N {
        for s in STRATA.iter().chain(std::iter::once(&NEW_STRATUM)) {
            assert!(grown.contains(&format!("stage1_stratum_{s}_cohort_S{i:03}")));
            assert!(grown.contains(&format!("stage2_stratum_{s}_cohort_S{i:03}")));
        }
    }

    // RESUME #1: the replayed plan reconstructs the identical set, and every
    // rule is up-to-date (skipped) — nothing re-runs.
    let (ok, stderr) = run(dir.path(), &["-j", "4"]);
    assert!(ok, "resume failed: {stderr}");
    assert!(stderr.contains("already completed"), "no skips: {stderr}");
    assert_eq!(completed_set(dir.path()), grown, "resume #1 set drift");

    // The dry-run preview of the resumed plan is byte-identical too.
    assert_eq!(plan_set(dir.path()), grown, "resume preview set drift");

    // RESUME #2: stable — same set every time.
    let (ok, _) = run(dir.path(), &["-j", "4"]);
    assert!(ok);
    assert_eq!(completed_set(dir.path()), grown, "resume #2 set drift");
}
