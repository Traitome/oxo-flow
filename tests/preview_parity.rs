//! Parity contract test matrix for `run` vs `dry-run` (issue #77).
//!
//! The anti-drift invariant, asserted per scenario:
//!
//! > On the SAME initial state, the `dry-run` preview's will-run set must be
//! > exactly the set of rules a real `run` executes (skip likewise).
//!
//! Each scenario first predicts (`dry-run --json`, parsing
//! `checkpoint_preview.plan`), then executes (`run`) on the identical state,
//! and asserts set equality. The executed set is captured via a side-channel
//! execution log: every rule's shell appends its own name to `exec_log.txt`
//! as its first command, so the log is file-system ground truth for "this
//! rule really ran" — no reliance on checkpoint bookkeeping.
//!
//! Because the scenarios cover every invalidation source (config changes,
//! content-hash manifests, metadata manifests, legacy checkpoints, missing
//! outputs, `when` flips, profile merges, target/sample subsets), any future
//! change to `run`'s invalidation or skip semantics that forgets to update
//! the preview turns CI red immediately.

use assert_cmd::Command;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Locate a workspace binary from the target directory (same approach as
/// `cli_integration.rs` — binaries live in sub-crates, so `CARGO_BIN_EXE_*`
/// is not set for the root integration-test package).
fn workspace_bin(name: &str) -> PathBuf {
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
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

fn oxo_flow() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

/// Run the CLI in `dir`, assert success, and return stdout.
fn run_ok(dir: &Path, args: &[&str]) -> String {
    let out = oxo_flow()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn CLI with {args:?}: {e}"));
    assert!(
        out.status.success(),
        "command `oxo-flow {}` failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Parse a `dry-run --json` output into (will-run set, will-skip set) from
/// `checkpoint_preview.plan`.
fn preview_plan(dir: &Path, dry_args: &[&str]) -> (HashSet<String>, HashSet<String>) {
    let mut args = vec!["dry-run", "--json"];
    args.extend_from_slice(dry_args);
    let stdout = run_ok(dir, &args);

    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("dry-run --json output is not valid JSON: {e}\n{stdout}"));
    let plan = json
        .pointer("/checkpoint_preview/plan")
        .and_then(|p| p.as_array())
        .unwrap_or_else(|| panic!("checkpoint_preview.plan missing in JSON: {stdout}"));

    let mut will_run = HashSet::new();
    let mut will_skip = HashSet::new();
    for entry in plan {
        let name = entry["name"]
            .as_str()
            .unwrap_or_else(|| panic!("plan entry without name: {entry}"))
            .to_string();
        match entry["status"].as_str() {
            Some(status) if status.starts_with("run-") => {
                will_run.insert(name);
            }
            Some("skip") | Some("skip-when-condition") => {
                will_skip.insert(name);
            }
            other => panic!("unknown plan status {other:?} for {name}: {entry}"),
        }
    }
    (will_run, will_skip)
}

/// Read the execution log — one executed rule name per line.
fn executed(dir: &Path) -> HashSet<String> {
    let log = dir.join(EXEC_LOG);
    match fs::read_to_string(&log) {
        Ok(contents) => contents.lines().map(str::to_string).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(e) => panic!("cannot read execution log {}: {e}", log.display()),
    }
}

/// The execution log file every rule appends its name to.
const EXEC_LOG: &str = "exec_log.txt";

/// The parity contract, one round:
///
/// 1. Truncate the execution log (only THIS run's executions count).
/// 2. `dry-run` with `dry_args` → predicted will-run set `W`.
/// 3. `run` with `run_args` on the identical state → executed set `S`.
/// 4. Assert `W == S` (and that no preview-skipped rule executed).
///
/// `run_args` receives the workflow path via `run_args[0]`.
fn assert_parity(
    dir: &Path,
    dry_args: &[&str],
    run_args: &[&str],
    context: &str,
) -> (HashSet<String>, HashSet<String>) {
    // Dry-run first: it is strictly read-only, so the state the prediction
    // sees is byte-for-byte the state the run starts from.
    let (predicted_run, predicted_skip) = preview_plan(dir, dry_args);

    let _ = fs::remove_file(dir.join(EXEC_LOG)); // truncate before the run

    let mut args = vec!["run"];
    args.extend_from_slice(run_args);
    args.push("-j");
    args.push("2");
    run_ok(dir, &args);

    let actual = executed(dir);
    assert_eq!(
        predicted_run, actual,
        "[{context}] parity violation: dry-run predicted {predicted_run:?} \
         but run executed {actual:?}"
    );
    let wrongly_skipped: Vec<&String> = predicted_skip.intersection(&actual).collect();
    assert!(
        wrongly_skipped.is_empty(),
        "[{context}] parity violation: dry-run predicted skip for rules the run \
         executed: {wrongly_skipped:?}"
    );
    (predicted_run, predicted_skip)
}

/// Write a workflow file and return its path relative to `dir`.
fn write_workflow(dir: &Path, name: &str, toml: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, toml).unwrap();
    path
}

/// Two-step chain `step1 → step2` over `in.txt → out1.txt → out2.txt`.
/// Every rule logs itself to the execution log before doing its work.
fn chain_workflow() -> String {
    r#"[workflow]
name = "parity-chain"
version = "1.0"

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt"

[[rules]]
name = "step2"
input = ["out1.txt"]
output = ["out2.txt"]
depends_on = ["step1"]
shell = "echo step2 >> exec_log.txt; cp out1.txt out2.txt"
"#
    .to_string()
}

/// Baseline: run the workflow once so every rule completes and the
/// checkpoint records manifests, then truncate the log.
fn baseline_run(dir: &Path, wf: &Path, args: &[&str]) {
    let mut full = vec!["run", wf.to_str().unwrap()];
    full.extend_from_slice(args);
    full.push("-j");
    full.push("2");
    run_ok(dir, &full);
    let _ = fs::remove_file(dir.join(EXEC_LOG));
}

/// The fresh workflow used by the no-checkpoint scenario: the same chain
/// plus an independent rule whose `when` condition is false from the start.
fn fresh_workflow() -> String {
    r#"[workflow]
name = "parity-fresh"
version = "1.0"

[config]
enable_gate = 0

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt"

[[rules]]
name = "step2"
input = ["out1.txt"]
output = ["out2.txt"]
depends_on = ["step1"]
shell = "echo step2 >> exec_log.txt; cp out1.txt out2.txt"

[[rules]]
name = "gate"
input = ["out2.txt"]
output = ["gated.txt"]
depends_on = ["step2"]
when = "config.enable_gate == 1"
shell = "echo gate >> exec_log.txt; cp out2.txt gated.txt"
"#
    .to_string()
}

// ─── The scenario matrix ────────────────────────────────────────────────────

/// 1. Fresh run, no checkpoint: every non-`when`-false rule is predicted and
///    executed; the `when`-false rule is in neither set.
#[test]
fn parity_fresh_run_without_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "fresh.oxoflow", &fresh_workflow());
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();

    let (predicted_run, predicted_skip) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "fresh run",
    );

    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "when-false rule must be excluded from the predicted run set"
    );
    assert!(predicted_skip.contains("gate"), "gate must be skipped");
    // The gate rule produced nothing — a real run agrees it never executed.
    assert!(!dir.path().join("gated.txt").exists());
}

/// 2. Same-size input rewrite: the content-hash path (≤ 64 MiB) detects it,
///    invalidating the consumer and cascading downstream.
#[test]
fn parity_same_size_input_rewrite_content_hash() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "chain.oxoflow", &chain_workflow());
    fs::write(dir.path().join("in.txt"), "data-aaaa").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Same length, different content — size and mtime alone cannot see it.
    fs::write(dir.path().join("in.txt"), "data-bbbb").unwrap();

    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "same-size rewrite",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "content change must invalidate the consumer and cascade downstream"
    );
}

/// 3. Large input (> 64 MiB, the content-hash threshold): manifests fall
///    back to size+mtime. A size change invalidates via the metadata path.
#[test]
fn parity_large_input_metadata_path() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "large.oxoflow", &chain_workflow());
    // 64 MiB + 1 byte exceeds MANIFEST_HASH_MAX_BYTES — the manifest records
    // size+mtime only, exactly like the engine's large-file policy.
    fs::write(dir.path().join("in.txt"), vec![b'x'; 64 * 1024 * 1024 + 1]).unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Grow the file by one byte: same content, new size + mtime.
    let mut grown = fs::read(dir.path().join("in.txt")).unwrap();
    grown.push(b'x');
    fs::write(dir.path().join("in.txt"), grown).unwrap();

    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "large-input size change",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "size change on a hashless manifest must invalidate and cascade"
    );
}

/// 4. Config key change: rules referencing the changed key invalidate, and
///    the invalidation cascades downstream.
#[test]
fn parity_config_key_change() {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"[workflow]
name = "parity-config"
version = "1.0"

[config]
threshold = 1

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt; echo {config.threshold} >> out1.txt"

[[rules]]
name = "step2"
input = ["out1.txt"]
output = ["out2.txt"]
depends_on = ["step1"]
shell = "echo step2 >> exec_log.txt; cp out1.txt out2.txt"
"#;
    let wf = write_workflow(dir.path(), "config.oxoflow", toml);
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Flip the config key the first rule references.
    let toml = toml.replace("threshold = 1", "threshold = 2");
    fs::write(&wf, toml).unwrap();

    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "config key change",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "config change must invalidate the referencing rule and cascade"
    );
}

/// 5. Profile fill difference: the baseline ran WITH a profile that filled a
///    config key; predicting and executing WITHOUT it must agree on the
///    resulting config-change invalidation.
#[test]
fn parity_profile_fill_difference() {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"[workflow]
name = "parity-profile"
version = "1.0"

[config]
mode = "slow"

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt; echo {config.mode}-{config.extra} >> out1.txt"
"#;
    let wf = write_workflow(dir.path(), "profile.oxoflow", toml);
    fs::create_dir_all(dir.path().join("profiles")).unwrap();
    fs::write(
        dir.path().join("profiles/batch.toml"),
        "[config]\nextra = \"X\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();
    baseline_run(dir.path(), &wf, &["--profile", "batch"]);

    // No profile on either side: `extra` is missing from the merged config,
    // so both the preview and the run must flag step1 as config-changed.
    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "profile fill difference",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string()]),
        "dropping the profile must invalidate exactly the rule referencing the filled key"
    );
}

/// 6. `when` condition flip: the gate rule toggles between executed and
///    skipped as the config threshold crosses its condition — in both
///    directions.
#[test]
fn parity_when_condition_flip() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = |threshold: u32| {
        format!(
            r#"[workflow]
name = "parity-when"
version = "1.0"

[config]
threshold = {threshold}

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt"

[[rules]]
name = "gate"
input = ["out1.txt"]
output = ["gated.txt"]
depends_on = ["step1"]
when = "config.threshold >= 10"
shell = "echo gate >> exec_log.txt; cp out1.txt gated.txt"
"#
        )
    };
    let wf = write_workflow(dir.path(), "when.oxoflow", &workflow(10));
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();
    baseline_run(dir.path(), &wf, &[]); // gate executes: threshold = 10

    // Flip to false: gate must be skipped, nothing else re-executes.
    fs::write(&wf, workflow(5)).unwrap();
    let (predicted_run, predicted_skip) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "when flip to false",
    );
    assert!(predicted_run.is_empty(), "nothing may re-execute");
    assert!(predicted_skip.contains("gate"), "gate must be when-skipped");

    // Flip back: gate re-executes.
    fs::write(&wf, workflow(10)).unwrap();
    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "when flip back to true",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["gate".to_string()]),
        "only the gate may re-execute when its condition becomes true again"
    );
}

/// 7. Output deleted: exactly the rule whose output disappeared re-executes;
///    its up-to-date upstream stays protected.
#[test]
fn parity_deleted_output() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "chain.oxoflow", &chain_workflow());
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    fs::remove_file(dir.path().join("out2.txt")).unwrap();

    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "deleted output",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step2".to_string()]),
        "only the rule with the missing output may re-execute"
    );
}

/// 8. Legacy checkpoint (no manifest hashes): the metadata comparison path
///    keeps everything protected until an actual size/mtime change, which
///    invalidates exactly like the preview predicts.
#[test]
fn parity_legacy_checkpoint_without_manifest_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "chain.oxoflow", &chain_workflow());
    fs::write(dir.path().join("in.txt"), "data-aaaa").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Rewrite the checkpoint as a pre-content-hashing format: strip every
    // manifest entry's `hash` so only size+mtime remain.
    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&checkpoint).unwrap()).unwrap();
    for entries in json["input_manifests"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        for entry in entries.as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("hash");
        }
    }
    fs::write(&checkpoint, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    // (a) Untouched state: the hashless comparison still matches — nothing runs.
    let (predicted_run, predicted_skip) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "legacy checkpoint, untouched",
    );
    assert!(
        predicted_run.is_empty(),
        "legacy hashless manifest must stay protected: {predicted_run:?}"
    );
    assert_eq!(
        predicted_skip,
        HashSet::from(["step1".to_string(), "step2".to_string()])
    );

    // (b) Same-size rewrite: without hashes the mtime comparison catches it.
    fs::write(dir.path().join("in.txt"), "data-bbbb").unwrap();
    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap()],
        &[wf.to_str().unwrap()],
        "legacy checkpoint, same-size rewrite",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "mtime-based comparison must invalidate and cascade"
    );
}

/// 9. `--target` subset: the execution set is the target plus its upstream
///    dependencies; an upstream invalidation re-executes exactly that set
///    while rules outside it stay protected.
#[test]
fn parity_target_subset() {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"[workflow]
name = "parity-target"
version = "1.0"

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "echo step1 >> exec_log.txt; cp in.txt out1.txt"

[[rules]]
name = "step2"
input = ["out1.txt"]
output = ["out2.txt"]
depends_on = ["step1"]
shell = "echo step2 >> exec_log.txt; cp out1.txt out2.txt"

[[rules]]
name = "step3"
input = ["out2.txt"]
output = ["out3.txt"]
depends_on = ["step2"]
shell = "echo step3 >> exec_log.txt; cp out2.txt out3.txt"
"#;
    let wf = write_workflow(dir.path(), "target.oxoflow", toml);
    fs::write(dir.path().join("in.txt"), "alpha").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Invalidate the root input, then scope both commands to step2's
    // upstream closure (step1, step2). step3 must stay protected.
    fs::write(dir.path().join("in.txt"), "beta!").unwrap();
    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap(), "--target", "step2"],
        &[wf.to_str().unwrap(), "--target", "step2"],
        "target subset",
    );
    assert_eq!(
        predicted_run,
        HashSet::from(["step1".to_string(), "step2".to_string()]),
        "the target's upstream closure re-executes, nothing outside it"
    );
    assert!(
        dir.path().join("out3.txt").exists(),
        "step3's output must survive the subset run untouched"
    );
}

/// 10. `--samples` subset: only the selected sample's rules (plus the
///     queue-level combine they feed) re-execute; the other sample's rules
///     stay protected.
#[test]
fn parity_samples_subset_with_queue_cascade() {
    let dir = tempfile::tempdir().unwrap();
    let toml = r#"[workflow]
name = "parity-samples"
version = "1.0"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "trim"
input = ["data/{sample}.raw"]
output = ["mid/{sample}.txt"]
shell = "echo trim_{group}_{sample} >> exec_log.txt; cp data/{sample}.raw mid/{sample}.txt"

[[rules]]
name = "align"
input = ["mid/{sample}.txt"]
output = ["out/{sample}.txt"]
depends_on = ["trim"]
shell = "echo align_{group}_{sample} >> exec_log.txt; cp mid/{sample}.txt out/{sample}.txt"

[[rules]]
name = "combine"
input = ["out/S1.txt", "out/S2.txt"]
output = ["out/all.txt"]
depends_on = ["align"]
shell = "echo combine >> exec_log.txt; cat out/S1.txt out/S2.txt > out/all.txt"
"#;
    let wf = write_workflow(dir.path(), "samples.oxoflow", toml);
    fs::create_dir_all(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/S1.raw"), "one").unwrap();
    fs::write(dir.path().join("data/S2.raw"), "two").unwrap();
    baseline_run(dir.path(), &wf, &[]);

    // Invalidate only S2's raw input, then scope both commands to S2.
    fs::write(dir.path().join("data/S2.raw"), "two!").unwrap();
    let (predicted_run, _) = assert_parity(
        dir.path(),
        &[wf.to_str().unwrap(), "--samples", "S2"],
        &[wf.to_str().unwrap(), "--samples", "S2"],
        "samples subset",
    );
    assert_eq!(
        predicted_run,
        HashSet::from([
            "trim_cohort_S2".to_string(),
            "align_cohort_S2".to_string(),
            "combine".to_string(),
        ]),
        "S2's rules and the queue-level combine re-execute; S1 stays protected"
    );
}
